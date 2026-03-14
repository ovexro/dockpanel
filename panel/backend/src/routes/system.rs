use axum::{extract::State, Json};

use crate::auth::AuthUser;
use crate::error::{agent_error, ApiError};
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
