use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::{AuthUser, ServerScope};
use crate::error::{internal_error, err, agent_error, ApiError};
use crate::models::Site;
use crate::routes::is_valid_domain;
use crate::services::activity;
use crate::AppState;

#[derive(serde::Deserialize)]
pub struct CreateStagingRequest {
    /// Custom staging domain (optional). Defaults to staging.{parent_domain}.
    pub domain: Option<String>,
}

/// Helper to fetch a site with ownership check.
async fn get_site(state: &AppState, id: Uuid, user_id: Uuid) -> Result<Site, ApiError> {
    sqlx::query_as::<_, Site>("SELECT * FROM sites WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("unknown", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))
}

/// POST /api/sites/{id}/staging — Create a staging environment.
///
/// 1. Validate parent site (must be active, not already a staging site)
/// 2. Generate or validate staging domain
/// 3. Create site record with parent_site_id
/// 4. Create nginx config via agent
/// 5. Clone files from production
pub async fn create(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
    Json(body): Json<CreateStagingRequest>,
) -> Result<(StatusCode, Json<Site>), ApiError> {
    let parent = get_site(&state, id, claims.sub).await?;

    if parent.status != "active" {
        return Err(err(StatusCode::BAD_REQUEST, "Parent site must be active"));
    }
    if parent.parent_site_id.is_some() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Cannot create staging from a staging site",
        ));
    }

    // Check if staging already exists for this site
    let existing: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM sites WHERE parent_site_id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("create staging", e))?;

    if existing.is_some() {
        return Err(err(
            StatusCode::CONFLICT,
            "A staging environment already exists for this site",
        ));
    }

    // Determine staging domain
    let staging_domain = match body.domain {
        Some(ref d) if !d.is_empty() => {
            if !is_valid_domain(d) {
                return Err(err(StatusCode::BAD_REQUEST, "Invalid staging domain format"));
            }
            d.clone()
        }
        _ => format!("staging.{}", parent.domain),
    };

    // Check domain uniqueness
    let dup: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM sites WHERE domain = $1")
        .bind(&staging_domain)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("create staging", e))?;

    if dup.is_some() {
        return Err(err(StatusCode::CONFLICT, "Staging domain already in use"));
    }

    // Insert staging site
    let staging: Site = sqlx::query_as(
        "INSERT INTO sites (user_id, server_id, domain, runtime, status, proxy_port, php_version, parent_site_id) \
         VALUES ($1, $2, $3, $4, 'creating', $5, $6, $7) RETURNING *",
    )
    .bind(claims.sub)
    .bind(server_id)
    .bind(&staging_domain)
    .bind(&parent.runtime)
    .bind(parent.proxy_port)
    .bind(&parent.php_version)
    .bind(id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("create staging", e))?;

    // Build agent request to create nginx config (same as regular site creation)
    let mut agent_body = serde_json::json!({
        "runtime": parent.runtime,
    });
    if let Some(port) = parent.proxy_port {
        agent_body["proxy_port"] = serde_json::json!(port);
    }
    if let Some(ref php) = parent.php_version {
        agent_body["php_socket"] =
            serde_json::json!(format!("unix:/run/php/php{php}-fpm.sock"));
    }

    let agent_path = format!("/nginx/sites/{}", staging_domain);
    if let Err(e) = agent.put(&agent_path, agent_body).await {
        tracing::error!("Agent error creating staging site {staging_domain}: {e}");
        sqlx::query("UPDATE sites SET status = 'error', updated_at = NOW() WHERE id = $1")
            .bind(staging.id)
            .execute(&state.db)
            .await
            .ok();
        return Err(agent_error("Staging configuration", e));
    }

    // Clone files from production to staging
    let clone_result = agent
        .post(
            "/staging/clone",
            Some(serde_json::json!({
                "source": parent.domain,
                "target": staging_domain,
            })),
        )
        .await;

    let synced_at = if clone_result.is_ok() {
        Some("NOW()")
    } else {
        tracing::warn!("File clone failed for staging {staging_domain}: {:?}", clone_result);
        None
    };

    // Update status to active
    let update_sql = if synced_at.is_some() {
        "UPDATE sites SET status = 'active', synced_at = NOW(), updated_at = NOW() WHERE id = $1 RETURNING *"
    } else {
        "UPDATE sites SET status = 'active', updated_at = NOW() WHERE id = $1 RETURNING *"
    };

    let updated: Site = sqlx::query_as(update_sql)
        .bind(staging.id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| internal_error("create staging", e))?;

    tracing::info!(
        "Staging created: {} → {} ({})",
        parent.domain,
        staging_domain,
        parent.runtime
    );
    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "staging.create",
        Some("site"),
        Some(&staging_domain),
        Some(&parent.domain),
        None,
    )
    .await;

    Ok((StatusCode::CREATED, Json(updated)))
}

