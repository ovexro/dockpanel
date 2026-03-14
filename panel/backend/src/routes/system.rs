use axum::{extract::State, Json};

use crate::auth::{AdminUser, AuthUser};
use crate::error::{agent_error, ApiError};
use crate::services::activity;
use crate::AppState;

/// GET /api/health — Public health check.
pub async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "service": "dockpanel-api",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

/// GET /api/system/info — Proxy to agent's system info (authenticated).
pub async fn info(
    State(state): State<AppState>,
    AuthUser(_claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = state
        .agent
        .get("/system/info")
        .await
        .map_err(|e| agent_error("System info", e))?;
    Ok(Json(data))
}

/// GET /api/agent/diagnostics — Proxy to agent's diagnostics (authenticated).
pub async fn diagnostics(
    State(state): State<AppState>,
    AuthUser(_claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = state
        .agent
        .get("/diagnostics")
        .await
        .map_err(|e| agent_error("Diagnostics", e))?;
    Ok(Json(data))
}

/// POST /api/agent/diagnostics/fix — Proxy to agent's diagnostics fix (admin).
pub async fn diagnostics_fix(
    State(state): State<AppState>,
    crate::auth::AdminUser(_claims): crate::auth::AdminUser,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = state
        .agent
        .post("/diagnostics/fix", Some(body))
        .await
        .map_err(|e| agent_error("Diagnostics fix", e))?;
    Ok(Json(data))
}

/// GET /api/system/updates — List available package updates (admin only).
pub async fn updates_list(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = state
        .agent
        .get("/system/updates")
        .await
        .map_err(|e| agent_error("System updates", e))?;
    Ok(Json(data))
}

/// POST /api/system/updates/apply — Apply package updates (admin only).
pub async fn updates_apply(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = state
        .agent
        .post("/system/updates/apply", Some(body))
        .await
        .map_err(|e| agent_error("Apply updates", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "system.updates.apply",
        Some("system"), Some("packages"), None, None,
    ).await;

    Ok(Json(data))
}

/// GET /api/system/updates/count — Get count of available updates (any authenticated user).
pub async fn updates_count(
    State(state): State<AppState>,
    AuthUser(_claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = state
        .agent
        .get("/system/updates/count")
        .await
        .map_err(|e| agent_error("Update count", e))?;
    Ok(Json(data))
}

/// POST /api/system/reboot — Reboot the system (admin only).
pub async fn system_reboot(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = state
        .agent
        .post("/system/reboot", None::<serde_json::Value>)
        .await
        .map_err(|e| agent_error("System reboot", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "system.reboot",
        Some("system"), Some("server"), None, None,
    ).await;

    Ok(Json(data))
}
