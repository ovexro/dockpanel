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
    /// Optional ACME profile ("classic" / "tlsserver" / "shortlived").
    /// Omit to let the CA pick its default.
    #[serde(default)]
    profile: Option<String>,
    /// Set on renewal: PEM of the existing cert being replaced. Enables the
    /// RFC 9773 `replaces` hint so the CA can correlate issuance history.
    #[serde(default)]
    replaces_pem: Option<String>,
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
    let opts = ssl::ProvisionOpts {
        profile: body.profile.as_deref(),
        replaces_pem: body.replaces_pem.as_deref(),
    };
    let cert_info = ssl::provision_cert(&account, &domain, Some(&opts))
        .await
        .map_err(|e| {
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
        waf_enabled: None,
        waf_mode: None,
        csp_policy: None,
        permissions_policy: None,
        bot_protection: None,
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
        "profile": cert_info.profile,
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
        waf_enabled: None,
        waf_mode: None,
        csp_policy: None,
        permissions_policy: None,
        bot_protection: None,
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

#[derive(Deserialize)]
struct RenewRequest {
    email: String,
    runtime: String,
    root: Option<String>,
    proxy_port: Option<u16>,
    php_socket: Option<String>,
    #[serde(default)]
    profile: Option<String>,
}

/// POST /ssl/{domain}/renew — Renew a Let's Encrypt certificate via
/// `instant_acme`, passing the existing cert PEM as the ARI `replaces` hint.
///
/// The prior implementation shelled out to `certbot renew`, which didn't
/// work for certs originally issued via `instant_acme` (certbot had no
/// record of them) and couldn't participate in the ARI replacement chain.
async fn renew(
    State(state): State<AppState>,
    Path(domain): Path<String>,
    Json(body): Json<RenewRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_domain(&domain) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid domain format" })),
        ));
    }

    tracing::info!("Renewing SSL certificate for {domain}");

    let account = ssl::load_or_create_account(&body.email).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
    })?;

    // Read the existing cert (if any) so we can send it as the ARI replaces
    // hint. Missing or unreadable is fine — the renewal just becomes a
    // fresh issuance from the CA's perspective.
    let cert_path = format!("/etc/dockpanel/ssl/{domain}/fullchain.pem");
    let replaces_pem = tokio::fs::read_to_string(&cert_path).await.ok();

    let opts = ssl::ProvisionOpts {
        profile: body.profile.as_deref(),
        replaces_pem: replaces_pem.as_deref(),
    };

    let cert_info = tokio::time::timeout(
        Duration::from_secs(120),
        ssl::provision_cert(&account, &domain, Some(&opts)),
    )
    .await
    .map_err(|_| {
        (
            StatusCode::GATEWAY_TIMEOUT,
            Json(serde_json::json!({ "error": "Certificate renewal timed out after 120s" })),
        )
    })?
    .map_err(|e| {
        tracing::error!("renew failed for {domain}: {e}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Renewal failed: {e}") })),
        )
    })?;

    // Regenerate nginx config so any config changes since the original
    // provision are picked up.
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
        waf_enabled: None,
        waf_mode: None,
        csp_policy: None,
        permissions_policy: None,
        bot_protection: None,
    };
    if let Err(e) = ssl::enable_ssl_for_site(&state.templates, &domain, &site_config).await {
        tracing::warn!("Nginx reload after renewal failed for {domain}: {e}");
    }

    tracing::info!("SSL certificate renewed for {domain}");
    Ok(Json(serde_json::json!({
        "ok": true,
        "domain": domain,
        "expiry": cert_info.expiry,
        "profile": cert_info.profile,
    })))
}

/// GET /ssl/profiles — List ACME profiles advertised by the CA.
async fn profiles(
    axum::extract::Query(q): axum::extract::Query<ProfilesQuery>,
) -> Result<Json<Vec<ssl::ProfileInfo>>, (StatusCode, Json<serde_json::Value>)> {
    let account = ssl::load_or_create_account(&q.email).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
    })?;
    Ok(Json(ssl::list_profiles(&account)))
}

#[derive(Deserialize)]
struct ProfilesQuery {
    email: String,
}

#[derive(Deserialize)]
struct AriQuery {
    email: String,
}

/// GET /ssl/{domain}/renewal-info — Fetch the ARI suggestion for a cert.
/// Always returns JSON. `suggestion: null` means the CA doesn't advertise
/// ARI or the cert couldn't be located on disk.
async fn renewal_info(
    Path(domain): Path<String>,
    axum::extract::Query(q): axum::extract::Query<AriQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_domain(&domain) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid domain format" })),
        ));
    }

    let account = ssl::load_or_create_account(&q.email).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
    })?;

    let cert_path = format!("/etc/dockpanel/ssl/{domain}/fullchain.pem");
    let suggestion = ssl::fetch_ari(&account, &cert_path).await;
    Ok(Json(serde_json::json!({ "suggestion": suggestion })))
}

/// DELETE /ssl/{domain} — Delete an SSL certificate from disk.
///
/// Certificates issued via instant_acme aren't tracked by certbot, so we do
/// a pure filesystem teardown. Revocation (ACME revokeCert) isn't performed
/// — with 45-day and 6-day certs in play, revocation is moot; the cert
/// expires quickly on its own and stapled OCSP is going away.
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

    let ssl_dir = format!("/etc/dockpanel/ssl/{domain}");
    if std::path::Path::new(&ssl_dir).exists() {
        tokio::fs::remove_dir_all(&ssl_dir).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("Failed to remove cert dir: {e}") })),
            )
        })?;
    }

    // Best-effort certbot cleanup for legacy certs migrated from v2.7.x.
    // Failure here is fine — the filesystem cleanup already succeeded.
    let _ = tokio::time::timeout(
        Duration::from_secs(30),
        safe_command("certbot")
            .args(["delete", "--cert-name", &domain, "--non-interactive"])
            .output(),
    )
    .await;

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
    #[serde(default)]
    profile: Option<String>,
    #[serde(default)]
    replaces_pem: Option<String>,
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

    let opts = ssl::ProvisionOpts {
        profile: body.profile.as_deref(),
        replaces_pem: body.replaces_pem.as_deref(),
    };
    let cert_info = ssl::provision_cert_dns01(
        &account,
        &domain,
        &body.cf_zone_id,
        &body.cf_api_token,
        body.cf_api_email.as_deref(),
        body.wildcard,
        Some(&opts),
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
                waf_enabled: None, waf_mode: None,
                csp_policy: None, permissions_policy: None, bot_protection: None,
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
        "profile": cert_info.profile,
    })))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/ssl/provision/{domain}", post(provision))
        .route("/ssl/provision-dns01/{domain}", post(provision_dns01))
        .route("/ssl/status/{domain}", get(status))
        .route("/ssl/profiles", get(profiles))
        .route("/ssl/{domain}/renewal-info", get(renewal_info))
        .route("/ssl/upload", post(upload_cert))
        .route("/ssl/{domain}/renew", post(renew))
        .route("/ssl/{domain}", delete(revoke))
}
