use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::{AuthUser, Claims, ServerScope};
use crate::error::{internal_error, err, agent_error, require_admin, ApiError};
use crate::services::activity;
use crate::services::agent::AgentHandle;
use crate::AppState;

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct Stack {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub yaml: String,
    pub service_count: i32,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Deserialize)]
pub struct CreateStackRequest {
    pub name: String,
    pub yaml: String,
}

#[derive(serde::Deserialize)]
pub struct UpdateStackRequest {
    pub yaml: String,
}

/// GET /api/stacks — List all stacks for the current user.
pub async fn list(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let stacks: Vec<Stack> = sqlx::query_as(
        "SELECT id, user_id, name, yaml, service_count, created_at, updated_at \
         FROM docker_stacks WHERE user_id = $1 AND server_id = $2 ORDER BY created_at DESC",
    )
    .bind(claims.sub)
    .bind(server_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("list stacks", e))?;

    // Get live container status from agent
    let apps = agent
        .get("/apps")
        .await
        .unwrap_or(serde_json::json!([]));

    let apps_arr = apps.as_array().cloned().unwrap_or_default();

    // Build response with live status per stack
    let result: Vec<serde_json::Value> = stacks
        .iter()
        .map(|stack| {
            let stack_id_str = stack.id.to_string();
            let services: Vec<&serde_json::Value> = apps_arr
                .iter()
                .filter(|a| a.get("stack_id").and_then(|v| v.as_str()) == Some(&stack_id_str))
                .collect();

            let running = services
                .iter()
                .filter(|a| a.get("status").and_then(|v| v.as_str()) == Some("running"))
                .count();
            let total = services.len();

            serde_json::json!({
                "id": stack.id,
                "name": stack.name,
                "service_count": stack.service_count,
                "running": running,
                "total": total,
                "status": if total == 0 { "removed" } else if running == total { "running" } else if running == 0 { "stopped" } else { "partial" },
                "services": services,
                "created_at": stack.created_at,
                "updated_at": stack.updated_at,
            })
        })
        .collect();

    Ok(Json(serde_json::json!(result)))
}

/// GET /api/stacks/{id} — Get stack details with live service status.
pub async fn get_one(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let stack: Stack = sqlx::query_as(
        "SELECT id, user_id, name, yaml, service_count, created_at, updated_at \
         FROM docker_stacks WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("get_one stacks", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Stack not found"))?;

    let apps = agent
        .get("/apps")
        .await
        .unwrap_or(serde_json::json!([]));

    let stack_id_str = stack.id.to_string();
    let services: Vec<&serde_json::Value> = apps
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter(|a| a.get("stack_id").and_then(|v| v.as_str()) == Some(&stack_id_str))
                .collect()
        })
        .unwrap_or_default();

    let running = services
        .iter()
        .filter(|a| a.get("status").and_then(|v| v.as_str()) == Some("running"))
        .count();

    Ok(Json(serde_json::json!({
        "id": stack.id,
        "name": stack.name,
        "yaml": stack.yaml,
        "service_count": stack.service_count,
        "running": running,
        "total": services.len(),
        "services": services,
        "created_at": stack.created_at,
        "updated_at": stack.updated_at,
    })))
}

/// POST /api/stacks — Create and deploy a new stack.
pub async fn create(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(server_id, agent): ServerScope,
    Json(body): Json<CreateStackRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    require_admin(&claims.role)?;

    if body.name.trim().is_empty() || body.name.len() > 100 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid stack name"));
    }
    if body.yaml.len() > 65536 {
        return Err(err(StatusCode::BAD_REQUEST, "YAML too large (max 64KB)"));
    }

    // Parse to get service count
    let parsed = agent
        .post(
            "/apps/compose/parse",
            Some(serde_json::json!({ "yaml": body.yaml })),
        )
        .await
        .map_err(|e| agent_error("Compose parse", e))?;

    let service_count = parsed
        .as_array()
        .map(|a| a.len() as i32)
        .unwrap_or(0);

    // Create DB record first to get the stack ID
    let stack: Stack = sqlx::query_as(
        "INSERT INTO docker_stacks (user_id, server_id, name, yaml, service_count) \
         VALUES ($1, $2, $3, $4, $5) \
         RETURNING id, user_id, name, yaml, service_count, created_at, updated_at",
    )
    .bind(claims.sub)
    .bind(server_id)
    .bind(&body.name)
    .bind(&body.yaml)
    .bind(service_count)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("create stacks", e))?;

    // Deploy with stack_id label
    let deploy_result = agent
        .post(
            "/apps/compose/deploy",
            Some(serde_json::json!({
                "yaml": body.yaml,
                "stack_id": stack.id.to_string(),
            })),
        )
        .await
        .map_err(|e| {
            // Rollback DB record on deploy failure
            let db = state.db.clone();
            let stack_id = stack.id;
            tokio::spawn(async move {
                let _ = sqlx::query("DELETE FROM docker_stacks WHERE id = $1")
                    .bind(stack_id)
                    .execute(&db)
                    .await;
            });
            agent_error("Stack deploy", e)
        })?;

    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "stack.create",
        Some("stack"),
        Some(&stack.name),
        None,
        None,
    )
    .await;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": stack.id,
            "name": stack.name,
            "service_count": service_count,
            "deploy_result": deploy_result,
        })),
    ))
}

