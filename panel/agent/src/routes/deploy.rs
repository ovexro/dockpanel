use axum::{
    extract::Path,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use super::{is_valid_domain, AppState};
use crate::safe_cmd::safe_command;
use crate::services::deploy;

type ApiErr = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: &str) -> ApiErr {
    (status, Json(serde_json::json!({ "error": msg })))
}

#[derive(Deserialize)]
pub struct DeployRequest {
    pub domain: String,
    pub repo_url: String,
    pub branch: String,
    pub deploy_script: Option<String>,
    pub key_path: Option<String>,
}

#[derive(Deserialize)]
pub struct AtomicDeployRequest {
    pub domain: String,
    pub repo_url: String,
    pub branch: String,
    pub deploy_script: Option<String>,
    pub key_path: Option<String>,
    pub keep_releases: Option<u32>,
    pub shared_dirs: Option<Vec<String>>,
}

#[derive(Deserialize)]
pub struct ActivateReleaseRequest {
    pub domain: String,
    pub release_id: String,
}

#[derive(Serialize)]
pub struct ReleaseInfoResponse {
    pub id: String,
    pub active: bool,
    pub commit_hash: Option<String>,
    pub created_at: String,
}

#[derive(Deserialize)]
pub struct KeygenRequest {
    pub domain: String,
}

/// POST /deploy/run — Clone/pull and optionally run deploy script.
async fn run_deploy(
    Json(body): Json<DeployRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    // Validate domain
    if !is_valid_domain(&body.domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain"));
    }

    // Validate repo_url
    if body.repo_url.is_empty() || body.repo_url.starts_with('-') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid repo_url"));
    }
    if !body.repo_url.starts_with("https://") && !body.repo_url.starts_with("http://")
        && !body.repo_url.starts_with("git@") && !body.repo_url.starts_with("ssh://") {
        return Err(err(StatusCode::BAD_REQUEST, "repo_url must use https://, http://, ssh://, or git@ protocol"));
    }

    // Validate branch
    if body.branch.is_empty() || body.branch.starts_with('-') || body.branch.contains("..") {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid branch name"));
    }

    tracing::info!("Deploying {} from {} ({})", body.domain, body.repo_url, body.branch);

    // 1. Clone or pull
    let git_result = deploy::clone_or_pull(
        &body.domain,
        &body.repo_url,
        &body.branch,
        body.key_path.as_deref(),
    )
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    if !git_result.success {
        return Ok(Json(serde_json::json!({
            "success": false,
            "output": git_result.output,
            "commit_hash": git_result.commit_hash,
            "duration_ms": git_result.duration_ms,
            "stage": "git",
        })));
    }

    let mut total_output = git_result.output;
    let total_duration = git_result.duration_ms;

    // 2. Run deploy script (if provided)
    if let Some(ref script) = body.deploy_script {
        if !script.trim().is_empty() {
            total_output.push_str("\n--- Deploy Script ---\n");

            let (script_ok, script_output) = deploy::run_script(&body.domain, script)
                .await
                .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

            total_output.push_str(&script_output);

            if !script_ok {
                return Ok(Json(serde_json::json!({
                    "success": false,
                    "output": total_output,
                    "commit_hash": git_result.commit_hash,
                    "duration_ms": total_duration,
                    "stage": "script",
                })));
            }
        }
    }

    tracing::info!(
        "Deploy complete for {} (commit: {:?})",
        body.domain,
        git_result.commit_hash
    );

    Ok(Json(serde_json::json!({
        "success": true,
        "output": total_output,
        "commit_hash": git_result.commit_hash,
        "duration_ms": total_duration,
    })))
}

/// POST /deploy/keygen — Generate SSH deploy key pair.
async fn keygen(
    Json(body): Json<KeygenRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_domain(&body.domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain"));
    }

    let (public_key, key_path) = deploy::generate_deploy_key(&body.domain)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    Ok(Json(serde_json::json!({
        "public_key": public_key,
        "key_path": key_path,
    })))
}

