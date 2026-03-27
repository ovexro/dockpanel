use crate::safe_cmd::safe_command;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::sse::{Event, KeepAlive, Sse},
    Json,
};
use futures::stream::StreamExt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::collections::HashMap;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::auth::ServerScope;
use crate::error::{internal_error, err, agent_error, paginate, ApiError};
use crate::models::Site;
use crate::routes::is_valid_domain;
use crate::routes::reseller_dashboard::check_reseller_quota;
use crate::services::activity;
use crate::services::extensions::fire_event;
use crate::services::notifications;
use crate::services::security_hardening;
use crate::AppState;

/// A single provisioning step event.
#[derive(Clone, Debug, serde::Serialize)]
pub struct ProvisionStep {
    pub step: String,
    pub label: String,
    pub status: String, // "pending", "in_progress", "done", "error"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Helper: emit a provisioning step to the broadcast channel + history.
fn emit_step(
    logs: &Arc<Mutex<HashMap<Uuid, (Vec<ProvisionStep>, broadcast::Sender<ProvisionStep>, Instant)>>>,
    site_id: Uuid,
    step: &str,
    label: &str,
    status: &str,
    message: Option<String>,
) {
    let ev = ProvisionStep {
        step: step.into(),
        label: label.into(),
        status: status.into(),
        message,
    };
    if let Ok(mut map) = logs.lock() {
        if let Some((history, tx, _)) = map.get_mut(&site_id) {
            history.push(ev.clone());
            let _ = tx.send(ev);
        }
    }
}

#[derive(serde::Deserialize)]
pub struct ListQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(serde::Deserialize)]
pub struct CreateSiteRequest {
    pub domain: String,
    pub runtime: Option<String>,
    pub proxy_port: Option<i32>,
    pub php_version: Option<String>,
    pub php_preset: Option<String>,
    /// Start command for node/python runtimes (e.g., "npm start", "gunicorn app:app")
    pub app_command: Option<String>,
    // One-click CMS install
    pub cms: Option<String>,
    pub site_title: Option<String>,
    pub admin_email: Option<String>,
    pub admin_user: Option<String>,
    pub admin_password: Option<String>,
}

/// GET /api/sites — List all sites for the current user.
pub async fn list(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(server_id, _agent): ServerScope,
    Query(params): Query<ListQuery>,
) -> Result<Json<Vec<Site>>, ApiError> {
    let (limit, offset) = paginate(params.limit, params.offset);

    let sites: Vec<Site> = sqlx::query_as(
        "SELECT * FROM sites WHERE user_id = $1 AND server_id = $2 ORDER BY created_at DESC LIMIT $3 OFFSET $4",
    )
    .bind(claims.sub)
    .bind(server_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("list sites", e))?;

    Ok(Json(sites))
}

