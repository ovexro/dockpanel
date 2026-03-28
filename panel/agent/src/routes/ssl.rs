use crate::safe_cmd::safe_command;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
use serde::Deserialize;
use std::time::Duration;

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
        custom_nginx: None,
        php_preset: None,
        app_command: None,
        fastcgi_cache: None,
        redis_cache: None,
        redis_db: None,
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

// ──────────────────────────────────────────────────────────────
// Custom SSL Certificate Upload
// ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct CustomCertRequest {
    domain: String,
    certificate: String,
    private_key: String,
}

/// POST /ssl/upload — Upload a custom SSL certificate.
async fn upload_cert(
    State(state): State<AppState>,
    Json(body): Json<CustomCertRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_domain(&body.domain) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid domain format" })),
        ));
    }
    if body.domain.is_empty() || body.certificate.is_empty() || body.private_key.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Domain, certificate, and private key required" })),
        ));
    }

    // Validate cert format
    if !body.certificate.contains("BEGIN CERTIFICATE") || !body.private_key.contains("BEGIN") {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid PEM format" })),
        ));
    }

    let ssl_dir = format!("/etc/dockpanel/ssl/{}", body.domain);
    tokio::fs::create_dir_all(&ssl_dir).await
        .map_err(|e| (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to create SSL dir: {e}") })),
        ))?;

    let cert_path = format!("{ssl_dir}/fullchain.pem");
    let key_path = format!("{ssl_dir}/privkey.pem");

    tokio::fs::write(&cert_path, &body.certificate).await
        .map_err(|e| (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to write cert: {e}") })),
        ))?;
    tokio::fs::write(&key_path, &body.private_key).await
        .map_err(|e| (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to write key: {e}") })),
        ))?;

    // Set permissions
    let _ = safe_command("chmod").args(["600", &key_path]).output().await;

    // Enable SSL in nginx — read existing config to determine runtime
    let site_conf = format!("/etc/nginx/sites-enabled/{}.conf", body.domain);
    let content = tokio::fs::read_to_string(&site_conf).await.unwrap_or_default();
    let is_proxy = content.contains("proxy_pass");

    let site_config = SiteConfig {
        runtime: if is_proxy { "proxy".to_string() } else { "php".to_string() },
        root: Some("/var/www".to_string()),
        proxy_port: if is_proxy {
            content.lines().find(|l| l.contains("proxy_pass"))
                .and_then(|l| l.split(':').last())
                .and_then(|s| s.trim_end_matches(';').trim().parse().ok())
        } else { None },
        php_socket: None,
        ssl: None, ssl_cert: None, ssl_key: None,
        rate_limit: None, max_upload_mb: None,
        php_memory_mb: None, php_max_workers: None,
        custom_nginx: None, php_preset: None, app_command: None,
        fastcgi_cache: None,
        redis_cache: None,
        redis_db: None,
    };

    ssl::enable_ssl_for_site(&state.templates, &body.domain, &site_config)
        .await
        .map_err(|e| (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to enable SSL: {e}") })),
        ))?;

    tracing::info!("Custom SSL certificate uploaded for {}", body.domain);
    Ok(Json(serde_json::json!({ "ok": true, "cert_path": cert_path, "key_path": key_path })))
}

/// POST /ssl/{domain}/renew — Force-renew a Let's Encrypt certificate.
async fn renew(
    Path(domain): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_domain(&domain) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid domain format" })),
        ));
    }

    tracing::info!("Force-renewing SSL certificate for {domain}");

    let output = tokio::time::timeout(
        Duration::from_secs(120),
        safe_command("certbot")
            .args(["renew", "--cert-name", &domain, "--force-renewal"])
            .output(),
    )
    .await
    .map_err(|_| {
        (
            StatusCode::GATEWAY_TIMEOUT,
            Json(serde_json::json!({ "error": "Certificate renewal timed out after 120s" })),
        )
    })?
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to run certbot: {e}") })),
        )
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::error!("certbot renew failed for {domain}: {stderr}");
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("certbot renew failed: {stderr}") })),
        ));
    }

    tracing::info!("SSL certificate renewed for {domain}");
    Ok(Json(serde_json::json!({ "ok": true, "domain": domain })))
}

