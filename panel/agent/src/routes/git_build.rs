use axum::{
    extract::State,
    http::StatusCode,
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use std::collections::HashMap;

use super::AppState;
use crate::services::{deploy, git_build};

type ApiErr = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: &str) -> ApiErr {
    (status, Json(serde_json::json!({ "error": msg })))
}

#[derive(Deserialize)]
struct CloneRequest {
    name: String,
    repo_url: String,
    branch: String,
    key_path: Option<String>,
}

#[derive(Deserialize)]
struct BuildRequest {
    name: String,
    #[serde(default = "default_dockerfile")]
    dockerfile: String,
    commit_hash: String,
}

fn default_dockerfile() -> String {
    "Dockerfile".to_string()
}

#[derive(Deserialize)]
struct DeployRequest {
    name: String,
    image_tag: String,
    container_port: u16,
    host_port: u16,
    #[serde(default)]
    env: HashMap<String, String>,
    domain: Option<String>,
}

#[derive(Deserialize)]
struct KeygenRequest {
    name: String,
}

#[derive(Deserialize)]
struct CleanupRequest {
    name: String,
}

#[derive(Deserialize)]
struct PruneRequest {
    name: String,
    #[serde(default = "default_keep")]
    keep: usize,
}

fn default_keep() -> usize {
    5
}

/// POST /git/clone — Clone or pull a Git repository.
async fn clone(
    Json(body): Json<CloneRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if body.name.is_empty() || body.name.len() > 64 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid name"));
    }
    if body.repo_url.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Missing repo_url"));
    }

    tracing::info!("Git clone: {} from {} ({})", body.name, body.repo_url, body.branch);

    let result = git_build::clone_or_pull(
        &body.name,
        &body.repo_url,
        &body.branch,
        body.key_path.as_deref(),
    )
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    Ok(Json(serde_json::json!({
        "commit_hash": result.commit_hash,
        "commit_message": result.commit_message,
    })))
}

/// POST /git/build — Build a Docker image from the cloned repo.
async fn build(
    Json(body): Json<BuildRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if body.name.is_empty() || body.name.len() > 64 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid name"));
    }

    tracing::info!("Git build: {} (commit: {})", body.name, body.commit_hash);

    let result = git_build::build_image(&body.name, &body.dockerfile, &body.commit_hash)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    Ok(Json(serde_json::json!({
        "image_tag": result.image_tag,
        "output": result.output,
    })))
}

/// POST /git/deploy — Deploy a container from a locally-built image.
async fn deploy_container(
    State(state): State<AppState>,
    Json(body): Json<DeployRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if body.name.is_empty() || body.name.len() > 64 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid name"));
    }
    if body.container_port == 0 || body.host_port == 0 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid port"));
    }

    tracing::info!(
        "Git deploy: {} (image: {}, port: {}→{})",
        body.name, body.image_tag, body.host_port, body.container_port
    );

    let result = git_build::deploy_or_update(
        &body.name,
        &body.image_tag,
        body.container_port,
        body.host_port,
        body.env,
        body.domain.as_deref(),
        &state.templates,
    )
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    Ok(Json(serde_json::json!({
        "container_id": result.container_id,
        "blue_green": result.blue_green,
    })))
}

/// POST /git/keygen — Generate SSH deploy key.
async fn keygen(
    Json(body): Json<KeygenRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if body.name.is_empty() || body.name.len() > 64 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid name"));
    }

    let (public_key, key_path) = deploy::generate_deploy_key(&body.name)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    Ok(Json(serde_json::json!({
        "public_key": public_key,
        "key_path": key_path,
    })))
}

/// POST /git/cleanup — Stop + remove container and clean up nginx/SSL/volumes.
async fn cleanup(
    Json(body): Json<CleanupRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if body.name.is_empty() || body.name.len() > 64 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid name"));
    }

    git_build::cleanup_container(&body.name)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    Ok(Json(serde_json::json!({ "success": true })))
}

/// POST /git/prune — Remove old Docker images, keeping the last N.
async fn prune(
    Json(body): Json<PruneRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if body.name.is_empty() || body.name.len() > 64 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid name"));
    }

    let pruned = git_build::prune_images(&body.name, body.keep)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    Ok(Json(serde_json::json!({ "pruned": pruned })))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/git/clone", post(clone))
        .route("/git/build", post(build))
        .route("/git/deploy", post(deploy_container))
        .route("/git/keygen", post(keygen))
        .route("/git/cleanup", post(cleanup))
        .route("/git/prune", post(prune))
}
