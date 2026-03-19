use axum::{
    extract::State,
    http::StatusCode,
    routing::post,
    Json, Router,
};

use crate::routes::AppState;
use crate::services::migration;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/migration/analyze", post(analyze))
        .route("/migration/import-site", post(import_site))
        .route("/migration/import-database", post(import_database))
        .route("/migration/cleanup", post(cleanup))
}

/// POST /migration/analyze — Extract and analyze a backup file
async fn analyze(
    State(_state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let path = body["path"].as_str().ok_or((StatusCode::BAD_REQUEST, "path required".into()))?;
    let source = body["source"].as_str().unwrap_or("cpanel");

    // Validate path exists
    if !std::path::Path::new(path).exists() {
        return Err((StatusCode::BAD_REQUEST, format!("File not found: {path}")));
    }

    match migration::analyze(path, source).await {
        Ok(inventory) => Ok(Json(serde_json::to_value(inventory).unwrap_or_default())),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e)),
    }
}

/// POST /migration/import-site — Copy files from extracted backup to /var/www/{domain}/
async fn import_site(
    State(_state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let migration_id = body["migration_id"].as_str().ok_or((StatusCode::BAD_REQUEST, "migration_id required".into()))?;
    let domain = body["domain"].as_str().ok_or((StatusCode::BAD_REQUEST, "domain required".into()))?;
    let source_dir = body["source_dir"].as_str().ok_or((StatusCode::BAD_REQUEST, "source_dir required".into()))?;

    match migration::import_site_files(migration_id, domain, source_dir).await {
        Ok(msg) => Ok(Json(serde_json::json!({ "ok": true, "message": msg }))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e)),
    }
}

/// POST /migration/import-database — Import SQL dump into a database container
async fn import_database(
    State(_state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let migration_id = body["migration_id"].as_str().ok_or((StatusCode::BAD_REQUEST, "migration_id required".into()))?;
    let sql_file = body["sql_file"].as_str().ok_or((StatusCode::BAD_REQUEST, "sql_file required".into()))?;
    let container_name = body["container_name"].as_str().ok_or((StatusCode::BAD_REQUEST, "container_name required".into()))?;
    let db_name = body["db_name"].as_str().ok_or((StatusCode::BAD_REQUEST, "db_name required".into()))?;
    let engine = body["engine"].as_str().unwrap_or("mysql");
    let user = body["user"].as_str().unwrap_or("root");
    let password = body["password"].as_str().ok_or((StatusCode::BAD_REQUEST, "password required".into()))?;

    match migration::import_database(migration_id, sql_file, container_name, db_name, engine, user, password).await {
        Ok(msg) => Ok(Json(serde_json::json!({ "ok": true, "message": msg }))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e)),
    }
}

/// POST /migration/cleanup — Remove temp extraction directory
async fn cleanup(
    State(_state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let migration_id = body["migration_id"].as_str().ok_or((StatusCode::BAD_REQUEST, "migration_id required".into()))?;

    match migration::cleanup(migration_id).await {
        Ok(()) => Ok(Json(serde_json::json!({ "ok": true }))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e)),
    }
}
