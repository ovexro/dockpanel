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
    memory_mb: Option<u64>,
    cpu_percent: Option<u64>,
    ssl_email: Option<String>,
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
        body.memory_mb,
        body.cpu_percent,
        body.ssl_email.as_deref(),
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

#[derive(Deserialize)]
struct LifecycleRequest {
    name: String,
}

/// POST /git/stop
async fn stop_container(Json(body): Json<LifecycleRequest>) -> Result<Json<serde_json::Value>, ApiErr> {
    if body.name.is_empty() { return Err(err(StatusCode::BAD_REQUEST, "Invalid name")); }
    let container_name = format!("dockpanel-git-{}", body.name);
    let docker = bollard::Docker::connect_with_local_defaults()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")))?;
    docker.stop_container(&container_name, Some(bollard::container::StopContainerOptions { t: 10 }))
        .await.map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")))?;
    Ok(Json(serde_json::json!({ "success": true })))
}

/// POST /git/start
async fn start_container(Json(body): Json<LifecycleRequest>) -> Result<Json<serde_json::Value>, ApiErr> {
    if body.name.is_empty() { return Err(err(StatusCode::BAD_REQUEST, "Invalid name")); }
    let container_name = format!("dockpanel-git-{}", body.name);
    let docker = bollard::Docker::connect_with_local_defaults()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")))?;
    docker.start_container(&container_name, None::<bollard::container::StartContainerOptions<String>>)
        .await.map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")))?;
    Ok(Json(serde_json::json!({ "success": true })))
}

/// POST /git/restart
async fn restart_container(Json(body): Json<LifecycleRequest>) -> Result<Json<serde_json::Value>, ApiErr> {
    if body.name.is_empty() { return Err(err(StatusCode::BAD_REQUEST, "Invalid name")); }
    let container_name = format!("dockpanel-git-{}", body.name);
    let docker = bollard::Docker::connect_with_local_defaults()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")))?;
    docker.restart_container(&container_name, Some(bollard::container::RestartContainerOptions { t: 10 }))
        .await.map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")))?;
    Ok(Json(serde_json::json!({ "success": true })))
}

#[derive(Deserialize)]
struct LogsRequest {
    name: String,
    #[serde(default = "default_log_lines")]
    lines: usize,
}
fn default_log_lines() -> usize { 200 }

/// POST /git/logs
async fn container_logs(Json(body): Json<LogsRequest>) -> Result<Json<serde_json::Value>, ApiErr> {
    if body.name.is_empty() { return Err(err(StatusCode::BAD_REQUEST, "Invalid name")); }
    let container_name = format!("dockpanel-git-{}", body.name);
    let docker = bollard::Docker::connect_with_local_defaults()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")))?;

    use bollard::container::LogsOptions;
    use tokio_stream::StreamExt;
    let mut logs = docker.logs(&container_name, Some(LogsOptions::<String> {
        stdout: true, stderr: true, tail: body.lines.to_string(), ..Default::default()
    }));
    let mut output = String::new();
    while let Some(Ok(log)) = logs.next().await {
        output.push_str(&log.to_string());
    }
    Ok(Json(serde_json::json!({ "logs": output })))
}

#[derive(Deserialize)]
struct HookRequest {
    name: String,
    command: String,
}

/// POST /git/hook — Run a command inside a git-deployed container (docker exec).
async fn run_hook(Json(body): Json<HookRequest>) -> Result<Json<serde_json::Value>, ApiErr> {
    if body.name.is_empty() { return Err(err(StatusCode::BAD_REQUEST, "Invalid name")); }
    if body.command.is_empty() { return Err(err(StatusCode::BAD_REQUEST, "Empty command")); }
    let container_name = format!("dockpanel-git-{}", body.name);

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        tokio::process::Command::new("docker")
            .args(["exec", &container_name, "sh", "-c", &body.command])
            .output()
    ).await
        .map_err(|_| err(StatusCode::GATEWAY_TIMEOUT, "Hook timed out (300s)"))?
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    // Truncate to 50KB
    let truncated = if combined.len() > 50_000 { format!("{}...\n[truncated]", &combined[..50_000]) } else { combined };

    Ok(Json(serde_json::json!({
        "success": output.status.success(),
        "output": truncated,
    })))
}

#[derive(Deserialize)]
struct PreBuildHookRequest {
    name: String,
    command: String,
}

/// POST /git/pre-build-hook — Run a command on the host in the git repo directory.
async fn pre_build_hook(Json(body): Json<PreBuildHookRequest>) -> Result<Json<serde_json::Value>, ApiErr> {
    if body.name.is_empty() { return Err(err(StatusCode::BAD_REQUEST, "Invalid name")); }
    if body.command.is_empty() { return Err(err(StatusCode::BAD_REQUEST, "Empty command")); }
    let git_dir = format!("/var/lib/dockpanel/git/{}", body.name);
    if !std::path::Path::new(&git_dir).exists() {
        return Err(err(StatusCode::NOT_FOUND, "Git repo not found"));
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        tokio::process::Command::new("sh")
            .args(["-c", &body.command])
            .current_dir(&git_dir)
            .env("HOME", &git_dir)
            .env("NODE_ENV", "production")
            .output()
    ).await
        .map_err(|_| err(StatusCode::GATEWAY_TIMEOUT, "Hook timed out (300s)"))?
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    let truncated = if combined.len() > 50_000 { format!("{}...\n[truncated]", &combined[..50_000]) } else { combined };

    Ok(Json(serde_json::json!({
        "success": output.status.success(),
        "output": truncated,
    })))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/git/clone", post(clone))
        .route("/git/build", post(build))
        .route("/git/deploy", post(deploy_container))
        .route("/git/keygen", post(keygen))
        .route("/git/cleanup", post(cleanup))
        .route("/git/prune", post(prune))
        .route("/git/stop", post(stop_container))
        .route("/git/start", post(start_container))
        .route("/git/restart", post(restart_container))
        .route("/git/logs", post(container_logs))
        .route("/git/hook", post(run_hook))
        .route("/git/pre-build-hook", post(pre_build_hook))
}
