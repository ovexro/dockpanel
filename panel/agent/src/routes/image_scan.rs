use axum::{
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use super::AppState;
use crate::services::image_scanner;

type ApiErr = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: &str) -> ApiErr {
    (status, Json(serde_json::json!({ "error": msg })))
}

#[derive(Deserialize)]
struct ScanRequest {
    image: String,
}

/// GET /image-scan/status — Whether the scanner binary is installed.
async fn status() -> Json<serde_json::Value> {
    let installed = image_scanner::is_installed().await;
    Json(serde_json::json!({
        "installed": installed,
        "scanner": "grype"
    }))
}

/// POST /image-scan/install — Install grype.
async fn install() -> Result<Json<serde_json::Value>, ApiErr> {
    image_scanner::install_grype()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(serde_json::json!({
        "ok": true,
        "message": "grype installed and vulnerability database primed"
    })))
}

/// POST /image-scan/uninstall — Remove grype binary and cache.
async fn uninstall() -> Result<Json<serde_json::Value>, ApiErr> {
    image_scanner::uninstall_grype()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(serde_json::json!({ "ok": true, "message": "grype removed" })))
}

/// POST /image-scan/scan — Scan a single Docker image.
async fn scan(Json(body): Json<ScanRequest>) -> Result<Json<image_scanner::ImageScanResult>, ApiErr> {
    let result = image_scanner::scan_image(&body.image)
        .await
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;
    Ok(Json(result))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/image-scan/status", get(status))
        .route("/image-scan/install", post(install))
        .route("/image-scan/uninstall", post(uninstall))
        .route("/image-scan/scan", post(scan))
}
