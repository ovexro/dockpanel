use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use std::collections::HashMap;

use crate::auth::AuthUser;
use crate::error::{err, agent_error, require_admin, ApiError};
use crate::routes::{is_valid_container_id, is_valid_name};
use crate::services::activity;
use crate::AppState;

#[derive(serde::Deserialize)]
pub struct DeployRequest {
    pub template_id: String,
    pub name: String,
    pub port: u16,
    pub env: Option<HashMap<String, String>>,
}

/// GET /api/apps/templates — List available app templates.
pub async fn list_templates(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let result = state
        .agent
        .get("/apps/templates")
        .await
        .map_err(|e| agent_error("Docker apps", e))?;

    Ok(Json(result))
}

/// POST /api/apps/deploy — Deploy a Docker app from template.
pub async fn deploy(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<DeployRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    require_admin(&claims.role)?;

    if !is_valid_name(&body.name) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid app name"));
    }

    if body.port == 0 {
        return Err(err(StatusCode::BAD_REQUEST, "Port must be between 1 and 65535"));
    }

    // Validate env vars: max 50 vars, max 4KB per value
    if let Some(ref env) = body.env {
        if env.len() > 50 {
            return Err(err(StatusCode::BAD_REQUEST, "Too many environment variables (max 50)"));
        }
        for (key, value) in env {
            if key.is_empty() || key.len() > 255 {
                return Err(err(StatusCode::BAD_REQUEST, "Invalid environment variable name"));
            }
            if value.len() > 4096 {
                return Err(err(StatusCode::BAD_REQUEST, "Environment variable value too large (max 4KB)"));
            }
        }
    }

    let agent_body = serde_json::json!({
        "template_id": body.template_id,
        "name": body.name,
        "port": body.port,
        "env": body.env.unwrap_or_default(),
    });

    let result = state
        .agent
        .post("/apps/deploy", Some(agent_body))
        .await
        .map_err(|e| agent_error("Docker deploy", e))?;

    tracing::info!("App deployed: {} ({})", body.name, body.template_id);
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "app.deploy",
        Some("app"), Some(&body.name), Some(&body.template_id), None,
    ).await;

    Ok((StatusCode::CREATED, Json(result)))
}

/// GET /api/apps — List deployed Docker apps.
pub async fn list_apps(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let result = state
        .agent
        .get("/apps")
        .await
        .map_err(|e| agent_error("Docker apps", e))?;

    Ok(Json(result))
}

/// POST /api/apps/{container_id}/stop — Stop an app.
pub async fn stop_app(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }

    let agent_path = format!("/apps/{}/stop", container_id);
    state
        .agent
        .post(&agent_path, None)
        .await
        .map_err(|e| agent_error("Container stop", e))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/apps/{container_id}/start — Start an app.
pub async fn start_app(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }

    let agent_path = format!("/apps/{}/start", container_id);
    state
        .agent
        .post(&agent_path, None)
        .await
        .map_err(|e| agent_error("Container start", e))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/apps/{container_id}/restart — Restart an app.
pub async fn restart_app(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }

    let agent_path = format!("/apps/{}/restart", container_id);
    state
        .agent
        .post(&agent_path, None)
        .await
        .map_err(|e| agent_error("Container restart", e))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /api/apps/{container_id}/logs — Get app logs.
pub async fn app_logs(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }

    let agent_path = format!("/apps/{}/logs", container_id);
    let result = state
        .agent
        .get(&agent_path)
        .await
        .map_err(|e| agent_error("Container logs", e))?;

    Ok(Json(result))
}

/// POST /api/apps/{container_id}/update — Pull latest image and recreate container.
pub async fn update_app(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }

    let agent_path = format!("/apps/{}/update", container_id);
    let result = state
        .agent
        .post(&agent_path, None)
        .await
        .map_err(|e| agent_error("Container update", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "app.update",
        Some("app"), Some(&container_id), None, None,
    ).await;

    Ok(Json(result))
}

/// GET /api/apps/{container_id}/env — Get container environment variables.
pub async fn app_env(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }

    let agent_path = format!("/apps/{}/env", container_id);
    let result = state
        .agent
        .get(&agent_path)
        .await
        .map_err(|e| agent_error("Container env", e))?;

    Ok(Json(result))
}

/// POST /api/apps/compose/parse — Parse docker-compose.yml and preview services.
pub async fn compose_parse(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let yaml = body["yaml"]
        .as_str()
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing 'yaml' field"))?;

    if yaml.len() > 65536 {
        return Err(err(StatusCode::BAD_REQUEST, "YAML too large (max 64KB)"));
    }

    let result = state
        .agent
        .post("/apps/compose/parse", Some(serde_json::json!({ "yaml": yaml })))
        .await
        .map_err(|e| agent_error("Compose parse", e))?;

    Ok(Json(result))
}

/// POST /api/apps/compose/deploy — Deploy services from docker-compose.yml.
pub async fn compose_deploy(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<serde_json::Value>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    require_admin(&claims.role)?;

    let yaml = body["yaml"]
        .as_str()
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing 'yaml' field"))?;

    if yaml.len() > 65536 {
        return Err(err(StatusCode::BAD_REQUEST, "YAML too large (max 64KB)"));
    }

    let result = state
        .agent
        .post("/apps/compose/deploy", Some(serde_json::json!({ "yaml": yaml })))
        .await
        .map_err(|e| agent_error("Docker deploy", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "app.compose_deploy",
        Some("app"), None, Some("compose"), None,
    ).await;

    Ok((StatusCode::CREATED, Json(result)))
}

/// DELETE /api/apps/{container_id} — Remove a deployed app.
pub async fn remove_app(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }

    let agent_path = format!("/apps/{}", container_id);
    state
        .agent
        .delete(&agent_path)
        .await
        .map_err(|e| agent_error("Container removal", e))?;

    tracing::info!("App removed: {}", container_id);
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "app.remove",
        Some("app"), Some(&container_id), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}
