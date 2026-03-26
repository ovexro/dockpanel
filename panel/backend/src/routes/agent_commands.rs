use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use std::time::Instant;
use uuid::Uuid;

use crate::error::{internal_error, err, ApiError};
use crate::AppState;

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct AgentCommand {
    pub id: Uuid,
    pub server_id: Uuid,
    pub action: String,
    pub payload: serde_json::Value,
    pub status: String,
    pub result: Option<serde_json::Value>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub picked_at: Option<chrono::DateTime<chrono::Utc>>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Helper: extract + verify Bearer token → returns server_id.
/// Uses hash-based lookup with plaintext fallback for unmigrated rows.
async fn auth_agent(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Uuid, ApiError> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "Missing authorization"))?;

    // Try hash-based lookup first
    let token_hash = crate::helpers::hash_agent_token(token);
    let row: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM servers WHERE agent_token_hash = $1")
            .bind(&token_hash)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("unknown", e))?;

    if let Some(r) = row {
        return Ok(r.0);
    }

    // Fallback: plaintext lookup for pre-migration rows
    let row: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM servers WHERE agent_token = $1 AND agent_token_hash IS NULL")
            .bind(token)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("unknown", e))?;

    row.map(|r| r.0)
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "Invalid token"))
}

/// GET /api/agent/commands — Agent polls for pending commands.
/// Returns up to 10 pending commands and marks them as 'running'.
pub async fn poll(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<AgentCommand>>, ApiError> {
    let server_id = auth_agent(&state, &headers).await?;

    // Rate limit: max 120 requests per minute per server_id
    {
        let mut limits = state.agent_rate_limits.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let entry = limits.entry(server_id).or_insert((0, now));
        if now.duration_since(entry.1).as_secs() >= 60 {
            *entry = (1, now);
        } else {
            entry.0 += 1;
            if entry.0 > 120 {
                return Err(err(StatusCode::TOO_MANY_REQUESTS, "Rate limit exceeded"));
            }
        }
    }

    // Fetch and claim pending commands atomically
    let commands: Vec<AgentCommand> = sqlx::query_as(
        "UPDATE agent_commands SET status = 'running', picked_at = NOW() \
         WHERE id IN (\
           SELECT id FROM agent_commands \
           WHERE server_id = $1 AND status = 'pending' \
           ORDER BY created_at ASC LIMIT 10 \
           FOR UPDATE SKIP LOCKED\
         ) RETURNING *",
    )
    .bind(server_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("poll", e))?;

    Ok(Json(commands))
}

#[derive(serde::Deserialize)]
pub struct CommandResult {
    pub command_id: Uuid,
    pub status: String,
    pub result: Option<serde_json::Value>,
}

/// POST /api/agent/commands/result — Agent reports command completion.
pub async fn report_result(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CommandResult>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let server_id = auth_agent(&state, &headers).await?;

    // Rate limit: max 120 requests per minute per server_id
    {
        let mut limits = state.agent_rate_limits.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let entry = limits.entry(server_id).or_insert((0, now));
        if now.duration_since(entry.1).as_secs() >= 60 {
            *entry = (1, now);
        } else {
            entry.0 += 1;
            if entry.0 > 120 {
                return Err(err(StatusCode::TOO_MANY_REQUESTS, "Rate limit exceeded"));
            }
        }
    }

    let status = match body.status.as_str() {
        "completed" | "failed" => body.status.as_str(),
        _ => return Err(err(StatusCode::BAD_REQUEST, "Status must be 'completed' or 'failed'")),
    };

    let result = sqlx::query(
        "UPDATE agent_commands SET status = $1, result = $2, completed_at = NOW() \
         WHERE id = $3 AND server_id = $4 AND status = 'running'",
    )
    .bind(status)
    .bind(&body.result)
    .bind(body.command_id)
    .bind(server_id)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("report result", e))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Command not found or not running"));
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}
