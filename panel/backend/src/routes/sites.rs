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

use crate::auth::AdminUser;
use crate::auth::AuthUser;
use crate::auth::Claims;
use crate::auth::ServerScope;
use crate::error::{internal_error, err, agent_error, paginate, ApiError};
use crate::models::Site;

/// `SELECT s.*` over [`crate::helpers::SITE_CALLER_PREDICATE`] — the full-row read
/// used by every per-site handler in this module. Built once; the predicate itself
/// is shared with the five other modules that resolve a site by caller.
static SITE_FOR_CALLER_ALL: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!("SELECT s.* FROM sites s WHERE {}", crate::helpers::SITE_CALLER_PREDICATE)
});
use crate::routes::is_valid_domain;
use crate::routes::reseller_dashboard::check_reseller_quota;
use crate::services::domain_claim;

/// Effective per-user site-creation ceiling per hour, shared by every handler
/// that writes a `sites` row so they cannot drift apart. `security_site_rate_limit`
/// holds the count; 0 means no limit. Absent row falls back to the seeded default of 3.
///
/// Visible to the crate since v2.110.0, when the staging module became the third
/// caller. It was private while two of the four site-creating handlers lived in
/// this file, which is precisely why the third one — in another module, and
/// therefore unable to call this even had its author looked — was written with a
/// ceiling of its own: none.
pub(crate) async fn site_rate_limit(pool: &sqlx::PgPool) -> i64 {
    match crate::services::security_hardening::get_setting_i64(pool, "security_site_rate_limit", 3)
        .await
    {
        n if n <= 0 => i64::MAX,
        n => n,
    }
}

/// Atomically reserve one site slot against the site owner's reseller quota. The
/// conditional UPDATE ... RETURNING closes the check-then-increment TOCTOU. Returns
/// Ok(true) if a slot was reserved (release it on a later failure), Ok(false) if the
/// user has no reseller (nothing to enforce), or Err(403) if the quota is exhausted.
/// Mirrors databases::reserve_reseller_db_slot.
async fn reserve_reseller_site_slot(state: &AppState, user_id: uuid::Uuid) -> Result<bool, ApiError> {
    let reserved: Option<i32> = sqlx::query_scalar(
        "UPDATE reseller_profiles SET used_sites = used_sites + 1, updated_at = NOW() \
         WHERE user_id = (SELECT reseller_id FROM users WHERE id = $1 AND reseller_id IS NOT NULL) \
           AND (max_sites IS NULL OR used_sites < max_sites) \
         RETURNING 1",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("reserve reseller site quota", e))?;

    if reserved.is_some() {
        return Ok(true);
    }
    let has_profile: Option<(uuid::Uuid,)> = sqlx::query_as(
        "SELECT rp.user_id FROM reseller_profiles rp \
         WHERE rp.user_id = (SELECT reseller_id FROM users WHERE id = $1 AND reseller_id IS NOT NULL)",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("reserve reseller site quota", e))?;

    if has_profile.is_some() {
        return Err(err(StatusCode::FORBIDDEN, "Reseller site quota exceeded"));
    }
    Ok(false)
}

/// Release a reserved site slot (compensating decrement) on a later failure.
async fn release_reseller_site_slot(state: &AppState, user_id: uuid::Uuid) {
    let _ = sqlx::query(
        "UPDATE reseller_profiles SET used_sites = GREATEST(used_sites - 1, 0), updated_at = NOW() \
         WHERE user_id = (SELECT reseller_id FROM users WHERE id = $1 AND reseller_id IS NOT NULL)",
    )
    .bind(user_id)
    .execute(&state.db)
    .await;
}
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