/// GET /api/sites/{id}/staging — Get staging site for a production site.
pub async fn get_staging(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Verify parent site ownership
    let _parent = get_site(&state, id, claims.sub).await?;

    let staging: Option<Site> =
        sqlx::query_as("SELECT * FROM sites WHERE parent_site_id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("get staging", e))?;

    match staging {
        Some(s) => {
            // Get disk usage
            let usage = agent
                .post(
                    "/staging/disk-usage",
                    Some(serde_json::json!({ "domain": s.domain })),
                )
                .await
                .ok()
                .and_then(|v| v.get("bytes").and_then(|b| b.as_u64()))
                .unwrap_or(0);

            Ok(Json(serde_json::json!({
                "exists": true,
                "site": s,
                "disk_usage_bytes": usage,
            })))
        }
        None => Ok(Json(serde_json::json!({ "exists": false }))),
    }
}

/// POST /api/sites/{id}/staging/sync — Sync production files → staging.
pub async fn sync_to_staging(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let parent = get_site(&state, id, claims.sub).await?;

    let staging: Site =
        sqlx::query_as("SELECT * FROM sites WHERE parent_site_id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("sync to staging", e))?
            .ok_or_else(|| err(StatusCode::NOT_FOUND, "No staging environment found"))?;

    agent
        .post(
            "/staging/sync",
            Some(serde_json::json!({
                "source": parent.domain,
                "target": staging.domain,
            })),
        )
        .await
        .map_err(|e| agent_error("Staging sync", e))?;

    // Update synced_at timestamp
    sqlx::query("UPDATE sites SET synced_at = NOW(), updated_at = NOW() WHERE id = $1")
        .bind(staging.id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("sync to staging", e))?;

    tracing::info!("Synced {} → {}", parent.domain, staging.domain);
    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "staging.sync",
        Some("site"),
        Some(&staging.domain),
        Some(&format!("{} → {}", parent.domain, staging.domain)),
        None,
    )
    .await;

    Ok(Json(serde_json::json!({ "ok": true, "message": format!("Synced {} → {}", parent.domain, staging.domain) })))
}

/// POST /api/sites/{id}/staging/push — Push staging files → production.
pub async fn push_to_prod(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let parent = get_site(&state, id, claims.sub).await?;

    let staging: Site =
        sqlx::query_as("SELECT * FROM sites WHERE parent_site_id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("push to prod", e))?
            .ok_or_else(|| err(StatusCode::NOT_FOUND, "No staging environment found"))?;

    agent
        .post(
            "/staging/sync",
            Some(serde_json::json!({
                "source": staging.domain,
                "target": parent.domain,
            })),
        )
        .await
        .map_err(|e| agent_error("Staging push", e))?;

    tracing::info!("Pushed {} → {}", staging.domain, parent.domain);
    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "staging.push",
        Some("site"),
        Some(&staging.domain),
        Some(&format!("{} → {}", staging.domain, parent.domain)),
        None,
    )
    .await;

    Ok(Json(serde_json::json!({ "ok": true, "message": format!("Pushed {} → {}", staging.domain, parent.domain) })))
}

/// DELETE /api/sites/{id}/staging — Delete the staging environment.
pub async fn destroy(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _parent = get_site(&state, id, claims.sub).await?;

    let staging: Site =
        sqlx::query_as("SELECT * FROM sites WHERE parent_site_id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("destroy", e))?
            .ok_or_else(|| err(StatusCode::NOT_FOUND, "No staging environment found"))?;

    // Remove nginx config
    let agent_path = format!("/nginx/sites/{}", staging.domain);
    agent
        .delete(&agent_path)
        .await
        .map_err(|e| agent_error("Staging removal", e))?;

    // Delete site files
    agent
        .post(
            "/staging/delete-files",
            Some(serde_json::json!({ "domain": staging.domain })),
        )
        .await
        .ok(); // Best-effort file cleanup

    // Delete from DB
    sqlx::query("DELETE FROM sites WHERE id = $1")
        .bind(staging.id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("destroy", e))?;

    tracing::info!("Staging deleted: {}", staging.domain);
    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "staging.delete",
        Some("site"),
        Some(&staging.domain),
        None,
        None,
    )
    .await;

    Ok(Json(serde_json::json!({ "ok": true, "domain": staging.domain })))
}
