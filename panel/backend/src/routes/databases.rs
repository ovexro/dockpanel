use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{err, paginate, ApiError};
use crate::AppState;

#[derive(serde::Deserialize)]
pub struct ListQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(serde::Deserialize)]
pub struct CreateDbRequest {
    pub site_id: Uuid,
    pub name: String,
    pub engine: Option<String>,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct Database {
    pub id: Uuid,
    pub site_id: Uuid,
    pub name: String,
    pub engine: String,
    pub db_user: String,
    pub container_id: Option<String>,
    pub port: Option<i32>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// GET /api/databases — List all databases for the current user.
pub async fn list(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Query(params): Query<ListQuery>,
) -> Result<Json<Vec<Database>>, ApiError> {
    let (limit, offset) = paginate(params.limit, params.offset);

    let dbs: Vec<Database> = sqlx::query_as(
        "SELECT d.id, d.site_id, d.name, d.engine, d.db_user, d.container_id, d.port, d.created_at \
         FROM databases d JOIN sites s ON d.site_id = s.id \
         WHERE s.user_id = $1 ORDER BY d.created_at DESC LIMIT $2 OFFSET $3",
    )
    .bind(claims.sub)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(dbs))
}

/// POST /api/databases — Create a new database.
pub async fn create(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<CreateDbRequest>,
) -> Result<(StatusCode, Json<Database>), ApiError> {
    // Verify site ownership
    let site_exists: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM sites WHERE id = $1 AND user_id = $2")
            .bind(body.site_id)
            .bind(claims.sub)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if site_exists.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Site not found"));
    }

    // Validate name
    if body.name.is_empty() || body.name.len() > 63 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid database name"));
    }
    if !body
        .name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Database name must be alphanumeric with underscores",
        ));
    }

    // Check uniqueness per-site
    let existing: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM databases WHERE site_id = $1 AND name = $2")
            .bind(body.site_id)
            .bind(&body.name)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if existing.is_some() {
        return Err(err(StatusCode::CONFLICT, "Database name already exists for this site"));
    }

    let engine = body.engine.as_deref().unwrap_or("postgres");
    if !["postgres", "mysql", "mariadb"].contains(&engine) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Engine must be postgres, mysql, or mariadb",
        ));
    }

    // Generate password and find available port
    let password = uuid::Uuid::new_v4().to_string().replace('-', "");
    let port = find_available_port(&state, engine).await?;

    // Call agent to create container
    let agent_body = serde_json::json!({
        "name": body.name,
        "engine": engine,
        "password": password,
        "port": port,
    });

    let result = state
        .agent
        .post("/databases", Some(agent_body))
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Failed to create database: {e}")))?;

    let container_id = result
        .get("container_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Encrypt password (store as-is for now, proper encryption later)
    let db_record: Database = sqlx::query_as(
        "INSERT INTO databases (site_id, name, engine, db_user, db_password_enc, container_id, port) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) \
         RETURNING id, site_id, name, engine, db_user, container_id, port, created_at",
    )
    .bind(body.site_id)
    .bind(&body.name)
    .bind(engine)
    .bind(&body.name)
    .bind(&password)
    .bind(&container_id)
    .bind(port)
    .fetch_one(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    tracing::info!("Database created: {} ({}, port {})", body.name, engine, port);

    Ok((StatusCode::CREATED, Json(db_record)))
}

/// GET /api/databases/{id}/credentials — Get database connection details.
pub async fn credentials(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let row: Option<(String, String, String, Option<i32>, Option<String>)> = sqlx::query_as(
        "SELECT d.name, d.engine, d.db_password_enc, d.port, d.container_id \
         FROM databases d JOIN sites s ON d.site_id = s.id \
         WHERE d.id = $1 AND s.user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let (name, engine, password, port, container_id) =
        row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Database not found"))?;

    let host = container_id
        .as_deref()
        .map(|_| format!("dockpanel-db-{name}"))
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let port = port.unwrap_or(5432);

    let connection_string = match engine.as_str() {
        "mysql" | "mariadb" => format!("mysql://{name}:{password}@127.0.0.1:{port}/{name}"),
        _ => format!("postgresql://{name}:{password}@127.0.0.1:{port}/{name}"),
    };

    Ok(Json(serde_json::json!({
        "host": "127.0.0.1",
        "port": port,
        "database": name,
        "username": name,
        "password": password,
        "engine": engine,
        "connection_string": connection_string,
        "internal_host": host,
    })))
}

/// DELETE /api/databases/{id} — Delete a database and its container.
pub async fn remove(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Verify ownership through site
    let db: Option<(Uuid, String, Option<String>)> = sqlx::query_as(
        "SELECT d.id, d.name, d.container_id FROM databases d \
         JOIN sites s ON d.site_id = s.id \
         WHERE d.id = $1 AND s.user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let (_, name, container_id) = db.ok_or_else(|| err(StatusCode::NOT_FOUND, "Database not found"))?;

    // Remove container via agent (must succeed before DB deletion)
    if let Some(cid) = &container_id {
        let agent_path = format!("/databases/{cid}");
        state.agent.delete(&agent_path).await
            .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Failed to remove database container: {e}")))?;
    }

    // Delete from DB
    sqlx::query("DELETE FROM databases WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    tracing::info!("Database deleted: {name}");

    Ok(Json(serde_json::json!({ "ok": true, "name": name })))
}

/// Find an available port using a single SQL query to find the first gap.
async fn find_available_port(state: &AppState, engine: &str) -> Result<i32, ApiError> {
    // Choose port range based on engine
    let (range_start, range_end) = match engine {
        "mysql" | "mariadb" => (3307, 3400),
        _ => (5433, 5500),
    };

    // Find first unused port in range with a single query
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT s.port FROM generate_series($1::int, $2::int) AS s(port) \
         WHERE s.port NOT IN (SELECT port FROM databases WHERE port IS NOT NULL) \
         LIMIT 1"
    )
    .bind(range_start)
    .bind(range_end)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    row.map(|(p,)| p).ok_or_else(|| err(
        StatusCode::INTERNAL_SERVER_ERROR,
        "No available ports for database",
    ))
}