/// POST /api/sites — Create a new site.
pub async fn create(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(server_id, agent): ServerScope,
    headers: HeaderMap,
    Json(body): Json<CreateSiteRequest>,
) -> Result<(StatusCode, Json<Site>), ApiError> {
    // Feature 9: Block site creation during lockdown
    if security_hardening::is_locked_down(&state.db).await {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "System is in lockdown mode"));
    }

    // Feature 3: Rate limit site creation (max N per user per hour)
    {
        let max_sites: i64 = security_hardening::get_setting_bool(&state.db, "security_site_rate_limit", true)
            .await
            .then(|| 3i64)
            .unwrap_or(999);
        let recent: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM sites WHERE user_id = $1 AND created_at > NOW() - INTERVAL '1 hour'"
        )
        .bind(claims.sub)
        .fetch_one(&state.db)
        .await
        .unwrap_or((0,));
        if recent.0 >= max_sites {
            // Feature 4: Record as suspicious event
            let _ = security_hardening::record_suspicious_event(
                &state.db, "site.rate_limit_hit", Some(&claims.email), None,
                Some(&format!("User tried to create site #{} in 1 hour", recent.0 + 1)),
            ).await;
            return Err(err(StatusCode::TOO_MANY_REQUESTS,
                &format!("Site creation rate limit: max {max_sites} sites per hour")));
        }
    }

    // Validate domain format
    if !is_valid_domain(&body.domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }

    // Block reserved panel domains
    let reserved = ["dockpanel.dev", "panel.example.com", "docs.dockpanel.dev"];
    let domain_lower = body.domain.to_lowercase();
    if reserved.iter().any(|r| domain_lower == *r || domain_lower.ends_with(&format!(".{r}"))) {
        return Err(err(StatusCode::FORBIDDEN, "This domain is reserved and cannot be used"));
    }

    let runtime = body.runtime.as_deref().unwrap_or("static");
    if !["static", "php", "proxy", "node", "python"].contains(&runtime) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Runtime must be static, php, proxy, node, or python",
        ));
    }

    if runtime == "proxy" && body.proxy_port.is_none() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "proxy_port is required for proxy runtime",
        ));
    }

    // Node/Python require app_command
    if (runtime == "node" || runtime == "python") && body.app_command.as_ref().map_or(true, |c| c.trim().is_empty()) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "app_command is required for node/python runtime",
        ));
    }

    // Validate app_command: reject shell injection, newlines, and non-whitelisted prefixes
    if let Some(ref cmd) = body.app_command {
        if cmd.contains('\n') || cmd.contains('\r') || cmd.contains('\0') {
            return Err(err(StatusCode::BAD_REQUEST, "app_command must not contain newlines or null bytes"));
        }
        let forbidden = ['`', '$', '|', ';', '&', '<', '>', '\\', '!', '{', '}'];
        if cmd.chars().any(|c| forbidden.contains(&c)) {
            return Err(err(StatusCode::BAD_REQUEST, "app_command contains forbidden characters"));
        }
        if cmd.contains("..") {
            return Err(err(StatusCode::BAD_REQUEST, "app_command must not contain '..'"));
        }
        if cmd.len() > 1024 {
            return Err(err(StatusCode::BAD_REQUEST, "app_command too long"));
        }
        // Whitelist allowed command prefixes per runtime
        if runtime == "node" {
            let valid = cmd.starts_with("node ") || cmd.starts_with("npm ")
                || cmd.starts_with("npx ") || cmd.starts_with("yarn ")
                || cmd.starts_with("pnpm ") || !cmd.contains(' ');
            if !valid {
                return Err(err(StatusCode::BAD_REQUEST,
                    "app_command for node must start with node/npm/npx/yarn/pnpm or be a bare filename"));
            }
        } else if runtime == "python" {
            let valid = cmd.starts_with("python") || cmd.starts_with("gunicorn ")
                || cmd.starts_with("uvicorn ") || cmd.starts_with("flask ")
                || cmd.starts_with("django") || !cmd.contains(' ');
            if !valid {
                return Err(err(StatusCode::BAD_REQUEST,
                    "app_command for python must start with python/gunicorn/uvicorn/flask/django or be a bare filename"));
            }
        }
    }

    if let Some(ref preset) = body.php_preset {
        if !["generic", "laravel", "wordpress", "drupal", "joomla", "symfony", "codeigniter", "magento"].contains(&preset.as_str()) {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "php_preset must be one of: generic, laravel, wordpress, drupal, joomla, symfony, codeigniter, magento",
            ));
        }
    }

    // Check domain uniqueness
    let existing: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM sites WHERE domain = $1")
            .bind(&body.domain)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("create sites", e))?;

    if existing.is_some() {
        return Err(err(StatusCode::CONFLICT, "Domain already exists"));
    }

    // Cross-table domain uniqueness: check git_deploys
    let git_conflict: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM git_deploys WHERE domain = $1 AND server_id = $2"
    )
    .bind(&body.domain)
    .bind(server_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("create sites", e))?;

    if git_conflict.is_some() {
        return Err(err(StatusCode::CONFLICT, "Domain already in use by a git deployment"));
    }

    // Check reseller quota before creating site
    check_reseller_quota(&state.db, claims.sub, "sites").await?;

    // Check reseller server isolation: user under a reseller can only use allocated servers
    let user_reseller: Option<(Option<uuid::Uuid>,)> = sqlx::query_as(
        "SELECT reseller_id FROM users WHERE id = $1"
    ).bind(claims.sub).fetch_optional(&state.db).await
        .map_err(|e| internal_error("reseller check", e))?;
    if let Some((Some(rid),)) = user_reseller {
        let allowed: Option<(uuid::Uuid,)> = sqlx::query_as(
            "SELECT id FROM reseller_servers WHERE reseller_id = $1 AND server_id = $2"
        ).bind(rid).bind(server_id).fetch_optional(&state.db).await
            .map_err(|e| internal_error("reseller server check", e))?;
        if allowed.is_none() {
            return Err(err(StatusCode::FORBIDDEN, "This server is not allocated to your reseller account"));
        }
    }

    // Insert site with status "creating" inside a transaction
    let mut tx = state.db.begin().await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Transaction start failed: {e}")))?;

    // Auto-allocate port for node/python runtimes
    let effective_proxy_port = if (runtime == "node" || runtime == "python") && body.proxy_port.is_none() {
        // Find first available port in 4000-4999 range
        let row: Option<(i32,)> = sqlx::query_as(
            "SELECT s.port FROM generate_series(5000, 5999) AS s(port) \
             WHERE s.port NOT IN (SELECT proxy_port FROM sites WHERE proxy_port IS NOT NULL AND server_id = $1) \
             LIMIT 1"
        )
        .bind(server_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| internal_error("create sites", e))?;
        row.map(|(p,)| p)
    } else {
        body.proxy_port
    };

    let site: Site = sqlx::query_as(
        "INSERT INTO sites (user_id, server_id, domain, runtime, status, proxy_port, php_version, php_preset, app_command) \
         VALUES ($1, $2, $3, $4, 'creating', $5, $6, $7, $8) RETURNING *",
    )
    .bind(claims.sub)
    .bind(server_id)
    .bind(&body.domain)
    .bind(runtime)
    .bind(effective_proxy_port)
    .bind(&body.php_version)
    .bind(body.php_preset.as_deref().unwrap_or("generic"))
    .bind(&body.app_command)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| {
        let msg = e.to_string();
        if msg.contains("duplicate key") || msg.contains("unique") {
            err(StatusCode::CONFLICT, "Domain already exists")
        } else {
            err(StatusCode::INTERNAL_SERVER_ERROR, &msg)
        }
    })?;

    // Create provisioning log channel
    let (broadcast_tx, _) = broadcast::channel::<ProvisionStep>(64);
    {
        let mut logs = state.provision_logs.lock().unwrap_or_else(|e| e.into_inner());
        logs.insert(site.id, (Vec::new(), broadcast_tx, Instant::now()));
    }
    let logs = state.provision_logs.clone();
    let site_id = site.id;

    emit_step(&logs, site_id, "nginx", "Configuring web server", "in_progress", None);

    // Build agent request body
    let mut agent_body = serde_json::json!({
        "runtime": runtime,
    });

    if let Some(port) = effective_proxy_port {
        agent_body["proxy_port"] = serde_json::json!(port);
    }
    if let Some(ref cmd) = body.app_command {
        agent_body["app_command"] = serde_json::json!(cmd);
    }
    if let Some(ref php) = body.php_version {
        agent_body["php_socket"] = serde_json::json!(format!("unix:/run/php/php{php}-fpm.sock"));
    }
    if let Some(ref preset) = body.php_preset {
        agent_body["php_preset"] = serde_json::json!(preset);
    }

    // Call agent to create nginx config
    let agent_path = format!("/nginx/sites/{}", body.domain);
    match agent.put(&agent_path, agent_body).await {
        Ok(_) => {
            emit_step(&logs, site_id, "nginx", "Configuring web server", "done", None);

            // Agent succeeded — commit the transaction so the site record is persisted
            // (background tasks like monitors, backups, SSL need the site to exist)
            tx.commit().await
                .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Transaction commit failed: {e}")))?;

            // Update status to active
            sqlx::query(
                "UPDATE sites SET status = 'active', updated_at = NOW() \
                 WHERE id = $1 AND status = 'creating'"
            )
                .bind(site.id)
                .execute(&state.db)
                .await
                .map_err(|e| internal_error("create sites", e))?;

            let updated: Site = sqlx::query_as("SELECT * FROM sites WHERE id = $1")
                .bind(site.id)
                .fetch_one(&state.db)
                .await
                .map_err(|e| internal_error("create sites", e))?;

            // GAP 50: Block direct external access to proxy port (only allow localhost via nginx)
            if let Some(port) = effective_proxy_port {
                let _ = agent.post("/security/firewall/rules", Some(serde_json::json!({
                    "port": port as u16,
                    "proto": "tcp",
                    "action": "deny",
                    "from": null
                }))).await;
                tracing::info!("Auto-firewall: blocked external access to port {port} for {}", body.domain);
            }

            tracing::info!("Site created: {} ({})", body.domain, runtime);
            let ip = crate::routes::client_ip(&headers);
            activity::log_activity(
                &state.db, claims.sub, &claims.email, "site.create",
                Some("site"), Some(&body.domain), Some(runtime), ip.as_deref(),
            ).await;

            // Panel notification
            notifications::notify_panel(&state.db, Some(claims.sub), &format!("Site created: {}", body.domain), &format!("New {} site is now active", runtime), "info", "site", None).await;

            fire_event(&state.db, "site.created", serde_json::json!({
                "site_id": site.id, "domain": site.domain, "runtime": site.runtime,
            }));

            // Increment reseller site counter
            let _ = sqlx::query(
                "UPDATE reseller_profiles SET used_sites = used_sites + 1, updated_at = NOW() \
                 WHERE user_id = (SELECT reseller_id FROM users WHERE id = $1 AND reseller_id IS NOT NULL)"
            ).bind(claims.sub).execute(&state.db).await;

            // NOTE: Auto-monitor creation disabled — on fresh installs without DNS
            // configured, auto-created monitors immediately show "down" which confuses
            // new users. Users can create monitors manually when ready.
            // See: https://github.com/ovexro/dockpanel/issues/XX

            // Auto-create backup schedule for every new site (daily 3 AM, 7 retention)
            {
                let backup_db = state.db.clone();
                let backup_site_id = site.id;
                tokio::spawn(async move {
                    let _ = sqlx::query(
                        "INSERT INTO backup_schedules (site_id, schedule, retention_count, enabled) \
                         VALUES ($1, '0 3 * * *', 7, true) ON CONFLICT (site_id) DO NOTHING"
                    ).bind(backup_site_id).execute(&backup_db).await;
                    tracing::info!("Auto-backup: created daily schedule for new site");
                });
            }

            // GAP 6: Auto-create secrets vault for the site
            {
                let vault_db = state.db.clone();
                let vault_site_id = site.id;
                let vault_user_id = claims.sub;
                let vault_domain = body.domain.clone();
                tokio::spawn(async move {
                    let _ = sqlx::query(
                        "INSERT INTO secret_vaults (user_id, name, description, site_id) \
                         VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING"
                    )
                    .bind(vault_user_id)
                    .bind(format!("{vault_domain} secrets"))
                    .bind(format!("Auto-created vault for {vault_domain}"))
                    .bind(vault_site_id)
                    .execute(&vault_db).await;
                    tracing::info!("Auto-vault: created for {vault_domain}");
                });
            }

            // GAP 15: Auto-create paused uptime monitor (activates after SSL provisioning)
            {
                let mon_db = state.db.clone();
                let mon_site_id = site.id;
                let mon_user_id = claims.sub;
                let mon_domain = body.domain.clone();
                tokio::spawn(async move {
                    let url = format!("https://{mon_domain}");
                    let _ = sqlx::query(
                        "INSERT INTO monitors (user_id, site_id, url, name, check_interval, status, enabled, monitor_type) \
                         VALUES ($1, $2, $3, $4, 60, 'pending', FALSE, 'http') ON CONFLICT DO NOTHING"
                    )
                    .bind(mon_user_id).bind(mon_site_id)
                    .bind(&url).bind(&mon_domain)
                    .execute(&mon_db).await;
                    tracing::info!("Auto-monitor: created (paused) for {mon_domain}");
                });
            }

            // GAP 4: Auto-create status page component if status page is enabled
            {
                let sp_db = state.db.clone();
                let sp_site_id = site.id;
                let sp_user_id = claims.sub;
                let sp_domain = body.domain.clone();
                tokio::spawn(async move {
                    let enabled: Option<(bool,)> = match sqlx::query_as(
                        "SELECT enabled FROM status_page_config WHERE user_id = $1"
                    ).bind(sp_user_id).fetch_optional(&sp_db).await {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::warn!("DB error checking status page config for auto-component: {e}");
                            None
                        }
                    };

                    if enabled.map(|(e,)| e).unwrap_or(false) {
                        let _ = sqlx::query(
                            "INSERT INTO status_page_components (user_id, name, description, group_name) \
                             VALUES ($1, $2, $3, 'Sites')"
                        )
                        .bind(sp_user_id).bind(&sp_domain)
                        .bind(format!("Auto-created for {sp_domain}"))
                        .execute(&sp_db).await;
                        tracing::info!("Auto-component: created status page component for {sp_domain}");
                    }
                });
            }

            // Auto-DNS: create A record if user has a DNS zone for this domain
            {
                let dns_domain = body.domain.clone();
                let dns_db = state.db.clone();
                let dns_logs = logs.clone();
                let dns_user_id = claims.sub;
                tokio::spawn(async move {
                    // Extract parent domain
                    let parts: Vec<&str> = dns_domain.splitn(3, '.').collect();
                    let parent = if parts.len() >= 3 {
                        format!("{}.{}", parts[parts.len() - 2], parts[parts.len() - 1])
                    } else {
                        dns_domain.clone()
                    };

                    let zone: Option<(String, Option<String>, Option<String>, Option<String>)> = match sqlx::query_as(
                        "SELECT provider, cf_zone_id, cf_api_token, cf_api_email FROM dns_zones WHERE domain = $1 AND user_id = $2"
                    ).bind(&parent).bind(dns_user_id).fetch_optional(&dns_db).await {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::warn!("DB error fetching DNS zone for auto-DNS on site create: {e}");
                            None
                        }
                    };

                    if let Some((provider, cf_zone_id, cf_api_token, cf_api_email)) = zone {
                        let server_ip = crate::helpers::detect_public_ip().await;

                        if provider == "cloudflare" {
                            if let (Some(zid), Some(tok)) = (cf_zone_id, cf_api_token) {
                                let client = reqwest::Client::new();
                                let headers = crate::helpers::cf_headers(&tok, cf_api_email.as_deref());
                                let _ = client.post(&format!("https://api.cloudflare.com/client/v4/zones/{zid}/dns_records"))
                                    .headers(headers)
                                    .json(&serde_json::json!({"type":"A","name":dns_domain,"content":server_ip,"proxied":true,"ttl":1}))
                                    .send().await;
                                tracing::info!("Auto-DNS: created A record {dns_domain} → {server_ip}");
                                emit_step(&dns_logs, site_id, "dns", "Creating DNS record", "done", None);
                            }
                        } else if provider == "powerdns" {
                            let pdns: Vec<(String, String)> = sqlx::query_as(
                                "SELECT key, value FROM settings WHERE key IN ('pdns_api_url', 'pdns_api_key')"
                            ).fetch_all(&dns_db).await.unwrap_or_default();
                            let purl = pdns.iter().find(|(k,_)| k == "pdns_api_url").map(|(_,v)| v.clone());
                            let pkey_enc = pdns.iter().find(|(k,_)| k == "pdns_api_key").map(|(_,v)| v.clone());
                            if let (Some(url), Some(key_enc)) = (purl, pkey_enc) {
                                let key = crate::services::secrets_crypto::decrypt_credential_from_env(&key_enc);
                                let zfqdn = if parent.ends_with('.') { parent.clone() } else { format!("{parent}.") };
                                let _ = reqwest::Client::new()
                                    .patch(&format!("{url}/api/v1/servers/localhost/zones/{zfqdn}"))
                                    .header("X-API-Key", &key)
                                    .json(&serde_json::json!({"rrsets":[{"name":format!("{dns_domain}."),"type":"A","ttl":300,"changetype":"REPLACE","records":[{"content":server_ip,"disabled":false}]}]}))
                                    .send().await;
                                tracing::info!("Auto-DNS (PowerDNS): created A record {dns_domain} → {server_ip}");
                                emit_step(&dns_logs, site_id, "dns", "Creating DNS record", "done", None);
                            }
                        }
                    }
                });
            }

            // Auto-SSL: try to provision Let's Encrypt cert in background
            let ssl_agent = agent.clone();
            let ssl_db = state.db.clone();
            let ssl_domain = body.domain.clone();
            let ssl_email = claims.email.clone();
            let ssl_runtime = runtime.to_string();
            let ssl_php_socket = body.php_version.as_ref().map(|v| format!("unix:/run/php/php{v}-fpm.sock"));
            let ssl_proxy_port = body.proxy_port;
            let ssl_php_preset = body.php_preset.clone();
            let ssl_root_path: Option<String> = None; // default root
            let ssl_logs = logs.clone();
            tokio::spawn(async move {
                // Retry SSL with backoff: 3s, 30s, 2m, 5m
                let delays = [3u64, 30, 120, 300];
                for (i, delay) in delays.iter().enumerate() {
                    tokio::time::sleep(Duration::from_secs(*delay)).await;
                    emit_step(&ssl_logs, site_id, "ssl", "Provisioning SSL certificate", "in_progress", None);
                    let mut ssl_body = serde_json::json!({
                        "email": ssl_email,
                        "runtime": ssl_runtime,
                        "php_socket": ssl_php_socket,
                        "proxy_port": ssl_proxy_port,
                    });
                    if let Some(ref preset) = ssl_php_preset {
                        ssl_body["php_preset"] = serde_json::json!(preset);
                    }
                    if let Some(ref root) = ssl_root_path {
                        ssl_body["root"] = serde_json::json!(root);
                    }
                    match ssl_agent.post(&format!("/ssl/provision/{ssl_domain}"), Some(ssl_body)).await {
                        Ok(result) => {
                            tracing::info!("Auto-SSL provisioned for {ssl_domain} (attempt {})", i + 1);
                            emit_step(&ssl_logs, site_id, "ssl", "Provisioning SSL certificate", "done", None);

                            // Parse cert details from agent response
                            let ssl_expiry = result
                                .get("expiry")
                                .and_then(|v| v.as_str())
                                .and_then(|s| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f UTC").ok())
                                .map(|dt| dt.and_utc());
                            let cert_path = result.get("cert_path").and_then(|v| v.as_str()).unwrap_or("").to_string();
                            let key_path = result.get("key_path").and_then(|v| v.as_str()).unwrap_or("").to_string();

                            // Update site DB record with SSL status
                            let _ = sqlx::query(
                                "UPDATE sites SET ssl_enabled = true, ssl_cert_path = $1, ssl_key_path = $2, \
                                 ssl_expiry = $3, updated_at = NOW() WHERE id = $4"
                            )
                            .bind(&cert_path)
                            .bind(&key_path)
                            .bind(ssl_expiry)
                            .bind(site_id)
                            .execute(&ssl_db)
                            .await;

                            // Activate paused monitors now that SSL is working
                            let _ = sqlx::query(
                                "UPDATE monitors SET enabled = TRUE WHERE site_id = $1 AND enabled = FALSE AND status = 'pending'"
                            )
                            .bind(site_id)
                            .execute(&ssl_db)
                            .await;

                            return; // Success, stop retrying
                        }
                        Err(e) => {
                            if i == delays.len() - 1 {
                                // Last attempt failed
                                tracing::info!("Auto-SSL failed for {ssl_domain} after {} attempts: {e}", i + 1);
                                emit_step(&ssl_logs, site_id, "ssl", "SSL certificate", "error",
                                    Some("Skipped — can be provisioned manually from site settings".into()));
                            } else {
                                tracing::info!("Auto-SSL attempt {} for {ssl_domain} failed, retrying in {}s", i + 1, delays[i + 1]);
                            }
                        }
                    }
                }

                // If no CMS install, this is the final step — emit complete
                // (For WordPress, the WP task emits complete)
            });

            // One-click CMS/framework install
            let cms_type = body.cms.as_deref().unwrap_or("");
            let needs_db = matches!(cms_type, "wordpress" | "laravel" | "drupal" | "joomla" | "codeigniter");
            let needs_install = matches!(cms_type, "wordpress" | "laravel" | "drupal" | "joomla" | "symfony" | "codeigniter");

            if needs_install {
                let cms_agent = agent.clone();
                let cms_domain = body.domain.clone();
                let cms_db = state.db.clone();
                let cms_name = cms_type.to_string();
                let cms_label = match cms_type {
                    "wordpress" => "WordPress",
                    "laravel" => "Laravel",
                    "drupal" => "Drupal",
                    "joomla" => "Joomla",
                    "symfony" => "Symfony",
                    "codeigniter" => "CodeIgniter",
                    _ => cms_type,
                }.to_string();
                let cms_title = body.site_title.clone().unwrap_or_else(|| body.domain.clone());
                let cms_email = body.admin_email.clone().unwrap_or_else(|| "admin@example.com".to_string());
                let cms_user = body.admin_user.clone().unwrap_or_else(|| "admin".to_string());
                let cms_pass = body.admin_password.clone().unwrap_or_else(|| {
                    use rand::Rng;
                    let mut rng = rand::thread_rng();
                    (0..16).map(|_| rng.sample(rand::distributions::Alphanumeric) as char).collect()
                });
                let cms_logs = logs.clone();
                let cms_jwt_secret = state.config.jwt_secret.clone();

                tokio::spawn(async move {
                    let db_name = cms_domain.replace('.', "_").replace('-', "_");
                    let db_user_name = db_name.clone();
                    let db_password: String = {
                        use rand::Rng;
                        let mut rng = rand::thread_rng();
                        (0..20).map(|_| rng.sample(rand::distributions::Alphanumeric) as char).collect()
                    };

                    // 1. Create database (if needed)
                    let mut db_host = String::new();
                    if needs_db {
                        emit_step(&cms_logs, site_id, "database", "Creating MySQL database", "in_progress", None);

                        let db_result = cms_agent.post("/databases", Some(serde_json::json!({
                            "engine": "mysql",
                            "name": db_name,
                            "password": db_password,
                        }))).await;

                        let (host, db_port, db_container_id) = match db_result {
                            Ok(resp) => {
                                let port = resp.get("port").and_then(|v| v.as_u64()).unwrap_or(3306) as u16;
                                let cid = resp.get("container_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                emit_step(&cms_logs, site_id, "database", "Creating MySQL database", "done", None);
                                (format!("127.0.0.1:{port}"), port as i32, cid)
                            }
                            Err(e) => {
                                tracing::error!("{cms_label} DB creation failed for {cms_domain}: {e}");
                                emit_step(&cms_logs, site_id, "database", "Creating MySQL database", "error",
                                    Some(format!("Database creation failed: {e}")));
                                emit_step(&cms_logs, site_id, "complete", "Provisioning failed", "error", None);
                                tokio::time::sleep(Duration::from_secs(30)).await;
                                cms_logs.lock().unwrap_or_else(|e| e.into_inner()).remove(&site_id);
                                return;
                            }
                        };
                        db_host = host;

                        let encrypted_db_password = crate::services::secrets_crypto::encrypt_credential(&db_password, &cms_jwt_secret)
                            .unwrap_or_else(|_| db_password.clone());
                        let _ = sqlx::query(
                            "INSERT INTO databases (site_id, engine, name, db_user, db_password_enc, container_id, port) \
                             VALUES ((SELECT id FROM sites WHERE domain = $1), 'mysql', $2, $3, $4, $5, $6) \
                             ON CONFLICT DO NOTHING",
                        )
                        .bind(&cms_domain)
                        .bind(&db_name)
                        .bind(&db_user_name)
                        .bind(&encrypted_db_password)
                        .bind(&db_container_id)
                        .bind(db_port)
                        .execute(&cms_db)
                        .await;

                        emit_step(&cms_logs, site_id, "db_init", "Waiting for database engine", "in_progress", None);
                        // Wait for MariaDB to be fully ready (TCP connects before MySQL is ready)
                        for _attempt in 1..=20 {
                            tokio::time::sleep(Duration::from_secs(2)).await;
                            let php_check = safe_command("php")
                                .args(["-r", "try { new PDO(getenv('DSN'), getenv('DB_USER'), getenv('DB_PASS')); echo 'OK'; } catch(Exception $e) { echo 'FAIL'; }"])
                                .env("DSN", format!("mysql:host={db_host};dbname={db_name}"))
                                .env("DB_USER", &db_user_name)
                                .env("DB_PASS", &db_password)
                                .output()
                                .await;
                            if let Ok(out) = php_check {
                                if String::from_utf8_lossy(&out.stdout).contains("OK") {
                                    break;
                                }
                            }
                        }
                        emit_step(&cms_logs, site_id, "db_init", "Database engine ready", "done", None);
                    }

                    // 2. Install CMS/framework
                    emit_step(&cms_logs, site_id, "install", &format!("Installing {cms_label}"), "in_progress", None);

                    let install_result = if cms_name == "wordpress" {
                        cms_agent.post(&format!("/wordpress/{cms_domain}/install"), Some(serde_json::json!({
                            "url": format!("https://{cms_domain}"),
                            "title": cms_title,
                            "admin_user": cms_user,
                            "admin_pass": cms_pass,
                            "admin_email": cms_email,
                            "db_name": db_name,
                            "db_user": db_user_name,
                            "db_pass": db_password,
                            "db_host": db_host,
                        }))).await
                    } else {
                        cms_agent.post(&format!("/cms/{cms_domain}/install"), Some(serde_json::json!({
                            "cms": cms_name,
                            "title": cms_title,
                            "admin_user": cms_user,
                            "admin_pass": cms_pass,
                            "admin_email": cms_email,
                            "db_name": db_name,
                            "db_user": db_user_name,
                            "db_pass": db_password,
                            "db_host": db_host,
                        }))).await
                    };

                    match install_result {
                        Ok(_) => {
                            tracing::info!("{cms_label} installed on {cms_domain}");
                            emit_step(&cms_logs, site_id, "install", &format!("Installing {cms_label}"), "done", None);

                            // Auto-create WordPress system cron
                            if cms_name == "wordpress" {
                                let cron_db = cms_db.clone();
                                let cron_domain = cms_domain.clone();
                                let cron_site_id = site_id;
                                tokio::spawn(async move {
                                    let command = format!("cd /var/www/{cron_domain}/public && php wp-cron.php > /dev/null 2>&1");
                                    let _ = sqlx::query(
                                        "INSERT INTO crons (site_id, label, command, schedule, enabled) \
                                         VALUES ($1, 'WordPress Cron', $2, '*/15 * * * *', true)"
                                    )
                                    .bind(cron_site_id)
                                    .bind(&command)
                                    .execute(&cron_db)
                                    .await;
                                    tracing::info!("Auto-cron: created WordPress cron for {cron_domain}");
                                });
                            }
                        }
                        Err(e) => {
                            tracing::error!("{cms_label} install failed for {cms_domain}: {e}");
                            emit_step(&cms_logs, site_id, "install", &format!("Installing {cms_label}"), "error",
                                Some(format!("{cms_label} install failed: {e}")));
                        }
                    }

                    emit_step(&cms_logs, site_id, "complete", "Site ready", "done", None);
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    cms_logs.lock().unwrap_or_else(|e| e.into_inner()).remove(&site_id);
                });
            } else {
                // Non-CMS site: emit complete after SSL (spawned separately)
                let final_logs = logs.clone();
                tokio::spawn(async move {
                    // Wait for SSL task to finish (SSL has 3s delay + ~5s provision)
                    tokio::time::sleep(Duration::from_secs(12)).await;
                    emit_step(&final_logs, site_id, "complete", "Site ready", "done", None);
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    final_logs.lock().unwrap_or_else(|e| e.into_inner()).remove(&site_id);
                });
            }

            Ok((StatusCode::CREATED, Json(updated)))
        }
        Err(e) => {
            // Agent call failed — roll back the transaction (INSERT is undone)
            tracing::error!("Agent error creating site {}: {e}", body.domain);

            crate::services::system_log::log_event(
                &state.db,
                "error",
                "api",
                &format!("Site creation failed: {}", body.domain),
                Some(&e.to_string()),
            ).await;

            // tx is dropped here, automatically rolling back the INSERT
            drop(tx);

            emit_step(&logs, site_id, "nginx", "Configuring web server", "error",
                Some(format!("Agent error: {e}")));
            emit_step(&logs, site_id, "complete", "Provisioning failed", "error", None);

            // Clean up provision log after delay
            let cleanup_logs = logs.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(30)).await;
                cleanup_logs.lock().unwrap_or_else(|e| e.into_inner()).remove(&site_id);
            });

            Err(agent_error("Site configuration", e))
        }
    }
}

