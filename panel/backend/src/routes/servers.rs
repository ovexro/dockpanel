use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{err, ApiError};
use crate::services::activity;
use crate::AppState;

#[derive(serde::Deserialize)]
pub struct CreateServerRequest {
    pub name: String,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct Server {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub ip_address: Option<String>,
    #[serde(skip_serializing)]
    pub agent_token: String,
    pub status: String,
    pub last_seen_at: Option<chrono::DateTime<chrono::Utc>>,
    pub os_info: Option<String>,
    pub cpu_cores: Option<i32>,
    pub ram_mb: Option<i32>,
    pub disk_gb: Option<i32>,
    pub agent_version: Option<String>,
    pub cpu_usage: Option<f32>,
    pub mem_used_mb: Option<i64>,
    pub uptime_secs: Option<i64>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// GET /api/servers — List current user's servers.
pub async fn list(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<Vec<Server>>, ApiError> {
    let servers: Vec<Server> =
        sqlx::query_as("SELECT * FROM servers WHERE user_id = $1 ORDER BY created_at DESC")
            .bind(claims.sub)
            .fetch_all(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(servers))
}

/// POST /api/servers — Register a new server. Returns agent token and install script.
pub async fn create(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<CreateServerRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let name = body.name.trim();
    if name.is_empty() || name.len() > 100 {
        return Err(err(StatusCode::BAD_REQUEST, "Name must be 1-100 characters"));
    }

    let agent_token = format!(
        "{}{}",
        Uuid::new_v4().to_string().replace('-', ""),
        Uuid::new_v4().to_string().replace('-', ""),
    );

    let server: Server = sqlx::query_as(
        "INSERT INTO servers (user_id, name, agent_token, status) \
         VALUES ($1, $2, $3, 'pending') RETURNING *",
    )
    .bind(claims.sub)
    .bind(name)
    .bind(&agent_token)
    .fetch_one(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    // Generate install script
    let install_script = format!(
        "curl -sSL https://dockpanel.dev/install.sh | sudo bash -s -- --token {} --server-id {}",
        agent_token, server.id
    );

    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "server.create",
        Some("server"),
        Some(name),
        None,
        None,
    )
    .await;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": server.id,
            "name": server.name,
            "status": server.status,
            "agent_token": agent_token,
            "install_script": install_script,
        })),
    ))
}

/// GET /api/servers/{id} — Get server details.
pub async fn get_one(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Server>, ApiError> {
    let server: Server =
        sqlx::query_as("SELECT * FROM servers WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(claims.sub)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
            .ok_or_else(|| err(StatusCode::NOT_FOUND, "Server not found"))?;

    Ok(Json(server))
}

/// DELETE /api/servers/{id} — Remove a server.
pub async fn remove(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let server: Server =
        sqlx::query_as("SELECT * FROM servers WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(claims.sub)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
            .ok_or_else(|| err(StatusCode::NOT_FOUND, "Server not found"))?;

    sqlx::query("DELETE FROM servers WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "server.delete",
        Some("server"),
        Some(&server.name),
        None,
        None,
    )
    .await;

    Ok(Json(serde_json::json!({ "ok": true, "name": server.name })))
}
