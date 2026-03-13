use axum::{extract::State, http::StatusCode, Json};

use crate::auth::AuthUser;
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
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    match state.agent.get("/system/info").await {
        Ok(data) => Ok(Json(data)),
        Err(e) => {
            tracing::error!("Agent system info error: {e}");
            Err((
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": format!("Agent unavailable: {e}") })),
            ))
        }
    }
}
