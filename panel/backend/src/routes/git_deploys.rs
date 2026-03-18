use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::sse::{Event, KeepAlive, Sse},
    Json,
};
use futures::stream::StreamExt;
use sha2::{Sha256, Digest};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{err, agent_error, require_admin, ApiError};
use crate::routes::is_valid_name;
use crate::routes::sites::ProvisionStep;
use crate::services::activity;
use crate::AppState;

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct GitDeploy {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub repo_url: String,
    pub branch: String,
    pub dockerfile: String,
    pub container_port: i32,
    pub host_port: i32,
    pub domain: Option<String>,
    pub env_vars: serde_json::Value,
    pub auto_deploy: bool,
    pub webhook_secret: String,
    pub deploy_key_public: Option<String>,
    pub deploy_key_path: Option<String>,
    pub container_id: Option<String>,
    pub image_tag: Option<String>,
    pub status: String,
    pub memory_mb: Option<i32>,
    pub cpu_percent: Option<i32>,
    pub ssl_email: Option<String>,
    pub pre_build_cmd: Option<String>,
    pub post_deploy_cmd: Option<String>,
    pub build_args: serde_json::Value,
    pub build_context: String,
    pub last_deploy: Option<chrono::DateTime<chrono::Utc>>,
    pub last_commit: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct GitDeployHistory {
    pub id: Uuid,
    pub git_deploy_id: Uuid,
    pub commit_hash: String,
    pub commit_message: Option<String>,
    pub image_tag: String,
    pub status: String,
    pub output: Option<String>,
    pub triggered_by: String,
    pub duration_ms: Option<i32>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Deserialize)]
pub struct CreateRequest {
    pub name: String,
    pub repo_url: String,
    pub branch: Option<String>,
    pub dockerfile: Option<String>,
    pub container_port: Option<i32>,
    pub domain: Option<String>,
    pub env_vars: Option<HashMap<String, String>>,
    pub auto_deploy: Option<bool>,
    pub memory_mb: Option<i32>,
    pub cpu_percent: Option<i32>,
    pub ssl_email: Option<String>,
    pub pre_build_cmd: Option<String>,
    pub post_deploy_cmd: Option<String>,
    pub build_args: Option<HashMap<String, String>>,
    pub build_context: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct UpdateRequest {
    pub repo_url: Option<String>,
    pub branch: Option<String>,
    pub dockerfile: Option<String>,
    pub container_port: Option<i32>,
    pub domain: Option<String>,
    pub env_vars: Option<HashMap<String, String>>,
    pub auto_deploy: Option<bool>,
    pub memory_mb: Option<i32>,
    pub cpu_percent: Option<i32>,
    pub ssl_email: Option<String>,
    pub pre_build_cmd: Option<String>,
    pub post_deploy_cmd: Option<String>,
    pub build_args: Option<HashMap<String, String>>,
    pub build_context: Option<String>,
}

/// GET /api/git-deploys — List all git deploys for the current user.
pub async fn list(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<Vec<GitDeploy>>, ApiError> {
    require_admin(&claims.role)?;

    let deploys: Vec<GitDeploy> = sqlx::query_as(
        "SELECT * FROM git_deploys WHERE user_id = $1 ORDER BY created_at DESC",
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(deploys))
}

/// POST /api/git-deploys — Create a new git deploy configuration.
pub async fn create(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<CreateRequest>,
) -> Result<(StatusCode, Json<GitDeploy>), ApiError> {
    require_admin(&claims.role)?;

    if !is_valid_name(&body.name) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid deploy name"));
    }

    if body.repo_url.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Repository URL is required"));
    }

    // Auto-allocate host_port: find first gap in 7000-7999
    let used_ports: Vec<(i32,)> = sqlx::query_as(
        "SELECT host_port FROM git_deploys ORDER BY host_port",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let used: Vec<i32> = used_ports.into_iter().map(|(p,)| p).collect();
    let host_port = (7000..=7999)
        .find(|p| !used.contains(p))
        .ok_or_else(|| err(StatusCode::CONFLICT, "No available ports in range 7000-7999"))?;

    // Generate webhook secret
    let webhook_secret: String = {
        use rand::Rng;
        let bytes: Vec<u8> = (0..32).map(|_| rand::thread_rng().r#gen::<u8>()).collect();
        hex::encode(bytes)
    };

    let branch = body.branch.as_deref().unwrap_or("main");
    let dockerfile = body.dockerfile.as_deref().unwrap_or("Dockerfile");
    let container_port = body.container_port.unwrap_or(3000);
    let auto_deploy = body.auto_deploy.unwrap_or(false);
    let env_vars = body
        .env_vars
        .as_ref()
        .map(|e| serde_json::to_value(e).unwrap_or_default())
        .unwrap_or(serde_json::json!({}));
    let build_args = body
        .build_args
        .as_ref()
        .map(|e| serde_json::to_value(e).unwrap_or_default())
        .unwrap_or(serde_json::json!({}));
    let build_context = body.build_context.as_deref().unwrap_or(".");

    let deploy: GitDeploy = sqlx::query_as(
        "INSERT INTO git_deploys (user_id, name, repo_url, branch, dockerfile, container_port, host_port, domain, env_vars, auto_deploy, webhook_secret, memory_mb, cpu_percent, ssl_email, pre_build_cmd, post_deploy_cmd, build_args, build_context) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18) \
         RETURNING *",
    )
    .bind(claims.sub)
    .bind(&body.name)
    .bind(body.repo_url.trim())
    .bind(branch)
    .bind(dockerfile)
    .bind(container_port)
    .bind(host_port)
    .bind(&body.domain)
    .bind(&env_vars)
    .bind(auto_deploy)
    .bind(&webhook_secret)
    .bind(body.memory_mb)
    .bind(body.cpu_percent)
    .bind(&body.ssl_email)
    .bind(&body.pre_build_cmd)
    .bind(&body.post_deploy_cmd)
    .bind(&build_args)
    .bind(build_context)
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        if e.to_string().contains("duplicate key") {
            err(StatusCode::CONFLICT, "A deploy with this name already exists")
        } else {
            err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
        }
    })?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "git_deploy.create",
        Some("git_deploy"), Some(&body.name), None, None,
    ).await;

    Ok((StatusCode::CREATED, Json(deploy)))
}

