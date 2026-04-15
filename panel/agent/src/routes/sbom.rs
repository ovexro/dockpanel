use axum::{
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use super::AppState;
use crate::services::sbom_scanner;

type ApiErr = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: &str) -> ApiErr {
    (status, Json(serde_json::json!({ "error": msg })))
}

#[derive(Deserialize)]
struct SbomRequest {
    image: String,
}

/// GET /sbom/status — Whether the syft binary is installed.
async fn status() -> Json<serde_json::Value> {
    let installed = sbom_scanner::is_installed().await;
    Json(serde_json::json!({
        "installed": installed,
        "scanner": "syft"
    }))
}

/// POST /sbom/install — Install syft.
async fn install() -> Result<Json<serde_json::Value>, ApiErr> {
    sbom_scanner::install_syft()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(serde_json::json!({
        "ok": true,
        "message": "syft installed"
    })))
}

/// POST /sbom/uninstall — Remove syft binary.
async fn uninstall() -> Result<Json<serde_json::Value>, ApiErr> {
    sbom_scanner::uninstall_syft()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(serde_json::json!({ "ok": true, "message": "syft removed" })))
}

/// POST /sbom/scan — Generate an SPDX SBOM for an image. Returns the raw
/// SPDX JSON document inside { "image": ..., "spdx": <object> } so the
/// backend can persist it without re-parsing.
async fn scan(Json(body): Json<SbomRequest>) -> Result<Json<serde_json::Value>, ApiErr> {
    let spdx = sbom_scanner::generate_sbom(&body.image)
        .await
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;
    let parsed: serde_json::Value = serde_json::from_str(&spdx)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("parse SPDX: {e}")))?;
    Ok(Json(serde_json::json!({
        "image": body.image,
        "spdx": parsed,
    })))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/sbom/status", get(status))
        .route("/sbom/install", post(install))
        .route("/sbom/uninstall", post(uninstall))
        .route("/sbom/scan", post(scan))
}
