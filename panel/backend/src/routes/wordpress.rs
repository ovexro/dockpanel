use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::{AuthUser, ServerScope};
use crate::error::{internal_error, agent_error, err, require_admin, ApiError};
use crate::services::activity;
use crate::services::agent::AgentHandle;
use crate::services::notifications;
use crate::AppState;

// ── Vulnerability-scan settings ────────────────────────────────────────────
// Companion to image_scans' settings pair (read_settings/get_settings/
// update_settings) — same shape, same "off by default" posture, distinct
// keys because this schedule sweeps WordPress sites, not Docker images.

#[derive(serde::Serialize)]
pub struct WpScanSettings {
    pub enabled: bool,
    pub interval_hours: i32,
}

#[derive(serde::Deserialize)]
pub struct UpdateWpScanSettings {
    pub enabled: bool,
    pub interval_hours: i32,
}

pub async fn read_wp_scan_settings(pool: &sqlx::PgPool) -> Result<(bool, i32), sqlx::Error> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT key, value FROM settings WHERE key IN ('wp_vuln_scan_enabled', 'wp_vuln_scan_interval_hours')",
    )
    .fetch_all(pool)
    .await?;

    let mut enabled = false;
    let mut hours = 24i32;
    for (k, v) in rows {
        match k.as_str() {
            "wp_vuln_scan_enabled" => enabled = v == "true",
            "wp_vuln_scan_interval_hours" => hours = v.parse().unwrap_or(24),
            _ => {}
        }
    }
    Ok((enabled, hours))
}