/// GET /api/git-deploys/{id} — Get a single git deploy.
pub async fn get_one(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<GitDeploy>, ApiError> {
    require_admin(&claims.role)?;

    let deploy: GitDeploy = sqlx::query_as(
        "SELECT * FROM git_deploys WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Git deploy not found"))?;

    Ok(Json(deploy))
}

/// PUT /api/git-deploys/{id} — Update a git deploy configuration.
pub async fn update(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateRequest>,
) -> Result<Json<GitDeploy>, ApiError> {
    require_admin(&claims.role)?;

    // Verify ownership
    let existing: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM git_deploys WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if existing.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Git deploy not found"));
    }

    let env_vars = body.env_vars.as_ref().map(|e| serde_json::to_value(e).unwrap_or_default());
    let build_args = body.build_args.as_ref().map(|e| serde_json::to_value(e).unwrap_or_default());

    let deploy: GitDeploy = sqlx::query_as(
        "UPDATE git_deploys SET \
         repo_url = COALESCE($1, repo_url), \
         branch = COALESCE($2, branch), \
         dockerfile = COALESCE($3, dockerfile), \
         container_port = COALESCE($4, container_port), \
         domain = COALESCE($5, domain), \
         env_vars = COALESCE($6, env_vars), \
         auto_deploy = COALESCE($7, auto_deploy), \
         memory_mb = $8, \
         cpu_percent = $9, \
         ssl_email = COALESCE($10, ssl_email), \
         pre_build_cmd = COALESCE($11, pre_build_cmd), \
         post_deploy_cmd = COALESCE($12, post_deploy_cmd), \
         build_args = COALESCE($13, build_args), \
         build_context = COALESCE($14, build_context), \
         updated_at = NOW() \
         WHERE id = $15 AND user_id = $16 \
         RETURNING *",
    )
    .bind(body.repo_url.as_deref())
    .bind(body.branch.as_deref())
    .bind(body.dockerfile.as_deref())
    .bind(body.container_port)
    .bind(body.domain.as_deref())
    .bind(env_vars)
    .bind(body.auto_deploy)
    .bind(body.memory_mb)
    .bind(body.cpu_percent)
    .bind(body.ssl_email.as_deref())
    .bind(body.pre_build_cmd.as_deref())
    .bind(body.post_deploy_cmd.as_deref())
    .bind(build_args)
    .bind(body.build_context.as_deref())
    .bind(id)
    .bind(claims.sub)
    .fetch_one(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(deploy))
}

/// DELETE /api/git-deploys/{id} — Remove a git deploy and its container.
pub async fn remove(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let deploy: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT name, domain FROM git_deploys WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let (name, _domain) = deploy.ok_or_else(|| err(StatusCode::NOT_FOUND, "Git deploy not found"))?;

    // Tell agent to stop and remove container + cleanup
    state
        .agent
        .post("/git/cleanup", Some(serde_json::json!({ "name": name })))
        .await
        .ok();

    // Delete from DB (CASCADE deletes history)
    sqlx::query("DELETE FROM git_deploys WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "git_deploy.remove",
        Some("git_deploy"), Some(&name), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/git-deploys/{id}/deploy — Trigger a build+deploy (async with SSE progress).
pub async fn deploy(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    require_admin(&claims.role)?;

    let config: GitDeploy = sqlx::query_as(
        "SELECT * FROM git_deploys WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Git deploy not found"))?;

    // Update status to building
    sqlx::query("UPDATE git_deploys SET status = 'building', updated_at = NOW() WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .ok();

    let deploy_id = Uuid::new_v4();

    let (tx, _) = broadcast::channel::<ProvisionStep>(32);
    {
        let mut logs = state.provision_logs.lock().unwrap();
        logs.insert(deploy_id, (Vec::new(), tx, Instant::now()));
    }

    spawn_deploy_task(
        state,
        deploy_id,
        config,
        claims.sub,
        claims.email,
        "manual",
    );

    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({
        "deploy_id": deploy_id,
        "message": "Deployment started",
    }))))
}

