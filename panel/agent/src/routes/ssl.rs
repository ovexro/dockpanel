use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use super::{is_valid_domain, AppState};
use crate::routes::nginx::SiteConfig;
use crate::services::ssl;

#[derive(Deserialize)]
struct ProvisionRequest {
    email: String,
    runtime: String,
    root: Option<String>,
    proxy_port: Option<u16>,
    php_socket: Option<String>,
}

/// POST /ssl/provision/{domain} — Provision Let's Encrypt cert and enable SSL.
async fn provision(
    State(state): State<AppState>,
    Path(domain): Path<String>,
    Json(body): Json<ProvisionRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_domain(&domain) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid domain format" })),
        ));
    }

    // 1. Load or create ACME account
    let account = ssl::load_or_create_account(&body.email).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
    })?;

    // 2. Provision certificate via HTTP-01 challenge
    let cert_info = ssl::provision_cert(&account, &domain).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
    })?;

    // 3. Rewrite nginx config with SSL enabled
    let site_config = SiteConfig {
        runtime: body.runtime,
        root: body.root,
        proxy_port: body.proxy_port,
        php_socket: body.php_socket,
        ssl: None,
        ssl_cert: None,
        ssl_key: None,
        rate_limit: None,
        max_upload_mb: None,
        php_memory_mb: None,
        php_max_workers: None,
    };

    ssl::enable_ssl_for_site(&state.templates, &domain, &site_config)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
        })?;

    Ok(Json(serde_json::json!({
        "success": true,
        "domain": domain,
        "cert_path": cert_info.cert_path,
        "key_path": cert_info.key_path,
        "expiry": cert_info.expiry,
    })))
}

/// GET /ssl/status/{domain} — Get SSL certificate status.
async fn status(
    Path(domain): Path<String>,
) -> Result<Json<ssl::CertStatus>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_domain(&domain) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid domain format" })),
        ));
    }

    Ok(Json(ssl::get_cert_status(&domain).await))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/ssl/provision/{domain}", post(provision))
        .route("/ssl/status/{domain}", get(status))
}
