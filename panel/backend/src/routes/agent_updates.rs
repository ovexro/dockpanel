use axum::{
    extract::State,
    http::StatusCode,
    Json,
};

use crate::auth::AuthUser;
use crate::error::{err, ApiError};
use crate::AppState;

/// GET /api/agent/version — Returns the latest agent version info.
/// Requires authentication.
pub async fn latest_version(
    State(state): State<AppState>,
    AuthUser(_claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Read version from settings, or return current agent version
    let version: Option<(String,)> = sqlx::query_as(
        "SELECT value FROM settings WHERE key = 'agent_latest_version'",
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let download_url: Option<(String,)> = sqlx::query_as(
        "SELECT value FROM settings WHERE key = 'agent_download_url'",
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let checksum: Option<(String,)> = sqlx::query_as(
        "SELECT value FROM settings WHERE key = 'agent_checksum'",
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(serde_json::json!({
        "version": version.map(|v| v.0).unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string()),
        "download_url": download_url.map(|v| v.0),
        "checksum": checksum.map(|v| v.0),
    })))
}
