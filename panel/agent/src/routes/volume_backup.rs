use axum::{
    extract::Path,
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};

use super::{is_valid_name, AppState};
use crate::services::{backups::BackupInfo, volume_backup};

type ApiErr = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: &str) -> ApiErr {
    (status, Json(serde_json::json!({ "error": msg })))
}

#[derive(serde::Deserialize)]
pub struct VolumeBackupRequest {
    pub volume_name: String,
    pub container_name: String,
}

/// POST /volume-backups/create — Backup a Docker volume.
async fn create(
    Json(req): Json<VolumeBackupRequest>,
) -> Result<Json<BackupInfo>, ApiErr> {
    if req.volume_name.is_empty() || req.container_name.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "volume_name and container_name required"));
    }
    // Validate names to prevent injection / path traversal
    if !is_valid_name(&req.volume_name) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid volume name"));
    }
    if !is_valid_name(&req.container_name) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container name"));
    }

    let info = volume_backup::backup_volume(&req.volume_name, &req.container_name)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(info))
}

/// GET /volume-backups/{container_name}/list — List volume backups.
async fn list(
    Path(container_name): Path<String>,
) -> Result<Json<Vec<BackupInfo>>, ApiErr> {
    if !is_valid_name(&container_name) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container name"));
    }
    let list = volume_backup::list_volume_backups(&container_name)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(list))
}

#[derive(serde::Deserialize)]
pub struct VolumeRestoreRequest {
    pub volume_name: String,
}

/// POST /volume-backups/{container_name}/restore/{filename} — Restore a volume backup.
async fn restore(
    Path((container_name, filename)): Path<(String, String)>,
    Json(req): Json<VolumeRestoreRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_name(&container_name) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container name"));
    }
    if !is_valid_name(&req.volume_name) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid volume name"));
    }
    volume_backup::restore_volume(&req.volume_name, &container_name, &filename)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(serde_json::json!({ "success": true })))
}

/// DELETE /volume-backups/{container_name}/{filename} — Delete a volume backup.
async fn remove(
    Path((container_name, filename)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_name(&container_name) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container name"));
    }
    volume_backup::delete_volume_backup(&container_name, &filename)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(serde_json::json!({ "success": true })))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/volume-backups/create", post(create))
        .route("/volume-backups/{container_name}/list", get(list))
        .route("/volume-backups/{container_name}/restore/{filename}", post(restore))
        .route("/volume-backups/{container_name}/{filename}", delete(remove))
}
