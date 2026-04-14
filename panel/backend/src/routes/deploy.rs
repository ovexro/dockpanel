use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::auth::{AuthUser, ServerScope};
use crate::error::{internal_error, err, agent_error, paginate, ApiError};
use crate::routes::is_safe_shell_command;
use crate::routes::sites::ProvisionStep;
use crate::services::activity;
use crate::services::agent::AgentHandle;
use crate::services::notifications;
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
    pub atomic_deploy: bool,
    pub keep_releases: i32,
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
    pub atomic_deploy: Option<bool>,
    pub keep_releases: Option<i32>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct ReleaseInfo {
    pub id: String,
    pub active: bool,
    pub commit_hash: Option<String>,
    pub created_at: String,
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
            .map_err(|e| internal_error("unknown", e))?;
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
    .map_err(|e| internal_error("get config", e))?;

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
    if !deploy_script.is_empty() {
        is_safe_shell_command(deploy_script)
            .map_err(|e| err(StatusCode::BAD_REQUEST, e))?;
    }
    let auto_deploy = body.auto_deploy.unwrap_or(false);
    let atomic_deploy = body.atomic_deploy.unwrap_or(false);
    let keep_releases = body.keep_releases.unwrap_or(5).clamp(2, 20);
    let webhook_secret = Uuid::new_v4().to_string().replace('-', "");

    let config: DeployConfig = sqlx::query_as(
        "INSERT INTO deploy_configs (site_id, repo_url, branch, deploy_script, auto_deploy, webhook_secret, atomic_deploy, keep_releases) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
         ON CONFLICT (site_id) DO UPDATE SET \
         repo_url = $2, branch = $3, deploy_script = $4, auto_deploy = $5, atomic_deploy = $7, keep_releases = $8, updated_at = NOW() \
         RETURNING *",
    )
    .bind(id)
    .bind(body.repo_url.trim())
    .bind(branch)
    .bind(deploy_script)
    .bind(auto_deploy)
    .bind(&webhook_secret)
    .bind(atomic_deploy)
    .bind(keep_releases)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("set config", e))?;

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
        .map_err(|e| internal_error("remove config", e))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/sites/{id}/deploy/trigger — Trigger a deployment (async with SSE).
pub async fn trigger(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    // Check for active critical/major incidents — block deploy during outage
    let active_incidents: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM managed_incidents \
         WHERE status NOT IN ('resolved', 'postmortem') \
         AND severity IN ('critical', 'major')"
    ).fetch_one(&state.db).await.unwrap_or((0,));

    if active_incidents.0 > 0 {
        return Err(err(StatusCode::CONFLICT,
            "Deploy blocked: active critical/major incident in progress. Resolve the incident first."));
    }

    let domain = get_site(&state, id, claims.sub).await?;

    let config: DeployConfig = sqlx::query_as(
        "SELECT * FROM deploy_configs WHERE site_id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("trigger", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "No deploy config found"))?;

    let deploy_id = Uuid::new_v4();

    let (tx, _) = broadcast::channel::<ProvisionStep>(32);
    {
        let mut logs = state.provision_logs.lock().unwrap_or_else(|e| e.into_inner());
        logs.insert(deploy_id, (Vec::new(), tx, Instant::now()));
    }

    let logs = state.provision_logs.clone();
    let db = state.db.clone();
    let user_id = claims.sub;
    let email = claims.email.clone();
    let domain_clone = domain.clone();

    tokio::spawn(async move {
        let emit = |step: &str, label: &str, status: &str, msg: Option<String>| {
            let ev = ProvisionStep {
                step: step.into(), label: label.into(), status: status.into(), message: msg,
            };
            if let Ok(mut map) = logs.lock() {
                if let Some((history, tx, _)) = map.get_mut(&deploy_id) {
                    history.push(ev.clone());
                    let _ = tx.send(ev);
                }
            }
        };

        emit("deploy", "Running deployment", "in_progress", None);

        match execute_deploy(&db, &agent, id, &domain_clone, &config, "manual").await {
            Ok(log) => {
                let ok = log.status == "success";
                emit("deploy", "Running deployment", if ok { "done" } else { "error" },
                    log.output.as_ref().map(|o| o.chars().take(500).collect()));
                emit("complete",
                    if ok { "Deployment complete" } else { "Deployment failed" },
                    if ok { "done" } else { "error" }, None);

                activity::log_activity(
                    &db, user_id, &email, "deploy.trigger",
                    Some("deploy"), Some(&domain_clone), log.commit_hash.as_deref(), Some(&log.status),
                ).await;
            }
            Err((_status, body)) => {
                let msg = body.0.get("error").and_then(|v| v.as_str())
                    .unwrap_or("Unknown error").to_string();
                emit("deploy", "Running deployment", "error", Some(msg));
                emit("complete", "Deployment failed", "error", None);
            }
        }

        tokio::time::sleep(Duration::from_secs(60)).await;
        logs.lock().unwrap_or_else(|e| e.into_inner()).remove(&deploy_id);
    });

    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({
        "deploy_id": deploy_id,
        "message": "Deployment started",
    }))))
}

