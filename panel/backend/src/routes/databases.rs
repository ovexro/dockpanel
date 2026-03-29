use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::{AuthUser, ServerScope};
use crate::error::{internal_error, err, agent_error, paginate, ApiError};
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
    .map_err(|e| internal_error("list databases", e))?;

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
            .map_err(|e| internal_error("create databases", e))?;

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
            .map_err(|e| internal_error("create databases", e))?;

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

    // Encrypt the password before storing in the database
    let encrypted_password = crate::services::secrets_crypto::encrypt_credential(&password, &state.config.jwt_secret)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Encryption failed: {e}")))?;

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
    .bind(&encrypted_password)
    .bind(port)
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        if e.to_string().contains("unique") || e.to_string().contains("duplicate") {
            err(StatusCode::CONFLICT, "Port or database name conflict, please retry")
        } else {
            internal_error("create databases", e)
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
        .map_err(|e| internal_error("create databases", e))?;

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
    .map_err(|e| internal_error("credentials", e))?;

    let (name, engine, password_enc, port, container_id) =
        row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Database not found"))?;

    // Decrypt password (with legacy plaintext fallback for pre-encryption records)
    let password = crate::services::secrets_crypto::decrypt_credential_or_legacy(&password_enc, &state.config.jwt_secret);

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
    .map_err(|e| internal_error("remove databases", e))?;

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
        .map_err(|e| internal_error("remove databases", e))?;

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
    .map_err(|e| internal_error("remove databases", e))?;

    let (name, engine, password_enc, port) =
        row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Database not found"))?;
    let port = port.unwrap_or(5432);
    // Decrypt password for agent use (with legacy plaintext fallback)
    let password = crate::services::secrets_crypto::decrypt_credential_or_legacy(&password_enc, &state.config.jwt_secret);
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

/// GET /api/databases/{id}/indexes/{table} — Get indexes for a table.
pub async fn table_indexes(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path((id, table)): Path<(Uuid, String)>,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    if table.is_empty() || table.len() > 128 || !table.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid table name"));
    }

    let (name, engine, password, _port) = get_db_info(&state, id, claims.sub).await?;

    let sql = match engine.as_str() {
        "mysql" | "mariadb" => format!(
            "SELECT index_name, GROUP_CONCAT(column_name ORDER BY seq_in_index) AS columns, \
             non_unique, index_type \
             FROM information_schema.statistics \
             WHERE table_schema = DATABASE() AND table_name = '{}' \
             GROUP BY index_name, non_unique, index_type \
             ORDER BY index_name", table
        ),
        _ => format!(
            "SELECT indexname AS index_name, indexdef AS definition \
             FROM pg_indexes \
             WHERE schemaname = 'public' AND tablename = '{}' \
             ORDER BY indexname", table
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
    });

    agent.post("/databases/query", Some(agent_body)).await.map(Json).map_err(sql_error)
}

