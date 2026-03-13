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
    port: u16,
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

    let db = database::create_database(&body.name, &body.engine, &body.password, body.port)
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
