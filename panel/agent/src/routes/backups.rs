use axum::{
    extract::Path,
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};

use super::{is_valid_domain, AppState};
use crate::services::backups;

type ApiErr = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: &str) -> ApiErr {
    (status, Json(serde_json::json!({ "error": msg })))
}

/// POST /backups/{domain}/create — Create a backup.
async fn create(
    Path(domain): Path<String>,
) -> Result<Json<backups::BackupInfo>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }

    let info = backups::create_backup(&domain)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(info))
}

/// GET /backups/{domain}/list — List backups.
async fn list(
    Path(domain): Path<String>,
) -> Result<Json<Vec<backups::BackupInfo>>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }

    let list = backups::list_backups(&domain)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(list))
}

/// POST /backups/{domain}/restore/{filename} — Restore from backup.
async fn restore(
    Path((domain, filename)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }

    backups::restore_backup(&domain, &filename)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(serde_json::json!({ "success": true })))
}

/// GET /backups/{domain}/browse/{filename} — List files in a backup archive.
async fn browse(
    Path((domain, filename)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }

    let files = backups::list_backup_files(&domain, &filename)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(serde_json::json!({ "files": files, "count": files.len() })))
}

#[derive(serde::Deserialize)]
struct RestoreFileRequest {
    path: String,
}

/// POST /backups/{domain}/restore-file/{filename} — Restore a single file from backup.
async fn restore_file(
    Path((domain, filename)): Path<(String, String)>,
    Json(body): Json<RestoreFileRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }

    backups::restore_single_file(&domain, &filename, &body.path)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(serde_json::json!({ "success": true, "restored_path": body.path })))
}

/// DELETE /backups/{domain}/{filename} — Delete a backup.
async fn remove(
    Path((domain, filename)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }

    backups::delete_backup(&domain, &filename)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(serde_json::json!({ "success": true })))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/backups/{domain}/create", post(create))
        .route("/backups/{domain}/list", get(list))
        .route("/backups/{domain}/browse/{filename}", get(browse))
        .route("/backups/{domain}/restore/{filename}", post(restore))
        .route("/backups/{domain}/restore-file/{filename}", post(restore_file))
        .route("/backups/{domain}/{filename}", delete(remove))
}
