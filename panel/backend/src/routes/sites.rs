use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
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
use crate::error::{err, agent_error, paginate, ApiError};
use crate::models::Site;
use crate::routes::is_valid_domain;
use crate::services::activity;
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
    Query(params): Query<ListQuery>,
) -> Result<Json<Vec<Site>>, ApiError> {
    let (limit, offset) = paginate(params.limit, params.offset);

    let sites: Vec<Site> = sqlx::query_as(
        "SELECT * FROM sites WHERE user_id = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3",
    )
    .bind(claims.sub)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(sites))
}

/// POST /api/sites — Create a new site.
pub async fn create(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<CreateSiteRequest>,
) -> Result<(StatusCode, Json<Site>), ApiError> {
    // Validate domain format
    if !is_valid_domain(&body.domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
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
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if existing.is_some() {
        return Err(err(StatusCode::CONFLICT, "Domain already exists"));
    }

    // Insert site with status "creating" inside a transaction
    let mut tx = state.db.begin().await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Transaction start failed: {e}")))?;

    // Auto-allocate port for node/python runtimes
    let effective_proxy_port = if (runtime == "node" || runtime == "python") && body.proxy_port.is_none() {
        // Find first available port in 4000-4999 range
        let row: Option<(i32,)> = sqlx::query_as(
            "SELECT s.port FROM generate_series(5000, 5999) AS s(port) \
             WHERE s.port NOT IN (SELECT proxy_port FROM sites WHERE proxy_port IS NOT NULL) \
             LIMIT 1"
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
        row.map(|(p,)| p)
    } else {
        body.proxy_port
    };

    let site: Site = sqlx::query_as(
        "INSERT INTO sites (user_id, domain, runtime, status, proxy_port, php_version, php_preset, app_command) \
         VALUES ($1, $2, $3, 'creating', $4, $5, $6, $7) RETURNING *",
    )
    .bind(claims.sub)
    .bind(&body.domain)
    .bind(runtime)
    .bind(effective_proxy_port)
    .bind(&body.php_version)
    .bind(body.php_preset.as_deref().unwrap_or("generic"))
    .bind(&body.app_command)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    // Create provisioning log channel
    let (broadcast_tx, _) = broadcast::channel::<ProvisionStep>(64);
    {
        let mut logs = state.provision_logs.lock().unwrap();
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
    match state.agent.put(&agent_path, agent_body).await {
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
                .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

            let updated: Site = sqlx::query_as("SELECT * FROM sites WHERE id = $1")
                .bind(site.id)
                .fetch_one(&state.db)
                .await
                .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

            tracing::info!("Site created: {} ({})", body.domain, runtime);
            activity::log_activity(
                &state.db, claims.sub, &claims.email, "site.create",
                Some("site"), Some(&body.domain), Some(runtime), None,
            ).await;

            // Auto-create uptime monitor (linked to site for cascade cleanup)
            {
                let monitor_db = state.db.clone();
                let monitor_domain = body.domain.clone();
                let monitor_user = claims.sub;
                let monitor_site_id = site.id;
                tokio::spawn(async move {
                    let url = format!("https://{monitor_domain}");
                    let _ = sqlx::query(
                        "INSERT INTO monitors (user_id, site_id, url, name, check_interval, alert_email) \
                         VALUES ($1, $2, $3, $4, 60, true) ON CONFLICT DO NOTHING"
                    )
                    .bind(monitor_user)
                    .bind(monitor_site_id)
                    .bind(&url)
                    .bind(&monitor_domain)
                    .execute(&monitor_db)
                    .await;
                    tracing::info!("Auto-monitor created for {monitor_domain}");
                });
            }

            // Auto-create backup schedule for first site
            {
                let backup_db = state.db.clone();
                let backup_site_id = site.id;
                let backup_user_id = claims.sub;
                tokio::spawn(async move {
                    // Check if user has any other sites
                    let site_count: Option<(i64,)> = sqlx::query_as(
                        "SELECT COUNT(*) FROM sites WHERE user_id = $1"
                    ).bind(backup_user_id).fetch_optional(&backup_db).await.ok().flatten();

                    if site_count.map(|(c,)| c).unwrap_or(0) <= 1 {
                        // First site — create default backup schedule (daily 3 AM, 7 retention)
                        let _ = sqlx::query(
                            "INSERT INTO backup_schedules (site_id, schedule, retention_count, enabled) \
                             VALUES ($1, '0 3 * * *', 7, true) ON CONFLICT (site_id) DO NOTHING"
                        ).bind(backup_site_id).execute(&backup_db).await;
                        tracing::info!("Auto-backup: created daily schedule for first site");
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

                    let zone: Option<(String, Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
                        "SELECT provider, cf_zone_id, cf_api_token, cf_api_email FROM dns_zones WHERE domain = $1 AND user_id = $2"
                    ).bind(&parent).bind(dns_user_id).fetch_optional(&dns_db).await.ok().flatten();

                    if let Some((provider, cf_zone_id, cf_api_token, cf_api_email)) = zone {
                        // Detect server's public IP (try external service first, fallback to local detection)
                        let server_ip = match reqwest::Client::new()
                            .get("https://api.ipify.org")
                            .timeout(std::time::Duration::from_secs(5))
                            .send().await
                        {
                            Ok(resp) => resp.text().await.unwrap_or_default().trim().to_string(),
                            Err(_) => {
                                std::net::UdpSocket::bind("0.0.0.0:0")
                                    .and_then(|s| { s.connect("8.8.8.8:53")?; s.local_addr() })
                                    .map(|a| a.ip().to_string()).unwrap_or_default()
                            }
                        };

                        if provider == "cloudflare" {
                            if let (Some(zid), Some(tok)) = (cf_zone_id, cf_api_token) {
                                let client = reqwest::Client::new();
                                let mut headers = reqwest::header::HeaderMap::new();
                                if let Some(em) = cf_api_email {
                                    headers.insert("X-Auth-Email", em.parse().unwrap_or_else(|_| "".parse().unwrap()));
                                    headers.insert("X-Auth-Key", tok.parse().unwrap_or_else(|_| "".parse().unwrap()));
                                } else {
                                    headers.insert("Authorization", format!("Bearer {tok}").parse().unwrap_or_else(|_| "".parse().unwrap()));
                                }
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
                            let pkey = pdns.iter().find(|(k,_)| k == "pdns_api_key").map(|(_,v)| v.clone());
                            if let (Some(url), Some(key)) = (purl, pkey) {
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
            let ssl_agent = state.agent.clone();
            let ssl_domain = body.domain.clone();
            let ssl_email = claims.email.clone();
            let ssl_runtime = runtime.to_string();
            let ssl_php_socket = body.php_version.as_ref().map(|v| format!("unix:/run/php/php{v}-fpm.sock"));
            let ssl_proxy_port = body.proxy_port;
            let ssl_logs = logs.clone();
            tokio::spawn(async move {
                // Retry SSL with backoff: 3s, 30s, 2m, 5m
                let delays = [3u64, 30, 120, 300];
                for (i, delay) in delays.iter().enumerate() {
                    tokio::time::sleep(Duration::from_secs(*delay)).await;
                    emit_step(&ssl_logs, site_id, "ssl", "Provisioning SSL certificate", "in_progress", None);
                    let ssl_body = serde_json::json!({
                        "email": ssl_email,
                        "runtime": ssl_runtime,
                        "php_socket": ssl_php_socket,
                        "proxy_port": ssl_proxy_port,
                    });
                    match ssl_agent.post(&format!("/ssl/provision/{ssl_domain}"), Some(ssl_body)).await {
                        Ok(_) => {
                            tracing::info!("Auto-SSL provisioned for {ssl_domain} (attempt {})", i + 1);
                            emit_step(&ssl_logs, site_id, "ssl", "Provisioning SSL certificate", "done", None);
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
                let cms_agent = state.agent.clone();
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
                                cms_logs.lock().unwrap().remove(&site_id);
                                return;
                            }
                        };
                        db_host = host;

                        let _ = sqlx::query(
                            "INSERT INTO databases (site_id, engine, name, db_user, db_password_enc, container_id, port) \
                             VALUES ((SELECT id FROM sites WHERE domain = $1), 'mysql', $2, $3, $4, $5, $6) \
                             ON CONFLICT DO NOTHING",
                        )
                        .bind(&cms_domain)
                        .bind(&db_name)
                        .bind(&db_user_name)
                        .bind(&db_password)
                        .bind(&db_container_id)
                        .bind(db_port)
                        .execute(&cms_db)
                        .await;

                        emit_step(&cms_logs, site_id, "db_init", "Waiting for database engine", "in_progress", None);
                        // Wait for MariaDB to be fully ready (TCP connects before MySQL is ready)
                        for _attempt in 1..=20 {
                            tokio::time::sleep(Duration::from_secs(2)).await;
                            let php_check = tokio::process::Command::new("php")
                                .args(["-r", &format!(
                                    "try {{ new PDO('mysql:host={db_host};dbname={db_name}', '{db_user_name}', '{db_password}'); echo 'OK'; }} catch(Exception $e) {{ echo 'FAIL'; }}"
                                )])
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
                    cms_logs.lock().unwrap().remove(&site_id);
                });
            } else {
                // Non-CMS site: emit complete after SSL (spawned separately)
                let final_logs = logs.clone();
                tokio::spawn(async move {
                    // Wait for SSL task to finish (SSL has 3s delay + ~5s provision)
                    tokio::time::sleep(Duration::from_secs(12)).await;
                    emit_step(&final_logs, site_id, "complete", "Site ready", "done", None);
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    final_logs.lock().unwrap().remove(&site_id);
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
                cleanup_logs.lock().unwrap().remove(&site_id);
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
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if exists.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Site not found"));
    }

    // Get broadcast receiver + snapshot of existing steps
    let (snapshot, rx) = {
        let logs = state.provision_logs.lock().unwrap();
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
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
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
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
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
    state
        .agent
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
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    tracing::info!("PHP version switched to {} for {}", version, site.domain);
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "site.php_switch",
        Some("site"), Some(&site.domain), Some(version), None,
    ).await;

    Ok(Json(updated))
}

/// GET /api/php/versions — List available PHP versions (proxy to agent).
pub async fn php_versions(
    State(state): State<AppState>,
    AuthUser(_claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = state
        .agent
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
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<InstallPhpRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if claims.role != "admin" {
        return Err(err(StatusCode::FORBIDDEN, "Admin only"));
    }

    let result = state
        .agent
        .post(
            "/php/install",
            Some(serde_json::json!({ "version": body.version })),
        )
        .await
        .map_err(|e| agent_error("PHP install", e))?;

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
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
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
        if custom.len() > 10240 {
            return Err(err(StatusCode::BAD_REQUEST, "Custom nginx directives must be under 10KB"));
        }
        if custom.contains('\0') {
            return Err(err(StatusCode::BAD_REQUEST, "Custom nginx directives contain invalid characters"));
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
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

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
    state
        .agent
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
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: Site = sqlx::query_as(
        "SELECT * FROM sites WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
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
        if let Err(e) = state.agent.delete(&format!("/databases/{container_id}")).await {
            tracing::warn!("Failed to remove database container {container_id}: {e}");
        }
    }

    // Remove nginx config + SSL + PHP pool + site files + logs
    let agent_path = format!("/nginx/sites/{}", site.domain);
    state.agent.delete(&agent_path).await
        .map_err(|e| agent_error("Site removal", e))?;

    // Delete monitors linked to this site (FK is SET NULL, not CASCADE)
    sqlx::query("DELETE FROM monitors WHERE site_id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .ok();

    // Delete from DB (CASCADE removes databases, backups, crons, etc.)
    sqlx::query("DELETE FROM sites WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    tracing::info!("Site deleted: {}", site.domain);
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "site.delete",
        Some("site"), Some(&site.domain), None, None,
    ).await;

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

            let zone: Option<(String, Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
                "SELECT provider, cf_zone_id, cf_api_token, cf_api_email FROM dns_zones WHERE domain = $1 AND user_id = $2"
            ).bind(&parent).bind(dns_user).fetch_optional(&dns_db).await.ok().flatten();

            if let Some((provider, cf_zone_id, cf_api_token, cf_api_email)) = zone {
                let server_ip = match reqwest::Client::new()
                    .get("https://api.ipify.org")
                    .timeout(std::time::Duration::from_secs(5))
                    .send().await
                {
                    Ok(resp) => resp.text().await.unwrap_or_default().trim().to_string(),
                    Err(_) => {
                        std::net::UdpSocket::bind("0.0.0.0:0")
                            .and_then(|s| { s.connect("8.8.8.8:53")?; s.local_addr() })
                            .map(|a| a.ip().to_string()).unwrap_or_default()
                    }
                };

                if provider == "cloudflare" {
                    if let (Some(zid), Some(tok)) = (cf_zone_id, cf_api_token) {
                        let client = reqwest::Client::new();
                        let mut headers = reqwest::header::HeaderMap::new();
                        if let Some(em) = cf_api_email {
                            headers.insert("X-Auth-Email", em.parse().unwrap_or_else(|_| "".parse().unwrap()));
                            headers.insert("X-Auth-Key", tok.parse().unwrap_or_else(|_| "".parse().unwrap()));
                        } else {
                            headers.insert("Authorization", format!("Bearer {tok}").parse().unwrap_or_else(|_| "".parse().unwrap()));
                        }
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
                    let pkey = pdns.iter().find(|(k,_)| k == "pdns_api_key").map(|(_,v)| v.clone());
                    if let (Some(url), Some(key)) = (purl, pkey) {
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
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    row.map(|(d,)| d)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))
}

/// GET /api/sites/{id}/redirects — List redirects.
pub async fn list_redirects(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = site_domain(&state, id, claims.sub).await?;
    let result = state
        .agent
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
    Path(id): Path<Uuid>,
    Json(body): Json<AddRedirectBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = site_domain(&state, id, claims.sub).await?;
    let result = state
        .agent
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
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = site_domain(&state, id, claims.sub).await?;
    let result = state
        .agent
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
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = site_domain(&state, id, claims.sub).await?;
    let result = state
        .agent
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
    Path(id): Path<Uuid>,
    Json(body): Json<PasswordProtectBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = site_domain(&state, id, claims.sub).await?;
    let result = state
        .agent
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
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = site_domain(&state, id, claims.sub).await?;
    let result = state
        .agent
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
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = site_domain(&state, id, claims.sub).await?;
    let result = state
        .agent
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
    Path(id): Path<Uuid>,
    Json(body): Json<AddAliasBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = site_domain(&state, id, claims.sub).await?;
    let result = state
        .agent
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
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = site_domain(&state, id, claims.sub).await?;
    let result = state
        .agent
        .post(
            &format!("/nginx/aliases/{domain}/remove"),
            Some(body),
        )
        .await
        .map_err(|e| agent_error("Domain aliases", e))?;
    Ok(Json(result))
}
