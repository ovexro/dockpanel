use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::{AdminUser, AuthUser, ServerScope};
use crate::error::{internal_error, err, agent_error, ApiError};
use crate::models::Site;
use crate::AppState;
use crate::services::activity;

/// POST /api/sites/{id}/ssl — Provision SSL certificate for a site.
pub async fn provision(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: Site = sqlx::query_as("SELECT * FROM sites WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(claims.sub)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("provision", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    if site.status != "active" {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Site must be active before provisioning SSL",
        ));
    }

    if site.ssl_enabled {
        return Err(err(StatusCode::CONFLICT, "SSL is already enabled"));
    }

    // Get admin email for ACME registration
    let (email,): (String,) =
        sqlx::query_as("SELECT email FROM users WHERE id = $1")
            .bind(claims.sub)
            .fetch_one(&state.db)
            .await
            .map_err(|e| internal_error("provision", e))?;

    // Build agent request
    let mut agent_body = serde_json::json!({
        "email": email,
        "runtime": site.runtime,
    });

    if let Some(port) = site.proxy_port {
        agent_body["proxy_port"] = serde_json::json!(port);
    }
    if let Some(ref php) = site.php_version {
        agent_body["php_socket"] = serde_json::json!(format!("/run/php/php{php}-fpm.sock"));
    }
    if let Some(ref root) = site.root_path {
        agent_body["root"] = serde_json::json!(root);
    }

    // Call agent to provision SSL
    let agent_path = format!("/ssl/provision/{}", site.domain);
    let result = agent
        .post(&agent_path, Some(agent_body))
        .await
        .map_err(|e| agent_error("SSL provisioning", e))?;

    // Parse expiry from agent response
    let ssl_expiry = result
        .get("expiry")
        .and_then(|v| v.as_str())
        .and_then(|s| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f UTC").ok())
        .map(|dt| dt.and_utc());

    if ssl_expiry.is_none() {
        tracing::warn!(
            "Could not parse SSL expiry for site {} (domain: {}). Raw value: {:?}",
            id, site.domain, result.get("expiry")
        );
    }

    let cert_path = result
        .get("cert_path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let key_path = result
        .get("key_path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Update site in DB
    sqlx::query(
        "UPDATE sites SET ssl_enabled = true, ssl_cert_path = $1, ssl_key_path = $2, \
         ssl_expiry = $3, updated_at = NOW() WHERE id = $4",
    )
    .bind(&cert_path)
    .bind(&key_path)
    .bind(ssl_expiry)
    .bind(id)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("provision", e))?;

    tracing::info!("SSL provisioned for {}", site.domain);

    // GAP 15: Auto-activate paused monitors now that SSL/DNS is working
    let _ = sqlx::query(
        "UPDATE monitors SET enabled = TRUE WHERE site_id = $1 AND enabled = FALSE AND status = 'pending'"
    )
    .bind(id)
    .execute(&state.db)
    .await;

    Ok(Json(serde_json::json!({
        "ok": true,
        "domain": site.domain,
        "ssl_enabled": true,
        "cert_path": cert_path,
        "expiry": ssl_expiry,
    })))
}

/// GET /api/sites/{id}/ssl — Get SSL status for a site.
pub async fn status(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: Site = sqlx::query_as("SELECT * FROM sites WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(claims.sub)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("status", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    // Also fetch live status from agent
    let agent_path = format!("/ssl/status/{}", site.domain);
    let agent_status = agent.get(&agent_path).await.ok();

    Ok(Json(serde_json::json!({
        "ssl_enabled": site.ssl_enabled,
        "cert_path": site.ssl_cert_path,
        "key_path": site.ssl_key_path,
        "expiry": site.ssl_expiry,
        "agent_status": agent_status,
    })))
}

/// POST /api/ssl/{id}/renew — Force-renew SSL certificate (admin only).
pub async fn renew(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: Site = sqlx::query_as("SELECT * FROM sites WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("ssl renew", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    if !site.ssl_enabled {
        return Err(err(StatusCode::BAD_REQUEST, "SSL is not enabled for this site"));
    }

    let agent_path = format!("/ssl/{}/renew", site.domain);
    agent
        .post_long(&agent_path, None, 120)
        .await
        .map_err(|e| agent_error("SSL renewal", e))?;

    // Refresh expiry from agent status
    let status_path = format!("/ssl/status/{}", site.domain);
    if let Ok(status) = agent.get(&status_path).await {
        if let Some(expiry_str) = status.get("not_after").and_then(|v| v.as_str()) {
            if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(expiry_str, "%Y-%m-%d %H:%M:%S%.f UTC") {
                let expiry = dt.and_utc();
                let _ = sqlx::query("UPDATE sites SET ssl_expiry = $1, updated_at = NOW() WHERE id = $2")
                    .bind(expiry)
                    .bind(id)
                    .execute(&state.db)
                    .await;
            }
        }
    }

    tracing::info!("SSL renewed for {} by {}", site.domain, claims.email);
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "ssl.renew",
        Some("site"), Some(&site.domain), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true, "domain": site.domain })))
}

/// DELETE /api/ssl/{id} — Revoke and delete SSL certificate (admin only).
pub async fn revoke(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: Site = sqlx::query_as("SELECT * FROM sites WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("ssl revoke", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    if !site.ssl_enabled {
        return Err(err(StatusCode::BAD_REQUEST, "SSL is not enabled for this site"));
    }

    let agent_path = format!("/ssl/{}", site.domain);
    agent
        .delete(&agent_path)
        .await
        .map_err(|e| agent_error("SSL deletion", e))?;

    // Clear SSL fields in DB
    sqlx::query(
        "UPDATE sites SET ssl_enabled = false, ssl_cert_path = NULL, ssl_key_path = NULL, \
         ssl_expiry = NULL, updated_at = NOW() WHERE id = $1",
    )
    .bind(id)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("ssl revoke", e))?;

    tracing::info!("SSL revoked for {} by {}", site.domain, claims.email);
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "ssl.revoke",
        Some("site"), Some(&site.domain), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true, "domain": site.domain })))
}
