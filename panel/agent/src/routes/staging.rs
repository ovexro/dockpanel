use axum::{
    extract::Json,
    http::StatusCode,
    routing::post,
    Router,
};
use serde::Deserialize;

use super::{is_valid_domain, AppState};
use crate::services::staging;

type ApiErr = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: &str) -> ApiErr {
    (status, Json(serde_json::json!({ "error": msg })))
}

#[derive(Deserialize)]
struct CloneRequest {
    source: String,
    target: String,
}

#[derive(Deserialize)]
struct SyncRequest {
    source: String,
    target: String,
}

#[derive(Deserialize)]
struct DeleteFilesRequest {
    domain: String,
}

#[derive(Deserialize)]
struct DiskUsageRequest {
    domain: String,
}

/// POST /staging/clone — Clone site files from source to target.
async fn clone_files(Json(body): Json<CloneRequest>) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_domain(&body.source) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid source domain"));
    }
    if !is_valid_domain(&body.target) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid target domain"));
    }

    let msg = staging::clone_files(&body.source, &body.target)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    Ok(Json(serde_json::json!({ "ok": true, "message": msg })))
}

/// POST /staging/sync — Sync files between two site directories.
async fn sync_files(Json(body): Json<SyncRequest>) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_domain(&body.source) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid source domain"));
    }
    if !is_valid_domain(&body.target) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid target domain"));
    }

    let msg = staging::sync_files(&body.source, &body.target)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    Ok(Json(serde_json::json!({ "ok": true, "message": msg })))
}

/// POST /staging/delete-files — Delete a site's file directory.
async fn delete_files(Json(body): Json<DeleteFilesRequest>) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_domain(&body.domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain"));
    }

    staging::delete_site_files(&body.domain)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /staging/disk-usage — Get disk usage for a site directory.
async fn disk_usage(Json(body): Json<DiskUsageRequest>) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_domain(&body.domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain"));
    }

    let bytes = staging::site_disk_usage(&body.domain)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    Ok(Json(serde_json::json!({ "bytes": bytes })))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/staging/clone", post(clone_files))
        .route("/staging/sync", post(sync_files))
        .route("/staging/delete-files", post(delete_files))
        .route("/staging/disk-usage", post(disk_usage))
}
