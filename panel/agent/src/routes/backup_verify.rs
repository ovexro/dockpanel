use axum::{
    http::StatusCode,
    routing::post,
    Json, Router,
};

use super::{is_valid_domain, AppState};
use crate::services::backup_verify;

type ApiErr = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: &str) -> ApiErr {
    (status, Json(serde_json::json!({ "error": msg })))
}

#[derive(serde::Deserialize)]
pub struct VerifySiteRequest {
    pub domain: String,
    pub filename: String,
}

/// POST /backups/verify/site — Verify a site backup.
async fn verify_site(
    Json(req): Json<VerifySiteRequest>,
) -> Result<Json<backup_verify::VerificationResult>, ApiErr> {
    if !is_valid_domain(&req.domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain"));
    }
    if req.filename.is_empty() || req.filename.contains("..") || req.filename.contains('/') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid filename"));
    }
    let result = backup_verify::verify_site_backup(&req.domain, &req.filename)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(result))
}

#[derive(serde::Deserialize)]
pub struct VerifyDbRequest {
    pub db_type: String,
    pub db_name: String,
    pub filename: String,
}

/// POST /backups/verify/database — Verify a database backup by restoring to temp container.
async fn verify_database(
    Json(req): Json<VerifyDbRequest>,
) -> Result<Json<backup_verify::VerificationResult>, ApiErr> {
    let result = backup_verify::verify_db_backup(&req.db_type, &req.db_name, &req.filename)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(result))
}

#[derive(serde::Deserialize)]
pub struct VerifyVolumeRequest {
    pub container_name: String,
    pub filename: String,
}

/// POST /backups/verify/volume — Verify a volume backup.
async fn verify_volume(
    Json(req): Json<VerifyVolumeRequest>,
) -> Result<Json<backup_verify::VerificationResult>, ApiErr> {
    let result = backup_verify::verify_volume_backup(&req.container_name, &req.filename)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(result))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/backups/verify/site", post(verify_site))
        .route("/backups/verify/database", post(verify_database))
        .route("/backups/verify/volume", post(verify_volume))
}