/// GET /api/wordpress/vuln-scan-settings
pub async fn get_scan_settings(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<WpScanSettings>, ApiError> {
    require_admin(&claims.role)?;
    let (enabled, interval_hours) = read_wp_scan_settings(&state.db)
        .await
        .map_err(|e| internal_error("read wp scan settings", e))?;
    Ok(Json(WpScanSettings { enabled, interval_hours }))
}

/// PUT /api/wordpress/vuln-scan-settings
pub async fn update_scan_settings(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<UpdateWpScanSettings>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    if !(1..=720).contains(&body.interval_hours) {
        return Err(err(StatusCode::BAD_REQUEST, "interval_hours must be 1..=720"));
    }

    for (key, value) in [
        ("wp_vuln_scan_enabled", if body.enabled { "true" } else { "false" }),
        ("wp_vuln_scan_interval_hours", body.interval_hours.to_string().as_str()),
    ] {
        sqlx::query("INSERT INTO settings (key, value) VALUES ($1, $2) \
                     ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value")
            .bind(key)
            .bind(value)
            .execute(&state.db)
            .await
            .map_err(|e| internal_error("save wp scan setting", e))?;
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// The `state_key` a site's WordPress vulnerability alert fires and resolves
/// under. Mirrors `image_scans::image_scan_state_key` — `alerts.state_key` is
/// `VARCHAR(100)` and a domain, while normally well under that, is not a
/// validated field, so the same truncate-then-hash shape applies rather than
/// let a long domain silently fail the INSERT and swallow the alert.
fn wp_vuln_state_key(domain: &str) -> String {
    const MAX: usize = 100;
    let readable = format!("wp_vuln:{domain}");
    if readable.len() <= MAX {
        return readable;
    }
    use sha2::{Digest, Sha256};
    let digest = hex::encode(Sha256::digest(domain.as_bytes()));
    let prefix = "wp_vuln:";
    let budget = MAX - prefix.len() - 17;
    let keep: String = domain.chars().take(budget).collect();
    format!("{prefix}{keep}-{}", &digest[..16])
}

/// Run a WordPress vulnerability scan via the agent, persist it, and fire or
/// resolve the alert it implies. Public so the manual endpoint below and the
/// background sweep (`services::wp_vuln_scanner`) share one path — before
/// this, only the manual button existed, so a critical CVE in a plugin nobody
/// happened to click Scan on stayed invisible forever.
pub async fn scan_and_store(
    pool: &sqlx::PgPool,
    site_id: Uuid,
    user_id: Uuid,
    domain: &str,
    agent: &AgentHandle,
) -> Result<serde_json::Value, ApiError> {
    let result = agent
        .post(&format!("/wordpress/{domain}/vuln-scan"), None::<serde_json::Value>)
        .await
        .map_err(|e| agent_error("Vulnerability scan", e))?;

    let total = result.get("total_vulns").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
    let critical = result.get("critical_count").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
    let high = result.get("high_count").and_then(|v| v.as_i64()).unwrap_or(0) as i32;

    sqlx::query(
        "INSERT INTO wp_vuln_scans (site_id, domain, total_vulns, critical_count, high_count, scan_data) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(site_id)
    .bind(domain)
    .bind(total)
    .bind(critical)
    .bind(high)
    .bind(&result)
    .execute(pool)
    .await
    .map_err(|e| { tracing::warn!("Failed to store scan result: {e}"); })
    .ok();

    // Resolve-then-fire, mirroring `image_scans::scan_and_store`: every scan
    // of this site first clears whatever alert the LAST scan of it raised,
    // then raises a new one only if THIS scan is still dirty. To the site's
    // OWNER, not "every admin" — unlike a shared Docker image, a WordPress
    // site belongs to one user, and `resolve_ssl_renewal_failure`'s own
    // convention for a per-site alert is the site's `user_id`.
    let state_key = wp_vuln_state_key(domain);
    notifications::resolve_alert(
        pool,
        user_id,
        None,
        Some(site_id),
        "wp_vuln_scan",
        &state_key,
        &format!("WordPress vulnerability alert resolved: {domain}"),
        &format!(
            "A later scan of {domain} found no critical or high severity plugin vulnerability — the earlier alert no longer applies."
        ),
    )
    .await;
    if critical > 0 || high > 0 {
        let severity = if critical > 0 { "critical" } else { "warning" };
        let title = format!("WordPress scan: {critical} critical, {high} high on {domain}");
        let message = format!(
            "A vulnerability scan of {domain} found {critical} critical, {high} high severity plugin issues. Review in the WordPress toolkit."
        );
        notifications::fire_alert(
            pool, user_id, None, Some(site_id), "wp_vuln_scan", &state_key, severity, &title, &message,
        )
        .await;
    }

    Ok(result)
}


/// GET /api/sites/{id}/wordpress — Detect WP + get info + auto-update status.
pub async fn info(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    let resp: serde_json::Value = agent
        .get(&format!("/wordpress/{domain}/info"))
        .await
        .map_err(|e| { tracing::warn!("WordPress info failed for {domain}: {e}"); err(StatusCode::BAD_GATEWAY, "WordPress service unavailable") })?;

    // Also get auto-update status
    let auto: serde_json::Value = agent
        .get(&format!("/wordpress/{domain}/auto-update"))
        .await
        .unwrap_or(serde_json::json!({ "enabled": false }));

    let mut result = resp;
    result["auto_update"] = auto
        .get("enabled")
        .cloned()
        .unwrap_or(serde_json::json!(false));

    Ok(Json(result))
}

/// POST /api/sites/{id}/wordpress/install — Install WordPress.
pub async fn install(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    let resp: serde_json::Value = agent
        .post(&format!("/wordpress/{domain}/install"), Some(body))
        .await
        .map_err(|e| agent_error("WordPress", e))?;

    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "wordpress.install",
        Some("site"),
        Some(&domain),
        None,
        None,
    )
    .await;

    Ok((StatusCode::CREATED, Json(resp)))
}

/// GET /api/sites/{id}/wordpress/plugins — List plugins.
pub async fn plugins(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    let resp: serde_json::Value = agent
        .get(&format!("/wordpress/{domain}/plugins"))
        .await
        .map_err(|e| { tracing::warn!("WordPress plugins failed for {domain}: {e}"); err(StatusCode::BAD_GATEWAY, "WordPress service unavailable") })?;

    Ok(Json(resp))
}

/// GET /api/sites/{id}/wordpress/themes — List themes.
pub async fn themes(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    let resp: serde_json::Value = agent
        .get(&format!("/wordpress/{domain}/themes"))
        .await
        .map_err(|e| { tracing::warn!("WordPress themes failed for {domain}: {e}"); err(StatusCode::BAD_GATEWAY, "WordPress service unavailable") })?;

    Ok(Json(resp))
}

/// POST /api/sites/{id}/wordpress/update/{target} — Update core/plugins/themes.
pub async fn update(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path((id, target)): Path<(Uuid, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    if !["core", "plugins", "themes"].contains(&target.as_str()) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid target"));
    }

    let resp: serde_json::Value = agent
        .post(
            &format!("/wordpress/{domain}/update/{target}"),
            None::<serde_json::Value>,
        )
        .await
        .map_err(|e| agent_error("WordPress", e))?;

    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        &format!("wordpress.update.{target}"),
        Some("site"),
        Some(&domain),
        None,
        None,
    )
    .await;

    Ok(Json(resp))
}

/// POST /api/sites/{id}/wordpress/plugin/{action} — Plugin action.
pub async fn plugin_action(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path((id, action)): Path<(Uuid, String)>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    const ALLOWED_PLUGIN_ACTIONS: &[&str] = &["activate", "deactivate", "delete", "update"];
    if !ALLOWED_PLUGIN_ACTIONS.contains(&action.as_str()) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid plugin action"));
    }

    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    let resp: serde_json::Value = agent
        .post(
            &format!("/wordpress/{domain}/plugin/{action}"),
            Some(body),
        )
        .await
        .map_err(|e| agent_error("WordPress", e))?;

    Ok(Json(resp))
}

/// POST /api/sites/{id}/wordpress/theme/{action} — Theme action.
pub async fn theme_action(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path((id, action)): Path<(Uuid, String)>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    const ALLOWED_THEME_ACTIONS: &[&str] = &["activate", "delete", "update"];
    if !ALLOWED_THEME_ACTIONS.contains(&action.as_str()) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid theme action"));
    }

    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    let resp: serde_json::Value = agent
        .post(
            &format!("/wordpress/{domain}/theme/{action}"),
            Some(body),
        )
        .await
        .map_err(|e| agent_error("WordPress", e))?;

    Ok(Json(resp))
}