/// GET /api/git-deploys/deploy/{deploy_id}/log — SSE stream of deploy progress.
pub async fn deploy_log(
    State(state): State<AppState>,
    AuthUser(_claims): AuthUser,
    Path(deploy_id): Path<Uuid>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, axum::BoxError>>>, ApiError> {
    let (snapshot, rx) = {
        let logs = state.provision_logs.lock().unwrap();
        match logs.get(&deploy_id) {
            Some((history, tx, _)) => (history.clone(), Some(tx.subscribe())),
            None => (Vec::new(), None),
        }
    };

    let rx = rx.ok_or_else(|| err(StatusCode::NOT_FOUND, "No active deploy"))?;

    let snapshot_stream = futures::stream::iter(
        snapshot.into_iter().map(|step| {
            let data = serde_json::to_string(&step).unwrap_or_default();
            Ok(Event::default().data(data))
        }),
    );

    let live_stream = BroadcastStream::new(rx).filter_map(|result| async {
        match result {
            Ok(step) => {
                let data = serde_json::to_string(&step).ok()?;
                Some(Ok(Event::default().data(data)))
            }
            Err(_) => None,
        }
    });

    Ok(
        Sse::new(snapshot_stream.chain(live_stream))
            .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("ping")),
    )
}

/// GET /api/git-deploys/{id}/history — List deploy history.
pub async fn history(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<GitDeployHistory>>, ApiError> {
    require_admin(&claims.role)?;

    // Verify ownership
    let exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM git_deploys WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if exists.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Git deploy not found"));
    }

    let entries: Vec<GitDeployHistory> = sqlx::query_as(
        "SELECT * FROM git_deploy_history WHERE git_deploy_id = $1 ORDER BY created_at DESC LIMIT 50",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(entries))
}

/// POST /api/git-deploys/{id}/rollback/{history_id} — Rollback to a previous image.
pub async fn rollback(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path((id, history_id)): Path<(Uuid, Uuid)>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    require_admin(&claims.role)?;

    let config: GitDeploy = sqlx::query_as(
        "SELECT * FROM git_deploys WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Git deploy not found"))?;

    let hist: GitDeployHistory = sqlx::query_as(
        "SELECT * FROM git_deploy_history WHERE id = $1 AND git_deploy_id = $2",
    )
    .bind(history_id)
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "History entry not found"))?;

    // Update status to building
    sqlx::query("UPDATE git_deploys SET status = 'building', updated_at = NOW() WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .ok();

    let deploy_id = Uuid::new_v4();

    let (tx, _) = broadcast::channel::<ProvisionStep>(32);
    {
        let mut logs = state.provision_logs.lock().unwrap();
        logs.insert(deploy_id, (Vec::new(), tx, Instant::now()));
    }

    let logs = state.provision_logs.clone();
    let agent = state.agent.clone();
    let db = state.db.clone();
    let user_id = claims.sub;
    let email = claims.email.clone();
    let deploy_name = config.name.clone();
    let rollback_image = hist.image_tag.clone();
    let rollback_commit = hist.commit_hash.clone();

    tokio::spawn(async move {
        let started = Instant::now();

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

        // Skip clone+build — go straight to deploy with the historical image
        emit("deploy", "Rolling back container", "in_progress", None);

        let mut deploy_body = serde_json::json!({
            "name": config.name,
            "image_tag": rollback_image,
            "container_port": config.container_port,
            "host_port": config.host_port,
            "env_vars": config.env_vars,
        });
        if let Some(ref domain) = config.domain {
            deploy_body["domain"] = serde_json::json!(domain);
        }
        if let Some(mem) = config.memory_mb {
            deploy_body["memory_mb"] = serde_json::json!(mem);
        }
        if let Some(cpu) = config.cpu_percent {
            deploy_body["cpu_percent"] = serde_json::json!(cpu);
        }

        match agent.post_long("/git/deploy", Some(deploy_body), 120).await {
            Ok(result) => {
                let blue_green = result.get("blue_green").and_then(|v| v.as_bool()).unwrap_or(false);
                if blue_green {
                    emit("deploy", "Rolling back container", "done", Some("Zero-downtime swap".into()));
                } else {
                    emit("deploy", "Rolling back container", "done", None);
                }
                emit("complete", "Rollback complete", "done", None);

                let container_id = result.get("container_id").and_then(|v| v.as_str()).unwrap_or("");
                let duration_ms = started.elapsed().as_millis() as i32;

                // Record history
                sqlx::query(
                    "INSERT INTO git_deploy_history (git_deploy_id, commit_hash, commit_message, image_tag, status, output, triggered_by, duration_ms) \
                     VALUES ($1, $2, $3, $4, 'success', $5, 'rollback', $6)",
                )
                .bind(id)
                .bind(&rollback_commit)
                .bind(format!("Rollback to {}", &rollback_commit[..7.min(rollback_commit.len())]))
                .bind(&rollback_image)
                .bind(format!("Rolled back to image {rollback_image}"))
                .bind(duration_ms)
                .execute(&db)
                .await
                .ok();

                // Update git_deploys
                sqlx::query(
                    "UPDATE git_deploys SET status = 'running', container_id = $1, image_tag = $2, last_deploy = NOW(), last_commit = $3, updated_at = NOW() WHERE id = $4",
                )
                .bind(container_id)
                .bind(&rollback_image)
                .bind(&rollback_commit)
                .bind(id)
                .execute(&db)
                .await
                .ok();

                tracing::info!("Git deploy rollback success: {deploy_name} → {rollback_image}");
                activity::log_activity(
                    &db, user_id, &email, "git_deploy.rollback",
                    Some("git_deploy"), Some(&deploy_name), Some(&rollback_image), None,
                ).await;
            }
            Err(e) => {
                emit("deploy", "Rolling back container", "error", Some(format!("{e}")));
                emit("complete", "Rollback failed", "error", None);

                let duration_ms = started.elapsed().as_millis() as i32;

                sqlx::query(
                    "INSERT INTO git_deploy_history (git_deploy_id, commit_hash, image_tag, status, output, triggered_by, duration_ms) \
                     VALUES ($1, $2, $3, 'failed', $4, 'rollback', $5)",
                )
                .bind(id)
                .bind(&rollback_commit)
                .bind(&rollback_image)
                .bind(format!("{e}"))
                .bind(duration_ms)
                .execute(&db)
                .await
                .ok();

                sqlx::query("UPDATE git_deploys SET status = 'failed', updated_at = NOW() WHERE id = $1")
                    .bind(id)
                    .execute(&db)
                    .await
                    .ok();

                tracing::error!("Git deploy rollback failed: {deploy_name}: {e}");
            }
        }

        tokio::time::sleep(Duration::from_secs(60)).await;
        logs.lock().unwrap().remove(&deploy_id);
    });

    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({
        "deploy_id": deploy_id,
        "message": "Rollback started",
    }))))
}

