use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
    Json,
};
use futures::stream::StreamExt;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{err, agent_error, require_admin, ApiError};
use crate::routes::{is_valid_container_id, is_valid_name};
use crate::routes::sites::ProvisionStep;
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

/// POST /api/apps/deploy — Deploy a Docker app from template (async with SSE progress).
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

    let deploy_id = Uuid::new_v4();

    // Create provisioning channel (reuse the same provision_logs map from AppState)
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
    let app_name = body.name.clone();
    let template = body.template_id.clone();

    let agent_body = serde_json::json!({
        "template_id": body.template_id,
        "name": body.name,
        "port": body.port,
        "env": body.env.unwrap_or_default(),
    });

    // Spawn background deploy task
    tokio::spawn(async move {
        let emit = |step: &str, label: &str, status: &str, msg: Option<String>| {
            let ev = ProvisionStep {
                step: step.into(),
                label: label.into(),
                status: status.into(),
                message: msg,
            };
            if let Ok(mut map) = logs.lock() {
                if let Some((history, tx, _)) = map.get_mut(&deploy_id) {
                    history.push(ev.clone());
                    let _ = tx.send(ev);
                }
            }
        };

        emit("pull", "Pulling Docker image", "in_progress", None);

        match agent.post("/apps/deploy", Some(agent_body)).await {
            Ok(_) => {
                emit("pull", "Pulling Docker image", "done", None);
                emit("start", "Starting container", "done", None);
                emit("complete", "App deployed", "done", None);

                tracing::info!("App deployed: {} ({})", app_name, template);
                activity::log_activity(
                    &db, user_id, &email, "app.deploy",
                    Some("app"), Some(&app_name), Some(&template), None,
                ).await;
            }
            Err(e) => {
                emit("pull", "Pulling Docker image", "error", Some(format!("Deploy failed: {e}")));
                emit("complete", "Deploy failed", "error", None);
                tracing::error!("App deploy failed: {} ({}): {e}", app_name, template);
            }
        }

        tokio::time::sleep(Duration::from_secs(30)).await;
        logs.lock().unwrap().remove(&deploy_id);
    });

    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({
        "deploy_id": deploy_id,
        "message": "Deployment started",
    }))))
}

/// GET /api/apps/deploy/{deploy_id}/log — SSE stream of deploy progress.
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

/// POST /api/apps/{container_id}/update — Pull latest image and recreate container (async with SSE).
pub async fn update_app(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(container_id): Path<String>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    require_admin(&claims.role)?;
    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }

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
    let cid = container_id.clone();

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

        emit("pull", "Pulling latest image", "in_progress", None);

        let agent_path = format!("/apps/{}/update", cid);
        match agent.post(&agent_path, None).await {
            Ok(_) => {
                emit("pull", "Pulling latest image", "done", None);
                emit("recreate", "Recreating container", "done", None);
                emit("complete", "App updated", "done", None);
                activity::log_activity(
                    &db, user_id, &email, "app.update",
                    Some("app"), Some(&cid), None, None,
                ).await;
                tracing::info!("App updated: {cid}");
            }
            Err(e) => {
                emit("pull", "Pulling latest image", "error", Some(format!("{e}")));
                emit("complete", "Update failed", "error", None);
                tracing::error!("App update failed: {cid}: {e}");
            }
        }

        tokio::time::sleep(Duration::from_secs(60)).await;
        logs.lock().unwrap().remove(&deploy_id);
    });

    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({
        "deploy_id": deploy_id,
        "message": "Update started",
    }))))
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