/// POST /deploy/atomic — Zero-downtime deploy with atomic symlink swap.
async fn atomic_deploy(
    Json(body): Json<AtomicDeployRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_domain(&body.domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain"));
    }
    if body.repo_url.is_empty() || body.repo_url.starts_with('-') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid repo_url"));
    }
    if !body.repo_url.starts_with("https://") && !body.repo_url.starts_with("http://")
        && !body.repo_url.starts_with("git@") && !body.repo_url.starts_with("ssh://") {
        return Err(err(StatusCode::BAD_REQUEST, "repo_url must use https://, http://, ssh://, or git@ protocol"));
    }
    if body.branch.is_empty() || body.branch.starts_with('-') || body.branch.contains("..") {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid branch name"));
    }

    let keep = body.keep_releases.unwrap_or(5).clamp(2, 20);

    // Default shared dirs for PHP sites (WordPress-compatible)
    let default_shared = vec![
        "wp-content/uploads".to_string(),
        ".env".to_string(),
    ];
    let shared = body.shared_dirs.as_ref().unwrap_or(&default_shared);
    let shared_refs: Vec<&str> = shared.iter().map(|s| s.as_str()).collect();

    // Validate shared paths (no traversal)
    for s in &shared_refs {
        if s.contains("..") || s.starts_with('/') {
            return Err(err(StatusCode::BAD_REQUEST, "Shared paths must not contain '..' or start with '/'"));
        }
    }

    tracing::info!("Atomic deploy {} from {} ({}) keep={}", body.domain, body.repo_url, body.branch, keep);

    let result = deploy::atomic_deploy(
        &body.domain,
        &body.repo_url,
        &body.branch,
        body.key_path.as_deref(),
        body.deploy_script.as_deref(),
        keep,
        &shared_refs,
    )
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    Ok(Json(serde_json::json!({
        "success": result.success,
        "output": result.output,
        "commit_hash": result.commit_hash,
        "duration_ms": result.duration_ms,
    })))
}

/// GET /deploy/releases/:domain — List releases for a site.
async fn list_releases(
    Path(domain): Path<String>,
) -> Result<Json<Vec<ReleaseInfoResponse>>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain"));
    }

    let releases = deploy::list_releases(&domain)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    let response: Vec<ReleaseInfoResponse> = releases
        .into_iter()
        .map(|r| ReleaseInfoResponse {
            id: r.id,
            active: r.active,
            commit_hash: r.commit_hash,
            created_at: r.created_at,
        })
        .collect();

    Ok(Json(response))
}

/// POST /deploy/activate — Activate (rollback to) a specific release.
async fn activate_release(
    Json(body): Json<ActivateReleaseRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_domain(&body.domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain"));
    }

    deploy::activate_release(&body.domain, &body.release_id)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    tracing::info!("Activated release {} for {}", body.release_id, body.domain);

    Ok(Json(serde_json::json!({
        "success": true,
        "message": format!("Release {} activated", body.release_id),
    })))
}

/// POST /sites/{domain}/laravel-migrate — Run Laravel migrations safely.
/// Uses argument arrays instead of shell interpolation to prevent injection.
async fn laravel_migrate(
    Path(domain): Path<String>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain"));
    }

    let site_dir = format!("/var/www/{domain}");
    if !std::path::Path::new(&site_dir).exists() {
        return Err(err(StatusCode::NOT_FOUND, "Site directory not found"));
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        safe_command("php")
            .args(["artisan", "migrate", "--force"])
            .current_dir(&site_dir)
            .output(),
    )
    .await
    .map_err(|_| err(StatusCode::GATEWAY_TIMEOUT, "Migration timed out"))?
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Migration failed: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if output.status.success() {
        tracing::info!("Laravel migrations completed for {domain}");
        Ok(Json(serde_json::json!({
            "success": true,
            "output": stdout,
        })))
    } else {
        tracing::warn!("Laravel migrations failed for {domain}: {stderr}");
        Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Migration failed: {stderr}")))
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/deploy/run", post(run_deploy))
        .route("/deploy/atomic", post(atomic_deploy))
        .route("/deploy/releases/{domain}", get(list_releases))
        .route("/deploy/activate", post(activate_release))
        .route("/deploy/keygen", post(keygen))
        .route("/sites/{domain}/laravel-migrate", post(laravel_migrate))
}
