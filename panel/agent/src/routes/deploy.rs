use axum::{
    http::StatusCode,
    routing::post,
    Json, Router,
};
use serde::Deserialize;

use super::{is_valid_domain, AppState};
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
    let mut total_duration = git_result.duration_ms;

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

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/deploy/run", post(run_deploy))
        .route("/deploy/keygen", post(keygen))
}