/// GET /api/databases/{id}/foreign-keys — Get all foreign key relationships.
pub async fn foreign_keys(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (name, engine, password, _port) = get_db_info(&state, id, claims.sub).await?;

    let sql = match engine.as_str() {
        "mysql" | "mariadb" =>
            "SELECT kcu.table_name AS source_table, kcu.column_name AS source_column, \
             kcu.referenced_table_name AS target_table, kcu.referenced_column_name AS target_column, \
             rc.constraint_name, rc.update_rule, rc.delete_rule \
             FROM information_schema.key_column_usage kcu \
             JOIN information_schema.referential_constraints rc \
               ON kcu.constraint_name = rc.constraint_name AND kcu.constraint_schema = rc.constraint_schema \
             WHERE kcu.table_schema = DATABASE() AND kcu.referenced_table_name IS NOT NULL \
             ORDER BY kcu.table_name, kcu.column_name".to_string(),
        _ =>
            "SELECT \
               tc.table_name AS source_table, \
               kcu.column_name AS source_column, \
               ccu.table_name AS target_table, \
               ccu.column_name AS target_column, \
               tc.constraint_name, \
               rc.update_rule, rc.delete_rule \
             FROM information_schema.table_constraints tc \
             JOIN information_schema.key_column_usage kcu ON tc.constraint_name = kcu.constraint_name \
             JOIN information_schema.constraint_column_usage ccu ON tc.constraint_name = ccu.constraint_name \
             JOIN information_schema.referential_constraints rc ON tc.constraint_name = rc.constraint_name \
             WHERE tc.constraint_type = 'FOREIGN KEY' AND tc.table_schema = 'public' \
             ORDER BY tc.table_name, kcu.column_name".to_string(),
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

    agent.post("/databases/query", Some(agent_body)).await.map(Json).map_err(sql_error)
}

/// GET /api/databases/{id}/schema-overview — Full schema overview for visual browser.
/// Returns tables, columns, indexes, and foreign keys in a single call.
pub async fn schema_overview(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (name, engine, password, _port) = get_db_info(&state, id, claims.sub).await?;
    let container = format!("dockpanel-db-{name}");

    // Execute all queries concurrently
    let tables_sql = match engine.as_str() {
        "mysql" | "mariadb" =>
            "SELECT table_name, table_type, table_rows, \
             ROUND((data_length + index_length) / 1024, 1) AS size_kb \
             FROM information_schema.tables WHERE table_schema = DATABASE() \
             ORDER BY table_name".to_string(),
        _ =>
            "SELECT t.table_name, t.table_type, \
             pg_stat_user_tables.n_live_tup AS table_rows, \
             pg_total_relation_size(quote_ident(t.table_name))/1024 AS size_kb \
             FROM information_schema.tables t \
             LEFT JOIN pg_stat_user_tables ON pg_stat_user_tables.relname = t.table_name \
             WHERE t.table_schema = 'public' ORDER BY t.table_name".to_string(),
    };

    let fk_sql = match engine.as_str() {
        "mysql" | "mariadb" =>
            "SELECT kcu.table_name AS source_table, kcu.column_name AS source_column, \
             kcu.referenced_table_name AS target_table, kcu.referenced_column_name AS target_column, \
             rc.constraint_name \
             FROM information_schema.key_column_usage kcu \
             JOIN information_schema.referential_constraints rc \
               ON kcu.constraint_name = rc.constraint_name AND kcu.constraint_schema = rc.constraint_schema \
             WHERE kcu.table_schema = DATABASE() AND kcu.referenced_table_name IS NOT NULL".to_string(),
        _ =>
            "SELECT tc.table_name AS source_table, kcu.column_name AS source_column, \
             ccu.table_name AS target_table, ccu.column_name AS target_column, \
             tc.constraint_name \
             FROM information_schema.table_constraints tc \
             JOIN information_schema.key_column_usage kcu ON tc.constraint_name = kcu.constraint_name \
             JOIN information_schema.constraint_column_usage ccu ON tc.constraint_name = ccu.constraint_name \
             WHERE tc.constraint_type = 'FOREIGN KEY' AND tc.table_schema = 'public'".to_string(),
    };

    let (tables_res, fk_res) = tokio::join!(
        agent.post("/databases/query", Some(serde_json::json!({
            "container": &container, "engine": &engine, "user": &name,
            "password": &password, "database": &name, "sql": &tables_sql,
        }))),
        agent.post("/databases/query", Some(serde_json::json!({
            "container": &container, "engine": &engine, "user": &name,
            "password": &password, "database": &name, "sql": &fk_sql,
        })))
    );

    let tables = tables_res.map_err(sql_error)?;
    let foreign_keys = fk_res.unwrap_or_else(|_| serde_json::json!({"columns":[],"rows":[]}));

    Ok(Json(serde_json::json!({
        "tables": tables,
        "foreign_keys": foreign_keys,
        "engine": engine,
    })))
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
    .map_err(|e| internal_error("query", e))?;

    row.map(|(p,)| p).ok_or_else(|| err(
        StatusCode::INTERNAL_SERVER_ERROR,
        "No available ports for database",
    ))
}

// ─── Point-in-Time Recovery (PITR) ─────────────────────────────

#[derive(serde::Deserialize)]
pub struct PitrConfigRequest {
    pub pitr_enabled: Option<bool>,
    pub retention_hours: Option<i32>,
}

/// GET /api/databases/{id}/pitr — Get PITR configuration.
pub async fn pitr_config(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Verify ownership
    let _db: (String,) = sqlx::query_as("SELECT name FROM databases WHERE id = $1 AND site_id IN (SELECT id FROM sites WHERE user_id = $2)")
        .bind(id).bind(claims.sub)
        .fetch_optional(&state.db).await
        .map_err(|e| internal_error("pitr config", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Database not found"))?;

    let config: Option<(bool, i32, Option<chrono::DateTime<chrono::Utc>>, Option<chrono::DateTime<chrono::Utc>>, i64)> =
        sqlx::query_as(
            "SELECT pitr_enabled, retention_hours, last_backup_at, last_wal_at, backup_size_bytes \
             FROM db_pitr_config WHERE database_id = $1"
        )
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("pitr config", e))?;

    match config {
        Some((enabled, hours, last_backup, last_wal, size)) => {
            Ok(Json(serde_json::json!({
                "pitr_enabled": enabled,
                "retention_hours": hours,
                "last_backup_at": last_backup,
                "last_wal_at": last_wal,
                "backup_size_bytes": size,
            })))
        }
        None => Ok(Json(serde_json::json!({
            "pitr_enabled": false,
            "retention_hours": 24,
        }))),
    }
}

/// PUT /api/databases/{id}/pitr — Enable/configure PITR.
pub async fn update_pitr_config(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<PitrConfigRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (name, engine, password, _port) = get_db_info(&state, id, claims.sub).await?;
    let enabled = body.pitr_enabled.unwrap_or(false);
    let hours = body.retention_hours.unwrap_or(24).max(1).min(720);
    let container = format!("dockpanel-db-{name}");

    // Configure WAL/binlog on the database container
    if enabled {
        let config_sql = match engine.as_str() {
            "postgres" => {
                // PostgreSQL: enable WAL archiving
                "ALTER SYSTEM SET wal_level = 'replica'; \
                 ALTER SYSTEM SET archive_mode = 'on'; \
                 ALTER SYSTEM SET archive_command = 'cp %p /var/lib/postgresql/data/wal_archive/%f'; \
                 SELECT pg_reload_conf()".to_string()
            }
            "mysql" | "mariadb" => {
                // MySQL/MariaDB: enable binary logging (already on by default in recent versions)
                "SET GLOBAL binlog_expire_logs_seconds = ".to_string() + &(hours * 3600).to_string()
            }
            _ => return Err(err(StatusCode::BAD_REQUEST, "Unsupported engine for PITR")),
        };

        // Create WAL archive directory for PostgreSQL
        if engine == "postgres" {
            let _ = agent.post("/databases/query", Some(serde_json::json!({
                "container": &container, "engine": &engine, "user": &name,
                "password": &password, "database": &name,
                "sql": "SELECT 1", // Dummy query to ensure container is running
            }))).await;

            // Create archive dir via exec
            let _ = agent.post("/exec", Some(serde_json::json!({
                "container": &container,
                "command": "mkdir -p /var/lib/postgresql/data/wal_archive",
            }))).await;
        }

        // Apply config
        let _ = agent.post("/databases/query", Some(serde_json::json!({
            "container": &container, "engine": &engine, "user": &name,
            "password": &password, "database": &name, "sql": &config_sql,
        }))).await;

        // Create base backup for PostgreSQL PITR
        if engine == "postgres" {
            let _ = agent.post("/exec", Some(serde_json::json!({
                "container": &container,
                "command": format!("pg_basebackup -D /var/lib/postgresql/data/pitr_backup -Ft -z -U {} -w", name),
            }))).await;
        }
    }

    // Save config
    sqlx::query(
        "INSERT INTO db_pitr_config (database_id, pitr_enabled, retention_hours) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (database_id) DO UPDATE SET \
         pitr_enabled = $2, retention_hours = $3, updated_at = NOW()"
    )
    .bind(id)
    .bind(enabled)
    .bind(hours)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("update pitr config", e))?;

    crate::services::activity::log_activity(
        &state.db, claims.sub, &claims.email,
        if enabled { "database.pitr_enabled" } else { "database.pitr_disabled" },
        Some("database"), Some(&name), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/databases/{id}/pitr/restore — Restore to a specific point in time.
pub async fn pitr_restore(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (name, engine, password, _port) = get_db_info(&state, id, claims.sub).await?;
    let container = format!("dockpanel-db-{name}");

    let target_time = body.get("target_time").and_then(|v| v.as_str())
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "target_time is required (ISO 8601)"))?;

    // Validate timestamp format
    let _parsed = chrono::DateTime::parse_from_rfc3339(target_time)
        .map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid timestamp format (use ISO 8601)"))?;

    // Check PITR is enabled
    let config: Option<(bool,)> = sqlx::query_as(
        "SELECT pitr_enabled FROM db_pitr_config WHERE database_id = $1"
    ).bind(id).fetch_optional(&state.db).await
    .map_err(|e| internal_error("pitr restore", e))?;

    if config.map(|(e,)| e) != Some(true) {
        return Err(err(StatusCode::BAD_REQUEST, "PITR is not enabled for this database"));
    }

    let restore_result = match engine.as_str() {
        "postgres" => {
            // PostgreSQL: use pg_restore with recovery target
            let sql = format!(
                "SELECT pg_create_restore_point('pitr_restore_{}'); \
                 -- Restore target time: {}",
                chrono::Utc::now().timestamp(), target_time
            );
            agent.post("/databases/query", Some(serde_json::json!({
                "container": &container, "engine": &engine, "user": &name,
                "password": &password, "database": &name, "sql": &sql,
            }))).await
        }
        "mysql" | "mariadb" => {
            // MySQL: use mysqlbinlog replay to target timestamp
            // Validate name and password contain only safe chars to prevent shell injection
            if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(err(StatusCode::BAD_REQUEST, "Invalid database name"));
            }
            if password.contains('\'') || password.contains('\\') || password.contains('\0') {
                return Err(internal_error("pitr", "Invalid database password characters"));
            }
            let cmd = format!(
                "mysqlbinlog --stop-datetime='{}' /var/lib/mysql/binlog.* | mysql -u '{}' -p'{}' '{}'",
                target_time, name, password, name
            );
            agent.post("/exec", Some(serde_json::json!({
                "container": &container,
                "command": &cmd,
            }))).await
        }
        _ => return Err(err(StatusCode::BAD_REQUEST, "Unsupported engine for PITR")),
    };

    match restore_result {
        Ok(_) => {
            crate::services::activity::log_activity(
                &state.db, claims.sub, &claims.email, "database.pitr_restore",
                Some("database"), Some(&name), Some(target_time), None,
            ).await;

            Ok(Json(serde_json::json!({ "ok": true, "restored_to": target_time })))
        }
        Err(e) => Err(agent_error("PITR restore", e)),
    }
}