/// POST /api/sites/{id}/wordpress/auto-update — Toggle auto-updates.
pub async fn set_auto_update(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    let resp: serde_json::Value = agent
        .post(&format!("/wordpress/{domain}/auto-update"), Some(body))
        .await
        .map_err(|e| agent_error("WordPress", e))?;

    Ok(Json(resp))
}

// ---------------------------------------------------------------------------
// WordPress Toolkit endpoints
// ---------------------------------------------------------------------------

/// GET /api/wordpress/sites — List all WordPress sites with overview info.
pub async fn all_wp_sites(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    // No admin check — query already filters by user_id so owners only see their own sites

    // Get all sites for this server
    let sites: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, domain FROM sites WHERE user_id = $1 AND server_id = $2 ORDER BY domain",
    )
    .bind(claims.sub)
    .bind(server_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("all wp sites", e))?;

    let mut wp_sites = Vec::new();

    for (site_id, domain) in &sites {
        // Check if WordPress is installed (quick detect)
        if let Ok(info) = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            agent.get(&format!("/wordpress/{domain}/info")),
        )
        .await
        .unwrap_or(Err(crate::services::agent::AgentError::Request(
            "timeout".into(),
        ))) {
            // It's a WP site
            let version = info
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let update_available = info
                .get("update_available")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            // Get last scan data if exists
            let scan: Option<(i32, i32)> = sqlx::query_as(
                "SELECT total_vulns, critical_count FROM wp_vuln_scans WHERE site_id = $1 ORDER BY scanned_at DESC LIMIT 1",
            )
            .bind(site_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();

            wp_sites.push(serde_json::json!({
                "site_id": site_id,
                "domain": domain,
                "wp_version": version,
                "update_available": update_available,
                "vulns": scan.as_ref().map(|s| s.0).unwrap_or(0),
                "critical_vulns": scan.as_ref().map(|s| s.1).unwrap_or(0),
            }));
        }
    }

    Ok(Json(serde_json::json!(wp_sites)))
}