/// GET /api/sites/{id}/provision-log — SSE stream of provisioning steps.
pub async fn provision_log(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, axum::BoxError>>>, ApiError> {
    // Verify ownership
    let exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM sites WHERE id = $1 AND user_id = $2"
    )
    .bind(id).bind(claims.sub)
    .fetch_optional(&state.db).await
    .map_err(|e| internal_error("provision log", e))?;

    if exists.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Site not found"));
    }

    // Get broadcast receiver + snapshot of existing steps
    let (snapshot, rx) = {
        let logs = state.provision_logs.lock().unwrap_or_else(|e| e.into_inner());
        match logs.get(&id) {
            Some((history, tx, _)) => (history.clone(), Some(tx.subscribe())),
            None => (Vec::new(), None),
        }
    };

    let rx = rx.ok_or_else(|| err(StatusCode::NOT_FOUND, "No active provisioning for this site"))?;

    // First yield snapshot events, then stream live updates
    let snapshot_stream = futures::stream::iter(
        snapshot.into_iter().map(|step| {
            let data = serde_json::to_string(&step).unwrap_or_default();
            Ok(Event::default().data(data))
        })
    );

    let live_stream = BroadcastStream::new(rx)
        .filter_map(|result| async {
            match result {
                Ok(step) => {
                    let data = serde_json::to_string(&step).ok()?;
                    Some(Ok(Event::default().data(data)))
                }
                Err(_) => None,
            }
        });

    let stream = snapshot_stream.chain(live_stream);

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    ))
}

