use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{err, paginate, ApiError};
use crate::services::activity;
use crate::AppState;

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct DeployConfig {
    pub id: Uuid,
    pub site_id: Uuid,
    pub repo_url: String,
    pub branch: String,
    pub deploy_script: String,
    pub auto_deploy: bool,
    pub webhook_secret: String,
    pub deploy_key_public: Option<String>,
    pub deploy_key_path: Option<String>,
    pub last_deploy: Option<chrono::DateTime<chrono::Utc>>,
    pub last_status: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct DeployLog {
    pub id: Uuid,
    pub site_id: Uuid,
    pub commit_hash: Option<String>,
    pub status: String,
    pub output: Option<String>,
    pub triggered_by: String,
    pub duration_ms: Option<i32>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Deserialize)]
pub struct SetDeployRequest {
    pub repo_url: String,
    pub branch: Option<String>,
    pub deploy_script: Option<String>,
    pub auto_deploy: Option<bool>,
}

#[derive(serde::Deserialize)]
pub struct LogsQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// Verify site ownership, return (domain, site_id).
async fn get_site(state: &AppState, site_id: Uuid, user_id: Uuid) -> Result<String, ApiError> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT domain FROM sites WHERE id = $1 AND user_id = $2")
            .bind(site_id)
            .bind(user_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    row.map(|(d,)| d)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))
}