/// POST /api/sites/{id}/deploy/keygen — Generate deploy key.
pub async fn keygen(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = get_site(&state, id, claims.sub).await?;

    let result = agent
        .post("/deploy/keygen", Some(serde_json::json!({ "domain": domain })))
        .await
        .map_err(|e| agent_error("Deploy key generation", e))?;

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
    .map_err(|e| internal_error("keygen", e))?;

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
    .map_err(|e| internal_error("logs", e))?;

    Ok(Json(logs))
}

/// POST /api/webhooks/deploy/{site_id}/{secret} — Webhook endpoint (no auth).
pub async fn webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((site_id, secret)): Path<(Uuid, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Validate Content-Type
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !content_type.is_empty() && !content_type.contains("application/json") {
        return Err(err(StatusCode::BAD_REQUEST, "Content-Type must be application/json"));
    }

    // Rate limit: max 10 attempts per site per hour
    {
        let mut attempts = state.webhook_attempts.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let entry = attempts.entry(site_id).or_insert((0, now));
        if now.duration_since(entry.1).as_secs() >= 3600 {
            // Window expired, reset
            *entry = (0, now);
        }
        if entry.0 >= 10 {
            return Err(err(StatusCode::TOO_MANY_REQUESTS, "Rate limit exceeded. Try again later."));
        }
    }

    // Fetch the deploy config by site_id only (we'll compare the secret in constant time)
    let config: DeployConfig = sqlx::query_as(
        "SELECT * FROM deploy_configs WHERE site_id = $1 AND auto_deploy = true",
    )
    .bind(site_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("webhook", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Invalid webhook"))?;

    // Constant-time secret comparison using subtle crate
    use subtle::ConstantTimeEq;
    if secret.as_bytes().ct_eq(config.webhook_secret.as_bytes()).unwrap_u8() != 1 {
        // Record failed attempt
        {
            let mut attempts = state.webhook_attempts.lock().unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            let entry = attempts.entry(site_id).or_insert((0, now));
            if now.duration_since(entry.1).as_secs() >= 3600 {
                *entry = (1, now);
            } else {
                entry.0 += 1;
            }
        }
        return Err(err(StatusCode::NOT_FOUND, "Invalid webhook"));
    }

    // Get domain
    let domain: Option<(String,)> = sqlx::query_as("SELECT domain FROM sites WHERE id = $1")
        .bind(site_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("webhook", e))?;

    let domain = domain.map(|(d,)| d).unwrap_or_default();

    // Check for active critical/major incidents — skip webhook deploy during outage
    let active_incidents: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM managed_incidents \
         WHERE status NOT IN ('resolved', 'postmortem') \
         AND severity IN ('critical', 'major')"
    ).fetch_one(&state.db).await.unwrap_or((0,));

    if active_incidents.0 > 0 {
        tracing::warn!("Deploy blocked for {domain}: active incident in progress");
        return Ok(Json(serde_json::json!({ "ok": false, "message": "Deploy skipped: active incident" })));
    }

    // Resolve the agent for this webhook (use local agent for webhook-triggered deploys)
    let agent = crate::services::agent::AgentHandle::Local(state.agents.local().clone());

    // Execute deploy in background (webhook should return quickly)
    let db = state.db.clone();
    let domain_clone = domain.clone();
    tokio::spawn(async move {
        let _ = execute_deploy(&db, &agent, site_id, &domain_clone, &config, "webhook").await;
    });

    Ok(Json(serde_json::json!({ "ok": true, "message": "Deployment triggered" })))
}

/// Execute a deployment: git clone/pull + run script.
async fn execute_deploy(
    db: &sqlx::PgPool,
    agent: &AgentHandle,
    site_id: Uuid,
    domain: &str,
    config: &DeployConfig,
    triggered_by: &str,
) -> Result<DeployLog, ApiError> {
    // Pre-deploy backup: snapshot before deploying (best-effort, don't block deploy on failure)
    let _ = agent.post(
        &format!("/backups/{}/create", domain),
        Some(serde_json::json!({"reason": "pre-deploy"})),
    ).await;
    tracing::info!("Pre-deploy backup requested for {domain}");

    // Choose atomic or standard deploy path
    let result = if config.atomic_deploy {
        let agent_body = serde_json::json!({
            "domain": domain,
            "repo_url": config.repo_url,
            "branch": config.branch,
            "deploy_script": if config.deploy_script.is_empty() { None } else { Some(&config.deploy_script) },
            "key_path": config.deploy_key_path,
            "keep_releases": config.keep_releases,
        });
        agent
            .post("/deploy/atomic", Some(agent_body))
            .await
            .map_err(|e| agent_error("Atomic deploy execution", e))?
    } else {
        let agent_body = serde_json::json!({
            "domain": domain,
            "repo_url": config.repo_url,
            "branch": config.branch,
            "deploy_script": if config.deploy_script.is_empty() { None } else { Some(&config.deploy_script) },
            "key_path": config.deploy_key_path,
        });
        agent
            .post("/deploy/run", Some(agent_body))
            .await
            .map_err(|e| agent_error("Deploy execution", e))?
    };

    let success = result.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
    let output = result.get("output").and_then(|v| v.as_str()).unwrap_or("");
    let commit_hash = result.get("commit_hash").and_then(|v| v.as_str());
    let duration_ms = result.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(0) as i32;
    let status = if success { "success" } else { "failed" };

    // GAP 26: Auto-rollback on failed deploy — restore from most recent pre-deploy backup
    if !success {
        let latest_backup: Option<(String,)> = sqlx::query_as(
            "SELECT filename FROM backups WHERE site_id = $1 ORDER BY created_at DESC LIMIT 1"
        )
        .bind(site_id)
        .fetch_optional(db)
        .await
        .ok()
        .flatten();

        if let Some((filename,)) = latest_backup {
            // Validate filename has no path separators or traversal
            if filename.contains('/') || filename.contains("..") || filename.contains('\0') {
                tracing::warn!("Auto-rollback: suspicious backup filename '{filename}' for {domain}, skipping");
            } else {
                let restore_path = format!("/backups/{}/restore/{}", domain, filename);
                match agent.post(&restore_path, None::<serde_json::Value>).await {
                    Ok(_) => tracing::info!("Auto-rollback: restored {filename} for {domain} after failed deploy"),
                    Err(e) => tracing::warn!("Auto-rollback failed for {domain}: {e}"),
                }
            }
        } else {
            tracing::warn!("Auto-rollback: no backup found for {domain}, skipping restore");
        }
    }

    // GAP 56: Auto-run database migrations for Laravel sites after successful deploy
    if success {
        let site_preset: Option<(Option<String>,)> = sqlx::query_as(
            "SELECT php_preset FROM sites WHERE id = $1"
        )
        .bind(site_id)
        .fetch_optional(db)
        .await
        .ok()
        .flatten();

        if let Some((Some(preset),)) = site_preset {
            if preset == "laravel" && crate::routes::is_valid_domain(&domain) {
                match agent.post(
                    &format!("/sites/{domain}/laravel-migrate"),
                    None,
                ).await {
                    Ok(_) => tracing::info!("Post-deploy: Laravel migrations run for {domain}"),
                    Err(e) => tracing::warn!("Post-deploy: Laravel migrations failed for {domain}: {e}"),
                }
            }
        }
    }

    // GAP 37: Post-deploy cache invalidation — clear nginx fastcgi cache for the domain
    if success {
        let _ = agent.post(
            "/diagnostics/fix",
            Some(serde_json::json!({ "fix_id": format!("clean-cache:{domain}") })),
        ).await;
        tracing::info!("Post-deploy cache invalidation requested for {domain}");
    }

    // Post-deploy health check: verify site is responding after successful deploy
    if success {
        let ssl_enabled: Option<(bool,)> = sqlx::query_as(
            "SELECT ssl_enabled FROM sites WHERE id = $1"
        ).bind(site_id).fetch_optional(db).await
            .map_err(|e| internal_error("deploy ssl check", e))?;
        let use_ssl = ssl_enabled.map(|(s,)| s).unwrap_or(false);

        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        let check_url = if use_ssl { format!("https://{}", domain) } else { format!("http://{}", domain) };
        if let Ok(client) = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .timeout(std::time::Duration::from_secs(10))
            .build()
        {
            match client.get(&check_url).send().await {
                Ok(resp) => {
                    let status_code = resp.status().as_u16();
                    if status_code >= 500 {
                        tracing::warn!("Post-deploy health check FAILED for {domain}: HTTP {status_code}");
                        let user_id_opt: Option<uuid::Uuid> = sqlx::query_scalar("SELECT user_id FROM sites WHERE id = $1")
                            .bind(site_id).fetch_optional(db).await
                            .map_err(|e| internal_error("deploy health check user lookup", e))?;
                        notifications::notify_panel(db, user_id_opt,
                            &format!("Deploy warning: {} returning HTTP {}", domain, status_code),
                            &format!("Deploy succeeded but the site is returning HTTP {}. Check your application logs.", status_code),
                            "warning", "deploy", None).await;
                    } else {
                        tracing::info!("Post-deploy health check OK for {domain}: HTTP {status_code}");
                    }
                }
                Err(e) => {
                    tracing::warn!("Post-deploy health check FAILED for {domain}: {e}");
                    let user_id_opt: Option<uuid::Uuid> = sqlx::query_scalar("SELECT user_id FROM sites WHERE id = $1")
                        .bind(site_id).fetch_optional(db).await
                        .map_err(|e| internal_error("deploy health check user lookup", e))?;
                    notifications::notify_panel(db, user_id_opt,
                        &format!("Deploy warning: {} unreachable", domain),
                        &format!("Deploy succeeded but the site is not responding: {}", e),
                        "warning", "deploy", None).await;
                }
            }
        }
    }

    // GAP 5: Auto-inject secrets from linked vault after successful deploy
    if success {
        let jwt_secret = std::env::var("JWT_SECRET").unwrap_or_default();
        if !jwt_secret.is_empty() {
            let inject_rows: Vec<(String, String)> = sqlx::query_as(
                "SELECT s.key, s.encrypted_value FROM secrets s \
                 JOIN secret_vaults v ON v.id = s.vault_id AND v.site_id = $1 \
                 WHERE s.auto_inject = TRUE"
            )
            .bind(site_id)
            .fetch_all(db).await.unwrap_or_default();

            if !inject_rows.is_empty() {
                let mut env_pairs = Vec::new();
                for (key, encrypted_value) in &inject_rows {
                    if let Ok(value) = crate::services::secrets_crypto::decrypt(encrypted_value, &jwt_secret) {
                        env_pairs.push(serde_json::json!({ "key": key, "value": value }));
                    }
                }
                if !env_pairs.is_empty() {
                    let body = serde_json::json!({ "vars": env_pairs });
                    let _ = agent.put(&format!("/nginx/env/{domain}"), body).await;
                    tracing::info!("Auto-injected {} secrets into {domain} after deploy", env_pairs.len());
                }
            }
        }
    }

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
    .fetch_one(db)
    .await
    .map_err(|e| internal_error("webhook", e))?;

    // Update deploy config status
    sqlx::query(
        "UPDATE deploy_configs SET last_deploy = NOW(), last_status = $1, updated_at = NOW() WHERE site_id = $2",
    )
    .bind(status)
    .bind(site_id)
    .execute(db)
    .await
    .ok();

    tracing::info!("Deploy {status} for {domain} (commit: {:?}, trigger: {triggered_by})", commit_hash);

    // Panel notification center
    let user_id: Option<uuid::Uuid> = sqlx::query_scalar("SELECT user_id FROM sites WHERE id = $1")
        .bind(site_id).fetch_optional(db).await
        .map_err(|e| internal_error("deploy notification user lookup", e))?;
    if success {
        notifications::notify_panel(db, user_id, &format!("Deploy complete: {}", domain), "Site deployment completed successfully", "info", "deploy", None).await;
    } else {
        notifications::notify_panel(db, user_id, &format!("Deploy failed: {}", domain), output, "critical", "deploy", None).await;
    }

    Ok(log)
}

/// GET /api/sites/{id}/deploy/releases — List releases for atomic deploy site.
pub async fn list_releases(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<ReleaseInfo>>, ApiError> {
    let domain = get_site(&state, id, claims.sub).await?;

    let result = agent
        .get(&format!("/deploy/releases/{domain}"))
        .await
        .map_err(|e| agent_error("List releases", e))?;

    let releases: Vec<ReleaseInfo> = serde_json::from_value(result)
        .unwrap_or_default();

    Ok(Json(releases))
}

/// POST /api/sites/{id}/deploy/rollback/{release_id} — Rollback to a specific release.
pub async fn rollback_release(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path((id, release_id)): Path<(Uuid, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = get_site(&state, id, claims.sub).await?;

    let result = agent
        .post("/deploy/activate", Some(serde_json::json!({
            "domain": domain,
            "release_id": release_id,
        })))
        .await
        .map_err(|e| agent_error("Rollback release", e))?;

    // Record rollback in deploy logs
    sqlx::query(
        "INSERT INTO deploy_logs (site_id, commit_hash, status, output, triggered_by, duration_ms) \
         VALUES ($1, $2, 'success', $3, 'rollback', 0)",
    )
    .bind(id)
    .bind(&release_id)
    .bind(format!("Rolled back to release {release_id}"))
    .execute(&state.db)
    .await
    .ok();

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "deploy.rollback",
        Some("deploy"), Some(&domain), Some(&release_id), Some("success"),
    ).await;

    // Reload nginx to pick up any config changes
    let _ = agent.post("/nginx/reload", None::<serde_json::Value>).await;

    Ok(Json(result))
}
