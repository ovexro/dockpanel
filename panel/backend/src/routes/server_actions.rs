use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::{AdminUser, AuthUser};
use crate::error::{internal_error, err, ApiError};
use crate::AppState;

#[derive(serde::Deserialize)]
pub struct DispatchRequest {
    pub action: String,
    pub payload: Option<serde_json::Value>,
}

const ALLOWED_ACTIONS: &[&str] = &[
    "restart_nginx", "restart_php", "restart_mysql", "restart_postgres",
    "reload_nginx", "reload_php", "check_services", "disk_cleanup",
    "update_packages", "restart_agent",
];

/// POST /api/servers/{id}/command — Dispatch a command to a server's agent.
pub async fn dispatch(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(server_id): Path<Uuid>,
    Json(body): Json<DispatchRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    if !ALLOWED_ACTIONS.contains(&body.action.as_str()) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid action"));
    }

    // Verify server belongs to user
    let exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM servers WHERE id = $1 AND user_id = $2",
    )
    .bind(server_id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("dispatch", e))?;

    if exists.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Server not found"));
    }

    let payload = body.payload.unwrap_or(serde_json::json!({}));

    let cmd: (Uuid,) = sqlx::query_as(
        "INSERT INTO agent_commands (server_id, action, payload) \
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(server_id)
    .bind(&body.action)
    .bind(&payload)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("dispatch", e))?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "command_id": cmd.0,
            "status": "pending",
        })),
    ))
}

/// GET /api/servers/{id}/commands — List recent commands for a server.
pub async fn list_commands(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(server_id): Path<Uuid>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    // Verify server belongs to user
    let exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM servers WHERE id = $1 AND user_id = $2",
    )
    .bind(server_id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("list commands", e))?;

    if exists.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Server not found"));
    }

    let rows: Vec<crate::routes::agent_commands::AgentCommand> = sqlx::query_as(
        "SELECT * FROM agent_commands WHERE server_id = $1 ORDER BY created_at DESC LIMIT 50",
    )
    .bind(server_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("list commands", e))?;

    let commands: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|c| {
            serde_json::json!({
                "id": c.id,
                "action": c.action,
                "status": c.status,
                "result": c.result,
                "created_at": c.created_at,
                "picked_at": c.picked_at,
                "completed_at": c.completed_at,
            })
        })
        .collect();

    Ok(Json(commands))
}

/// GET /api/servers/{id}/command/{cmd_id} — Get single command status.
pub async fn command_status(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path((server_id, cmd_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let row: Option<crate::routes::agent_commands::AgentCommand> = sqlx::query_as(
        "SELECT ac.* FROM agent_commands ac \
         JOIN servers s ON s.id = ac.server_id \
         WHERE ac.id = $1 AND ac.server_id = $2 AND s.user_id = $3",
    )
    .bind(cmd_id)
    .bind(server_id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("command status", e))?;

    let cmd = row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Command not found"))?;

    Ok(Json(serde_json::json!({
        "id": cmd.id,
        "action": cmd.action,
        "payload": cmd.payload,
        "status": cmd.status,
        "result": cmd.result,
        "created_at": cmd.created_at,
        "picked_at": cmd.picked_at,
        "completed_at": cmd.completed_at,
    })))
}