/// GET /api/sites/{id}/deploy — Get deploy config.
pub async fn get_config(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Option<DeployConfig>>, ApiError> {
    get_site(&state, id, claims.sub).await?;

    let config: Option<DeployConfig> = sqlx::query_as(
        "SELECT * FROM deploy_configs WHERE site_id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(config))
}

/// PUT /api/sites/{id}/deploy — Set/update deploy config.
pub async fn set_config(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<SetDeployRequest>,
) -> Result<Json<DeployConfig>, ApiError> {
    let domain = get_site(&state, id, claims.sub).await?;

    if body.repo_url.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Repository URL is required"));
    }

    let branch = body.branch.as_deref().unwrap_or("main");
    let deploy_script = body.deploy_script.as_deref().unwrap_or("");
    let auto_deploy = body.auto_deploy.unwrap_or(false);
    let webhook_secret = Uuid::new_v4().to_string().replace('-', "");

    let config: DeployConfig = sqlx::query_as(
        "INSERT INTO deploy_configs (site_id, repo_url, branch, deploy_script, auto_deploy, webhook_secret) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (site_id) DO UPDATE SET \
         repo_url = $2, branch = $3, deploy_script = $4, auto_deploy = $5, updated_at = NOW() \
         RETURNING *",
    )
    .bind(id)
    .bind(body.repo_url.trim())
    .bind(branch)
    .bind(deploy_script)
    .bind(auto_deploy)
    .bind(&webhook_secret)
    .fetch_one(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    tracing::info!("Deploy config set for {domain}: {}", body.repo_url);
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "deploy.config",
        Some("deploy"), Some(&domain), Some(&body.repo_url), None,
    ).await;

    Ok(Json(config))
}

/// DELETE /api/sites/{id}/deploy — Remove deploy config.
pub async fn remove_config(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    get_site(&state, id, claims.sub).await?;

    sqlx::query("DELETE FROM deploy_configs WHERE site_id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/sites/{id}/deploy/trigger — Trigger a deployment.
pub async fn trigger(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<DeployLog>, ApiError> {
    let domain = get_site(&state, id, claims.sub).await?;

    let config: DeployConfig = sqlx::query_as(
        "SELECT * FROM deploy_configs WHERE site_id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "No deploy config found"))?;

    let log = execute_deploy(&state, id, &domain, &config, "manual").await?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "deploy.trigger",
        Some("deploy"), Some(&domain), log.commit_hash.as_deref(), Some(&log.status),
    ).await;

    Ok(Json(log))
}

/// POST /api/sites/{id}/deploy/keygen — Generate deploy key.
pub async fn keygen(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = get_site(&state, id, claims.sub).await?;

    let result = state
        .agent
        .post("/deploy/keygen", Some(serde_json::json!({ "domain": domain })))
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Keygen failed: {e}")))?;

    let public_key = result.get("public_key").and_then(|v| v.as_str()).unwrap_or("");
    let key_path = result.get("key_path").and_then(|v| v.as_str()).unwrap_or("");

    // Store in deploy config
    sqlx::query(
        "UPDATE deploy_configs SET deploy_key_public = $1, deploy_key_path = $2, updated_at = NOW() WHERE site_id = $3",
    )
    .bind(public_key)
    .bind(key_path)
    .bind(id)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(serde_json::json!({
        "public_key": public_key,
    })))
}

/// GET /api/sites/{id}/deploy/logs — List deploy logs.
pub async fn logs(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Query(params): Query<LogsQuery>,
) -> Result<Json<Vec<DeployLog>>, ApiError> {
    get_site(&state, id, claims.sub).await?;

    let (limit, offset) = paginate(params.limit, params.offset);

    let logs: Vec<DeployLog> = sqlx::query_as(
        "SELECT * FROM deploy_logs WHERE site_id = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3",
    )
    .bind(id)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(logs))
}

/// POST /api/webhooks/deploy/{site_id}/{secret} — Webhook endpoint (no auth).
pub async fn webhook(
    State(state): State<AppState>,
    Path((site_id, secret)): Path<(Uuid, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Verify secret
    let config: DeployConfig = sqlx::query_as(
        "SELECT * FROM deploy_configs WHERE site_id = $1 AND webhook_secret = $2 AND auto_deploy = true",
    )
    .bind(site_id)
    .bind(&secret)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Invalid webhook"))?;

    // Get domain
    let domain: Option<(String,)> = sqlx::query_as("SELECT domain FROM sites WHERE id = $1")
        .bind(site_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let domain = domain.map(|(d,)| d).unwrap_or_default();

    // Execute deploy in background (webhook should return quickly)
    let state_clone = state.clone();
    let domain_clone = domain.clone();
    tokio::spawn(async move {
        let _ = execute_deploy(&state_clone, site_id, &domain_clone, &config, "webhook").await;
    });

    Ok(Json(serde_json::json!({ "ok": true, "message": "Deployment triggered" })))
}

/// Execute a deployment: git clone/pull + run script.
async fn execute_deploy(
    state: &AppState,
    site_id: Uuid,
    domain: &str,
    config: &DeployConfig,
    triggered_by: &str,
) -> Result<DeployLog, ApiError> {
    let agent_body = serde_json::json!({
        "domain": domain,
        "repo_url": config.repo_url,
        "branch": config.branch,
        "deploy_script": if config.deploy_script.is_empty() { None } else { Some(&config.deploy_script) },
        "key_path": config.deploy_key_path,
    });

    let result = state
        .agent
        .post("/deploy/run", Some(agent_body))
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Deploy failed: {e}")))?;

    let success = result.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
    let output = result.get("output").and_then(|v| v.as_str()).unwrap_or("");
    let commit_hash = result.get("commit_hash").and_then(|v| v.as_str());
    let duration_ms = result.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(0) as i32;
    let status = if success { "success" } else { "failed" };

    // Record log
    let log: DeployLog = sqlx::query_as(
        "INSERT INTO deploy_logs (site_id, commit_hash, status, output, triggered_by, duration_ms) \
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING *",
    )
    .bind(site_id)
    .bind(commit_hash)
    .bind(status)
    .bind(output)
    .bind(triggered_by)
    .bind(duration_ms)
    .fetch_one(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    // Update deploy config status
    sqlx::query(
        "UPDATE deploy_configs SET last_deploy = NOW(), last_status = $1, updated_at = NOW() WHERE site_id = $2",
    )
    .bind(status)
    .bind(site_id)
    .execute(&state.db)
    .await
    .ok();

    tracing::info!("Deploy {status} for {domain} (commit: {:?}, trigger: {triggered_by})", commit_hash);

    Ok(log)
}