/// GET /api/sites/{id} — Get site details.
pub async fn get_one(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Site>, ApiError> {
    let site: Site = sqlx::query_as(
        "SELECT * FROM sites WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("get_one sites", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    Ok(Json(site))
}

/// PUT /api/sites/{id}/php — Switch PHP version for a site.
#[derive(serde::Deserialize)]
pub struct SwitchPhpRequest {
    pub version: String,
}

pub async fn switch_php(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
    Json(body): Json<SwitchPhpRequest>,
) -> Result<Json<Site>, ApiError> {
    let version = body.version.trim();

    if !["8.1", "8.2", "8.3", "8.4"].contains(&version) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Invalid PHP version. Allowed: 8.1, 8.2, 8.3, 8.4",
        ));
    }

    let site: Site = sqlx::query_as(
        "SELECT * FROM sites WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("switch php", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    if site.runtime != "php" {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "PHP version can only be changed on PHP sites",
        ));
    }

    let mut agent_body = serde_json::json!({
        "runtime": "php",
        "php_socket": format!("unix:/run/php/php{version}-fpm.sock"),
    });

    if let Some(ref preset) = site.php_preset {
        agent_body["php_preset"] = serde_json::json!(preset);
    }
    if let Some(ref custom) = site.custom_nginx {
        agent_body["custom_nginx"] = serde_json::json!(custom);
    }
    if site.ssl_enabled {
        agent_body["ssl"] = serde_json::json!(true);
        if let Some(ref cert) = site.ssl_cert_path {
            agent_body["ssl_cert"] = serde_json::json!(cert);
        }
        if let Some(ref key) = site.ssl_key_path {
            agent_body["ssl_key"] = serde_json::json!(key);
        }
    }

    let agent_path = format!("/nginx/sites/{}", site.domain);
    agent
        .put(&agent_path, agent_body)
        .await
        .map_err(|e| agent_error("Nginx update", e))?;

    let updated: Site = sqlx::query_as(
        "UPDATE sites SET php_version = $1, updated_at = NOW() WHERE id = $2 RETURNING *",
    )
    .bind(version)
    .bind(id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("switch php", e))?;

    tracing::info!("PHP version switched to {} for {}", version, site.domain);
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "site.php_switch",
        Some("site"), Some(&site.domain), Some(version), None,
    ).await;

    Ok(Json(updated))
}