/// POST /api/stacks/{id}/start — Start all services in a stack.
pub async fn start(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    stack_action(&state, &claims, &agent, id, "start").await
}

/// POST /api/stacks/{id}/stop — Stop all services in a stack.
pub async fn stop(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    stack_action(&state, &claims, &agent, id, "stop").await
}

/// POST /api/stacks/{id}/restart — Restart all services in a stack.
pub async fn restart(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    stack_action(&state, &claims, &agent, id, "restart").await
}

/// DELETE /api/stacks/{id} — Remove all services and delete the stack.
pub async fn remove(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let stack: Option<(Uuid, String)> =
        sqlx::query_as("SELECT id, name FROM docker_stacks WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(claims.sub)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("remove stacks", e))?;

    let (_, name) = stack.ok_or_else(|| err(StatusCode::NOT_FOUND, "Stack not found"))?;

    // Remove all containers
    let result = agent
        .post(
            "/apps/stack/action",
            Some(serde_json::json!({
                "stack_id": id.to_string(),
                "action": "remove",
            })),
        )
        .await;

    // Delete DB record even if container removal had partial failures
    sqlx::query("DELETE FROM docker_stacks WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("remove stacks", e))?;

    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "stack.remove",
        Some("stack"),
        Some(&name),
        None,
        None,
    )
    .await;

    Ok(Json(serde_json::json!({
        "ok": true,
        "name": name,
        "agent_result": result.ok(),
    })))
}

/// PUT /api/stacks/{id} — Update stack by removing old containers and redeploying.
pub async fn update(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateStackRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    if body.yaml.len() > 65536 {
        return Err(err(StatusCode::BAD_REQUEST, "YAML too large (max 64KB)"));
    }

    // Verify ownership
    let exists: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM docker_stacks WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(claims.sub)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("update stacks", e))?;

    if exists.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Stack not found"));
    }

    // Parse new YAML
    let parsed = agent
        .post(
            "/apps/compose/parse",
            Some(serde_json::json!({ "yaml": body.yaml })),
        )
        .await
        .map_err(|e| agent_error("Compose parse", e))?;

    let service_count = parsed.as_array().map(|a| a.len() as i32).unwrap_or(0);

    // Remove old containers
    let _ = agent
        .post(
            "/apps/stack/action",
            Some(serde_json::json!({
                "stack_id": id.to_string(),
                "action": "remove",
            })),
        )
        .await;

    // Deploy new containers with same stack_id
    let deploy_result = agent
        .post(
            "/apps/compose/deploy",
            Some(serde_json::json!({
                "yaml": body.yaml,
                "stack_id": id.to_string(),
            })),
        )
        .await
        .map_err(|e| agent_error("Stack redeploy", e))?;

    // Update DB record
    sqlx::query(
        "UPDATE docker_stacks SET yaml = $1, service_count = $2, updated_at = NOW() WHERE id = $3",
    )
    .bind(&body.yaml)
    .bind(service_count)
    .bind(id)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("update stacks", e))?;

    Ok(Json(serde_json::json!({
        "ok": true,
        "service_count": service_count,
        "deploy_result": deploy_result,
    })))
}

/// Internal helper for start/stop/restart stack actions.
async fn stack_action(
    state: &AppState,
    claims: &Claims,
    agent: &AgentHandle,
    id: Uuid,
    action: &str,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    // Verify ownership
    let exists: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM docker_stacks WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(claims.sub)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("update stacks", e))?;

    if exists.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Stack not found"));
    }

    let result = agent
        .post(
            "/apps/stack/action",
            Some(serde_json::json!({
                "stack_id": id.to_string(),
                "action": action,
            })),
        )
        .await
        .map_err(|e| agent_error(&format!("Stack {action}"), e))?;

    Ok(Json(result))
}
