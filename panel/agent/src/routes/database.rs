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

/// Find an available port for a database container (scans 3307-3399).
fn find_free_port(engine: &str) -> Result<u16, String> {
    let base = if engine == "postgres" { 5433 } else { 3307 };
    for port in base..(base + 100) {
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

    let port = match body.port {
        Some(p) => p,
        None => find_free_port(&body.engine).map_err(|e| {
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

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/databases", post(create).get(list))
        .route("/databases/{container_id}", delete(remove))
}