/// GET /api/php/versions — List available PHP versions (proxy to agent).
pub async fn php_versions(
    State(_state): State<AppState>,
    AuthUser(_claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent
        .get("/php/versions")
        .await
        .map_err(|e| agent_error("Site agent operation", e))?;

    Ok(Json(result))
}

/// POST /api/php/install — Install a PHP version (proxy to agent, admin only).
#[derive(serde::Deserialize)]
pub struct InstallPhpRequest {
    pub version: String,
}

pub async fn php_install(
    State(_state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<InstallPhpRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if claims.role != "admin" {
        return Err(err(StatusCode::FORBIDDEN, "Admin only"));
    }

    let result = agent
        .post(
            "/php/install",
            Some(serde_json::json!({ "version": body.version })),
        )
        .await
        .map_err(|e| agent_error("PHP install", e))?;

    Ok(Json(result))
}

/// POST /api/php/uninstall — Uninstall a specific PHP version (admin only).
pub async fn php_uninstall(
    State(_state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<InstallPhpRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if claims.role != "admin" {
        return Err(err(StatusCode::FORBIDDEN, "Admin only"));
    }

    let result = agent
        .post(
            "/php/uninstall",
            Some(serde_json::json!({ "version": body.version })),
        )
        .await
        .map_err(|e| agent_error("PHP uninstall", e))?;

    Ok(Json(result))
}

/// PUT /api/sites/{id}/limits — Update per-site resource limits.
#[derive(serde::Deserialize)]
pub struct UpdateLimitsRequest {
    pub rate_limit: Option<i32>,
    pub max_upload_mb: Option<i32>,
    pub php_memory_mb: Option<i32>,
    pub php_max_workers: Option<i32>,
    pub custom_nginx: Option<String>,
}

pub async fn update_limits(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateLimitsRequest>,
) -> Result<Json<Site>, ApiError> {
    let site: Site = sqlx::query_as(
        "SELECT * FROM sites WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("update limits", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    if let Some(rl) = body.rate_limit {
        if rl < 1 || rl > 10000 {
            return Err(err(StatusCode::BAD_REQUEST, "Rate limit must be between 1 and 10000"));
        }
    }
    let max_upload = body.max_upload_mb.unwrap_or(site.max_upload_mb);
    if max_upload < 1 || max_upload > 10240 {
        return Err(err(StatusCode::BAD_REQUEST, "Max upload must be between 1 and 10240 MB"));
    }
    let php_memory = body.php_memory_mb.unwrap_or(site.php_memory_mb);
    if php_memory < 32 || php_memory > 4096 {
        return Err(err(StatusCode::BAD_REQUEST, "PHP memory must be between 32 and 4096 MB"));
    }
    let php_workers = body.php_max_workers.unwrap_or(site.php_max_workers);
    if php_workers < 1 || php_workers > 100 {
        return Err(err(StatusCode::BAD_REQUEST, "PHP workers must be between 1 and 100"));
    }

    if let Some(ref custom) = body.custom_nginx {
        if !custom.is_empty() {
            super::is_safe_nginx_config(custom)
                .map_err(|e| err(StatusCode::BAD_REQUEST, e))?;
        }
    }

    let custom_nginx = body.custom_nginx.as_deref();
    let updated: Site = sqlx::query_as(
        "UPDATE sites SET rate_limit = $1, max_upload_mb = $2, php_memory_mb = $3, php_max_workers = $4, \
         custom_nginx = $5, updated_at = NOW() WHERE id = $6 RETURNING *",
    )
    .bind(body.rate_limit)
    .bind(max_upload)
    .bind(php_memory)
    .bind(php_workers)
    .bind(custom_nginx)
    .bind(id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("update limits", e))?;

    let mut agent_body = serde_json::json!({
        "runtime": site.runtime,
        "rate_limit": body.rate_limit,
        "max_upload_mb": max_upload,
        "php_memory_mb": php_memory,
        "php_max_workers": php_workers,
    });
    if let Some(ref custom) = body.custom_nginx {
        agent_body["custom_nginx"] = serde_json::json!(custom);
    } else if let Some(ref existing) = site.custom_nginx {
        agent_body["custom_nginx"] = serde_json::json!(existing);
    }
    if let Some(ref preset) = site.php_preset {
        agent_body["php_preset"] = serde_json::json!(preset);
    }

    if let Some(port) = site.proxy_port {
        agent_body["proxy_port"] = serde_json::json!(port);
    }
    if let Some(ref php) = site.php_version {
        agent_body["php_socket"] = serde_json::json!(format!("unix:/run/php/php{php}-fpm.sock"));
    }
    if site.ssl_enabled {
        agent_body["ssl"] = serde_json::json!(true);
        if let Some(ref cert) = site.ssl_cert_path {
            agent_body["ssl_cert"] = serde_json::json!(cert);
        }
        if let Some(ref key) = site.ssl_key_path {
            agent_body["ssl_key"] = serde_json::json!(key);
        }
    }

    let agent_path = format!("/nginx/sites/{}", site.domain);
    agent
        .put(&agent_path, agent_body)
        .await
        .map_err(|e| agent_error("Resource limits", e))?;

    tracing::info!("Resource limits updated for {}", site.domain);
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "site.limits",
        Some("site"), Some(&site.domain), None, None,
    ).await;

    Ok(Json(updated))
}

/// DELETE /api/sites/{id} — Delete a site and all associated resources.
pub async fn remove(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: Site = sqlx::query_as(
        "SELECT * FROM sites WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("remove sites", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    // Remove database containers before CASCADE deletes the records
    let databases: Vec<(String,)> = sqlx::query_as(
        "SELECT container_id FROM databases WHERE site_id = $1 AND container_id IS NOT NULL AND container_id != ''",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    for (container_id,) in &databases {
        if let Err(e) = agent.delete(&format!("/databases/{container_id}")).await {
            tracing::warn!("Failed to remove database container {container_id}: {e}");
        }
    }

    // GAP 50: Remove firewall rule for proxy port on site deletion
    if let Some(port) = site.proxy_port {
        // Get current firewall rules and find the matching rule number to delete
        if let Ok(fw_status) = agent.get("/security/firewall").await {
            if let Some(rules) = fw_status.get("rules").and_then(|v| v.as_array()) {
                for rule in rules {
                    let rule_port = rule.get("port").and_then(|v| v.as_str()).unwrap_or("");
                    let rule_action = rule.get("action").and_then(|v| v.as_str()).unwrap_or("");
                    if rule_port == format!("{port}/tcp") && rule_action.to_lowercase().contains("deny") {
                        if let Some(num) = rule.get("number").and_then(|v| v.as_u64()) {
                            let _ = agent.delete(&format!("/security/firewall/rules/{num}")).await;
                            tracing::info!("Auto-firewall: removed deny rule for port {port}");
                        }
                    }
                }
            }
        }
    }

    // Remove nginx config + SSL + PHP pool + site files + logs
    let agent_path = format!("/nginx/sites/{}", site.domain);
    agent.delete(&agent_path).await
        .map_err(|e| agent_error("Site removal", e))?;

    // Remove cron entries from system crontab before CASCADE deletes DB records
    let crons: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM crons WHERE site_id = $1",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    for (cron_id,) in &crons {
        if let Err(e) = agent.delete(&format!("/crons/remove/{cron_id}")).await {
            tracing::warn!("Failed to remove crontab entry {cron_id}: {e}");
        }
    }

    // Delete monitors linked to this site (FK is SET NULL, not CASCADE)
    sqlx::query("DELETE FROM monitors WHERE site_id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .ok();

    // Clean up status page components matching this domain
    sqlx::query("DELETE FROM status_page_components WHERE name = $1")
        .bind(&site.domain)
        .execute(&state.db)
        .await
        .ok();

    // Pre-delete backup: snapshot site files before permanent deletion (best-effort)
    let _ = agent.post(
        &format!("/backups/{}/create", site.domain),
        Some(serde_json::json!({"reason": "pre-delete"})),
    ).await;

    // Delete from DB (CASCADE removes databases, backups, crons, etc.)
    sqlx::query("DELETE FROM sites WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("remove sites", e))?;

    // Decrement reseller site counter
    let _ = sqlx::query(
        "UPDATE reseller_profiles SET used_sites = GREATEST(used_sites - 1, 0), updated_at = NOW() \
         WHERE user_id = (SELECT reseller_id FROM users WHERE id = $1 AND reseller_id IS NOT NULL)"
    ).bind(claims.sub).execute(&state.db).await;

    tracing::info!("Site deleted: {}", site.domain);
    let ip = crate::routes::client_ip(&headers);
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "site.delete",
        Some("site"), Some(&site.domain), None, ip.as_deref(),
    ).await;

    // Panel notification
    notifications::notify_panel(&state.db, Some(claims.sub), &format!("Site deleted: {}", site.domain), "Site and all associated resources have been removed", "info", "site", None).await;

    fire_event(&state.db, "site.deleted", serde_json::json!({
        "domain": &site.domain,
    }));

    // Auto-cleanup DNS record (best-effort, don't fail the delete)
    {
        let dns_domain = site.domain.clone();
        let dns_db = state.db.clone();
        let dns_user = claims.sub;
        tokio::spawn(async move {
            // Extract parent domain
            let parts: Vec<&str> = dns_domain.splitn(3, '.').collect();
            let parent = if parts.len() >= 3 {
                format!("{}.{}", parts[parts.len() - 2], parts[parts.len() - 1])
            } else {
                dns_domain.clone()
            };

            let zone: Option<(String, Option<String>, Option<String>, Option<String>)> = match sqlx::query_as(
                "SELECT provider, cf_zone_id, cf_api_token, cf_api_email FROM dns_zones WHERE domain = $1 AND user_id = $2"
            ).bind(&parent).bind(dns_user).fetch_optional(&dns_db).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("DB error fetching DNS zone for auto-cleanup on site delete: {e}");
                    None
                }
            };

            if let Some((provider, cf_zone_id, cf_api_token, cf_api_email)) = zone {
                let server_ip = crate::helpers::detect_public_ip().await;

                if provider == "cloudflare" {
                    if let (Some(zid), Some(tok)) = (cf_zone_id, cf_api_token) {
                        let client = reqwest::Client::new();
                        let headers = crate::helpers::cf_headers(&tok, cf_api_email.as_deref());
                        // Find the A record for this domain
                        if let Ok(resp) = client.get(&format!("https://api.cloudflare.com/client/v4/zones/{zid}/dns_records?type=A&name={dns_domain}"))
                            .headers(headers.clone()).send().await {
                            if let Ok(data) = resp.json::<serde_json::Value>().await {
                                if let Some(records) = data.get("result").and_then(|r| r.as_array()) {
                                    for record in records {
                                        if let (Some(rid), Some(content)) = (record.get("id").and_then(|v| v.as_str()), record.get("content").and_then(|v| v.as_str())) {
                                            if content == server_ip {
                                                let _ = client.delete(&format!("https://api.cloudflare.com/client/v4/zones/{zid}/dns_records/{rid}"))
                                                    .headers(headers.clone()).send().await;
                                                tracing::info!("Auto-DNS cleanup: deleted A record {dns_domain}");
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else if provider == "powerdns" {
                    let pdns: Vec<(String, String)> = sqlx::query_as(
                        "SELECT key, value FROM settings WHERE key IN ('pdns_api_url', 'pdns_api_key')"
                    ).fetch_all(&dns_db).await.unwrap_or_default();
                    let purl = pdns.iter().find(|(k,_)| k == "pdns_api_url").map(|(_,v)| v.clone());
                    let pkey_enc = pdns.iter().find(|(k,_)| k == "pdns_api_key").map(|(_,v)| v.clone());
                    if let (Some(url), Some(key_enc)) = (purl, pkey_enc) {
                        let key = crate::services::secrets_crypto::decrypt_credential_from_env(&key_enc);
                        let zfqdn = if parent.ends_with('.') { parent } else { format!("{parent}.") };
                        let _ = reqwest::Client::new()
                            .patch(&format!("{url}/api/v1/servers/localhost/zones/{zfqdn}"))
                            .header("X-API-Key", &key)
                            .json(&serde_json::json!({"rrsets":[{"name":format!("{dns_domain}."),"type":"A","ttl":300,"changetype":"DELETE","records":[]}]}))
                            .send().await;
                        tracing::info!("Auto-DNS cleanup (PowerDNS): deleted A record {dns_domain}");
                    }
                }
            }
        });
    }

    Ok(Json(serde_json::json!({ "ok": true, "domain": site.domain })))
}

// ──────────────────────────────────────────────────────────────
// Redirect Rules (proxy to agent)
// ──────────────────────────────────────────────────────────────

/// Helper: get site domain from site ID + user ID.
async fn site_domain(state: &AppState, site_id: Uuid, user_id: Uuid) -> Result<String, ApiError> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT domain FROM sites WHERE id = $1 AND user_id = $2")
            .bind(site_id)
            .bind(user_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("remove sites", e))?;

    row.map(|(d,)| d)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))
}

/// GET /api/sites/{id}/redirects — List redirects.
pub async fn list_redirects(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = site_domain(&state, id, claims.sub).await?;
    let result = agent
        .get(&format!("/nginx/redirects/{domain}"))
        .await
        .map_err(|e| agent_error("Redirects", e))?;
    Ok(Json(result))
}

#[derive(serde::Deserialize)]
pub struct AddRedirectBody {
    pub source: String,
    pub target: String,
    #[serde(default = "default_301")]
    pub redirect_type: String,
}

fn default_301() -> String {
    "301".to_string()
}

/// POST /api/sites/{id}/redirects — Add a redirect.
pub async fn add_redirect(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
    Json(body): Json<AddRedirectBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Validate source: must start with / and contain no shell metacharacters
    if !body.source.starts_with('/') || body.source.contains(|c: char| matches!(c, ';' | '|' | '&' | '$' | '`' | '\'' | '"' | '\\' | '\n' | '\r' | '\0')) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid redirect source: must start with / and contain no shell metacharacters"));
    }
    // Validate target: must be a valid URL (http/https) or a valid path (starts with /)
    if !(body.target.starts_with("http://") || body.target.starts_with("https://") || body.target.starts_with('/')) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid redirect target: must be a URL (http/https) or path (starts with /)"));
    }
    if body.target.contains(|c: char| matches!(c, ';' | '|' | '&' | '$' | '`' | '\'' | '"' | '\\' | '\n' | '\r' | '\0')) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid redirect target: contains shell metacharacters"));
    }

    let domain = site_domain(&state, id, claims.sub).await?;
    let result = agent
        .post(
            "/nginx/redirects/add",
            Some(serde_json::json!({
                "domain": domain,
                "source": body.source,
                "target": body.target,
                "redirect_type": body.redirect_type,
            })),
        )
        .await
        .map_err(|e| agent_error("Redirects", e))?;
    Ok(Json(result))
}

/// POST /api/sites/{id}/redirects/remove — Remove a redirect.
pub async fn remove_redirect(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = site_domain(&state, id, claims.sub).await?;
    let result = agent
        .post(
            &format!("/nginx/redirects/{domain}/remove"),
            Some(body),
        )
        .await
        .map_err(|e| agent_error("Redirects", e))?;
    Ok(Json(result))
}

// ──────────────────────────────────────────────────────────────
// Password Protection (proxy to agent)
// ──────────────────────────────────────────────────────────────

/// GET /api/sites/{id}/password-protect — List protected paths.
pub async fn list_protected(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = site_domain(&state, id, claims.sub).await?;
    let result = agent
        .get(&format!("/nginx/password-protect/{domain}"))
        .await
        .map_err(|e| agent_error("Password protection", e))?;
    Ok(Json(result))
}

#[derive(serde::Deserialize)]
pub struct PasswordProtectBody {
    pub path: String,
    pub username: String,
    pub password: String,
}

/// POST /api/sites/{id}/password-protect — Enable password protection.
pub async fn add_password_protect(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
    Json(body): Json<PasswordProtectBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Validate path: no directory traversal, no shell metacharacters
    if body.path.contains("..") || body.path.contains(|c: char| matches!(c, ';' | '|' | '&' | '$' | '`' | '\'' | '"' | '\\' | '\n' | '\r' | '\0')) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid path: must not contain '..' or shell metacharacters"));
    }
    // Validate username: alphanumeric + underscore/hyphen only
    if body.username.is_empty() || !body.username.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid username: must be alphanumeric (underscores and hyphens allowed)"));
    }

    let domain = site_domain(&state, id, claims.sub).await?;
    let result = agent
        .post(
            "/nginx/password-protect",
            Some(serde_json::json!({
                "domain": domain,
                "path": body.path,
                "username": body.username,
                "password": body.password,
            })),
        )
        .await
        .map_err(|e| agent_error("Password protection", e))?;
    Ok(Json(result))
}

/// POST /api/sites/{id}/password-protect/remove — Remove password protection.
pub async fn remove_password_protect(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = site_domain(&state, id, claims.sub).await?;
    let result = agent
        .post(
            &format!("/nginx/password-protect/{domain}/remove"),
            Some(body),
        )
        .await
        .map_err(|e| agent_error("Password protection", e))?;
    Ok(Json(result))
}

// ──────────────────────────────────────────────────────────────
// Domain Aliases (proxy to agent)
// ──────────────────────────────────────────────────────────────

/// GET /api/sites/{id}/aliases — List domain aliases.
pub async fn list_aliases(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = site_domain(&state, id, claims.sub).await?;
    let result = agent
        .get(&format!("/nginx/aliases/{domain}"))
        .await
        .map_err(|e| agent_error("Domain aliases", e))?;
    Ok(Json(result))
}

#[derive(serde::Deserialize)]
pub struct AddAliasBody {
    pub alias: String,
}

/// POST /api/sites/{id}/aliases — Add a domain alias.
pub async fn add_alias(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
    Json(body): Json<AddAliasBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !is_valid_domain(&body.alias) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid alias: must be a valid domain name"));
    }

    let domain = site_domain(&state, id, claims.sub).await?;
    let result = agent
        .post(
            "/nginx/aliases/add",
            Some(serde_json::json!({
                "domain": domain,
                "alias": body.alias,
            })),
        )
        .await
        .map_err(|e| agent_error("Domain aliases", e))?;
    Ok(Json(result))
}

/// POST /api/sites/{id}/aliases/remove — Remove a domain alias.
pub async fn remove_alias(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = site_domain(&state, id, claims.sub).await?;
    let result = agent
        .post(
            &format!("/nginx/aliases/{domain}/remove"),
            Some(body),
        )
        .await
        .map_err(|e| agent_error("Domain aliases", e))?;
    Ok(Json(result))
}

// ──────────────────────────────────────────────────────────────
// Access Logs, Traffic Stats, PHP Errors, Health Check
// ──────────────────────────────────────────────────────────────

/// GET /api/sites/{id}/access-logs — View nginx access/error logs for a site.
pub async fn access_logs(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = site_domain(&state, id, claims.sub).await?;
    let lines = params.get("lines").unwrap_or(&"200".to_string()).clone();
    let log_type = params.get("type").unwrap_or(&"access".to_string()).clone();
    let path = format!(
        "/nginx/site-logs/{}?lines={}&log_type={}",
        domain, lines, log_type
    );
    let result = agent
        .get(&path)
        .await
        .map_err(|e| agent_error("Site logs", e))?;
    Ok(Json(result))
}

/// GET /api/sites/{id}/stats — Basic traffic stats from access log.
pub async fn site_stats(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = site_domain(&state, id, claims.sub).await?;
    let result = agent
        .get(&format!("/nginx/site-stats/{domain}"))
        .await
        .map_err(|e| agent_error("Site stats", e))?;
    Ok(Json(result))
}

/// GET /api/sites/{id}/php-errors — View PHP-FPM error log for a site.
pub async fn php_errors(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = site_domain(&state, id, claims.sub).await?;
    let result = agent
        .get(&format!("/nginx/php-errors/{domain}"))
        .await
        .map_err(|e| agent_error("PHP errors", e))?;
    Ok(Json(result))
}

/// GET /api/sites/{id}/health — Check if site is responding.
pub async fn health_check(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = site_domain(&state, id, claims.sub).await?;

    // Check if site has SSL
    let ssl: Option<(bool,)> = sqlx::query_as("SELECT ssl_enabled FROM sites WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
    let ssl_enabled = ssl.map(|(s,)| s).unwrap_or(false);

    let url = if ssl_enabled {
        format!("https://{domain}")
    } else {
        format!("http://{domain}")
    };

    let start = std::time::Instant::now();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .danger_accept_invalid_certs(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_default();

    match client.get(&url).send().await {
        Ok(resp) => {
            let elapsed = start.elapsed().as_millis() as u32;
            let status = resp.status().as_u16();
            Ok(Json(serde_json::json!({
                "healthy": status < 500,
                "status": status,
                "response_time_ms": elapsed,
                "url": url,
            })))
        }
        Err(e) => {
            let elapsed = start.elapsed().as_millis() as u32;
            Ok(Json(serde_json::json!({
                "healthy": false,
                "status": 0,
                "response_time_ms": elapsed,
                "error": format!("{e}"),
                "url": url,
            })))
        }
    }
}

// ──────────────────────────────────────────────────────────────
// Composite Health Summary
// ──────────────────────────────────────────────────────────────

/// GET /api/sites/{id}/health-summary — Composite site health score.
pub async fn health_summary(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let db = &state.db;

    // Verify ownership
    let site: Option<(String, bool, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT domain, ssl_enabled, ssl_expiry FROM sites WHERE id = $1 AND user_id = $2"
    ).bind(id).bind(claims.sub).fetch_optional(db).await
        .map_err(|e| internal_error("health summary", e))?;

    let (domain, ssl_enabled, ssl_expiry) = site.ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    let now = chrono::Utc::now();

    // SSL status
    let ssl_days_until_expiry = ssl_expiry.map(|exp| (exp - now).num_days());

    // Backup freshness
    let last_backup: Option<(chrono::DateTime<chrono::Utc>,)> = sqlx::query_as(
        "SELECT created_at FROM backups WHERE site_id = $1 ORDER BY created_at DESC LIMIT 1"
    ).bind(id).fetch_optional(db).await
        .map_err(|e| internal_error("health summary backup check", e))?;
    let backup_hours_since = last_backup.map(|(t,)| (now - t).num_hours());

    // Uptime: latest monitor status + response time
    let monitor: Option<(String, Option<i32>, bool)> = sqlx::query_as(
        "SELECT status, last_response_ms, enabled FROM monitors WHERE site_id = $1 ORDER BY created_at DESC LIMIT 1"
    ).bind(id).fetch_optional(db).await
        .map_err(|e| internal_error("health summary monitor check", e))?;
    let (monitor_status, response_time, monitor_enabled) = monitor
        .map(|(s, r, e)| (Some(s), r, e))
        .unwrap_or((None, None, false));

    // Compute score 0-100
    let mut score: i32 = 100;

    // No SSL: -25
    if !ssl_enabled {
        score -= 25;
    } else if let Some(days) = ssl_days_until_expiry {
        // SSL expiring in <7 days: -15, <30 days: -5
        if days < 0 {
            score -= 25; // expired
        } else if days < 7 {
            score -= 15;
        } else if days < 30 {
            score -= 5;
        }
    }

    // Stale backup: no backup in 48h: -20, no backup at all: -30
    match backup_hours_since {
        None => score -= 30,
        Some(h) if h > 48 => score -= 20,
        Some(h) if h > 24 => score -= 10,
        _ => {}
    }

    // Monitor down: -20, slow response (>2s): -10
    if let Some(ref status) = monitor_status {
        if status == "down" {
            score -= 20;
        }
    }
    if let Some(rt) = response_time {
        if rt > 5000 {
            score -= 15;
        } else if rt > 2000 {
            score -= 10;
        } else if rt > 1000 {
            score -= 5;
        }
    }

    // No monitor at all or disabled: -5
    if monitor_status.is_none() || !monitor_enabled {
        score -= 5;
    }

    score = score.max(0);

    Ok(Json(serde_json::json!({
        "domain": domain,
        "ssl_status": {
            "enabled": ssl_enabled,
            "days_until_expiry": ssl_days_until_expiry,
        },
        "backup_freshness": {
            "last_backup": last_backup.map(|(t,)| t),
            "hours_since": backup_hours_since,
        },
        "uptime": {
            "status": monitor_status,
            "response_time_ms": response_time,
            "monitor_enabled": monitor_enabled,
        },
        "score": score,
    })))
}

// ──────────────────────────────────────────────────────────────
// Site Cloning
// ──────────────────────────────────────────────────────────────

/// POST /api/sites/{id}/clone — Clone site to a new domain.
pub async fn clone_site(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let target_domain = body.get("domain").and_then(|v| v.as_str())
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Target domain required"))?;

    if !is_valid_domain(target_domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid target domain format"));
    }

    // Get source site
    let source: Option<Site> = sqlx::query_as("SELECT * FROM sites WHERE id = $1 AND user_id = $2")
        .bind(id).bind(claims.sub).fetch_optional(&state.db).await
        .map_err(|e| internal_error("clone site", e))?;
    let source = source.ok_or_else(|| err(StatusCode::NOT_FOUND, "Source site not found"))?;

    // Create new site record
    let new_site: Site = sqlx::query_as(
        "INSERT INTO sites (user_id, server_id, domain, runtime, status, php_version, root_path, rate_limit, max_upload_mb, php_memory_mb, php_max_workers, php_preset, app_command) \
         VALUES ($1, $2, $3, $4, 'active', $5, $6, $7, $8, $9, $10, $11, $12) RETURNING *"
    )
    .bind(claims.sub)
    .bind(server_id)
    .bind(target_domain)
    .bind(&source.runtime)
    .bind(&source.php_version)
    .bind(&source.root_path)
    .bind(source.rate_limit)
    .bind(source.max_upload_mb)
    .bind(source.php_memory_mb)
    .bind(source.php_max_workers)
    .bind(&source.php_preset)
    .bind(&source.app_command)
    .fetch_one(&state.db).await
    .map_err(|e| {
        if e.to_string().contains("duplicate") || e.to_string().contains("unique") {
            err(StatusCode::CONFLICT, "A site with this domain already exists")
        } else {
            internal_error("clone site", e)
        }
    })?;

    // Clone files via agent
    agent.post("/nginx/clone-site", Some(serde_json::json!({
        "source_domain": source.domain,
        "target_domain": target_domain,
    }))).await.map_err(|e| agent_error("Clone", e))?;

    // Set up nginx for new site
    let mut nginx_body = serde_json::json!({
        "runtime": source.runtime,
        "root": "/var/www",
    });
    if let Some(port) = source.proxy_port {
        nginx_body["proxy_port"] = serde_json::json!(port);
    }
    if let Some(ref php) = source.php_version {
        nginx_body["php_socket"] = serde_json::json!(format!("unix:/run/php/php{php}-fpm.sock"));
    }
    if let Some(ref preset) = source.php_preset {
        nginx_body["php_preset"] = serde_json::json!(preset);
    }

    agent.put(&format!("/nginx/sites/{target_domain}"), nginx_body).await
        .map_err(|e| agent_error("Nginx config", e))?;

    activity::log_activity(&state.db, claims.sub, &claims.email, "site.clone",
        Some("site"), Some(target_domain), Some(&source.domain), None).await;

    fire_event(&state.db, "site.created", serde_json::json!({
        "site_id": new_site.id, "domain": target_domain, "runtime": &source.runtime, "cloned_from": &source.domain,
    }));

    // Auto-create backup schedule for cloned site (daily 3 AM, 7 retention)
    {
        let backup_db = state.db.clone();
        let backup_site_id = new_site.id;
        tokio::spawn(async move {
            let _ = sqlx::query(
                "INSERT INTO backup_schedules (site_id, schedule, retention_count, enabled) \
                 VALUES ($1, '0 3 * * *', 7, true) ON CONFLICT (site_id) DO NOTHING"
            ).bind(backup_site_id).execute(&backup_db).await;
            tracing::info!("Auto-backup: created daily schedule for cloned site");
        });
    }

    // Auto-create secrets vault for the cloned site
    {
        let vault_db = state.db.clone();
        let vault_site_id = new_site.id;
        let vault_user_id = claims.sub;
        let vault_domain = target_domain.to_string();
        tokio::spawn(async move {
            let _ = sqlx::query(
                "INSERT INTO secret_vaults (user_id, name, description, site_id) \
                 VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING"
            )
            .bind(vault_user_id)
            .bind(format!("{vault_domain} secrets"))
            .bind(format!("Auto-created vault for {vault_domain}"))
            .bind(vault_site_id)
            .execute(&vault_db).await;
            tracing::info!("Auto-vault: created for cloned site {vault_domain}");
        });
    }

    // Auto-create status page component if status page is enabled
    {
        let sp_db = state.db.clone();
        let sp_user_id = claims.sub;
        let sp_domain = target_domain.to_string();
        tokio::spawn(async move {
            let enabled: Option<(bool,)> = match sqlx::query_as(
                "SELECT enabled FROM status_page_config WHERE user_id = $1"
            ).bind(sp_user_id).fetch_optional(&sp_db).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("DB error checking status page config for cloned site auto-component: {e}");
                    None
                }
            };

            if enabled.map(|(e,)| e).unwrap_or(false) {
                let _ = sqlx::query(
                    "INSERT INTO status_page_components (user_id, name, description, group_name) \
                     VALUES ($1, $2, $3, 'Sites')"
                )
                .bind(sp_user_id).bind(&sp_domain)
                .bind(format!("Auto-created for {sp_domain}"))
                .execute(&sp_db).await;
                tracing::info!("Auto-component: created status page component for cloned site {sp_domain}");
            }
        });
    }

    Ok((StatusCode::CREATED, Json(serde_json::json!({ "ok": true, "site_id": new_site.id, "domain": target_domain }))))
}

// ──────────────────────────────────────────────────────────────
// Custom SSL Upload
// ──────────────────────────────────────────────────────────────

/// POST /api/sites/{id}/ssl/upload — Upload custom SSL certificate.
pub async fn upload_ssl(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = site_domain(&state, id, claims.sub).await?;

    let mut agent_body = body.clone();
    agent_body["domain"] = serde_json::json!(domain);

    agent.post("/ssl/upload", Some(agent_body)).await
        .map_err(|e| agent_error("SSL upload", e))?;

    // Update DB
    sqlx::query("UPDATE sites SET ssl_enabled = true, updated_at = NOW() WHERE id = $1")
        .bind(id).execute(&state.db).await.ok();

    activity::log_activity(&state.db, claims.sub, &claims.email, "ssl.upload",
        Some("site"), Some(&domain), None, None).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

// ──────────────────────────────────────────────────────────────
// PHP Extensions Manager
// ──────────────────────────────────────────────────────────────

/// GET /api/php/extensions/{version} — List PHP extensions.
pub async fn php_extensions(
    State(_state): State<AppState>,
    AuthUser(_claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(version): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent.get(&format!("/php/extensions/{version}")).await
        .map_err(|e| agent_error("PHP extensions", e))?;
    Ok(Json(result))
}

/// POST /api/php/extensions/install — Install a PHP extension.
pub async fn install_php_extension(
    State(_state): State<AppState>,
    AuthUser(_claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    agent.post("/php/extensions/install", Some(body)).await
        .map_err(|e| agent_error("PHP extension", e))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ──────────────────────────────────────────────────────────────
// Environment Variables
// ──────────────────────────────────────────────────────────────

/// GET /api/sites/{id}/env — Read environment variables.
pub async fn get_env_vars(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = site_domain(&state, id, claims.sub).await?;
    let result = agent.get(&format!("/nginx/env/{domain}")).await
        .map_err(|e| agent_error("Env vars", e))?;
    Ok(Json(result))
}

/// PUT /api/sites/{id}/env — Write environment variables.
pub async fn set_env_vars(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = site_domain(&state, id, claims.sub).await?;
    agent.put(&format!("/nginx/env/{domain}"), body).await
        .map_err(|e| agent_error("Env vars", e))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// PUT /api/sites/{id}/domain — Rename a site's domain.
pub async fn rename_domain(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Get current site
    let site: crate::models::Site = sqlx::query_as(
        "SELECT * FROM sites WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("rename domain", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    let new_domain = body.get("new_domain")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    if !is_valid_domain(&new_domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }

    if new_domain == site.domain {
        return Err(err(StatusCode::BAD_REQUEST, "New domain is the same as current domain"));
    }

    // Check uniqueness
    let existing: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM sites WHERE domain = $1")
            .bind(&new_domain)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("rename domain", e))?;

    if existing.is_some() {
        return Err(err(StatusCode::CONFLICT, "Domain already exists"));
    }

    let git_conflict: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM git_deploys WHERE domain = $1"
    )
    .bind(&new_domain)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("rename domain", e))?;

    if git_conflict.is_some() {
        return Err(err(StatusCode::CONFLICT, "Domain already in use by a git deployment"));
    }

    // Call agent to rename nginx config, site dir, logs
    let old_domain = site.domain.clone();
    agent.post(
        &format!("/nginx/sites/{}/rename", old_domain),
        Some(serde_json::json!({ "new_domain": new_domain })),
    ).await.map_err(|e| agent_error("Domain rename", e))?;

    // Update site record
    sqlx::query("UPDATE sites SET domain = $1, updated_at = NOW() WHERE id = $2")
        .bind(&new_domain)
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("rename domain", e))?;

    // Update monitors linked to this site
    let new_url = format!("https://{new_domain}");
    sqlx::query("UPDATE monitors SET name = $1, url = $2 WHERE site_id = $3")
        .bind(&new_domain)
        .bind(&new_url)
        .bind(id)
        .execute(&state.db)
        .await
        .ok();

    // Update status page components
    sqlx::query("UPDATE status_page_components SET name = $1 WHERE name = $2")
        .bind(&new_domain)
        .bind(&old_domain)
        .execute(&state.db)
        .await
        .ok();

    tracing::info!("Domain renamed: {old_domain} → {new_domain}");
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "site.rename_domain",
        Some("site"), Some(&new_domain), Some(&old_domain), None,
    ).await;

    notifications::notify_panel(&state.db, Some(claims.sub),
        &format!("Domain renamed: {old_domain} → {new_domain}"),
        "Site domain has been updated", "info", "site", None,
    ).await;

    Ok(Json(serde_json::json!({
        "ok": true,
        "old_domain": old_domain,
        "new_domain": new_domain,
    })))
}

/// PUT /api/sites/{id}/toggle — Enable or disable a site without deleting it.
pub async fn toggle_enabled(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: crate::models::Site = sqlx::query_as(
        "SELECT * FROM sites WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("toggle site", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    let enabled = body.get("enabled")
        .and_then(|v| v.as_bool())
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing 'enabled' boolean field"))?;

    if enabled == site.enabled {
        return Ok(Json(serde_json::json!({
            "ok": true,
            "enabled": enabled,
            "message": if enabled { "Site is already enabled" } else { "Site is already disabled" },
        })));
    }

    // Call agent to enable/disable the nginx config
    let action = if enabled { "enable" } else { "disable" };
    agent.post(
        &format!("/nginx/sites/{}/{action}", site.domain),
        None,
    ).await.map_err(|e| agent_error("Toggle site", e))?;

    // Update DB
    sqlx::query("UPDATE sites SET enabled = $1, updated_at = NOW() WHERE id = $2")
        .bind(enabled)
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("toggle site", e))?;

    let action_label = if enabled { "enabled" } else { "disabled" };
    tracing::info!("Site {} {action_label}", site.domain);
    activity::log_activity(
        &state.db, claims.sub, &claims.email,
        &format!("site.{action_label}"),
        Some("site"), Some(&site.domain), None, None,
    ).await;

    notifications::notify_panel(&state.db, Some(claims.sub),
        &format!("Site {action_label}: {}", site.domain),
        &format!("Site has been {action_label}"), "info", "site", None,
    ).await;

    Ok(Json(serde_json::json!({
        "ok": true,
        "enabled": enabled,
    })))
}
