use axum::{
    extract::Path,
    http::StatusCode,
    routing::{delete, post},
    Json, Router,
};
use serde::Deserialize;

use super::{is_valid_container_id, is_valid_name, AppState};
use crate::services::database;

#[derive(Deserialize)]
struct CreateDbRequest {
    name: String,
    engine: String,
    password: String,
    port: Option<u16>,
}

/// Find an available port for a database container.
/// Uses both TCP bind check AND Docker container inspection to avoid races.
async fn find_free_port(engine: &str) -> Result<u16, String> {
    let base: u16 = if engine == "postgres" { 5433 } else { 3307 };

    // Collect ports already used by Docker containers
    let mut used_ports = std::collections::HashSet::new();
    if let Ok(docker) = bollard::Docker::connect_with_local_defaults() {
        use bollard::container::ListContainersOptions;
        let containers = docker
            .list_containers(Some(ListContainersOptions::<String> { all: true, ..Default::default() }))
            .await
            .unwrap_or_default();
        for c in &containers {
            if let Some(ports) = &c.ports {
                for p in ports {
                    if let Some(pub_port) = p.public_port {
                        used_ports.insert(pub_port);
                    }
                }
            }
        }
    }

    for port in base..(base + 100) {
        if used_ports.contains(&port) {
            continue;
        }
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return Ok(port);
        }
    }
    Err("No free port available for database".into())
}

/// POST /databases — Create a new database container.
async fn create(
    Json(body): Json<CreateDbRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_name(&body.name) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid database name" })),
        ));
    }

    if !["mysql", "mariadb", "postgres"].contains(&body.engine.as_str()) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Engine must be mysql, mariadb, or postgres" })),
        ));
    }

    if body.password.is_empty() || body.password.len() > 128
        || body.password.contains('\n') || body.password.contains('\r') || body.password.contains('\0') {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid password" })),
        ));
    }

    let port = match body.port {
        Some(p) => p,
        None => find_free_port(&body.engine).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
        })?,
    };

    let db = database::create_database(&body.name, &body.engine, &body.password, port)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
        })?;

    Ok(Json(serde_json::json!({
        "success": true,
        "container_id": db.container_id,
        "name": db.name,
        "port": db.port,
        "engine": db.engine,
    })))
}

/// DELETE /databases/{container_id} — Remove a database container.
async fn remove(
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container ID" })),
        ));
    }

    database::remove_database(&container_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
        })?;

    Ok(Json(serde_json::json!({ "success": true })))
}

/// GET /databases — List all managed database containers.
async fn list() -> Result<Json<Vec<database::DbContainer>>, (StatusCode, Json<serde_json::Value>)>
{
    let dbs = database::list_databases().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
    })?;

    Ok(Json(dbs))
}

#[derive(Deserialize)]
struct QueryDbRequest {
    container: String,
    engine: String,
    user: String,
    password: String,
    database: String,
    sql: String,
}

/// POST /databases/query — Execute a SQL query inside a database container.
async fn query_db(
    Json(body): Json<QueryDbRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if body.sql.len() > 10_000 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Query too long (max 10KB)" })),
        ));
    }
    if body.sql.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Query is empty" })),
        ));
    }
    if !body.container.starts_with("dockpanel-db-") {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container name" })),
        ));
    }
    let suffix = &body.container["dockpanel-db-".len()..];
    if !is_valid_name(suffix) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container name" })),
        ));
    }

    // Validate engine
    if !["mysql", "mariadb", "postgres"].contains(&body.engine.as_str()) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Engine must be mysql, mariadb, or postgres" })),
        ));
    }
    // Validate user and database names
    if !is_valid_name(&body.user) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid username" })),
        ));
    }
    if !is_valid_name(&body.database) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid database name" })),
        ));
    }
    // Validate password
    if body.password.is_empty() || body.password.len() > 128
        || body.password.contains('\n') || body.password.contains('\r') || body.password.contains('\0') {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid password" })),
        ));
    }

    let result = database::execute_query(
        &body.container,
        &body.engine,
        &body.user,
        &body.password,
        &body.database,
        &body.sql,
    )
    .await
    .map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e })),
        )
    })?;

    Ok(Json(serde_json::json!({
        "columns": result.columns,
        "rows": result.rows,
        "row_count": result.row_count,
        "execution_time_ms": result.execution_time_ms,
        "truncated": result.truncated,
    })))
}

#[derive(Deserialize)]
struct ResetPasswordRequest {
    container: String,
    engine: String,
    user: String,
    old_password: String,
    new_password: String,
}

/// POST /databases/reset-password — Reset a database user's password.
async fn reset_password(
    Json(body): Json<ResetPasswordRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // Validate container name
    if !body.container.starts_with("dockpanel-db-") {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container name" })),
        ));
    }
    let suffix = &body.container["dockpanel-db-".len()..];
    if !is_valid_name(suffix) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container name" })),
        ));
    }

    if !["mysql", "mariadb", "postgres"].contains(&body.engine.as_str()) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Engine must be mysql, mariadb, or postgres" })),
        ));
    }

    if !is_valid_name(&body.user) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid username" })),
        ));
    }

    // Validate passwords
    for (label, pw) in [("old_password", &body.old_password), ("new_password", &body.new_password)] {
        if pw.is_empty() || pw.len() > 128
            || pw.contains('\n') || pw.contains('\r') || pw.contains('\0')
        {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("Invalid {label}") })),
            ));
        }
    }

    database::reset_password(
        &body.container,
        &body.engine,
        &body.user,
        &body.old_password,
        &body.new_password,
    )
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
    })?;

    Ok(Json(serde_json::json!({ "success": true })))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/databases", post(create).get(list))
        .route("/databases/query", post(query_db))
        .route("/databases/reset-password", post(reset_password))
        .route("/databases/{container_id}", delete(remove))
}
