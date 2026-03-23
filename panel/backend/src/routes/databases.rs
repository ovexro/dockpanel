use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::{AuthUser, ServerScope};
use crate::error::{err, agent_error, paginate, ApiError};
use crate::routes::reseller_dashboard::check_reseller_quota;
use crate::services::agent::AgentError;
use crate::AppState;

/// Convert an agent error to a user-facing error for SQL operations.
/// Unlike `agent_error()`, this passes through the actual SQL error message.
fn sql_error(e: AgentError) -> ApiError {
    match e {
        AgentError::Status(_code, body) => {
            // Try to extract "error" field from agent JSON response
            let msg = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v.get("error").and_then(|e| e.as_str().map(String::from)))
                .unwrap_or(body);
            err(StatusCode::BAD_REQUEST, &msg)
        }
        other => agent_error("SQL query", other),
    }
}

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
    ServerScope(server_id, _agent): ServerScope,
    Query(params): Query<ListQuery>,
) -> Result<Json<Vec<Database>>, ApiError> {
    let (limit, offset) = paginate(params.limit, params.offset);

    let dbs: Vec<Database> = sqlx::query_as(
        "SELECT d.id, d.site_id, d.name, d.engine, d.db_user, d.container_id, d.port, d.created_at \
         FROM databases d JOIN sites s ON d.site_id = s.id \
         WHERE s.user_id = $1 AND s.server_id = $2 ORDER BY d.created_at DESC LIMIT $3 OFFSET $4",
    )
    .bind(claims.sub)
    .bind(server_id)
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
    ServerScope(_server_id, agent): ServerScope,
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

    // Check reseller quota before creating database
    check_reseller_quota(&state.db, claims.sub, "databases").await?;

    // Generate password and find available port
    let password = uuid::Uuid::new_v4().to_string().replace('-', "");
    let port = find_available_port(&state, engine).await?;

    // Insert DB record first to atomically claim the port (unique index prevents races).
    // container_id is empty until the agent creates it.
    let db_record: Database = sqlx::query_as(
        "INSERT INTO databases (site_id, name, engine, db_user, db_password_enc, container_id, port) \
         VALUES ($1, $2, $3, $4, $5, '', $6) \
         RETURNING id, site_id, name, engine, db_user, container_id, port, created_at",
    )
    .bind(body.site_id)
    .bind(&body.name)
    .bind(engine)
    .bind(&body.name)
    .bind(&password)
    .bind(port)
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        if e.to_string().contains("unique") || e.to_string().contains("duplicate") {
            err(StatusCode::CONFLICT, "Port or database name conflict, please retry")
        } else {
            err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
        }
    })?;

    // Call agent to create container
    let agent_body = serde_json::json!({
        "name": body.name,
        "engine": engine,
        "password": password,
        "port": port,
    });

    let result = match agent.post("/databases", Some(agent_body)).await {
        Ok(r) => r,
        Err(e) => {
            // Clean up the DB record if agent fails
            let _ = sqlx::query("DELETE FROM databases WHERE id = $1")
                .bind(db_record.id)
                .execute(&state.db)
                .await;
            return Err(agent_error("Database creation", e));
        }
    };

    let container_id = result
        .get("container_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Update with the actual container_id
    sqlx::query("UPDATE databases SET container_id = $1 WHERE id = $2")
        .bind(&container_id)
        .bind(db_record.id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    // Increment reseller database counter
    let _ = sqlx::query(
        "UPDATE reseller_profiles SET used_databases = used_databases + 1, updated_at = NOW() \
         WHERE user_id = (SELECT reseller_id FROM users WHERE id = $1 AND reseller_id IS NOT NULL)"
    ).bind(claims.sub).execute(&state.db).await;

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
    ServerScope(_server_id, agent): ServerScope,
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
        agent.delete(&agent_path).await
            .map_err(|e| agent_error("Database removal", e))?;
    }

    // Delete from DB
    sqlx::query("DELETE FROM databases WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    // Decrement reseller database counter
    let _ = sqlx::query(
        "UPDATE reseller_profiles SET used_databases = GREATEST(used_databases - 1, 0), updated_at = NOW() \
         WHERE user_id = (SELECT reseller_id FROM users WHERE id = $1 AND reseller_id IS NOT NULL)"
    ).bind(claims.sub).execute(&state.db).await;

    tracing::info!("Database deleted: {name}");

    Ok(Json(serde_json::json!({ "ok": true, "name": name })))
}

/// Helper: fetch database info (name, engine, password) with ownership check.
async fn get_db_info(
    state: &AppState,
    id: Uuid,
    user_id: Uuid,
) -> Result<(String, String, String, i32), ApiError> {
    let row: Option<(String, String, String, Option<i32>)> = sqlx::query_as(
        "SELECT d.name, d.engine, d.db_password_enc, d.port \
         FROM databases d JOIN sites s ON d.site_id = s.id \
         WHERE d.id = $1 AND s.user_id = $2",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let (name, engine, password, port) =
        row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Database not found"))?;
    let port = port.unwrap_or(5432);
    Ok((name, engine, password, port))
}

/// GET /api/databases/{id}/tables — List tables in the database.
pub async fn tables(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (name, engine, password, _port) = get_db_info(&state, id, claims.sub).await?;

    let sql = match engine.as_str() {
        "mysql" | "mariadb" => {
            "SELECT table_name, table_type, table_rows, \
             ROUND((data_length + index_length) / 1024, 1) AS size_kb \
             FROM information_schema.tables WHERE table_schema = DATABASE() \
             ORDER BY table_name"
                .to_string()
        }
        _ => {
            "SELECT t.table_name, t.table_type, \
             pg_catalog.pg_size_pretty(pg_catalog.pg_total_relation_size(quote_ident(t.table_name))) AS size \
             FROM information_schema.tables t \
             WHERE t.table_schema = 'public' ORDER BY t.table_name"
                .to_string()
        }
    };

    let container = format!("dockpanel-db-{name}");
    let agent_body = serde_json::json!({
        "container": container,
        "engine": engine,
        "user": name,
        "password": password,
        "database": name,
        "sql": sql,
    });

    agent
        .post("/databases/query", Some(agent_body))
        .await
        .map(Json)
        .map_err(sql_error)
}

/// GET /api/databases/{id}/tables/{table} — Get table schema (columns, types).
pub async fn table_schema(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path((id, table)): Path<(Uuid, String)>,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Validate table name to prevent injection
    if table.is_empty()
        || table.len() > 128
        || !table
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid table name"));
    }

    let (name, engine, password, _port) = get_db_info(&state, id, claims.sub).await?;

    let (sql, params) = match engine.as_str() {
        "mysql" | "mariadb" => (
            "SELECT column_name, column_type, is_nullable, column_default, column_key, extra \
             FROM information_schema.columns \
             WHERE table_schema = DATABASE() AND table_name = ? \
             ORDER BY ordinal_position"
                .to_string(),
            vec![table.clone()],
        ),
        _ => (
            "SELECT column_name, data_type, character_maximum_length, is_nullable, column_default \
             FROM information_schema.columns \
             WHERE table_schema = 'public' AND table_name = $1 \
             ORDER BY ordinal_position"
                .to_string(),
            vec![table.clone()],
        ),
    };

    let container = format!("dockpanel-db-{name}");
    let agent_body = serde_json::json!({
        "container": container,
        "engine": engine,
        "user": name,
        "password": password,
        "database": name,
        "sql": sql,
        "params": params,
    });

    agent
        .post("/databases/query", Some(agent_body))
        .await
        .map(Json)
        .map_err(sql_error)
}

#[derive(serde::Deserialize)]
pub struct SqlQueryRequest {
    pub sql: String,
}

/// POST /api/databases/{id}/query — Execute a SQL query.
pub async fn query(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<SqlQueryRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.sql.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Query is empty"));
    }
    if body.sql.len() > 10_000 {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Query too long (max 10KB)",
        ));
    }

    let (name, engine, password, _port) = get_db_info(&state, id, claims.sub).await?;

    let container = format!("dockpanel-db-{name}");
    let agent_body = serde_json::json!({
        "container": container,
        "engine": engine,
        "user": name,
        "password": password,
        "database": name,
        "sql": body.sql,
    });

    agent
        .post("/databases/query", Some(agent_body))
        .await
        .map(Json)
        .map_err(sql_error)
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