#[derive(serde::Deserialize)]
pub struct BulkUpdateRequest {
    pub site_ids: Vec<Uuid>,
    pub target: String,
}

/// POST /api/wordpress/bulk-update — Bulk update plugins/themes across sites.
pub async fn bulk_update(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(server_id, agent): ServerScope,
    Json(body): Json<BulkUpdateRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    if !["plugins", "themes", "core", "all"].contains(&body.target.as_str()) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Target must be plugins, themes, core, or all",
        ));
    }

    if body.site_ids.len() > 50 {
        return Err(err(StatusCode::BAD_REQUEST, "Maximum 50 sites per bulk update"));
    }

    let mut results = Vec::new();

    for site_id in &body.site_ids {
        let domain: Option<(String,)> = sqlx::query_as(
            &format!("SELECT s.domain FROM sites s WHERE {} AND s.server_id = $3", crate::helpers::SITE_CALLER_PREDICATE),
        )
        .bind(site_id)
        .bind(claims.sub)
        .bind(server_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();

        let domain = match domain {
            Some((d,)) => d,
            None => {
                results.push(serde_json::json!({"site_id": site_id, "ok": false, "message": "Site not found"}));
                continue;
            }
        };

        match agent
            .post(
                &format!("/wordpress/{domain}/update/{}", body.target),
                None,
            )
            .await
        {
            Ok(r) => {
                let updated = r.get("updated").and_then(|v| v.as_i64()).unwrap_or(0);
                results.push(serde_json::json!({"site_id": site_id, "domain": domain, "ok": true, "updated": updated}));
            }
            Err(e) => {
                results.push(serde_json::json!({"site_id": site_id, "domain": domain, "ok": false, "message": format!("{e}")}));
            }
        }
    }

    Ok(Json(serde_json::json!({ "results": results })))
}

