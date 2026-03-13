use axum::{
    extract::Path,
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
use serde::Deserialize;
use std::collections::HashMap;

use super::{is_valid_container_id, is_valid_name, AppState};
use crate::services::compose;
use crate::services::docker_apps;

#[derive(Deserialize)]
struct DeployRequest {
    template_id: String,
    name: String,
    port: u16,
    #[serde(default)]
    env: HashMap<String, String>,
}

/// GET /apps/templates — List all available app templates.
async fn templates() -> Json<Vec<docker_apps::AppTemplate>> {
    Json(docker_apps::list_templates())
}

/// POST /apps/deploy — Deploy an app from a template.
async fn deploy(
    Json(body): Json<DeployRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_name(&body.name) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid app name" })),
        ));
    }

    let result =
        docker_apps::deploy_app(&body.template_id, &body.name, body.port, body.env)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e })),
                )
            })?;

    Ok(Json(serde_json::json!({
        "success": true,
        "container_id": result.container_id,
        "name": result.name,
        "port": result.port,
    })))
}

/// GET /apps — List all deployed apps.
async fn list() -> Result<Json<Vec<docker_apps::DeployedApp>>, (StatusCode, Json<serde_json::Value>)>
{
    let apps = docker_apps::list_deployed_apps().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
    })?;

    Ok(Json(apps))
}

/// POST /apps/{container_id}/stop — Stop a running app.
async fn stop(
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container ID" })),
        ));
    }

    docker_apps::stop_app(&container_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
        })?;

    Ok(Json(serde_json::json!({ "success": true })))
}

/// POST /apps/{container_id}/start — Start a stopped app.
async fn start(
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container ID" })),
        ));
    }

    docker_apps::start_app(&container_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
        })?;

    Ok(Json(serde_json::json!({ "success": true })))
}

/// POST /apps/{container_id}/restart — Restart an app.
async fn restart(
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container ID" })),
        ));
    }

    docker_apps::restart_app(&container_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
        })?;

    Ok(Json(serde_json::json!({ "success": true })))
}

/// GET /apps/{container_id}/logs — Get app container logs.
async fn logs(
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container ID" })),
        ));
    }

    let output = docker_apps::get_app_logs(&container_id, 200)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
        })?;

    Ok(Json(serde_json::json!({ "logs": output })))
}

/// DELETE /apps/{container_id} — Remove a deployed app.
async fn remove(
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container ID" })),
        ));
    }

    docker_apps::remove_app(&container_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
        })?;

    Ok(Json(serde_json::json!({ "success": true })))
}

#[derive(Deserialize)]
struct ComposeParseRequest {
    yaml: String,
}

/// POST /apps/compose/parse — Parse docker-compose.yml and return services preview.
async fn compose_parse(
    Json(body): Json<ComposeParseRequest>,
) -> Result<Json<Vec<compose::ComposeService>>, (StatusCode, Json<serde_json::Value>)> {
    let services = compose::parse_compose(&body.yaml).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e })),
        )
    })?;

    Ok(Json(services))
}

/// POST /apps/compose/deploy — Deploy services from parsed compose file.
async fn compose_deploy(
    Json(body): Json<ComposeParseRequest>,
) -> Result<Json<compose::ComposeDeployResult>, (StatusCode, Json<serde_json::Value>)> {
    let services = compose::parse_compose(&body.yaml).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e })),
        )
    })?;

    let result = compose::deploy_compose(&services).await;
    Ok(Json(result))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/apps/templates", get(templates))
        .route("/apps/deploy", post(deploy))
        .route("/apps/compose/parse", post(compose_parse))
        .route("/apps/compose/deploy", post(compose_deploy))
        .route("/apps", get(list))
        .route("/apps/{container_id}", delete(remove))
        .route("/apps/{container_id}/stop", post(stop))
        .route("/apps/{container_id}/start", post(start))
        .route("/apps/{container_id}/restart", post(restart))
        .route("/apps/{container_id}/logs", get(logs))
}
