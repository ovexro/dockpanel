use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::{AuthUser, ServerScope};
use crate::error::{internal_error, agent_error, err, require_admin, ApiError};
use crate::services::activity;
use crate::AppState;

/// Helper: get site domain after verifying ownership.
async fn site_domain(state: &AppState, id: Uuid, user_id: Uuid) -> Result<String, ApiError> {
    let site = sqlx::query_as::<_, crate::models::Site>(
        "SELECT * FROM sites WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("unknown", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    Ok(site.domain)
}

/// GET /api/sites/{id}/wordpress — Detect WP + get info + auto-update status.
pub async fn info(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = site_domain(&state, id, claims.sub).await?;

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
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let domain = site_domain(&state, id, claims.sub).await?;

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
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = site_domain(&state, id, claims.sub).await?;

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
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = site_domain(&state, id, claims.sub).await?;

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
    ServerScope(_server_id, agent): ServerScope,
    Path((id, target)): Path<(Uuid, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = site_domain(&state, id, claims.sub).await?;

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
    ServerScope(_server_id, agent): ServerScope,
    Path((id, action)): Path<(Uuid, String)>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    const ALLOWED_PLUGIN_ACTIONS: &[&str] = &["activate", "deactivate", "delete", "update"];
    if !ALLOWED_PLUGIN_ACTIONS.contains(&action.as_str()) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid plugin action"));
    }

    let domain = site_domain(&state, id, claims.sub).await?;

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
    ServerScope(_server_id, agent): ServerScope,
    Path((id, action)): Path<(Uuid, String)>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    const ALLOWED_THEME_ACTIONS: &[&str] = &["activate", "delete", "update"];
    if !ALLOWED_THEME_ACTIONS.contains(&action.as_str()) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid theme action"));
    }

    let domain = site_domain(&state, id, claims.sub).await?;

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
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = site_domain(&state, id, claims.sub).await?;

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
            "SELECT domain FROM sites WHERE id = $1 AND user_id = $2 AND server_id = $3",
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
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: crate::models::Site = sqlx::query_as("SELECT * FROM sites WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(claims.sub)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("vuln scan", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    let result = agent
        .post(&format!("/wordpress/{}/vuln-scan", site.domain), None)
        .await
        .map_err(|e| agent_error("Vulnerability scan", e))?;

    let total = result
        .get("total_vulns")
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as i32;
    let critical = result
        .get("critical_count")
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as i32;
    let high = result
        .get("high_count")
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as i32;

    // Store scan result
    sqlx::query(
        "INSERT INTO wp_vuln_scans (site_id, domain, total_vulns, critical_count, high_count, scan_data) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(&site.domain)
    .bind(total)
    .bind(critical)
    .bind(high)
    .bind(&result)
    .execute(&state.db)
    .await
    .map_err(|e| { tracing::warn!("Failed to store scan result: {e}"); })
    .ok();

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
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: crate::models::Site = sqlx::query_as("SELECT * FROM sites WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(claims.sub)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("security check", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    let result = agent
        .get(&format!("/wordpress/{}/security-check", site.domain))
        .await
        .map_err(|e| agent_error("Security check", e))?;

    Ok(Json(result))
}

/// POST /api/sites/{id}/wordpress/harden — Apply security fixes.
pub async fn wp_harden(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: crate::models::Site = sqlx::query_as("SELECT * FROM sites WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(claims.sub)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("wp harden", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

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
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = site_domain(&state, id, claims.sub).await?;

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