/// DELETE /ssl/{domain} — Revoke and delete a Let's Encrypt certificate.
async fn revoke(
    Path(domain): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_domain(&domain) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid domain format" })),
        ));
    }

    tracing::info!("Deleting SSL certificate for {domain}");

    let output = tokio::time::timeout(
        Duration::from_secs(60),
        safe_command("certbot")
            .args(["delete", "--cert-name", &domain])
            .output(),
    )
    .await
    .map_err(|_| {
        (
            StatusCode::GATEWAY_TIMEOUT,
            Json(serde_json::json!({ "error": "Certificate deletion timed out after 60s" })),
        )
    })?
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to run certbot: {e}") })),
        )
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::error!("certbot delete failed for {domain}: {stderr}");
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("certbot delete failed: {stderr}") })),
        ));
    }

    // Also clean up custom cert directory if it exists
    let ssl_dir = format!("/etc/dockpanel/ssl/{domain}");
    let _ = tokio::fs::remove_dir_all(&ssl_dir).await;

    tracing::info!("SSL certificate deleted for {domain}");
    Ok(Json(serde_json::json!({ "ok": true, "domain": domain })))
}

// ── DNS-01 wildcard SSL ─────────────────────────────────────────────

#[derive(Deserialize)]
struct Dns01ProvisionRequest {
    email: String,
    cf_zone_id: String,
    cf_api_token: String,
    cf_api_email: Option<String>,
    wildcard: bool,
}

/// POST /ssl/provision-dns01/{domain} — Provision cert via DNS-01 (Cloudflare).
async fn provision_dns01(
    State(state): State<AppState>,
    Path(domain): Path<String>,
    Json(body): Json<Dns01ProvisionRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_domain(&domain) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid domain format" })),
        ));
    }

    let account = ssl::load_or_create_account(&body.email).await.map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e })))
    })?;

    let cert_info = ssl::provision_cert_dns01(
        &account,
        &domain,
        &body.cf_zone_id,
        &body.cf_api_token,
        body.cf_api_email.as_deref(),
        body.wildcard,
    )
    .await
    .map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e })))
    })?;

    // If NOT wildcard, enable SSL in nginx for this domain
    // (wildcard certs are applied per-site by the backend)
    if !body.wildcard {
        let site_conf = format!("/etc/nginx/sites-enabled/{domain}.conf");
        if std::path::Path::new(&site_conf).exists() {
            let content = tokio::fs::read_to_string(&site_conf).await.unwrap_or_default();
            let is_proxy = content.contains("proxy_pass");

            let site_config = SiteConfig {
                runtime: if is_proxy { "proxy".to_string() } else { "php".to_string() },
                root: Some("/var/www".to_string()),
                proxy_port: if is_proxy {
                    content.lines().find(|l| l.contains("proxy_pass"))
                        .and_then(|l| l.split(':').last())
                        .and_then(|s| s.trim_end_matches(';').trim().parse().ok())
                } else { None },
                php_socket: None,
                ssl: None, ssl_cert: None, ssl_key: None,
                rate_limit: None, max_upload_mb: None,
                php_memory_mb: None, php_max_workers: None,
                custom_nginx: None, php_preset: None, app_command: None,
                fastcgi_cache: None, redis_cache: None, redis_db: None,
            };

            ssl::enable_ssl_for_site(&state.templates, &domain, &site_config)
                .await
                .map_err(|e| {
                    (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e })))
                })?;
        }
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "domain": domain,
        "wildcard": body.wildcard,
        "cert_path": cert_info.cert_path,
        "key_path": cert_info.key_path,
        "expiry": cert_info.expiry,
    })))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/ssl/provision/{domain}", post(provision))
        .route("/ssl/provision-dns01/{domain}", post(provision_dns01))
        .route("/ssl/status/{domain}", get(status))
        .route("/ssl/upload", post(upload_cert))
        .route("/ssl/{domain}/renew", post(renew))
        .route("/ssl/{domain}", delete(revoke))
}