/// POST /api/git-deploys/{id}/keygen — Generate SSH deploy key.
pub async fn keygen(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let deploy: Option<(String,)> = sqlx::query_as(
        "SELECT name FROM git_deploys WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let (name,) = deploy.ok_or_else(|| err(StatusCode::NOT_FOUND, "Git deploy not found"))?;

    let result = state
        .agent
        .post("/git/keygen", Some(serde_json::json!({ "name": name })))
        .await
        .map_err(|e| agent_error("Deploy key generation", e))?;

    let public_key = result.get("public_key").and_then(|v| v.as_str()).unwrap_or("");
    let key_path = result.get("key_path").and_then(|v| v.as_str()).unwrap_or("");

    sqlx::query(
        "UPDATE git_deploys SET deploy_key_public = $1, deploy_key_path = $2, updated_at = NOW() WHERE id = $3",
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

/// POST /api/git-deploys/{id}/stop
pub async fn stop(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let config: GitDeploy = sqlx::query_as("SELECT * FROM git_deploys WHERE id = $1 AND user_id = $2")
        .bind(id).bind(claims.sub).fetch_optional(&state.db).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Git deploy not found"))?;
    state.agent.post("/git/stop", Some(serde_json::json!({ "name": config.name }))).await
        .map_err(|e| agent_error("Stop container", e))?;
    sqlx::query("UPDATE git_deploys SET status = 'stopped', updated_at = NOW() WHERE id = $1")
        .bind(id).execute(&state.db).await.ok();
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/git-deploys/{id}/start
pub async fn start(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let config: GitDeploy = sqlx::query_as("SELECT * FROM git_deploys WHERE id = $1 AND user_id = $2")
        .bind(id).bind(claims.sub).fetch_optional(&state.db).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Git deploy not found"))?;
    state.agent.post("/git/start", Some(serde_json::json!({ "name": config.name }))).await
        .map_err(|e| agent_error("Start container", e))?;
    sqlx::query("UPDATE git_deploys SET status = 'running', updated_at = NOW() WHERE id = $1")
        .bind(id).execute(&state.db).await.ok();
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/git-deploys/{id}/restart
pub async fn restart(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let config: GitDeploy = sqlx::query_as("SELECT * FROM git_deploys WHERE id = $1 AND user_id = $2")
        .bind(id).bind(claims.sub).fetch_optional(&state.db).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Git deploy not found"))?;
    state.agent.post("/git/restart", Some(serde_json::json!({ "name": config.name }))).await
        .map_err(|e| agent_error("Restart container", e))?;
    sqlx::query("UPDATE git_deploys SET status = 'running', updated_at = NOW() WHERE id = $1")
        .bind(id).execute(&state.db).await.ok();
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /api/git-deploys/{id}/logs
pub async fn container_logs(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let config: GitDeploy = sqlx::query_as("SELECT * FROM git_deploys WHERE id = $1 AND user_id = $2")
        .bind(id).bind(claims.sub).fetch_optional(&state.db).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Git deploy not found"))?;
    let result = state.agent.post("/git/logs", Some(serde_json::json!({ "name": config.name }))).await
        .map_err(|e| agent_error("Container logs", e))?;
    Ok(Json(result))
}

/// POST /api/webhooks/git/{id}/{secret} — Webhook endpoint (no auth).
pub async fn webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, secret)): Path<(Uuid, String)>,
    body: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Validate Content-Type
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !content_type.is_empty() && !content_type.contains("application/json") {
        return Err(err(StatusCode::BAD_REQUEST, "Content-Type must be application/json"));
    }

    // Rate limit: max 10 attempts per deploy per hour
    {
        let mut attempts = state.webhook_attempts.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let entry = attempts.entry(id).or_insert((0, now));
        if now.duration_since(entry.1).as_secs() >= 3600 {
            *entry = (0, now);
        }
        if entry.0 >= 10 {
            return Err(err(StatusCode::TOO_MANY_REQUESTS, "Rate limit exceeded. Try again later."));
        }
    }

    // Fetch the git deploy config
    let config: GitDeploy = sqlx::query_as(
        "SELECT * FROM git_deploys WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Invalid webhook"))?;

    // Constant-time secret comparison via SHA256 hash
    let provided_hash = {
        let mut h = Sha256::new();
        h.update(secret.as_bytes());
        h.finalize()
    };
    let stored_hash = {
        let mut h = Sha256::new();
        h.update(config.webhook_secret.as_bytes());
        h.finalize()
    };
    if provided_hash != stored_hash {
        // Record failed attempt
        {
            let mut attempts = state.webhook_attempts.lock().unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            let entry = attempts.entry(id).or_insert((0, now));
            if now.duration_since(entry.1).as_secs() >= 3600 {
                *entry = (1, now);
            } else {
                entry.0 += 1;
            }
        }
        return Err(err(StatusCode::NOT_FOUND, "Invalid webhook"));
    }

    if !config.auto_deploy {
        return Err(err(StatusCode::BAD_REQUEST, "Auto-deploy is not enabled for this project"));
    }

    // Parse body to check branch (GitHub/GitLab push payload)
    if let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&body) {
        if let Some(ref_field) = payload.get("ref").and_then(|v| v.as_str()) {
            let push_branch = ref_field.strip_prefix("refs/heads/").unwrap_or(ref_field);
            if push_branch != config.branch {
                return Ok(Json(serde_json::json!({
                    "ok": true,
                    "message": format!("Skipped: push to '{push_branch}', configured branch is '{}'", config.branch),
                })));
            }
        }
    }

    // Update status to building
    sqlx::query("UPDATE git_deploys SET status = 'building', updated_at = NOW() WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .ok();

    let deploy_id = Uuid::new_v4();

    let (tx, _) = broadcast::channel::<ProvisionStep>(32);
    {
        let mut logs = state.provision_logs.lock().unwrap();
        logs.insert(deploy_id, (Vec::new(), tx, Instant::now()));
    }

    // Get user email for activity log
    let user_email: Option<(String,)> = sqlx::query_as(
        "SELECT email FROM users WHERE id = $1",
    )
    .bind(config.user_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    let email = user_email.map(|(e,)| e).unwrap_or_default();
    let owner_id = config.user_id;

    spawn_deploy_task(
        state,
        deploy_id,
        config,
        owner_id,
        email,
        "webhook",
    );

    Ok(Json(serde_json::json!({
        "ok": true,
        "message": "Deploy triggered",
    })))
}

/// Spawn the background clone → build → deploy task.
fn spawn_deploy_task(
    state: AppState,
    deploy_id: Uuid,
    config: GitDeploy,
    user_id: Uuid,
    email: String,
    triggered_by: &str,
) {
    let logs = state.provision_logs.clone();
    let agent = state.agent.clone();
    let db = state.db.clone();
    let deploy_name = config.name.clone();
    let git_deploy_id = config.id;
    let triggered = triggered_by.to_string();

    tokio::spawn(async move {
        let started = Instant::now();

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

        // Build clone body
        let mut clone_body = serde_json::json!({
            "name": config.name,
            "repo_url": config.repo_url,
            "branch": config.branch,
        });
        if let Some(ref key_path) = config.deploy_key_path {
            clone_body["key_path"] = serde_json::json!(key_path);
        }

        // Step 1: Clone
        emit("clone", "Cloning repository", "in_progress", None);
        let clone_result = agent.post_long("/git/clone", Some(clone_body), 300).await;
        let (commit_hash, commit_message) = match &clone_result {
            Ok(result) => {
                emit("clone", "Cloning repository", "done", None);
                let hash = result.get("commit_hash").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
                let msg = result.get("commit_message").and_then(|v| v.as_str()).map(|s| s.to_string());
                (hash, msg)
            }
            Err(e) => {
                emit("clone", "Cloning repository", "error", Some(format!("{e}")));
                emit("complete", "Deploy failed", "error", None);

                let duration_ms = started.elapsed().as_millis() as i32;
                sqlx::query(
                    "INSERT INTO git_deploy_history (git_deploy_id, commit_hash, image_tag, status, output, triggered_by, duration_ms) \
                     VALUES ($1, 'unknown', '', 'failed', $2, $3, $4)",
                )
                .bind(git_deploy_id)
                .bind(format!("Clone failed: {e}"))
                .bind(&triggered)
                .bind(duration_ms)
                .execute(&db)
                .await
                .ok();

                sqlx::query("UPDATE git_deploys SET status = 'failed', updated_at = NOW() WHERE id = $1")
                    .bind(git_deploy_id)
                    .execute(&db)
                    .await
                    .ok();

                tracing::error!("Git deploy clone failed: {deploy_name}: {e}");
                tokio::time::sleep(Duration::from_secs(60)).await;
                logs.lock().unwrap().remove(&deploy_id);
                return;
            }
        };

        // Pre-build hook (runs in git dir on host, before docker build)
        if let Some(ref cmd) = config.pre_build_cmd {
            if !cmd.trim().is_empty() {
                emit("pre_build", "Running pre-build hook", "in_progress", None);
                match agent.post_long("/git/pre-build-hook", Some(serde_json::json!({ "name": config.name, "command": cmd })), 330).await {
                    Ok(result) => {
                        let success = result.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
                        if success {
                            emit("pre_build", "Running pre-build hook", "done", None);
                        } else {
                            let output = result.get("output").and_then(|v| v.as_str()).unwrap_or("");
                            emit("pre_build", "Pre-build hook failed", "error", Some(output.to_string()));
                        }
                    }
                    Err(e) => {
                        emit("pre_build", "Pre-build hook failed", "error", Some(format!("{e}")));
                    }
                }
            }
        }

        // Step 2: Build
        emit("build", "Building Docker image", "in_progress", None);

        let build_body = serde_json::json!({
            "name": config.name,
            "dockerfile": config.dockerfile,
            "commit_hash": commit_hash,
            "build_args": config.build_args,
            "build_context": config.build_context,
        });

        let image_tag = match agent.post_long("/git/build", Some(build_body), 660).await {
            Ok(result) => {
                emit("build", "Building Docker image", "done", None);
                result.get("image_tag").and_then(|v| v.as_str()).unwrap_or("unknown").to_string()
            }
            Err(e) => {
                emit("build", "Building Docker image", "error", Some(format!("{e}")));
                emit("complete", "Deploy failed", "error", None);

                let duration_ms = started.elapsed().as_millis() as i32;
                sqlx::query(
                    "INSERT INTO git_deploy_history (git_deploy_id, commit_hash, commit_message, image_tag, status, output, triggered_by, duration_ms) \
                     VALUES ($1, $2, $3, $4, 'failed', $5, $6, $7)",
                )
                .bind(git_deploy_id)
                .bind(&commit_hash)
                .bind(&commit_message)
                .bind("")
                .bind(format!("Build failed: {e}"))
                .bind(&triggered)
                .bind(duration_ms)
                .execute(&db)
                .await
                .ok();

                sqlx::query("UPDATE git_deploys SET status = 'failed', updated_at = NOW() WHERE id = $1")
                    .bind(git_deploy_id)
                    .execute(&db)
                    .await
                    .ok();

                tracing::error!("Git deploy build failed: {deploy_name}: {e}");
                tokio::time::sleep(Duration::from_secs(60)).await;
                logs.lock().unwrap().remove(&deploy_id);
                return;
            }
        };

        // Step 3: Deploy
        emit("deploy", "Deploying container", "in_progress", None);

        let mut deploy_body = serde_json::json!({
            "name": config.name,
            "image_tag": image_tag,
            "container_port": config.container_port,
            "host_port": config.host_port,
            "env_vars": config.env_vars,
        });
        if let Some(ref domain) = config.domain {
            deploy_body["domain"] = serde_json::json!(domain);
        }
        if let Some(mem) = config.memory_mb {
            deploy_body["memory_mb"] = serde_json::json!(mem);
        }
        if let Some(cpu) = config.cpu_percent {
            deploy_body["cpu_percent"] = serde_json::json!(cpu);
        }
        if let Some(ref ssl_email) = config.ssl_email {
            deploy_body["ssl_email"] = serde_json::json!(ssl_email);
        }

        match agent.post_long("/git/deploy", Some(deploy_body), 120).await {
            Ok(result) => {
                let blue_green = result.get("blue_green").and_then(|v| v.as_bool()).unwrap_or(false);
                if blue_green {
                    emit("deploy", "Deploying container", "done", Some("Zero-downtime swap".into()));
                } else {
                    emit("deploy", "Deploying container", "done", None);
                }
                emit("complete", "Deploy complete", "done", None);

                let container_id = result.get("container_id").and_then(|v| v.as_str()).unwrap_or("");
                let duration_ms = started.elapsed().as_millis() as i32;

                // Record success history
                sqlx::query(
                    "INSERT INTO git_deploy_history (git_deploy_id, commit_hash, commit_message, image_tag, status, output, triggered_by, duration_ms) \
                     VALUES ($1, $2, $3, $4, 'success', $5, $6, $7)",
                )
                .bind(git_deploy_id)
                .bind(&commit_hash)
                .bind(&commit_message)
                .bind(&image_tag)
                .bind(if blue_green { "Deployed with zero-downtime swap" } else { "Deployed successfully" })
                .bind(&triggered)
                .bind(duration_ms)
                .execute(&db)
                .await
                .ok();

                // Update git_deploys
                sqlx::query(
                    "UPDATE git_deploys SET status = 'running', container_id = $1, image_tag = $2, last_deploy = NOW(), last_commit = $3, updated_at = NOW() WHERE id = $4",
                )
                .bind(container_id)
                .bind(&image_tag)
                .bind(&commit_hash)
                .bind(git_deploy_id)
                .execute(&db)
                .await
                .ok();

                tracing::info!("Git deploy success: {deploy_name} ({commit_hash})");
                activity::log_activity(
                    &db, user_id, &email, "git_deploy.deploy",
                    Some("git_deploy"), Some(&deploy_name), Some(&commit_hash), Some("success"),
                ).await;

                // Post-deploy hook
                if let Some(ref cmd) = config.post_deploy_cmd {
                    if !cmd.trim().is_empty() {
                        emit("post_deploy", "Running post-deploy hook", "in_progress", None);
                        match agent.post_long("/git/hook", Some(serde_json::json!({ "name": config.name, "command": cmd })), 330).await {
                            Ok(result) => {
                                let success = result.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
                                let output = result.get("output").and_then(|v| v.as_str()).unwrap_or("");
                                if success {
                                    emit("post_deploy", "Post-deploy hook complete", "done", None);
                                } else {
                                    emit("post_deploy", "Post-deploy hook failed", "error", Some(output.to_string()));
                                }
                            }
                            Err(e) => {
                                emit("post_deploy", "Post-deploy hook failed", "error", Some(format!("{e}")));
                            }
                        }
                    }
                }

                // Deploy notification
                {
                    let notify_db = db.clone();
                    let notify_name = deploy_name.clone();
                    let notify_commit = commit_hash.clone();
                    let notify_user = user_id;
                    tokio::spawn(async move {
                        if let Some(channels) = crate::services::notifications::get_user_channels(&notify_db, notify_user, None).await {
                            let subject = format!("Deploy successful: {notify_name}");
                            let message = format!("Git deploy '{notify_name}' deployed successfully (commit: {notify_commit})");
                            let html = format!(
                                "<div style=\"font-family:sans-serif\"><h2 style=\"color:#22c55e\">Deploy Successful</h2>\
                                 <p><strong>{notify_name}</strong> deployed successfully.</p>\
                                 <p>Commit: <code>{notify_commit}</code></p>\
                                 <p style=\"color:#6b7280;font-size:14px\">Time: {}</p></div>",
                                chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC"),
                            );
                            crate::services::notifications::send_notification(&notify_db, &channels, &subject, &message, &html).await;
                        }
                    });
                }

                // Auto-rollback monitor: watch container for 2 minutes after deploy
                {
                    let monitor_db = db.clone();
                    let monitor_agent = agent.clone();
                    let monitor_name = deploy_name.clone();
                    let monitor_gd_id = git_deploy_id;
                    let monitor_user = user_id;
                    let monitor_email_str = email.clone();
                    let monitor_image = image_tag.clone();
                    let monitor_config_name = config.name.clone();
                    let monitor_config_port = config.container_port;
                    let monitor_config_host_port = config.host_port;
                    let monitor_config_domain = config.domain.clone();

                    tokio::spawn(async move {
                        // Check container health every 15s for 2 minutes
                        for _ in 0..8 {
                            tokio::time::sleep(Duration::from_secs(15)).await;

                            // Check if container is still running
                            match monitor_agent.post("/git/logs", Some(serde_json::json!({ "name": monitor_config_name, "lines": 1 }))).await {
                                Ok(_) => {} // Container is responding — alive
                                Err(_) => {
                                    // Container might be down — check status
                                    let container_name = format!("dockpanel-git-{monitor_config_name}");
                                    tracing::warn!("Auto-rollback: container {container_name} may have crashed, checking...");

                                    // Get last successful deploy before this one
                                    let prev: Option<(String, String)> = sqlx::query_as(
                                        "SELECT image_tag, commit_hash FROM git_deploy_history \
                                         WHERE git_deploy_id = $1 AND status = 'success' AND image_tag != $2 \
                                         ORDER BY created_at DESC LIMIT 1"
                                    )
                                    .bind(monitor_gd_id)
                                    .bind(&monitor_image)
                                    .fetch_optional(&monitor_db)
                                    .await
                                    .ok()
                                    .flatten();

                                    if let Some((prev_image, prev_commit)) = prev {
                                        tracing::warn!("Auto-rollback: rolling back {monitor_name} to {prev_image}");

                                        // Deploy the previous image
                                        let mut rollback_body = serde_json::json!({
                                            "name": monitor_config_name,
                                            "image_tag": prev_image,
                                            "container_port": monitor_config_port,
                                            "host_port": monitor_config_host_port,
                                        });
                                        if let Some(ref domain) = monitor_config_domain {
                                            rollback_body["domain"] = serde_json::json!(domain);
                                        }

                                        if monitor_agent.post_long("/git/deploy", Some(rollback_body), 120).await.is_ok() {
                                            // Record rollback in history
                                            sqlx::query(
                                                "INSERT INTO git_deploy_history (git_deploy_id, commit_hash, image_tag, status, output, triggered_by) \
                                                 VALUES ($1, $2, $3, 'success', 'Auto-rollback after container crash', 'auto-rollback')"
                                            )
                                            .bind(monitor_gd_id)
                                            .bind(&prev_commit)
                                            .bind(&prev_image)
                                            .execute(&monitor_db)
                                            .await
                                            .ok();

                                            // Update git_deploys
                                            sqlx::query("UPDATE git_deploys SET image_tag = $1, last_commit = $2, updated_at = NOW() WHERE id = $3")
                                                .bind(&prev_image)
                                                .bind(&prev_commit)
                                                .bind(monitor_gd_id)
                                                .execute(&monitor_db)
                                                .await
                                                .ok();

                                            // Notify
                                            if let Some(channels) = crate::services::notifications::get_user_channels(&monitor_db, monitor_user, None).await {
                                                let subject = format!("Auto-rollback: {monitor_name}");
                                                let message = format!("Container '{monitor_name}' crashed after deploy. Auto-rolled back to {prev_commit}.");
                                                let html = format!(
                                                    "<div style=\"font-family:sans-serif\"><h2 style=\"color:#f59e0b\">Auto-Rollback</h2>\
                                                     <p>Container <strong>{monitor_name}</strong> crashed after deployment.</p>\
                                                     <p>Automatically rolled back to commit <code>{prev_commit}</code>.</p></div>"
                                                );
                                                crate::services::notifications::send_notification(&monitor_db, &channels, &subject, &message, &html).await;
                                            }

                                            activity::log_activity(
                                                &monitor_db, monitor_user, &monitor_email_str, "git_deploy.auto_rollback",
                                                Some("git_deploy"), Some(&monitor_name), Some(&prev_commit), None,
                                            ).await;

                                            tracing::info!("Auto-rollback complete: {monitor_name} → {prev_image}");
                                        }
                                    }
                                    return; // Stop monitoring after rollback
                                }
                            }
                        }
                        tracing::info!("Auto-rollback monitor: {monitor_name} healthy for 2 minutes, monitoring stopped");
                    });
                }
            }
            Err(e) => {
                emit("deploy", "Deploying container", "error", Some(format!("{e}")));
                emit("complete", "Deploy failed", "error", None);

                let duration_ms = started.elapsed().as_millis() as i32;

                sqlx::query(
                    "INSERT INTO git_deploy_history (git_deploy_id, commit_hash, commit_message, image_tag, status, output, triggered_by, duration_ms) \
                     VALUES ($1, $2, $3, $4, 'failed', $5, $6, $7)",
                )
                .bind(git_deploy_id)
                .bind(&commit_hash)
                .bind(&commit_message)
                .bind(&image_tag)
                .bind(format!("Deploy failed: {e}"))
                .bind(&triggered)
                .bind(duration_ms)
                .execute(&db)
                .await
                .ok();

                sqlx::query("UPDATE git_deploys SET status = 'failed', updated_at = NOW() WHERE id = $1")
                    .bind(git_deploy_id)
                    .execute(&db)
                    .await
                    .ok();

                tracing::error!("Git deploy failed: {deploy_name}: {e}");
                activity::log_activity(
                    &db, user_id, &email, "git_deploy.deploy",
                    Some("git_deploy"), Some(&deploy_name), Some(&commit_hash), Some("failed"),
                ).await;

                // Deploy failure notification
                {
                    let notify_db = db.clone();
                    let notify_name = deploy_name.clone();
                    let notify_commit = commit_hash.clone();
                    let notify_user = user_id;
                    let notify_err = format!("{e}");
                    tokio::spawn(async move {
                        if let Some(channels) = crate::services::notifications::get_user_channels(&notify_db, notify_user, None).await {
                            let subject = format!("Deploy FAILED: {notify_name}");
                            let message = format!("Git deploy '{notify_name}' failed (commit: {notify_commit}): {notify_err}");
                            let html = format!(
                                "<div style=\"font-family:sans-serif\"><h2 style=\"color:#ef4444\">Deploy Failed</h2>\
                                 <p><strong>{notify_name}</strong> deployment failed.</p>\
                                 <p>Commit: <code>{notify_commit}</code></p>\
                                 <p>Error: {notify_err}</p>\
                                 <p style=\"color:#6b7280;font-size:14px\">Time: {}</p></div>",
                                chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC"),
                            );
                            crate::services::notifications::send_notification(&notify_db, &channels, &subject, &message, &html).await;
                        }
                    });
                }
            }
        }

        tokio::time::sleep(Duration::from_secs(60)).await;
        logs.lock().unwrap().remove(&deploy_id);
    });
}
