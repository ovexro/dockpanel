use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{agent_error, err, ApiError};
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
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    Ok(site.domain)
}

/// GET /api/sites/{id}/wordpress — Detect WP + get info + auto-update status.
pub async fn info(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = site_domain(&state, id, claims.sub).await?;

    let resp: serde_json::Value = state
        .agent
        .get(&format!("/wordpress/{domain}/info"))
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &e.to_string()))?;

    // Also get auto-update status
    let auto: serde_json::Value = state
        .agent
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
    let domain = site_domain(&state, id, claims.sub).await?;

    let resp: serde_json::Value = state
        .agent
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
    let domain = site_domain(&state, id, claims.sub).await?;

    let resp: serde_json::Value = state
        .agent
        .get(&format!("/wordpress/{domain}/plugins"))
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &e.to_string()))?;

    Ok(Json(resp))
}

/// GET /api/sites/{id}/wordpress/themes — List themes.
pub async fn themes(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = site_domain(&state, id, claims.sub).await?;

    let resp: serde_json::Value = state
        .agent
        .get(&format!("/wordpress/{domain}/themes"))
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &e.to_string()))?;

    Ok(Json(resp))
}

/// POST /api/sites/{id}/wordpress/update/{target} — Update core/plugins/themes.
pub async fn update(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path((id, target)): Path<(Uuid, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = site_domain(&state, id, claims.sub).await?;

    if !["core", "plugins", "themes"].contains(&target.as_str()) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid target"));
    }

    let resp: serde_json::Value = state
        .agent
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
    let domain = site_domain(&state, id, claims.sub).await?;

    let resp: serde_json::Value = state
        .agent
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
    let domain = site_domain(&state, id, claims.sub).await?;

    let resp: serde_json::Value = state
        .agent
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
    let domain = site_domain(&state, id, claims.sub).await?;

    let resp: serde_json::Value = state
        .agent
        .post(&format!("/wordpress/{domain}/auto-update"), Some(body))
        .await
        .map_err(|e| agent_error("WordPress", e))?;

    Ok(Json(resp))
}