/// Store a credential DockPanel generated on the user's behalf into the site's
/// auto-created vault.
///
/// Encrypts with `secrets::get_encryption_key` — the same derivation the Secrets
/// Manager decrypts with, so the value is actually retrievable in the UI rather
/// than merely written somewhere.
///
/// Best-effort by design: provisioning must not fail because bookkeeping did.
/// But every failure is logged at error level, because the failure mode this
/// exists to kill (s252 F3) was a password generated, handed to wp-cli, and
/// dropped on the floor beside an empty vault — with nothing said anywhere.
async fn store_site_secret(
    pool: &sqlx::PgPool,
    jwt_secret: &str,
    vault_id: Uuid,
    key: &str,
    value: &str,
    description: &str,
) -> bool {
    let enc_key = crate::routes::secrets::get_encryption_key(jwt_secret);
    let encrypted = match crate::services::secrets_crypto::encrypt(value, &enc_key) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("Auto-vault: could not encrypt '{key}': {e}");
            return false;
        }
    };

    match sqlx::query(
        "INSERT INTO secrets (vault_id, key, encrypted_value, description, secret_type, updated_by) \
         VALUES ($1, $2, $3, $4, 'password', 'dockpanel') \
         ON CONFLICT (vault_id, key) DO UPDATE SET \
           encrypted_value = EXCLUDED.encrypted_value, \
           description = EXCLUDED.description, \
           version = secrets.version + 1, \
           updated_at = NOW()",
    )
    .bind(vault_id)
    .bind(key)
    .bind(&encrypted)
    .bind(description)
    .execute(pool)
    .await
    {
        Ok(_) => true,
        Err(e) => {
            tracing::error!("Auto-vault: could not store '{key}': {e}");
            false
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
///
/// Scoped to the caller, with no role branch, and every per-site read below is
/// the same. That is the single ownership axis the `client` role rests on — an
/// account that OWNS the row passes all of it natively. See `list_for_admin`
/// for the one read that is allowed to look past it, and why it had to exist.
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

/// One site as an admin sees it on the all-sites view, with its owner resolved.
///
/// Deliberately NOT [`Site`]. `Site` is `#[derive(FromRow)]` over `SELECT *` and
/// is bound at three dozen call sites in this crate; a field with no matching
/// column breaks every one of them at runtime, and a struct that carried an
/// owner would sooner or later be handed to a per-site handler that must keep
/// asking "is this yours?". This projection holds only what the list renders,
/// so it cannot be mistaken for a site the caller may act on.
#[derive(serde::Serialize, sqlx::FromRow)]
pub struct AdminSiteRow {
    pub id: Uuid,
    pub domain: String,
    pub runtime: String,
    pub status: String,
    pub enabled: bool,
    pub ssl_enabled: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub parent_site_id: Option<Uuid>,
    pub user_id: Uuid,
    pub owner_email: Option<String>,
    pub owner_role: Option<String>,
}

/// GET /api/admin/sites — every site on this server, with its owner. Admin only.
///
/// **This route exists because `transfer` was a one-way door.** Ownership is one
/// axis and `list` above is scoped to the caller with no role branch — so the
/// moment an admin handed a site to a client, the row left their list, `get_one`
/// answered 404, and the Transfer control, which is rendered only on the site's
/// own page, became unreachable. There was no way back through the panel at all,
/// while `docs/guides/roles-and-ownership.md` promised the opposite in print.
/// Reported from the field on #51 by the operator who had just used the feature.
///
/// It is a READ, and only a read — this route lists, and nothing more. What it is
/// no longer doing is standing in for the capability: when it shipped, seeing a
/// site and handing it back was the whole of what an administrator could do with
/// one they did not own, and the operator it was built for said that was not
/// enough on a server he is responsible for. Acting on those sites is now decided
/// by [`crate::helpers::SITE_CALLER_PREDICATE`], one predicate shared by every
/// per-site handler, which admits the owner or an administrator of the machine
/// the site runs on.
///
/// This read stays a narrow projection anyway. A caller that may act on a site
/// asks for it by id through the ordinary handlers; nothing needs a list row it
/// can mistake for an actionable site.
pub async fn list_for_admin(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(server_id, _agent): ServerScope,
    Query(params): Query<ListQuery>,
) -> Result<Json<Vec<AdminSiteRow>>, ApiError> {
    let (limit, offset) = paginate(params.limit, params.offset);

    let sites: Vec<AdminSiteRow> = sqlx::query_as(
        "SELECT s.id, s.domain, s.runtime, s.status, s.enabled, s.ssl_enabled, \
                s.created_at, s.parent_site_id, s.user_id, \
                u.email AS owner_email, u.role AS owner_role \
         FROM sites s LEFT JOIN users u ON u.id = s.user_id \
         WHERE s.server_id = $1 ORDER BY s.created_at DESC LIMIT $2 OFFSET $3",
    )
    .bind(server_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("list sites for admin", e))?;

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

    // Feature 3: Rate limit site creation (max N per user per hour).
    // `security_site_rate_limit` is a COUNT, not a flag: the migration seeds it
    // '3', but until v2.46.0 it was read with get_setting_bool, so '3' != "true"
    // read as false and the limit became 999 — the seed disabled the very
    // feature it configures, on every install that ran the migration. 0 = off.
    {
        let max_sites: i64 = site_rate_limit(&state.db).await;
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

    // Format, reserved and every ownership check, in one call — see
    // services::domain_claim. `body.domain` is not used past this point; the
    // normalised form is what gets stored and sent to the agent.
    let domain = domain_claim::ensure_claimable(
        &state.db,
        &state.agents,
        &body.domain,
        &headers,
        domain_claim::Holder::New,
        &claims.role,
    )
    .await?;

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

    // Validate an explicitly-supplied proxy port. Unvalidated, it (a) renders as
    // `proxy_pass http://127.0.0.1:<port>` → loopback SSRF into internal
    // services, and (b) is fed to the auto-firewall `ufw deny <port>/tcp`, which
    // would clobber a global allow rule (e.g. deny 443 → box-wide outage).
    // Auto-allocated node/python ports (chosen below from a free 5000-5999 slot)
    // are inherently safe and skip this.
    if let Some(port) = body.proxy_port {
        if !is_safe_proxy_port(port) {
            return Err(err(StatusCode::BAD_REQUEST,
                "proxy_port must be between 1024 and 65535 and not a reserved/system port"));
        }
        let taken: Option<(Uuid,)> = sqlx::query_as(
            "SELECT id FROM sites WHERE proxy_port = $1 AND server_id = $2 AND user_id <> $3",
        )
        .bind(port)
        .bind(server_id)
        .bind(claims.sub)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("proxy port check", e))?;
        if taken.is_some() {
            return Err(err(StatusCode::CONFLICT,
                "That port is already in use by another site on this server"));
        }
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

    // Domain uniqueness — sites, git deploys AND Docker apps — was checked by
    // `domain_claim::ensure_claimable` above, before any of this work started.

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
    .bind(&domain)
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

    // Create provisioning log channel. This is the entry that carries the
    // generated CMS admin password on its "credentials" step, so the owner
    // recorded here is what keeps it from any other account.
    crate::helpers::register_provision_log(
        &state.provision_logs,
        &state.deploy_owners,
        site.id,
        claims.sub,
        64,
    );
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
    agent_body["fastcgi_cache"] = serde_json::json!(false);
    agent_body["redis_cache"] = serde_json::json!(false);
    agent_body["redis_db"] = serde_json::json!(0);
    agent_body["waf_enabled"] = serde_json::json!(false);
    agent_body["waf_mode"] = serde_json::json!("detection");

    // Atomically reserve the reseller site-quota slot BEFORE the agent creates the
    // nginx vhost — closes the check-then-increment TOCTOU without orphaning an nginx
    // config on a quota-race (the open tx below rolls back the uncommitted site row if
    // this rejects). The early check_reseller_quota above is a fast-path UX pre-check.
    let site_slot_reserved = reserve_reseller_site_slot(&state, claims.sub).await?;

    // Call agent to create nginx config
    let agent_path = format!("/nginx/sites/{}", body.domain);
    match agent.put(&agent_path, agent_body).await {
        Ok(_) => {
            emit_step(&logs, site_id, "nginx", "Configuring web server", "done", None);

            // Agent succeeded — commit the transaction so the site record is persisted
            // (background tasks like monitors, backups, SSL need the site to exist)
            if let Err(e) = tx.commit().await {
                if site_slot_reserved {
                    release_reseller_site_slot(&state, claims.sub).await;
                }
                return Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Transaction commit failed: {e}")));
            }

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

            // Reseller site counter already incremented atomically by the reserve above.

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

            // GAP 6: Auto-create secrets vault for the site.
            //
            // Created INLINE (not in a spawned task) so its id is available to the
            // CMS installer below. Until v2.28.0 this was fire-and-forget, and the
            // CMS task raced it — which is part of why the auto-generated admin
            // password was never stored anywhere and the vault sat empty (F3).
            let site_vault_id: Option<Uuid> = sqlx::query_scalar(
                "INSERT INTO secret_vaults (user_id, name, description, site_id) \
                 VALUES ($1, $2, $3, $4) RETURNING id"
            )
            .bind(claims.sub)
            .bind(format!("{} secrets", body.domain))
            .bind(format!("Auto-created vault for {}", body.domain))
            .bind(site.id)
            .fetch_optional(&state.db)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!("Auto-vault: could not create vault for {}: {e}", body.domain);
                None
            });
            if site_vault_id.is_some() {
                tracing::info!("Auto-vault: created for {}", body.domain);
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
                let _sp_site_id = site.id;
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
                // The host this site is being created ON. Auto-DNS must publish
                // THIS machine's address, not the panel's — see
                // `helpers::public_ip_for_server`. Here the two agree only on a
                // single-box install; on a fleet the record used to point every
                // member's site at the panel, which made the site unreachable at
                // the very name the form had just claimed for it.
                let dns_server_id = server_id;
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
                        let Some(server_ip) =
                            crate::helpers::public_ip_for_server(&dns_db, Some(dns_server_id)).await
                        else {
                            // Refusing is the whole point: an A record naming the
                            // wrong machine is worse than no record, because it
                            // resolves and answers.
                            emit_step(&dns_logs, site_id, "dns", "Creating DNS record", "failed", Some("could not determine this server's public address".to_string()));
                            return;
                        };

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

            // Decided up front because the SSL task's outcome channel below has to
            // be handed to whichever branch owns the terminal step.
            let cms_type = body.cms.as_deref().unwrap_or("");
            let needs_db = matches!(cms_type, "wordpress" | "laravel" | "drupal" | "joomla" | "codeigniter");
            let needs_install = matches!(cms_type, "wordpress" | "laravel" | "drupal" | "joomla" | "symfony" | "codeigniter");

            // Auto-SSL: try to provision Let's Encrypt cert in background.
            //
            // Resolve the ACME contact BEFORE spawning. A contact address in a
            // reserved TLD (the operator registered the panel as e.g.
            // admin@box.test) makes Let's Encrypt refuse the account, so every
            // order fails — and before s253 that failed four times in total
            // silence: nothing in the UI, nothing in journalctl, then a permanent
            // give-up. Resolving here lets the real reason become a visible step.
            let acme_contact =
                crate::routes::ssl::resolve_acme_contact(&state.db, &claims.email).await;
            let ssl_agent = agent.clone();
            let ssl_db = state.db.clone();
            let ssl_domain = body.domain.clone();
            let ssl_email = match acme_contact {
                Ok(addr) => addr,
                Err(reason) => {
                    tracing::warn!("Auto-SSL skipped for {}: {reason}", body.domain);
                    emit_step(&logs, site_id, "ssl", "SSL certificate", "error", Some(reason));
                    String::new()
                }
            };
            let ssl_runtime = runtime.to_string();
            let ssl_php_socket = body.php_version.as_ref().map(|v| format!("unix:/run/php/php{v}-fpm.sock"));
            let ssl_proxy_port = body.proxy_port;
            let ssl_php_preset = body.php_preset.clone();
            let ssl_root_path: Option<String> = None; // default root
            let ssl_logs = logs.clone();
            // Lets the terminal step below report what actually happened instead
            // of asserting success on a timer (s252 F2).
            let (ssl_tx, ssl_rx) = tokio::sync::oneshot::channel::<bool>();
            let mut ssl_tx = Some(ssl_tx);
            tokio::spawn(async move {
                // Empty means the contact could not be resolved above; the reason
                // has already been emitted as a step, so don't burn four retries
                // against an order the CA is certain to refuse.
                if ssl_email.is_empty() {
                    if let Some(tx) = ssl_tx.take() {
                        let _ = tx.send(false);
                    }
                    return;
                }
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
                                .and_then(crate::helpers::parse_agent_cert_expiry);
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

                            if let Some(tx) = ssl_tx.take() {
                                let _ = tx.send(true);
                            }
                            return; // Success, stop retrying
                        }
                        Err(e) => {
                            if i == delays.len() - 1 {
                                // Last attempt failed. Say WHY — this used to read
                                // "Skipped", which described neither the four
                                // attempts that had just happened nor their cause.
                                tracing::info!("Auto-SSL failed for {ssl_domain} after {} attempts: {e}", i + 1);
                                emit_step(&ssl_logs, site_id, "ssl", "SSL certificate", "error",
                                    Some(format!(
                                        "Could not issue a certificate after {} attempts: {e}. \
                                         The site is served over HTTP; open the site's SSL section to retry.",
                                        i + 1
                                    )));
                                if let Some(tx) = ssl_tx.take() {
                                    let _ = tx.send(false);
                                }
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
                // Whether WE generated this password decides whether the user has
                // any other copy of it. If they typed one, don't echo it back.
                let cms_pass_generated = body.admin_password.is_none();
                let cms_pass = body.admin_password.clone().unwrap_or_else(|| {
                    use rand::Rng;
                    let mut rng = rand::rng();
                    (0..16).map(|_| rng.sample(rand::distr::Alphanumeric) as char).collect()
                });
                let cms_admin_user = cms_user.clone();
                let cms_vault_id = site_vault_id;
                let cms_logs = logs.clone();
                let cms_jwt_secret = state.config.jwt_secret.clone();

                tokio::spawn(async move {
                    let db_name = cms_domain.replace('.', "_").replace('-', "_");
                    let db_user_name = db_name.clone();
                    let db_password: String = {
                        use rand::Rng;
                        let mut rng = rand::rng();
                        (0..20).map(|_| rng.sample(rand::distr::Alphanumeric) as char).collect()
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
                    let mut install_ok = true;
                    emit_step(&cms_logs, site_id, "install", &format!("Installing {cms_label}"), "in_progress", None);

                    // Install at the scheme the site can actually serve RIGHT NOW.
                    //
                    // Auto-SSL is still running in a task beside this one and may
                    // yet fail — DNS not pointed at the box is the ordinary first
                    // site. Committing WordPress to the secure URL regardless used
                    // to leave it 302ing to a scheme with no certificate, i.e.
                    // dead on both, while the panel called it active. Plain HTTP
                    // always works; the promotion below moves it across the moment
                    // there is a certificate to move it to.
                    let install_result = if cms_name == "wordpress" {
                        cms_agent.post(&format!("/wordpress/{cms_domain}/install"), Some(serde_json::json!({
                            "url": format!("http://{cms_domain}"),
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

                            // Take custody of the credentials we just generated.
                            // The form says "Auto-generated if blank", so a user who
                            // takes that at its word has no other copy — before
                            // v2.28.0 the password went to wp-cli and nowhere else,
                            // locking them out of the site they had just made.
                            if let Some(vault_id) = cms_vault_id {
                                let stored_pass = store_site_secret(
                                    &cms_db, &cms_jwt_secret, vault_id,
                                    &format!("{cms_name}_admin_password"),
                                    &cms_pass,
                                    &format!("{cms_label} admin password for {cms_domain}"),
                                ).await;
                                store_site_secret(
                                    &cms_db, &cms_jwt_secret, vault_id,
                                    &format!("{cms_name}_admin_user"),
                                    &cms_admin_user,
                                    &format!("{cms_label} admin username for {cms_domain}"),
                                ).await;
                                if needs_db {
                                    store_site_secret(
                                        &cms_db, &cms_jwt_secret, vault_id,
                                        "database_password", &db_password,
                                        &format!("Database password for {db_name}"),
                                    ).await;
                                }

                                // Reveal a password we generated exactly once, at
                                // the moment it is created. It is also in the vault,
                                // so this is convenience, not the system of record —
                                // and the provisioning stream is owner-scoped
                                // (provision_log checks user_id).
                                if stored_pass && cms_pass_generated {
                                    emit_step(
                                        &cms_logs, site_id, "credentials",
                                        &format!("{cms_label} admin credentials"), "done",
                                        Some(format!(
                                            "Username {cms_admin_user} · password {cms_pass} — \
                                             saved to the \"{cms_domain} secrets\" vault. \
                                             Copy it now; this is the only time it is shown here."
                                        )),
                                    );
                                } else if stored_pass {
                                    emit_step(
                                        &cms_logs, site_id, "credentials",
                                        &format!("{cms_label} admin credentials"), "done",
                                        Some(format!(
                                            "Saved to the \"{cms_domain} secrets\" vault."
                                        )),
                                    );
                                } else {
                                    emit_step(
                                        &cms_logs, site_id, "credentials",
                                        &format!("{cms_label} admin credentials"), "error",
                                        Some("Could not be saved to the site vault — \
                                              reset the admin password from the site's tools.".into()),
                                    );
                                }
                            }

                            // Auto-create WordPress system cron
                            if cms_name == "wordpress" {
                                let cron_db = cms_db.clone();
                                let cron_domain = cms_domain.clone();
                                let cron_site_id = site_id;
                                tokio::spawn(async move {
                                    // No `> /dev/null 2>&1`: the agent's cron
                                    // filter blocks "> /dev/", and this row is
                                    // INSERTed straight into the database
                                    // without passing through that filter — so
                                    // the product was writing a job its own
                                    // guard rejects, and every later cron sync
                                    // failed for the whole box. cron discards
                                    // output on its own when MAILTO is empty;
                                    // the redirect bought nothing.
                                    let command = format!("cd /var/www/{cron_domain}/public && php wp-cron.php");
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
                            install_ok = false;
                            emit_step(&cms_logs, site_id, "install", &format!("Installing {cms_label}"), "error",
                                Some(format!("{cms_label} install failed: {e}")));
                        }
                    }

                    // The terminal step must report what happened. It used to say
                    // "Site ready / done" unconditionally — including for an
                    // install that had just failed on the line above (s252 F2).
                    //
                    // It also has to account for SSL, which is the half of F2 that
                    // bites most often: the CMS installs fine, auto-SSL doesn't, and
                    // "Site ready" over a site with no HTTPS is the same false claim
                    // in a different costume. Bounded wait — the site is genuinely
                    // usable meanwhile, so past the bound we say HTTPS is still in
                    // progress rather than guessing either way.
                    let ssl_outcome = tokio::time::timeout(Duration::from_secs(45), ssl_rx).await;
                    if install_ok {
                        match ssl_outcome {
                            Ok(Ok(true)) => {
                                // The certificate can land while the install is
                                // still running, and at that moment there is no
                                // WordPress whose URL the agent could move. Ask
                                // again now that there is; it is a no-op if the
                                // agent already did it during provisioning.
                                let promoted = if cms_name == "wordpress" {
                                    cms_agent
                                        .post(&format!("/wordpress/{cms_domain}/promote-https"), None)
                                        .await
                                        .map_err(|e| e.to_string())
                                } else {
                                    Ok(serde_json::Value::Null)
                                };
                                match promoted {
                                    Ok(_) => {
                                        emit_step(&cms_logs, site_id, "complete", "Site ready", "done", None);
                                    }
                                    // HTTPS is live and nginx redirects to it, but
                                    // the site's own links still say HTTP. Saying
                                    // "ready" here would be the same false claim
                                    // this step was rebuilt to stop making.
                                    Err(e) => {
                                        tracing::warn!(
                                            "Canonical URL promotion failed for {cms_domain}: {e}"
                                        );
                                        emit_step(
                                            &cms_logs, site_id, "complete",
                                            &format!("{cms_label} installed — HTTPS live, site still set to HTTP"),
                                            "error",
                                            Some(format!(
                                                "The certificate is installed, but the site's address \
                                                 could not be switched to HTTPS: {e}. Update the site \
                                                 address in {cms_label} settings."
                                            )),
                                        );
                                    }
                                }
                            }
                            Ok(Ok(false)) => {
                                emit_step(
                                    &cms_logs, site_id, "complete",
                                    &format!("{cms_label} installed — HTTPS not configured"), "error",
                                    Some("The site is served over HTTP. See the SSL step above for \
                                          the reason, and retry from the site's SSL section.".into()),
                                );
                            }
                            _ => {
                                emit_step(
                                    &cms_logs, site_id, "complete",
                                    &format!("{cms_label} installed — HTTPS still in progress"), "done",
                                    Some("DockPanel is still trying to issue a certificate. \
                                          The site's SSL section shows the result.".into()),
                                );
                            }
                        }
                    } else {
                        emit_step(
                            &cms_logs, site_id, "complete",
                            &format!("Site created — {cms_label} install failed"), "error",
                            Some("The site and its database exist, but the application was not \
                                  installed. See the failed step above.".into()),
                        );
                    }
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    cms_logs.lock().unwrap_or_else(|e| e.into_inner()).remove(&site_id);
                });
            } else {
                // Non-CMS site: the terminal step reports the SSL outcome rather
                // than assuming it. This used to sleep a flat 12s and declare
                // "Site ready / done" — while Auto-SSL was still retrying at 30s,
                // 120s and 300s behind that success claim, and often never
                // succeeded at all (s252 F2).
                let final_logs = logs.clone();
                tokio::spawn(async move {
                    // Bound the wait: the full retry ladder runs ~7.5 minutes and
                    // nobody should watch a spinner that long for a site which is
                    // already serving. Past the bound we say HTTPS is still in
                    // progress, which is true, rather than that it is done.
                    match tokio::time::timeout(Duration::from_secs(75), ssl_rx).await {
                        Ok(Ok(true)) => {
                            emit_step(&final_logs, site_id, "complete", "Site ready", "done", None);
                        }
                        Ok(Ok(false)) => {
                            emit_step(
                                &final_logs, site_id, "complete",
                                "Site created — HTTPS not configured", "error",
                                Some("The site is served over HTTP. Open the site's SSL section \
                                      for the reason and to retry.".into()),
                            );
                        }
                        // Sender dropped, or we ran out of patience: SSL is still
                        // in flight or its task went away. Either way, unknown.
                        _ => {
                            emit_step(
                                &final_logs, site_id, "complete",
                                "Site created — HTTPS still in progress", "done",
                                Some("DockPanel is still trying to issue a certificate. \
                                      The site's SSL section shows the result.".into()),
                            );
                        }
                    }
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
            // Release the reseller quota slot reserved just before the agent call.
            if site_slot_reserved {
                release_reseller_site_slot(&state, claims.sub).await;
            }

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
        &format!("SELECT s.id FROM sites s WHERE {}", crate::helpers::SITE_CALLER_PREDICATE)
    )
    .bind(id).bind(claims.sub)
    .fetch_optional(&state.db).await
    .map_err(|e| internal_error("provision log", e))?;

    if exists.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Site not found"));
    }

    // The `sites` row check above is kept as well as the per-key owner check
    // below: this is the one stream that deliberately carries a credential, and
    // it is worth two independent reasons to refuse rather than one.
    let (snapshot, rx) = crate::helpers::open_provision_log(
        &state.provision_logs,
        &state.deploy_owners,
        id,
        claims.sub,
        "No active provisioning for this site",
    )?;

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
    // The two arms of [`crate::helpers::site_domain_for_caller`], for the whole
    // row rather than the domain. This is the read the site's own page makes, so
    // it is what decides whether an administrator can open a site at all — and
    // until now it could not, which left the operator who had just handed a site
    // to a client with a page that answered 404 and a control that lived only on
    // it (#51).
    let site: Site = sqlx::query_as(&SITE_FOR_CALLER_ALL)
        .bind(id)
        .bind(claims.sub)
        .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("get_one sites", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    Ok(Json(site))
}

/// PHP versions this panel will accept from a client.
///
/// One list for both handlers below, where there were two identical literals.
/// It is still a second copy of the agent's own `ALLOWED_VERSIONS`, and
/// deliberately so — this side has to reject a bad version before it spawns
/// anything, and cannot ask an agent that may be a release behind. The
/// frontend no longer carries a third and fourth copy: both pickers read the
/// live list from `GET /api/php/versions`, which is the agent's answer for the
/// server the site actually runs on.
const PHP_VERSIONS: &[&str] = &["8.1", "8.2", "8.3", "8.4", "8.5"];

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

    if !PHP_VERSIONS.contains(&version) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            &format!("Invalid PHP version. Allowed: {}", PHP_VERSIONS.join(", ")),
        ));
    }

    let site: Site = sqlx::query_as(
        SITE_FOR_CALLER_ALL.as_str(),
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("switch php", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;
    let agent = crate::helpers::agent_for_site_server(&state, site.server_id, &site.domain).await?;

    if site.runtime != "php" {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "PHP version can only be changed on PHP sites",
        ));
    }

    // Rebuild the FULL vhost from current config with only php_version changed.
    // A hand-rolled partial body would silently drop the site's WAF / CSP /
    // Permissions-Policy / bot-protection on every PHP switch.
    let mut updated_site = site.clone();
    updated_site.php_version = Some(version.to_string());
    let agent_body = build_nginx_body(&updated_site);

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

/// The runtimes this switch will move a site BETWEEN — deliberately only two.
///
/// `proxy`, `node` and `python` are excluded, and not for caution's sake: they
/// are a different shape of change. Each needs a `proxy_port`, node/python
/// additionally need an `app_command` that `create_site` puts through a
/// per-runtime prefix whitelist, and — the part that actually bites — the agent's
/// `document_root_for` (`agent/src/services/nginx.rs:52`) maps those three to the
/// site directory itself while `static` and `php` BOTH map to `{site_dir}/public`.
/// So static⇄php is purely a vhost rebuild, whereas anything involving a proxying
/// runtime is a vhost rebuild PLUS moving the document root out from under files
/// the operator already put there. Issue #99 asked for html→PHP and was answered
/// with exactly this half; the other half is a separate feature, not a longer
/// version of this list.
const SWITCHABLE_RUNTIMES: &[&str] = &["static", "php"];

/// PUT /api/sites/{id}/runtime — Switch a site between the static and PHP runtimes.
#[derive(serde::Deserialize)]
pub struct SwitchRuntimeRequest {
    pub runtime: String,
    /// Required when switching TO php. Ignored when switching to static.
    pub php_version: Option<String>,
}

pub async fn switch_runtime(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<SwitchRuntimeRequest>,
) -> Result<Json<Site>, ApiError> {
    let target = body.runtime.trim();

    if !SWITCHABLE_RUNTIMES.contains(&target) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            &format!(
                "Runtime can only be switched between {}. Changing to or from a proxying \
                 runtime moves the document root and is not supported here.",
                SWITCHABLE_RUNTIMES.join(" and ")
            ),
        ));
    }

    let site: Site = sqlx::query_as(SITE_FOR_CALLER_ALL.as_str())
        .bind(id)
        .bind(claims.sub)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("switch runtime", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;
    let agent = crate::helpers::agent_for_site_server(&state, site.server_id, &site.domain).await?;

    if !SWITCHABLE_RUNTIMES.contains(&site.runtime.as_str()) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            &format!(
                "This site runs the '{}' runtime. Only {} sites can be switched.",
                site.runtime,
                SWITCHABLE_RUNTIMES.join(" and ")
            ),
        ));
    }

    if site.runtime == target {
        return Err(err(
            StatusCode::BAD_REQUEST,
            &format!("Site is already running the '{target}' runtime"),
        ));
    }

    // A PHP target needs a version, and it must be stated rather than guessed: a
    // site that was PHP once still carries its old `php_version`, and silently
    // reusing it would switch the operator onto a version they never chose on
    // this trip through the control.
    let php_version = if target == "php" {
        let version = body
            .php_version
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                err(
                    StatusCode::BAD_REQUEST,
                    "php_version is required when switching to the PHP runtime",
                )
            })?;
        if !PHP_VERSIONS.contains(&version) {
            return Err(err(
                StatusCode::BAD_REQUEST,
                &format!("Invalid PHP version. Allowed: {}", PHP_VERSIONS.join(", ")),
            ));
        }
        Some(version.to_string())
    } else {
        None
    };

    // Rebuild the FULL vhost with only the runtime (and its PHP version) changed,
    // for the same reason `switch_php` does: a hand-rolled partial body drops the
    // site's WAF / CSP / Permissions-Policy / bot-protection.
    //
    // The agent side needs no new code. `put_site` already refuses a PHP version
    // the server does not have (with the message that offers to install it),
    // writes the per-site FPM pool, reloads PHP-FPM, and only re-points nginx once
    // the pool socket actually exists — the fail-safe that stops a switch from
    // 502-ing the site, which is the failure the reporter had already hit by hand.
    let mut updated_site = site.clone();
    updated_site.runtime = target.to_string();
    updated_site.php_version = php_version.clone();
    let agent_body = build_nginx_body(&updated_site);

    let agent_path = format!("/nginx/sites/{}", site.domain);
    agent
        .put(&agent_path, agent_body)
        .await
        .map_err(|e| agent_error("Nginx update", e))?;

    // Only after the vhost is actually live. A failed agent write above leaves the
    // row describing what is really being served.
    let updated: Site = sqlx::query_as(
        "UPDATE sites SET runtime = $1, php_version = $2, updated_at = NOW() \
         WHERE id = $3 RETURNING *",
    )
    .bind(target)
    .bind(php_version.as_deref())
    .bind(id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("switch runtime", e))?;

    tracing::info!(
        "Runtime switched {} -> {} for {}",
        site.runtime,
        target,
        site.domain
    );
    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "site.runtime_switch",
        Some("site"),
        Some(&site.domain),
        Some(&match php_version.as_deref() {
            Some(v) => format!("{} -> {} {}", site.runtime, target, v),
            None => format!("{} -> {}", site.runtime, target),
        }),
        None,
    )
    .await;

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

/// POST /api/php/install — Install a PHP version (admin only).
///
/// Accepted, not performed: returns `202 { install_id }` and streams the outcome
/// over the same `/api/services/install/{id}/log` SSE the Services page already
/// uses. Installing a PHP version can mean adding deb.sury.org or a module
/// stream and then unpacking fifteen packages, which is minutes — comfortably
/// past the panel's own 300s `TimeoutLayer`, never mind Cloudflare — so holding
/// the request open could only ever produce a timeout for the installs that
/// needed the most time.
#[derive(serde::Deserialize)]
pub struct InstallPhpRequest {
    pub version: String,
}

pub async fn php_install(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<InstallPhpRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    if claims.role != "admin" {
        return Err(err(StatusCode::FORBIDDEN, "Admin only"));
    }

    // Reject here as well as on the agent. The agent is the authority, but this
    // side spawns and answers 202 before it has heard from anything, so an
    // unchecked version would be reported as "started" and only contradicted
    // later, inside a log the caller has to go and read.
    if !PHP_VERSIONS.contains(&body.version.as_str()) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            &format!("Invalid PHP version. Allowed: {}", PHP_VERSIONS.join(", ")),
        ));
    }

    crate::routes::system::install_service_with_log(
        &state,
        agent,
        claims.sub,
        &claims.email,
        &format!("PHP {}", body.version),
        "/php/install",
        Some(serde_json::json!({ "version": body.version })),
    )
    .await
}

/// POST /api/php/uninstall — Uninstall a specific PHP version (admin only).
///
/// Accepted and logged, like the install: same 202 + `install_id`, same SSE.
pub async fn php_uninstall(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<InstallPhpRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    if claims.role != "admin" {
        return Err(err(StatusCode::FORBIDDEN, "Admin only"));
    }

    if !PHP_VERSIONS.contains(&body.version.as_str()) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            &format!("Invalid PHP version. Allowed: {}", PHP_VERSIONS.join(", ")),
        ));
    }

    // Same inversion the install had, in the other direction: the agent budgets
    // 300s for the purge and its `apt-get autoremove` afterwards, and this side
    // was giving it 60. The purge is not cancelled by that — only the account of
    // it is, which is worse for a destructive operation than for an additive one.
    crate::routes::system::install_service_with_log(
        &state,
        agent,
        claims.sub,
        &claims.email,
        &format!("PHP {} (uninstall)", body.version),
        "/php/uninstall",
        Some(serde_json::json!({ "version": body.version })),
    )
    .await
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
        SITE_FOR_CALLER_ALL.as_str(),
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("update limits", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;
    let agent = crate::helpers::agent_for_site_server(&state, site.server_id, &site.domain).await?;

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

    // Rebuild the FULL vhost from the post-update site (includes the new limits +
    // custom_nginx AND the site's WAF/CSP/Permissions-Policy/bot-protection that
    // a hand-rolled partial body used to drop).
    let agent_body = build_nginx_body(&updated);

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
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: Site = sqlx::query_as(
        SITE_FOR_CALLER_ALL.as_str(),
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("remove sites", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    // Every destructive step below names this site by DOMAIN and runs on whichever host
    // this handle points at. Taking the handle from the caller's selection meant a site on
    // a fleet member had its databases, its firewall rule, its Redis index and its webroot
    // aimed at the panel host instead — and the firewall arm is the one step that is not
    // domain-namespaced, so it could match an unrelated rule there. The row names the host.
    let agent =
        crate::helpers::agent_for_site_server(&state, site.server_id, &site.domain).await?;

    // Pre-delete backup: snapshot the site BEFORE anything destructive runs.
    //
    // This used to sit forty lines further down, AFTER `DELETE /nginx/sites/{domain}`
    // — whose agent handler does `remove_dir_all("/var/www/{domain}")` — and after the
    // database containers were destroyed. `create_backup` opens with
    // `if !site_root.exists() { return Err(...) }`, so by the time it ran the site root
    // was always gone: the call returned an error 100% of the time, and `let _ =`
    // discarded it. The "backup before permanent deletion" had never written a byte on
    // this path, and the comment above it said it had.
    //
    // It is still best-effort — a failed snapshot must not strand a site the operator
    // asked to delete — but a failure is now LOGGED rather than swallowed, and the
    // databases are included, since they are still alive at this point.
    let predelete_dbs = crate::routes::backups::site_databases(&state, id).await;
    match agent
        .post(
            &format!("/backups/{}/create", site.domain),
            Some(serde_json::json!({
                "reason": "pre-delete",
                "databases": predelete_dbs.specs,
            })),
        )
        .await
    {
        Ok(info) => tracing::info!(
            "Pre-delete backup for {} created: {}",
            site.domain,
            info.get("filename").and_then(|v| v.as_str()).unwrap_or("(unnamed)")
        ),
        Err(e) => tracing::warn!(
            "Pre-delete backup for {} FAILED — continuing with deletion: {e}",
            site.domain
        ),
    }

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

    // Flush Redis DB for this site if Redis cache was enabled
    if site.redis_cache {
        agent.post(
            &format!("/nginx/sites/{}/redis/purge", site.domain),
            Some(serde_json::json!({ "redis_db": site.redis_db })),
        ).await.map_err(|e| tracing::warn!("Best-effort Redis purge failed for {}: {e}", site.domain)).ok();
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

    // Clean up status page components matching this domain.
    //
    // Scoped to the owner. `status_page_components.name` is free text with no
    // unique constraint, and the owning module's own delete keys on
    // `id = $1 AND user_id = $2` (incidents.rs) — this statement had dropped
    // both, so deleting a site removed every OTHER account's component that
    // happened to carry the same name, taking its monitor links with it through
    // the ON DELETE CASCADE and logging nothing against the actor.
    sqlx::query("DELETE FROM status_page_components WHERE name = $1 AND user_id = $2")
        .bind(&site.domain)
        .bind(claims.sub)
        .execute(&state.db)
        .await
        .ok();

    // (The pre-delete backup used to be here — see the note above the version that
    // now runs before the destructive steps.)

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
        // The host this site actually ran on. The cleanup below deletes a record
        // only when its content matches this address, so reading the PANEL's
        // address here meant a fleet member's record never matched and outlived
        // the site — a dangling A record still resolving to a machine that no
        // longer serves the name, which is a subdomain-takeover surface and not
        // merely untidy.
        let dns_server_id = site.server_id;
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
                let Some(server_ip) =
                    crate::helpers::public_ip_for_server(&dns_db, dns_server_id).await
                else {
                    tracing::warn!("Auto-DNS cleanup: could not resolve the public address of the server {dns_domain} lived on — leaving the record in place rather than deleting one that may belong to another host");
                    return;
                };

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

/// Helper: resolve a site this caller may act on, and return its domain.
///
/// Now one line over [`crate::helpers::site_domain_for_caller`], which is shared
/// with the five modules that each carried their own copy of this query. Read the
/// rules there — in particular why only the admin arm is scoped by server.
async fn site_domain(state: &AppState, site_id: Uuid, claims: &Claims) -> Result<String, ApiError> {
    crate::helpers::site_domain_for_caller(state, site_id, claims).await
}

/// GET /api/sites/{id}/redirects — List redirects.
pub async fn list_redirects(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;
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

    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;
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
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;
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
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;
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

    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;
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
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;
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
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;
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
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(body): Json<AddAliasBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !is_valid_domain(&body.alias) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid alias: must be a valid domain name"));
    }

    // This handler needs the host as a VALUE, not just as a handle, so it reads both
    // from the row rather than going through the domain-only resolver. Taking the id
    // from the caller's selection meant the collision check below ran against one
    // machine while the alias was written on another.
    let (domain, site_server_id): (String, Option<Uuid>) = sqlx::query_as(&format!(
        "SELECT s.domain, s.server_id FROM sites s WHERE {}",
        crate::helpers::SITE_CALLER_PREDICATE
    ))
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("add alias", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    let agent = crate::helpers::agent_for_site_server(&state, site_server_id, &domain).await?;
    // An alias becomes an nginx `server_name` on the caller's own vhost. Without
    // this guard any tenant could attach ANOTHER tenant's (or the panel's own)
    // live domain as an alias and hijack its traffic / intercept its ACME
    // HTTP-01 challenge. Reject reserved domains and any domain already served
    // by a site or git deployment on this server.
    let alias = ensure_domain_available(&state, &body.alias, &headers, &claims.role).await?;

    let result = agent
        .post(
            "/nginx/aliases/add",
            Some(serde_json::json!({
                "domain": domain,
                "alias": alias,
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
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;
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
    Path(id): Path<Uuid>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;
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
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;
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
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;
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
    let domain = site_domain(&state, id, &claims).await?;

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
        &format!("SELECT s.domain, s.ssl_enabled, s.ssl_expiry FROM sites s WHERE {}", crate::helpers::SITE_CALLER_PREDICATE)
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
        "SELECT status, last_response_time, enabled FROM monitors WHERE site_id = $1 ORDER BY created_at DESC LIMIT 1"
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
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let target_domain = body.get("domain").and_then(|v| v.as_str())
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Target domain required"))?;

    if !is_valid_domain(target_domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid target domain format"));
    }

    // Get source site
    let source: Option<Site> = sqlx::query_as(SITE_FOR_CALLER_ALL.as_str())
        .bind(id).bind(claims.sub).fetch_optional(&state.db).await
        .map_err(|e| internal_error("clone site", e))?;
    let source = source.ok_or_else(|| err(StatusCode::NOT_FOUND, "Source site not found"))?;

    // A clone is ONE agent operation: `/nginx/clone-site` reads the source
    // docroot and writes the target docroot on the same box. So unlike the other
    // handlers in this family the fix is not "dispatch from the row" — the target
    // is genuinely a new site on the selected server, and the source is genuinely
    // wherever it already lives. When those differ there is no host that can do
    // the job, and the old code asked the SELECTED one, which has no such
    // docroot. Everything expensive is already committed by the time the agent
    // says so: the reseller quota slot is reserved and the `sites` row inserted
    // before the call, so the caller was left owning a registered,
    // quota-consuming site whose files never arrived.
    //
    // Refuse up front instead. Copying a docroot between machines is a feature
    // (it needs a transfer), not something to fake by pointing at the wrong disk.
    if source.server_id != Some(server_id) {
        return Err(err(
            StatusCode::CONFLICT,
            "This site lives on a different server. Select that server to clone it — \
             cloning across servers is not supported yet.",
        ));
    }

    // clone_site creates a brand-new site and must enforce the SAME admission
    // controls create() does — otherwise it is a create() with none of the
    // guards: it bypassed lockdown, the per-hour rate limit, the reseller quota,
    // the reserved-domain block, and the cross-table (git_deploys) uniqueness
    // check, letting a tenant mass-create sites during a lockdown and even
    // overwrite another tenant's git-deployed domain.
    if security_hardening::is_locked_down(&state.db).await {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "System is in lockdown mode"));
    }
    {
        let max_sites: i64 = site_rate_limit(&state.db).await;
        let recent: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM sites WHERE user_id = $1 AND created_at > NOW() - INTERVAL '1 hour'",
        )
        .bind(claims.sub)
        .fetch_one(&state.db)
        .await
        .unwrap_or((0,));
        if recent.0 >= max_sites {
            return Err(err(StatusCode::TOO_MANY_REQUESTS,
                &format!("Site creation rate limit: max {max_sites} sites per hour")));
        }
    }
    let target_domain = &ensure_domain_available(&state, target_domain, &headers, &claims.role).await?;
    // Atomically reserve the reseller site-quota slot AFTER the (fallible) domain checks
    // so a rejected clone cannot leak a quota slot; this mirrors create(), where the
    // reserve is the last pre-check before the mutation. Release it if the INSERT fails;
    // if a later agent step fails the site row persists and legitimately holds the slot.
    let clone_slot_reserved = reserve_reseller_site_slot(&state, claims.sub).await?;

    // Create new site record
    let new_site: Site = match sqlx::query_as::<_, Site>(
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
    {
        Ok(s) => s,
        Err(e) => {
            if clone_slot_reserved {
                release_reseller_site_slot(&state, claims.sub).await;
            }
            let msg = e.to_string();
            return Err(if msg.contains("duplicate") || msg.contains("unique") {
                err(StatusCode::CONFLICT, "A site with this domain already exists")
            } else {
                internal_error("clone site", e)
            });
        }
    };

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
    nginx_body["fastcgi_cache"] = serde_json::json!(source.fastcgi_cache);
    nginx_body["redis_cache"] = serde_json::json!(source.redis_cache);
    nginx_body["redis_db"] = serde_json::json!(source.redis_db);
    nginx_body["waf_enabled"] = serde_json::json!(source.waf_enabled);
    nginx_body["waf_mode"] = serde_json::json!(source.waf_mode);

    agent.put(&format!("/nginx/sites/{target_domain}"), nginx_body).await
        .map_err(|e| agent_error("Nginx config", e))?;

    activity::log_activity(&state.db, claims.sub, &claims.email, "site.clone",
        Some("site"), Some(target_domain), Some(&source.domain), None).await;

    // Reseller site counter already incremented atomically by the reserve above.

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
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    let mut agent_body = body.clone();
    agent_body["domain"] = serde_json::json!(domain);

    agent.post("/ssl/upload", Some(agent_body)).await
        .map_err(|e| agent_error("SSL upload", e))?;

    // Update DB
    if let Err(e) = sqlx::query("UPDATE sites SET ssl_enabled = true, updated_at = NOW() WHERE id = $1")
        .bind(id).execute(&state.db).await {
        tracing::warn!("Failed to update ssl_enabled for site {id}: {e}");
    }

    // Re-render the FULL vhost from the site's DB config — the same compensation
    // every other SSL path already performs.
    //
    // The agent's `/ssl/upload` handler receives only {domain, certificate,
    // private_key} and has to invent the other nineteen fields of the SiteConfig it
    // renders, so it writes a vhost with WAF off, bot-protection off, no CSP, no
    // Permissions-Policy, no custom_nginx, default rate limits — and, because it
    // cannot know the site's PHP version, an unversioned `php-fpm.sock` that exists
    // on no modern Debian or Ubuntu. So uploading a certificate did not merely strip
    // a hardened site's security directives (with the panel's own toggles still
    // reading ON, since the DB row is untouched): on a PHP site it took the site
    // OFF THE AIR with a 502.
    //
    // v2.18.0 introduced this compensation for exactly this reason and applied it to
    // provision, renew and force-renew. Upload is the fourth sibling and was missed —
    // a fix to one instance of a pattern owes a grep for the pattern.
    crate::routes::ssl::rebuild_vhost_after_ssl(&state, &agent, id).await;

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
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Installing a PHP extension is a system/package-level op on the shared server,
    // admin-only exactly like its version-management siblings php_install/php_uninstall.
    if claims.role != "admin" {
        return Err(err(StatusCode::FORBIDDEN, "Admin only"));
    }
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
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;
    let result = agent.get(&format!("/nginx/env/{domain}")).await
        .map_err(|e| agent_error("Env vars", e))?;
    Ok(Json(result))
}

/// PUT /api/sites/{id}/env — Write environment variables.
pub async fn set_env_vars(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;
    agent.put(&format!("/nginx/env/{domain}"), body).await
        .map_err(|e| agent_error("Env vars", e))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// PUT /api/sites/{id}/domain — Rename a site's domain.
pub async fn rename_domain(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Get current site
    let site: crate::models::Site = sqlx::query_as(
        SITE_FOR_CALLER_ALL.as_str(),
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("rename domain", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    // ⚠ The scope binding this replaces was spelled `_server_id` and was NOT unused —
    // the underscore silenced the warning while the value was still being passed to
    // the claim check below. Renaming a domain rewrites this site's vhost, so both
    // the check and the write have to name the host the row does.
    let agent =
        crate::helpers::agent_for_site_server(&state, site.server_id, &site.domain).await?;
    let requested = body.get("new_domain")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    // Format, reserved, and every owner — including Docker apps, whose domain the
    // panel could not see before and which the agent's 8-step rename below would
    // have walked straight over (it checks that the SOURCE vhost exists and never
    // that the DESTINATION is free).
    let new_domain = domain_claim::ensure_claimable(
        &state.db,
        &state.agents,
        &requested,
        &headers,
        domain_claim::Holder::Site(id),
        &claims.role,
    )
    .await?;

    if new_domain == domain_claim::normalise(&site.domain) {
        return Err(err(StatusCode::BAD_REQUEST, "New domain is the same as current domain"));
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

    // Update monitors linked to this site.
    //
    // Keep whatever scheme the monitor already had. This used to rebuild the URL
    // as `https://{new_domain}` unconditionally, which is not a rename — it also
    // switched the monitor onto a scheme nobody asked it to check. That is wrong
    // for a site with no certificate, and it is not recoverable from
    // `sites.ssl_enabled` either, because a monitor's URL is editable by hand
    // from the Monitors screen. A domain rename should change the domain.
    sqlx::query(
        "UPDATE monitors SET name = $1, \
         url = CASE WHEN url LIKE 'http://%' THEN 'http://' || $1 ELSE 'https://' || $1 END \
         WHERE site_id = $2",
    )
    .bind(&new_domain)
    .bind(id)
    .execute(&state.db)
    .await
    .ok();

    // Update status page components — the mutating twin of the unscoped delete
    // in `remove`, and the same fix: without `user_id` a rename renamed every
    // other account's component that shared the old domain string.
    sqlx::query("UPDATE status_page_components SET name = $1 WHERE name = $2 AND user_id = $3")
        .bind(&new_domain)
        .bind(&old_domain)
        .bind(claims.sub)
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
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: crate::models::Site = sqlx::query_as(
        SITE_FOR_CALLER_ALL.as_str(),
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("toggle site", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;
    let agent = crate::helpers::agent_for_site_server(&state, site.server_id, &site.domain).await?;

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

/// PUT /api/sites/{id}/fastcgi-cache — Toggle FastCGI cache for a PHP site.
pub async fn toggle_fastcgi_cache(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: crate::models::Site = sqlx::query_as(
        SITE_FOR_CALLER_ALL.as_str(),
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("toggle fastcgi cache", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;
    let agent = crate::helpers::agent_for_site_server(&state, site.server_id, &site.domain).await?;

    if site.runtime != "php" {
        return Err(err(StatusCode::BAD_REQUEST, "FastCGI cache is only available for PHP sites"));
    }

    let enabled = body.get("enabled")
        .and_then(|v| v.as_bool())
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing 'enabled' boolean field"))?;

    if enabled == site.fastcgi_cache {
        return Ok(Json(serde_json::json!({
            "ok": true,
            "fastcgi_cache": enabled,
            "message": if enabled { "FastCGI cache is already enabled" } else { "FastCGI cache is already disabled" },
        })));
    }

    // Rebuild the FULL vhost with only fastcgi_cache changed — a hand-rolled
    // partial body used to drop the site's WAF/CSP/Permissions-Policy/bot-protection.
    let mut updated_site = site.clone();
    updated_site.fastcgi_cache = enabled;
    agent.put(
        &format!("/nginx/sites/{}", site.domain),
        build_nginx_body(&updated_site),
    ).await.map_err(|e| agent_error("FastCGI cache", e))?;

    // Update DB
    sqlx::query("UPDATE sites SET fastcgi_cache = $1, updated_at = NOW() WHERE id = $2")
        .bind(enabled)
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("toggle fastcgi cache", e))?;

    let action = if enabled { "enabled" } else { "disabled" };
    tracing::info!("FastCGI cache {action} for {}", site.domain);
    activity::log_activity(
        &state.db, claims.sub, &claims.email,
        &format!("site.fastcgi_cache.{action}"),
        Some("site"), Some(&site.domain), None, None,
    ).await;

    notifications::notify_panel(&state.db, Some(claims.sub),
        &format!("FastCGI cache {action}: {}", site.domain),
        &format!("FastCGI cache has been {action}"), "info", "site", None,
    ).await;

    Ok(Json(serde_json::json!({
        "ok": true,
        "fastcgi_cache": enabled,
    })))
}

/// POST /api/sites/{id}/fastcgi-cache/purge — Purge FastCGI cache for a site.
pub async fn purge_fastcgi_cache(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: crate::models::Site = sqlx::query_as(
        SITE_FOR_CALLER_ALL.as_str(),
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("purge fastcgi cache", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;
    let agent = crate::helpers::agent_for_site_server(&state, site.server_id, &site.domain).await?;

    if !site.fastcgi_cache {
        return Err(err(StatusCode::BAD_REQUEST, "FastCGI cache is not enabled for this site"));
    }

    agent.post(
        &format!("/nginx/sites/{}/cache/purge", site.domain),
        None,
    ).await.map_err(|e| agent_error("Purge cache", e))?;

    tracing::info!("FastCGI cache purged for {}", site.domain);
    activity::log_activity(
        &state.db, claims.sub, &claims.email,
        "site.fastcgi_cache.purge",
        Some("site"), Some(&site.domain), None, None,
    ).await;

    Ok(Json(serde_json::json!({
        "ok": true,
        "message": format!("FastCGI cache purged for {}", site.domain),
    })))
}

/// PUT /api/sites/{id}/redis-cache — Toggle Redis object cache for a PHP site.
pub async fn toggle_redis_cache(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: crate::models::Site = sqlx::query_as(
        SITE_FOR_CALLER_ALL.as_str(),
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("toggle redis cache", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;
    let agent = crate::helpers::agent_for_site_server(&state, site.server_id, &site.domain).await?;

    if site.runtime != "php" {
        return Err(err(StatusCode::BAD_REQUEST, "Redis object cache is only available for PHP sites"));
    }

    let enabled = body.get("enabled")
        .and_then(|v| v.as_bool())
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing 'enabled' boolean field"))?;

    if enabled == site.redis_cache {
        return Ok(Json(serde_json::json!({
            "ok": true,
            "redis_cache": enabled,
            "message": if enabled { "Redis cache is already enabled" } else { "Redis cache is already disabled" },
        })));
    }

    // Assign unique Redis DB number (0-15) when enabling
    let redis_db = if enabled {
        let used: Vec<(i32,)> = sqlx::query_as(
            "SELECT redis_db FROM sites WHERE redis_cache = true AND id != $1"
        )
        .bind(id)
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("redis db allocation", e))?;

        let used_dbs: std::collections::HashSet<i32> = used.into_iter().map(|(db,)| db).collect();
        (1..=15).find(|n| !used_dbs.contains(n))
            .ok_or_else(|| err(StatusCode::CONFLICT, "All Redis DB slots (1-15) are in use"))?
    } else {
        0
    };

    // Configure Redis on the agent
    if enabled {
        agent.post(
            &format!("/nginx/sites/{}/redis/enable", site.domain),
            Some(serde_json::json!({
                "redis_db": redis_db,
                "php_preset": site.php_preset,
            })),
        ).await.map_err(|e| agent_error("Redis cache enable", e))?;
    } else {
        agent.post(
            &format!("/nginx/sites/{}/redis/disable", site.domain),
            None,
        ).await.map_err(|e| agent_error("Redis cache disable", e))?;
    }

    // Rebuild the FULL vhost with only redis settings changed — a hand-rolled
    // partial body used to drop the site's WAF/CSP/Permissions-Policy/bot-protection.
    let mut updated_site = site.clone();
    updated_site.redis_cache = enabled;
    updated_site.redis_db = redis_db;
    agent.put(
        &format!("/nginx/sites/{}", site.domain),
        build_nginx_body(&updated_site),
    ).await.map_err(|e| agent_error("Redis nginx config", e))?;

    // Update DB
    sqlx::query("UPDATE sites SET redis_cache = $1, redis_db = $2, updated_at = NOW() WHERE id = $3")
        .bind(enabled)
        .bind(redis_db)
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("toggle redis cache", e))?;

    let action = if enabled { "enabled" } else { "disabled" };
    tracing::info!("Redis cache {action} for {} (db: {redis_db})", site.domain);
    activity::log_activity(
        &state.db, claims.sub, &claims.email,
        &format!("site.redis_cache.{action}"),
        Some("site"), Some(&site.domain), None, None,
    ).await;

    notifications::notify_panel(&state.db, Some(claims.sub),
        &format!("Redis cache {action}: {}", site.domain),
        &format!("Redis object cache has been {action} (DB {redis_db})"), "info", "site", None,
    ).await;

    Ok(Json(serde_json::json!({
        "ok": true,
        "redis_cache": enabled,
        "redis_db": redis_db,
    })))
}

/// POST /api/sites/{id}/redis-cache/purge — Flush Redis cache for a site.
pub async fn purge_redis_cache(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: crate::models::Site = sqlx::query_as(
        SITE_FOR_CALLER_ALL.as_str(),
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("purge redis cache", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;
    let agent = crate::helpers::agent_for_site_server(&state, site.server_id, &site.domain).await?;

    if !site.redis_cache {
        return Err(err(StatusCode::BAD_REQUEST, "Redis cache is not enabled for this site"));
    }

    agent.post(
        &format!("/nginx/sites/{}/redis/purge", site.domain),
        Some(serde_json::json!({ "redis_db": site.redis_db })),
    ).await.map_err(|e| agent_error("Purge Redis", e))?;

    tracing::info!("Redis cache purged for {} (db: {})", site.domain, site.redis_db);
    activity::log_activity(
        &state.db, claims.sub, &claims.email,
        "site.redis_cache.purge",
        Some("site"), Some(&site.domain), None, None,
    ).await;

    Ok(Json(serde_json::json!({
        "ok": true,
        "message": format!("Redis cache purged for {}", site.domain),
    })))
}

/// PUT /api/sites/{id}/waf — Toggle WAF and set mode for a site.
pub async fn toggle_waf(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: crate::models::Site = sqlx::query_as(
        SITE_FOR_CALLER_ALL.as_str(),
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("toggle waf", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;
    let agent = crate::helpers::agent_for_site_server(&state, site.server_id, &site.domain).await?;

    let enabled = body.get("enabled").and_then(|v| v.as_bool())
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing 'enabled' boolean"))?;
    let mode = body.get("mode").and_then(|v| v.as_str()).unwrap_or("detection");

    if mode != "detection" && mode != "prevention" {
        return Err(err(StatusCode::BAD_REQUEST, "Mode must be 'detection' or 'prevention'"));
    }

    // Configure WAF on agent
    if enabled {
        agent.post(
            &format!("/nginx/sites/{}/waf/configure", site.domain),
            Some(serde_json::json!({ "mode": mode })),
        ).await.map_err(|e| agent_error("WAF configure", e))?;
    }

    // Rebuild the FULL vhost with only WAF settings changed — a hand-rolled
    // partial body used to drop the site's CSP/Permissions-Policy/bot-protection.
    let mut updated_site = site.clone();
    updated_site.waf_enabled = enabled;
    updated_site.waf_mode = mode.to_string();
    agent.put(
        &format!("/nginx/sites/{}", site.domain),
        build_nginx_body(&updated_site),
    ).await.map_err(|e| agent_error("WAF nginx config", e))?;

    // Update DB
    sqlx::query("UPDATE sites SET waf_enabled = $1, waf_mode = $2, updated_at = NOW() WHERE id = $3")
        .bind(enabled)
        .bind(mode)
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("toggle waf", e))?;

    let action = if enabled { format!("enabled ({mode})") } else { "disabled".to_string() };
    tracing::info!("WAF {action} for {}", site.domain);
    activity::log_activity(
        &state.db, claims.sub, &claims.email,
        &format!("site.waf.{}", if enabled { "enabled" } else { "disabled" }),
        Some("site"), Some(&site.domain), None, None,
    ).await;

    notifications::notify_panel(&state.db, Some(claims.sub),
        &format!("WAF {action}: {}", site.domain),
        &format!("Web Application Firewall has been {action}"), "info", "site", None,
    ).await;

    Ok(Json(serde_json::json!({
        "ok": true,
        "waf_enabled": enabled,
        "waf_mode": mode,
    })))
}

/// GET /api/sites/{id}/waf/logs — Get recent WAF events for a site.
pub async fn waf_logs(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: crate::models::Site = sqlx::query_as(
        SITE_FOR_CALLER_ALL.as_str(),
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("waf logs", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;
    let agent = crate::helpers::agent_for_site_server(&state, site.server_id, &site.domain).await?;

    let result = agent
        .get(&format!("/nginx/sites/{}/waf/logs?limit=50", site.domain))
        .await
        .map_err(|e| agent_error("WAF logs", e))?;

    Ok(Json(result))
}

/// POST /api/sites/{id}/optimize-images — Convert site images to WebP/AVIF.
pub async fn optimize_images(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: crate::models::Site = sqlx::query_as(
        SITE_FOR_CALLER_ALL.as_str(),
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("optimize images", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;
    let agent = crate::helpers::agent_for_site_server(&state, site.server_id, &site.domain).await?;

    let format = body.get("format").and_then(|v| v.as_str()).unwrap_or("webp");
    let quality = body.get("quality").and_then(|v| v.as_u64()).unwrap_or(80);

    if format != "webp" && format != "avif" {
        return Err(err(StatusCode::BAD_REQUEST, "Format must be 'webp' or 'avif'"));
    }

    let result = agent
        .post_long(
            &format!("/nginx/sites/{}/optimize-images", site.domain),
            Some(serde_json::json!({ "format": format, "quality": quality })),
            300,
        )
        .await
        .map_err(|e| agent_error("Image optimization", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email,
        "site.optimize_images",
        Some("site"), Some(&site.domain), Some(format), None,
    ).await;

    Ok(Json(result))
}

/// Reserved/system/panel ports a user-supplied `proxy_port` must never be.
/// Choosing one enables loopback SSRF (`proxy_pass http://127.0.0.1:<port>`)
/// into internal services AND lets the auto-firewall `ufw deny <port>/tcp`
/// clobber a global allow rule (e.g. deny 443 → box-wide HTTPS outage).
fn is_safe_proxy_port(port: i32) -> bool {
    const RESERVED: [i32; 20] = [
        22, 25, 53, 80, 110, 143, 443, 465, 587, 993, 995, 3306, 5432, 6379,
        27017, 11211, 3080, 8443, 9443, 2019,
    ];
    (1024..=65535).contains(&port) && !RESERVED.contains(&port)
}

/// Shared new-domain guard for clone_site and add_alias.
///
/// This used to hold the checks itself, with a comment saying it existed "so the
/// guard set create() enforces cannot drift" — while `create()` did not call it
/// and six other domain-introducing paths did not exist as far as it knew. The
/// checks now live in [`crate::services::domain_claim`], which every path calls,
/// and this is the thin adapter for the two callers that pass a `&AppState`.
async fn ensure_domain_available(
    state: &AppState,
    domain: &str,
    headers: &HeaderMap,
    claimant_role: &str,
) -> Result<String, ApiError> {
    domain_claim::ensure_claimable(
        &state.db,
        &state.agents,
        domain,
        headers,
        domain_claim::Holder::New,
        claimant_role,
    )
    .await
}

/// Build the full nginx agent body from a Site model. Shared by all config-rebuild paths.
pub(crate) fn build_nginx_body(site: &crate::models::Site) -> serde_json::Value {
    let mut body = serde_json::json!({
        "runtime": site.runtime,
        "fastcgi_cache": site.fastcgi_cache,
        "redis_cache": site.redis_cache,
        "redis_db": site.redis_db,
        "waf_enabled": site.waf_enabled,
        "waf_mode": site.waf_mode,
        "rate_limit": site.rate_limit,
        "max_upload_mb": site.max_upload_mb,
        "php_memory_mb": site.php_memory_mb,
        "php_max_workers": site.php_max_workers,
        "csp_policy": site.csp_policy,
        "permissions_policy": site.permissions_policy,
        "bot_protection": site.bot_protection,
    });
    if let Some(ref preset) = site.php_preset {
        body["php_preset"] = serde_json::json!(preset);
    }
    if let Some(ref custom) = site.custom_nginx {
        body["custom_nginx"] = serde_json::json!(custom);
    }
    if let Some(ref php) = site.php_version {
        body["php_socket"] = serde_json::json!(format!("unix:/run/php/php{php}-fpm.sock"));
    }
    if site.ssl_enabled {
        body["ssl"] = serde_json::json!(true);
        if let Some(ref cert) = site.ssl_cert_path {
            body["ssl_cert"] = serde_json::json!(cert);
        }
        if let Some(ref key) = site.ssl_key_path {
            body["ssl_key"] = serde_json::json!(key);
        }
    }
    if site.runtime == "proxy" || site.runtime == "node" || site.runtime == "python" {
        body["proxy_port"] = serde_json::json!(site.proxy_port);
    }
    body
}

/// PUT /api/sites/{id}/security-headers — Update CSP and Permissions-Policy.
pub async fn update_security_headers(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: crate::models::Site = sqlx::query_as(
        SITE_FOR_CALLER_ALL.as_str(),
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("security headers", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;
    let agent = crate::helpers::agent_for_site_server(&state, site.server_id, &site.domain).await?;

    let csp = body.get("csp_policy").and_then(|v| v.as_str()).map(|s| s.to_string());
    let perms = body.get("permissions_policy").and_then(|v| v.as_str()).map(|s| s.to_string());

    // Validate CSP (max 4KB, no nginx directive injection). Both values render
    // verbatim into `add_header ... "<value>" always;`, so a `"` (or `\`/brace)
    // would break out of the quoted argument and inject arbitrary root-run nginx
    // directives (location/alias/access_log/proxy_pass). is_safe_header_value
    // keeps `;` (a real CSP needs it) but rejects the break-out characters.
    if let Some(ref csp_val) = csp {
        if csp_val.len() > 4096 {
            return Err(err(StatusCode::BAD_REQUEST, "CSP policy must be under 4KB"));
        }
        if !crate::routes::is_safe_header_value(csp_val) {
            return Err(err(StatusCode::BAD_REQUEST, "CSP policy contains invalid characters"));
        }
    }
    if let Some(ref perms_val) = perms {
        if perms_val.len() > 2048 {
            return Err(err(StatusCode::BAD_REQUEST, "Permissions-Policy must be under 2KB"));
        }
        if !crate::routes::is_safe_header_value(perms_val) {
            return Err(err(StatusCode::BAD_REQUEST, "Permissions-Policy contains invalid characters"));
        }
    }

    // Update DB
    sqlx::query("UPDATE sites SET csp_policy = $1, permissions_policy = $2, updated_at = NOW() WHERE id = $3")
        .bind(&csp)
        .bind(&perms)
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("security headers", e))?;

    // Rebuild nginx with updated headers
    let mut updated_site = site.clone();
    updated_site.csp_policy = csp.clone();
    updated_site.permissions_policy = perms.clone();
    let agent_body = build_nginx_body(&updated_site);

    agent.put(
        &format!("/nginx/sites/{}", site.domain),
        agent_body,
    ).await.map_err(|e| agent_error("Security headers nginx config", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email,
        "site.security_headers",
        Some("site"), Some(&site.domain), None, None,
    ).await;

    Ok(Json(serde_json::json!({
        "ok": true,
        "csp_policy": csp,
        "permissions_policy": perms,
    })))
}

/// PUT /api/sites/{id}/bot-protection — Toggle bot protection mode.
pub async fn toggle_bot_protection(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: crate::models::Site = sqlx::query_as(
        SITE_FOR_CALLER_ALL.as_str(),
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("bot protection", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;
    let agent = crate::helpers::agent_for_site_server(&state, site.server_id, &site.domain).await?;

    let mode = body.get("mode").and_then(|v| v.as_str()).unwrap_or("off");
    if !["off", "rate-limit", "challenge", "block"].contains(&mode) {
        return Err(err(StatusCode::BAD_REQUEST, "Mode must be 'off', 'rate-limit', 'challenge', or 'block'"));
    }

    // Update DB
    sqlx::query("UPDATE sites SET bot_protection = $1, updated_at = NOW() WHERE id = $2")
        .bind(mode)
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("bot protection", e))?;

    // Rebuild nginx with bot protection
    let mut updated_site = site.clone();
    updated_site.bot_protection = mode.to_string();
    let agent_body = build_nginx_body(&updated_site);

    agent.put(
        &format!("/nginx/sites/{}", site.domain),
        agent_body,
    ).await.map_err(|e| agent_error("Bot protection nginx config", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email,
        &format!("site.bot_protection.{mode}"),
        Some("site"), Some(&site.domain), None, None,
    ).await;

    Ok(Json(serde_json::json!({
        "ok": true,
        "bot_protection": mode,
    })))
}

#[derive(serde::Deserialize)]
pub struct TransferSiteRequest {
    /// The new owner, by email. Email rather than id because the operator doing
    /// this is reading a list of people, not a list of UUIDs.
    pub email: String,
}

/// The tables that keep their own copy of `user_id` beside a `site_id`.
///
/// Derived, not remembered: these are every table in the schema carrying BOTH
/// columns. Everything else a site owns — `databases`, `crons`, `backups`,
/// `ssl_certificates` — reaches its owner THROUGH `site_id`, so it follows the
/// transfer with no statement of its own. Getting this list wrong is the failure
/// mode that matters: a row left behind keeps the previous owner able to see and
/// act on part of a site they no longer hold, and nothing would report it.
const OWNERSHIP_DENORMALIZED_TABLES: [&str; 4] =
    ["alerts", "monitors", "secret_vaults", "whmcs_service_map"];

/// POST /api/sites/{id}/transfer — hand a site to another account. Admin only.
///
/// The point of the whole `client` design (GitHub #51). DockPanel has exactly one
/// ownership axis, `sites.user_id`, and 108 ownership-scoped reads that all say
/// `WHERE user_id = $1`. Moving the row therefore moves the site completely, and
/// every one of those reads keeps working without being touched — which is why
/// this is a transfer rather than an access-control list. An ACL would have had
/// to widen all 108, keep 57 `claims.sub` INSERTs in step, and get two name-keyed
/// cleanups right whose own comments record that they already shipped a
/// cross-account delete once.
///
/// ⚠ Transfer is EXCLUSIVE: the previous owner loses the site, **including when
/// the previous owner is an admin**. This comment used to claim the role itself
/// kept an administrator's view of a transferred site, and that was never true:
/// `list` and every per-site read in this file are `WHERE user_id = $1` with no
/// role branch, so a transferring admin lost the row from their list and got 404
/// from `get_one`. Because the Transfer control was rendered only on the site's
/// own page, the handover had no way back through the panel at all.
/// `list_for_admin` is that way back; this handler needed no change, because its
/// own lookup below is by id and is deliberately not ownership-filtered.
///
/// One transaction, because a half-transferred site is worse than a failed one:
/// the row would answer to the new owner while its alerts and secret vaults still
/// answered to the old one.
pub async fn transfer(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(body): Json<TransferSiteRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if claims.role != "admin" {
        return Err(err(StatusCode::FORBIDDEN, "Admin only"));
    }

    let site: Option<(Uuid, String, Uuid)> =
        sqlx::query_as("SELECT id, domain, user_id FROM sites WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("load site", e))?;
    let (site_id, domain, previous_owner) =
        site.ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    let email = body.email.trim().to_ascii_lowercase();
    let recipient: Option<(Uuid, String)> =
        sqlx::query_as("SELECT id, role FROM users WHERE lower(email) = $1")
            .bind(&email)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("load recipient", e))?;
    let (new_owner, new_owner_role) =
        recipient.ok_or_else(|| err(StatusCode::NOT_FOUND, "No account with that email"))?;

    if new_owner == previous_owner {
        return Err(err(
            StatusCode::CONFLICT,
            "That account already owns this site",
        ));
    }
    // A suspended account must not be handed a live site: `auth.rs` rejects its
    // every request, so the site would land somewhere nobody can reach.
    if new_owner_role == "suspended" {
        return Err(err(
            StatusCode::CONFLICT,
            "That account is suspended. Restore it before transferring a site to it.",
        ));
    }

    let mut tx = state
        .db
        .begin()
        .await
        .map_err(|e| internal_error("begin transfer", e))?;

    // ⚠ A STAGING ENVIRONMENT IS A SECOND `sites` ROW, AND IT USED TO STAY BEHIND.
    //
    // `staging::create` inserts one (`routes/staging.rs:168`) carrying
    // `parent_site_id` (20260312700000_staging_sites.sql:2) and the CREATOR's
    // `user_id`. This statement was `WHERE id = $2`, so the clone did not move with
    // its parent — and the previous owner kept, on a site they no longer hold: the
    // staging domain in their Sites list, a `www-data` shell inside a full copy of
    // the new owner's document root (`wp-config.php` and its database credentials
    // included), and the push-to-production control, which writes that copy OVER
    // the new owner's live site. A departed owner with a WRITE into the new owner's
    // production is the worst version of the failure this handler's own comment
    // names: "a row left behind keeps the previous owner able to see and act on
    // part of a site they no longer hold, and nothing would report it."
    //
    // Staging cannot nest — `staging::create` refuses a parent that is itself
    // staging (`staging.rs:106`) — so one `OR parent_site_id` is the whole tree,
    // not the first level of one.
    let moved_ids: Vec<Uuid> = sqlx::query_as::<_, (Uuid,)>(
        "UPDATE sites SET user_id = $1 WHERE id = $2 OR parent_site_id = $2 RETURNING id",
    )
    .bind(new_owner)
    .bind(site_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| internal_error("transfer site", e))?
    .into_iter()
    .map(|(id,)| id)
    .collect();

    let mut moved = serde_json::Map::new();
    for table in OWNERSHIP_DENORMALIZED_TABLES {
        // The table name comes from a const array in this file and never from a
        // request, so the format! cannot carry anything an caller supplied.
        //
        // Keyed on the ids the UPDATE above actually moved, not on `site_id = $2`:
        // the staging row's alerts, monitors and vaults belong to the staging site,
        // and scoping this to the parent alone would move the parent's dependents
        // while leaving the child's with the departed owner — the same split this
        // fix exists to close, one level down.
        let n = sqlx::query(&format!(
            "UPDATE {table} SET user_id = $1 WHERE site_id = ANY($2)"
        ))
        .bind(new_owner)
        .bind(&moved_ids)
        .execute(&mut *tx)
        .await
        .map_err(|e| internal_error("transfer dependent rows", e))?
        .rows_affected();
        moved.insert(table.to_string(), serde_json::json!(n));
    }

    tx.commit()
        .await
        .map_err(|e| internal_error("commit transfer", e))?;

    crate::services::activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "site.transfer",
        Some("site"),
        Some(&domain),
        Some(&format!("{previous_owner} -> {new_owner} ({email})")),
        crate::routes::client_ip(&headers).as_deref(),
    )
    .await;

    Ok(Json(serde_json::json!({
        "ok": true,
        "domain": domain,
        "previous_owner": previous_owner,
        "new_owner": new_owner,
        "dependent_rows_moved": moved,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── proxy_port validation — s241 H4 (SSRF + firewall-clobber DoS) ────

    #[test]
    fn proxy_port_rejects_reserved_and_out_of_range() {
        // Reserved / system / panel ports are rejected (would enable loopback
        // SSRF and let the auto-firewall `ufw deny` clobber a global allow).
        for p in [22, 25, 80, 443, 3306, 5432, 6379, 3080, 8443, 9443] {
            assert!(!is_safe_proxy_port(p), "port {p} must be rejected");
        }
        // Out-of-range rejected.
        assert!(!is_safe_proxy_port(80));
        assert!(!is_safe_proxy_port(1023));
        assert!(!is_safe_proxy_port(70000));
        assert!(!is_safe_proxy_port(-1));
        // Ordinary high app ports are accepted (incl. the 5000-5999 app range).
        assert!(is_safe_proxy_port(8080));
        assert!(is_safe_proxy_port(3000));
        assert!(is_safe_proxy_port(5001));
        assert!(is_safe_proxy_port(65535));
    }
}
