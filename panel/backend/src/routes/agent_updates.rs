use axum::{
    extract::State,
    Json,
};

use crate::auth::AuthUser;
use crate::error::{internal_error, ApiError};
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
    .map_err(|e| internal_error("latest version", e))?;

    let download_url: Option<(String,)> = sqlx::query_as(
        "SELECT value FROM settings WHERE key = 'agent_download_url'",
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("latest version", e))?;

    let checksum: Option<(String,)> = sqlx::query_as(
        "SELECT value FROM settings WHERE key = 'agent_checksum'",
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("latest version", e))?;

    Ok(Json(serde_json::json!({
        "version": version.map(|v| v.0).unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string()),
        "download_url": download_url.map(|v| v.0),
        "checksum": checksum.map(|v| v.0),
    })))
}