/// POST /api/sites/{id}/wordpress/vuln-scan — Scan a site for vulnerabilities.
pub async fn vuln_scan(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: crate::models::Site = sqlx::query_as(&format!("SELECT s.* FROM sites s WHERE {}", crate::helpers::SITE_CALLER_PREDICATE))
        .bind(id)
        .bind(claims.sub)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("vuln scan", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;
    let agent = crate::helpers::agent_for_site_server(&state, site.server_id, &site.domain).await?;

    let result = scan_and_store(&state.db, id, site.user_id, &site.domain, &agent).await?;

    crate::services::activity::log_activity(
        &state.db, claims.sub, &claims.email, "wordpress.vuln_scan",
        Some("site"), Some(&site.domain), None, None,
    ).await;

    Ok(Json(result))
}

/// GET /api/sites/{id}/wordpress/security-check — Check security hardening.
pub async fn security_check(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: crate::models::Site = sqlx::query_as(&format!("SELECT s.* FROM sites s WHERE {}", crate::helpers::SITE_CALLER_PREDICATE))
        .bind(id)
        .bind(claims.sub)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("security check", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;
    let agent = crate::helpers::agent_for_site_server(&state, site.server_id, &site.domain).await?;

    let result = agent
        .get(&format!("/wordpress/{}/security-check", site.domain))
        .await
        .map_err(|e| agent_error("Security check", e))?;

    // Persist one row per check, matching the sibling `wp_vuln_scans` table
    // this migration created alongside `wp_hardening` — that one gets written
    // from `vuln_scan` below; this table never got its own writer, so every
    // hardening check has always been a pure live round-trip with no history:
    // refresh the page, restart the backend, or come back next week and there
    // is no record of what a site's hardening status was or when it was last
    // checked. `UPSERT` on `(site_id, check_name)` so a re-check updates the
    // existing row rather than accumulating one per run.
    if let Some(checks) = result.as_array() {
        for check in checks {
            let (Some(check_name), Some(status)) =
                (check.get("name").and_then(|v| v.as_str()), check.get("status").and_then(|v| v.as_str()))
            else {
                continue;
            };
            let details = check.get("description").and_then(|v| v.as_str()).unwrap_or("");
            sqlx::query(
                "INSERT INTO wp_hardening (site_id, check_name, status, details, checked_at) \
                 VALUES ($1, $2, $3, $4, NOW()) \
                 ON CONFLICT (site_id, check_name) DO UPDATE SET \
                 status = EXCLUDED.status, details = EXCLUDED.details, checked_at = EXCLUDED.checked_at",
            )
            .bind(id)
            .bind(check_name)
            .bind(status)
            .bind(details)
            .execute(&state.db)
            .await
            .map_err(|e| tracing::warn!("Failed to persist wp_hardening row for {check_name}: {e}"))
            .ok();
        }
    }

    Ok(Json(result))
}

/// GET /api/sites/{id}/wordpress/hardening-history — Last persisted result
/// per check, from `wp_hardening`.
///
/// `security_check` above has written this table on every run since the
/// migration that created it (2026-03-19) — but nothing ever read it back.
/// The symptom the write path's own code comment describes ("refresh the
/// page, restart the backend, or come back next week and there is no record
/// of what a site's hardening status was or when it was last checked") was
/// still literally true of the running product because only the storage
/// half of that fix ever shipped. This is the read half.
#[derive(serde::Serialize, sqlx::FromRow)]
pub struct HardeningHistoryRow {
    check_name: String,
    status: String,
    details: Option<String>,
    checked_at: chrono::DateTime<chrono::Utc>,
}

pub async fn hardening_history(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<HardeningHistoryRow>>, ApiError> {
    let site: crate::models::Site = sqlx::query_as(&format!("SELECT s.* FROM sites s WHERE {}", crate::helpers::SITE_CALLER_PREDICATE))
        .bind(id)
        .bind(claims.sub)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("hardening history", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    let rows: Vec<HardeningHistoryRow> = sqlx::query_as(
        "SELECT check_name, status, details, checked_at FROM wp_hardening \
         WHERE site_id = $1 ORDER BY check_name",
    )
    .bind(site.id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("hardening history", e))?;

    Ok(Json(rows))
}

/// POST /api/sites/{id}/wordpress/harden — Apply security fixes.
pub async fn wp_harden(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: crate::models::Site = sqlx::query_as(&format!("SELECT s.* FROM sites s WHERE {}", crate::helpers::SITE_CALLER_PREDICATE))
        .bind(id)
        .bind(claims.sub)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("wp harden", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;
    let agent = crate::helpers::agent_for_site_server(&state, site.server_id, &site.domain).await?;

    let result = agent
        .post(&format!("/wordpress/{}/harden", site.domain), Some(body))
        .await
        .map_err(|e| agent_error("Security hardening", e))?;

    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "wordpress.harden",
        Some("site"),
        Some(&site.domain),
        None,
        None,
    )
    .await;

    Ok(Json(result))
}

/// POST /api/sites/{id}/wordpress/update-safe — Update WP with snapshot + auto-rollback.
pub async fn update_safe(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    let result = agent
        .post_long(
            &format!("/wordpress/{domain}/update-with-rollback"),
            None,
            300,
        )
        .await
        .map_err(|e| agent_error("WP safe update", e))?;

    let rolled_back = result.get("rolled_back").and_then(|v| v.as_bool()).unwrap_or(false);
    let action = if rolled_back { "wordpress.update.rollback" } else { "wordpress.update.safe" };

    activity::log_activity(
        &state.db, claims.sub, &claims.email,
        action, Some("site"), Some(&domain), None, None,
    ).await;

    Ok(Json(result))
}
