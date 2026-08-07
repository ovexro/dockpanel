use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::{AuthUser, ServerScope};
use crate::error::{internal_error, err, agent_error, paginate, ApiError};
use crate::services::agent::AgentError;
use crate::AppState;

/// Hard per-account cap on total databases, independent of any reseller quota.
/// Bounds how much of the shared, host-wide DB port pool + host RAM a single
/// tenant can consume so one account cannot deny database creation to everyone.
const MAX_DATABASES_PER_USER: i64 = 25;

/// Database names that collide with a system/admin identity. Since v2.20.0 the tenant
/// `{name}` becomes a real postgres role that OWNS database `{name}` (M1: the tenant is
/// no longer the cluster superuser), so a name matching the cluster's own superuser /
/// template DBs (postgres) or system schemas (mariadb) must be rejected. Case-insensitive.
fn is_reserved_db_name(name: &str) -> bool {
    const RESERVED: &[&str] = &[
        "postgres",
        "template0",
        "template1",
        "mysql",
        "sys",
        "information_schema",
        "performance_schema",
    ];
    let lower = name.to_ascii_lowercase();
    RESERVED.contains(&lower.as_str())
}

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
            // Strip the Docker-daemon infra prefix so only the DB engine's SQL
            // diagnostic is surfaced (don't leak container/daemon internals).
            let msg = msg
                .strip_prefix("Error response from daemon: ")
                .unwrap_or(&msg)
                .to_string();
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
    Json(body): Json<CreateDbRequest>,
) -> Result<(StatusCode, Json<Database>), ApiError> {
    // Verify site ownership — and take the host from the same row while we are
    // here. The container is created on whichever host the handle points at and
    // then recorded against this site, so a misdispatch leaves a `databases` row
    // naming a container that does not exist on the site's own machine, while the
    // wrong machine keeps the container and the port. The row that decides WHICH
    // SITE has to decide WHICH HOST too.
    let site: Option<(Option<Uuid>, String)> = sqlx::query_as(&format!(
        "SELECT s.server_id, s.domain FROM sites s WHERE {}",
        crate::helpers::SITE_CALLER_PREDICATE
    ))
    .bind(body.site_id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("create databases", e))?;

    let (site_server_id, site_domain) =
        site.ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    let agent =
        crate::helpers::agent_for_site_server(&state, site_server_id, &site_domain).await?;

    // Hard per-account cap on total databases (independent of reseller quota) so a
    // single tenant cannot exhaust the shared, host-wide DB port pool / host RAM.
    let (db_count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM databases d JOIN sites s ON d.site_id = s.id WHERE s.user_id = $1",
    )
    .bind(claims.sub)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("create databases", e))?;
    if db_count >= MAX_DATABASES_PER_USER {
        return Err(err(
            StatusCode::FORBIDDEN,
            &format!("Database limit reached ({MAX_DATABASES_PER_USER} per account)"),
        ));
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

    if is_reserved_db_name(&body.name) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Database name is reserved (system/admin name)",
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

    // Generate password and find available port
    let password = uuid::Uuid::new_v4().to_string().replace('-', "");
    let port = find_available_port(&state, engine).await?;

    // Encrypt the password before storing in the database
    let encrypted_password = crate::services::secrets_crypto::encrypt_credential(&password, &state.config.jwt_secret)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Encryption failed: {e}")))?;

    // Atomically reserve one slot against the caller's reseller quota (if any). The old
    // check-then-increment was TOCTOU-racy — concurrent creates all read the same stale
    // `used_databases` and each incremented past `max`. A conditional UPDATE ... RETURNING
    // makes the check and the increment one statement. Released on any failure below.
    let reserved_quota = reserve_reseller_db_slot(&state, claims.sub).await?;

    // Insert DB record first to atomically claim the port (unique index prevents races).
    // container_id is empty until the agent creates it.
    let db_record: Database = match sqlx::query_as(
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
    {
        Ok(r) => r,
        Err(e) => {
            if reserved_quota { release_reseller_db_slot(&state, claims.sub).await; }
            if e.to_string().contains("unique") || e.to_string().contains("duplicate") {
                return Err(err(StatusCode::CONFLICT, "Port or database name conflict, please retry"));
            }
            return Err(internal_error("create databases", e));
        }
    };

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
            // Clean up the DB record + release the reserved quota slot if the agent fails.
            let _ = sqlx::query("DELETE FROM databases WHERE id = $1")
                .bind(db_record.id)
                .execute(&state.db)
                .await;
            if reserved_quota { release_reseller_db_slot(&state, claims.sub).await; }
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

    // (Reseller counter already incremented atomically by reserve_reseller_db_slot above.)

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
) -> Result<Json<serde_json::Value>, ApiError> {
    // Verify ownership through site — and take the host from the same row while we
    // are here. The join to `sites` was already carrying the ownership term; the
    // host it also knows about was thrown away, and the agent came from the request
    // header instead. That is a destructive mismatch in both directions: the panel
    // row is deleted whether or not the container was, so dispatching to the wrong
    // machine either destroys a same-named `dockpanel-db-{name}` container that
    // belongs to somebody else's tenant, or fails to find one while the real
    // container is orphaned on its own host with its row already gone.
    let db: Option<(Uuid, String, Option<String>, Option<Uuid>, String)> = sqlx::query_as(
        "SELECT d.id, d.name, d.container_id, s.server_id, s.domain FROM databases d \
         JOIN sites s ON d.site_id = s.id \
         WHERE d.id = $1 AND s.user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("remove databases", e))?;

    let (_, name, container_id, site_server_id, site_domain) =
        db.ok_or_else(|| err(StatusCode::NOT_FOUND, "Database not found"))?;

    let agent =
        crate::helpers::agent_for_site_server(&state, site_server_id, &site_domain).await?;

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

/// Helper: fetch database info (name, engine, password, port) — and the id of the server
/// the owning site actually runs on — with an ownership check.
///
/// The join to `sites` was here all along for the ownership term, and `sites` is where the
/// authoritative `server_id` lives; the column simply was not selected. So every caller
/// below answered WHICH DATABASE from this row and then took WHICH HOST from `ServerScope`
/// — an `X-Server-Id` request header that falls back to the LOCAL agent whenever the caller
/// owns no `servers` row. No non-admin ever owns one (the only INSERT is admin-gated), so
/// for a tenant whose database lives on a fleet member that fallback was always the wrong
/// machine, and nothing in the response said so.
///
/// What that leaked matters as much as where it dispatched: this helper returns the
/// DECRYPTED database password, and every caller forwards it to the agent it chose, which
/// spends it on a `docker exec -e MYSQL_PWD=…` child process. A misdispatch therefore
/// handed one tenant's live credential to a host their database does not run on, in the
/// argv/environment of a process on that host — a disclosure that happened on ordinary
/// read-only calls like listing tables, with no error for anyone to notice.
///
/// The row is the authority for the host exactly as it is for the ownership check, so the
/// server id now comes back in the same tuple and each caller resolves its handle from it
/// via `agent_for_site_server`, which REFUSES rather than substituting a reachable host.
/// That helper's third argument only labels its refusal log line; no domain is loaded on
/// these paths, so callers identify the target by database name.
///
/// `Option<Uuid>` because the column is `NOT NULL` in the schema but optional in the
/// structs that predate the fleet work — a `None` means a row older than the backfill, and
/// guessing a host for it is the thing that must not happen.
async fn get_db_info(
    state: &AppState,
    id: Uuid,
    user_id: Uuid,
) -> Result<(String, String, String, i32, Option<Uuid>), ApiError> {
    let row: Option<(String, String, String, Option<i32>, Option<String>, Option<Uuid>)> = sqlx::query_as(
        "SELECT d.name, d.engine, d.db_password_enc, d.port, d.container_id, s.server_id \
         FROM databases d JOIN sites s ON d.site_id = s.id \
         WHERE d.id = $1 AND s.user_id = $2",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("remove databases", e))?;

    let (name, engine, password_enc, port, container_id, site_server_id) =
        row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Database not found"))?;
    // Security (H1, v2.19.0): every exec/reset path resolves the target container by
    // the (per-site, NON-unique) name `dockpanel-db-{name}`. create() inserts the row
    // with an EMPTY container_id and only fills it after the agent confirms the
    // container was created; on a global Docker name collision the row is deleted, but
    // during the agent round-trip a tenant's transient colliding-name row is visible.
    // Docker's global name uniqueness guarantees a row with a NON-EMPTY container_id
    // owns its uniquely-named container, so refusing to operate on an empty container_id
    // closes the cross-tenant reset/query race for every get_db_info caller at once.
    if container_id.as_deref().unwrap_or("").is_empty() {
        return Err(err(StatusCode::NOT_FOUND, "Database not found"));
    }
    let port = port.unwrap_or(5432);
    // Decrypt password for agent use (with legacy plaintext fallback)
    let password = crate::services::secrets_crypto::decrypt_credential_or_legacy(&password_enc, &state.config.jwt_secret);
    Ok((name, engine, password, port, site_server_id))
}

/// GET /api/databases/{id}/tables — List tables in the database.
pub async fn tables(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (name, engine, password, _port, site_server_id) =
        get_db_info(&state, id, claims.sub).await?;
    let agent = crate::helpers::agent_for_site_server(&state, site_server_id, &name).await?;

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

    let (name, engine, password, _port, site_server_id) =
        get_db_info(&state, id, claims.sub).await?;
    let agent = crate::helpers::agent_for_site_server(&state, site_server_id, &name).await?;

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

    let (name, engine, password, _port, site_server_id) =
        get_db_info(&state, id, claims.sub).await?;
    // The host comes from the site's row, never from the caller's server switcher. This
    // request body carries the tenant's DECRYPTED database password to the agent, which
    // puts it in a `docker exec -e MYSQL_PWD=…` child process; sent to a machine the
    // database does not run on, the credential is disclosed to that host — and the query
    // either runs against a same-named container belonging to someone else or fails with a
    // "not found" that reads like an ordinary error.
    let agent = crate::helpers::agent_for_site_server(&state, site_server_id, &name).await?;

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
) -> Result<Json<serde_json::Value>, ApiError> {
    if table.is_empty() || table.len() > 128 || !table.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid table name"));
    }

    let (name, engine, password, _port, site_server_id) =
        get_db_info(&state, id, claims.sub).await?;
    let agent = crate::helpers::agent_for_site_server(&state, site_server_id, &name).await?;

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
) -> Result<Json<serde_json::Value>, ApiError> {
    let (name, engine, password, _port, site_server_id) =
        get_db_info(&state, id, claims.sub).await?;
    let agent = crate::helpers::agent_for_site_server(&state, site_server_id, &name).await?;

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
) -> Result<Json<serde_json::Value>, ApiError> {
    let (name, engine, password, _port, site_server_id) =
        get_db_info(&state, id, claims.sub).await?;
    // Both halves of the join below go through this one handle, so it has to be the
    // site's own host — otherwise the two concurrent queries carry the decrypted
    // password to a foreign machine twice over.
    let agent = crate::helpers::agent_for_site_server(&state, site_server_id, &name).await?;
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

/// Atomically reserve one database slot against the caller's reseller quota.
/// Returns Ok(true) if a slot was reserved (caller must release it on a later
/// failure), Ok(false) if the user has no reseller (nothing to enforce/release),
/// or Err(403) if the reseller's database quota is exhausted. The conditional
/// UPDATE ... RETURNING makes the quota check and the increment a single atomic
/// statement, closing the check-then-increment TOCTOU.
async fn reserve_reseller_db_slot(state: &AppState, user_id: Uuid) -> Result<bool, ApiError> {
    let reserved: Option<i32> = sqlx::query_scalar(
        "UPDATE reseller_profiles SET used_databases = used_databases + 1, updated_at = NOW() \
         WHERE user_id = (SELECT reseller_id FROM users WHERE id = $1 AND reseller_id IS NOT NULL) \
           AND (max_databases IS NULL OR used_databases < max_databases) \
         RETURNING 1",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("reserve reseller quota", e))?;

    if reserved.is_some() {
        return Ok(true); // slot reserved + counter incremented atomically
    }

    // 0 rows updated: either the user has no reseller/profile (nothing to enforce),
    // or the profile exists but the quota condition failed (quota exhausted).
    let has_profile: Option<(Uuid,)> = sqlx::query_as(
        "SELECT rp.user_id FROM reseller_profiles rp \
         WHERE rp.user_id = (SELECT reseller_id FROM users WHERE id = $1 AND reseller_id IS NOT NULL)",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("reserve reseller quota", e))?;

    if has_profile.is_some() {
        return Err(err(StatusCode::FORBIDDEN, "Reseller database quota exceeded"));
    }
    Ok(false) // no reseller → nothing reserved
}

/// Release a reserved reseller database slot (compensating decrement) when a
/// create fails after `reserve_reseller_db_slot` succeeded.
async fn release_reseller_db_slot(state: &AppState, user_id: Uuid) {
    let _ = sqlx::query(
        "UPDATE reseller_profiles SET used_databases = GREATEST(used_databases - 1, 0), updated_at = NOW() \
         WHERE user_id = (SELECT reseller_id FROM users WHERE id = $1 AND reseller_id IS NOT NULL)",
    )
    .bind(user_id)
    .execute(&state.db)
    .await;
}

/// Find an available port using a single SQL query to find the first gap.
async fn find_available_port(state: &AppState, engine: &str) -> Result<i32, ApiError> {
    // Choose port range based on engine. Ranges are intentionally wide so that the
    // per-account cap (MAX_DATABASES_PER_USER) leaves ample headroom and one tenant
    // cannot exhaust the shared, host-wide pool.
    let (range_start, range_end) = match engine {
        "mysql" | "mariadb" => (3307, 3600),
        _ => (5433, 5700),
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
    _scope: ServerScope,
    Json(body): Json<PitrConfigRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Ownership + container-readiness check (get_db_info also enforces the H1 guard).
    // No agent call on this path — the handler only persists the intent flag (see the
    // note below), so the server id this now returns has nothing to dispatch.
    let (name, _engine, _password, _port, _site_server_id) =
        get_db_info(&state, id, claims.sub).await?;
    let enabled = body.pitr_enabled.unwrap_or(false);
    let hours = body.retention_hours.unwrap_or(24).max(1).min(720);

    // NOTE (v2.19.0): the previous implementation ran `ALTER SYSTEM SET archive_mode='on'`
    // on enable but the disable path had no inverse — so archiving was un-revertable through
    // the panel and grew WAL without bound after a container restart (the archive_command
    // targeted a directory that the accompanying mkdir/pg_basebackup never created, because
    // those posted to a non-existent agent `/exec` route and were swallowed with `let _ =`).
    // The harmful/dead container mutation is removed; we persist only the intent flag until
    // real, symmetric WAL archiving is implemented (pitr_restore returns 501 until then).
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
    _scope: ServerScope,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Ownership + container-readiness check (get_db_info also enforces the H1 guard).
    let _ = get_db_info(&state, id, claims.sub).await?;

    // Honesty (v2.19.0): point-in-time restore was never functional and used to LIE.
    // The postgres branch only wrote a WAL restore-point marker (pg_create_restore_point
    // performs NO rollback) then returned {ok:true, restored_to:...}; the mysql branch +
    // the WAL-archive setup posted to a non-existent agent `/exec` route so nothing ran.
    // Return an explicit 501 instead of a false success until real PITR is implemented
    // (this also retires the dead mysqlbinlog shell string that only reached the 404 route).
    Err(err(
        StatusCode::NOT_IMPLEMENTED,
        "Point-in-time restore is not yet implemented",
    ))
}

/// POST /api/databases/{id}/reset-password — Reset the database password.
/// Generates a new random password, updates the database container via the agent,
/// then stores the encrypted password in the panel DB and returns it to the user.
pub async fn reset_password(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Fetch current db info with ownership check (returns decrypted old password)
    let (name, engine, old_password, _port, site_server_id) =
        get_db_info(&state, id, claims.sub).await?;

    // The host MUST come from the site's row here, and this is the handler where taking
    // it from the request header did the most damage. The agent's mysql/mariadb branch
    // resets the account with `docker exec <container> mariadb -u root --skip-password
    // -e "ALTER USER …"` — it never checks the `old_password` this body sends, so
    // possession of the container name is the whole authorization. Container names are
    // `dockpanel-db-{name}` and are unique only per host, so a request dispatched to the
    // wrong machine rewrites the password of ANY same-named container living there, for
    // a tenant who has nothing to do with the caller. `ServerScope` made that the DEFAULT
    // rather than the edge case: a non-admin owns no `servers` row, so their scope always
    // fell back to the local agent even when their database lives on a fleet member.
    // The panel row is then updated with the new password regardless, so the real
    // database is left with a credential the panel no longer knows.
    let agent = crate::helpers::agent_for_site_server(&state, site_server_id, &name).await?;

    // Generate a new random password (same pattern as create())
    let new_password = uuid::Uuid::new_v4().to_string().replace('-', "");

    // Reset password in the actual database container via the agent
    let container = format!("dockpanel-db-{name}");
    let agent_body = serde_json::json!({
        "container": container,
        "engine": engine,
        "user": name,
        "old_password": old_password,
        "new_password": new_password,
    });

    agent
        .post("/databases/reset-password", Some(agent_body))
        .await
        .map_err(|e| agent_error("Database password reset", e))?;

    // Encrypt the new password
    let encrypted = crate::services::secrets_crypto::encrypt_credential(&new_password, &state.config.jwt_secret)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Encryption failed: {e}")))?;

    // Update the encrypted password in the panel database
    sqlx::query("UPDATE databases SET db_password_enc = $1 WHERE id = $2")
        .bind(&encrypted)
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("reset password", e))?;

    // Log activity
    crate::services::activity::log_activity(
        &state.db, claims.sub, &claims.email, "database.password_reset",
        Some("database"), Some(&name), None, None,
    ).await;

    tracing::info!("Database password reset: {name}");

    Ok(Json(serde_json::json!({
        "ok": true,
        "password": new_password,
    })))
}

#[cfg(test)]
mod tests {
    use super::is_reserved_db_name;

    #[test]
    fn reserved_db_names_rejected_case_insensitive() {
        for n in ["postgres", "Postgres", "TEMPLATE1", "template0", "mysql", "information_schema", "performance_schema", "sys"] {
            assert!(is_reserved_db_name(n), "{n} should be reserved");
        }
        for n in ["blog", "app_db", "wordpress", "shop", "my_data"] {
            assert!(!is_reserved_db_name(n), "{n} should be allowed");
        }
    }
}
