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

/// Split a certificate-issuance failure into "the CA said so" and "this machine
/// broke", and label only the first.
///
/// Both come back from the same call as a 500, and the panel is right to hide a
/// 500 behind an incident id — an operator can do nothing with an unwritable
/// directory. But the commonest failure by far is the CA declining a challenge,
/// and answering THAT with a reference number throws away the only sentence that
/// would have helped. The label is how the panel tells them apart without
/// widening its rule to the status.
///
/// The marker is applied at the arms where the CA actually spoke, so anything
/// unmarked — including any arm added later — stays an internal fault. That is
/// the safe default: a missing label costs a reference number, a wrong one sends
/// somebody to check their DNS while their disk is full.
fn ca_or_internal(e: String) -> (StatusCode, Json<serde_json::Value>) {
    match e.strip_prefix(ssl::CA_DECLINED) {
        Some(reason) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": reason, "code": "acme_order_failed" })),
        ),
        None => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        ),
    }
}

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
    // Only what the CA actually said is labelled. The panel hides every agent 5xx
    // behind an incident id — correctly, because this route also fails for reasons
    // an operator can do nothing about (the account load above, and a full disk or
    // an unwritable directory inside the call below) — and the label is what lets
    // it tell those apart WITHOUT widening that rule to the status.
    // ⚠ The panel carries the same literal; they cannot share a constant across
    // crates, so a pin compares both spellings.
    let cert_info = ssl::provision_cert(&account, &domain, Some(&opts))
        .await
        .map_err(|e| ca_or_internal(e))?;

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

    let canonical = ssl::enable_ssl_for_site(&state.templates, &domain, &site_config)
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
        // Reported so the panel can say so out loud rather than leaving the
        // operator with a site that serves HTTPS and advertises HTTP.
        "canonical_url": canonical,
    })))
}

/// GET /ssl/certificates — Every certificate on this host.
///
/// Unauthenticated in the same sense every agent route is: the agent listens on
/// the panel's private channel. It returns expiry metadata only — no key
/// material, no paths — so it says nothing an operator reading the box could not
/// already see.
async fn list_certificates() -> Json<Vec<ssl::CertStatus>> {
    Json(ssl::list_cert_status().await)
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
    // Write the private key 0600-at-creation. The previous write-then-async-chmod
    // left the key world/group-readable (0644) for the duration of a chmod
    // subprocess — a local disclosure race on a shared box.
    ssl::write_key_file(&key_path, &body.private_key).await
        .map_err(|e| (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        ))?;

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

    let canonical = ssl::enable_ssl_for_site(&state.templates, &body.domain, &site_config)
        .await
        .map_err(|e| (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to enable SSL: {e}") })),
        ))?;

    // Hand back the expiry of the certificate just written. The panel records it
    // so an uploaded certificate is visible to the countdown, the expiry ladder
    // and the auto-healer; without it the panel has to ask again in a second
    // round trip that can fail on its own, and a failed read there used to leave
    // the column NULL — invisible to all three.
    let status = ssl::get_cert_status(&body.domain).await;

    tracing::info!("Custom SSL certificate uploaded for {}", body.domain);
    Ok(Json(serde_json::json!({
        "ok": true,
        "cert_path": cert_path,
        "key_path": key_path,
        "expiry": status.not_after,
        "issuer": status.issuer,
        "canonical_url": canonical,
    })))
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
        let (status, mut body) = ca_or_internal(e);
        // The wrapper every agent ever shipped puts on this route. A panel newer
        // than its agent recognises renewals by it alone, so it has to stay.
        if let Some(msg) = body.0.get("error").and_then(|m| m.as_str()) {
            let wrapped = format!("Renewal failed: {msg}");
            body.0["error"] = serde_json::json!(wrapped);
        }
        (status, body)
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
    let mut canonical = None;
    match ssl::enable_ssl_for_site(&state.templates, &domain, &site_config).await {
        Ok(outcome) => canonical = Some(outcome),
        Err(e) => tracing::warn!("Nginx reload after renewal failed for {domain}: {e}"),
    }

    tracing::info!("SSL certificate renewed for {domain}");
    Ok(Json(serde_json::json!({
        "ok": true,
        "domain": domain,
        "expiry": cert_info.expiry,
        "profile": cert_info.profile,
        "canonical_url": canonical,
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

    // A DNS-01 wildcard is provisioned once under the zone apex and every site
    // in the zone points its `ssl_certificate` at that one directory, so this
    // path is not necessarily this domain's alone to delete.
    let ssl_dir = format!("/etc/dockpanel/ssl/{domain}");
    if std::path::Path::new(&ssl_dir).exists() {
        if crate::services::ownership::cert_dir_in_use_elsewhere(&domain) {
            tracing::warn!(
                "Leaving {ssl_dir} in place — another vhost still points at it \
                 (a shared wildcard). Removing it would make `nginx -t` fail for \
                 that site and leave nginx down at the next restart."
            );
        } else {
            tokio::fs::remove_dir_all(&ssl_dir).await.map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": format!("Failed to remove cert dir: {e}") })),
                )
            })?;
        }
    }

    // What used to be here: `certbot delete --cert-name {domain}
    // --non-interactive`, described as "best-effort cleanup for legacy certs
    // migrated from v2.7.x".
    //
    // Neither tree issues through certbot at HEAD — provisioning is instant_acme
    // into /etc/dockpanel/ssl, above — so on any install newer than v2.7.x every
    // lineage this could name was created by the OPERATOR, out of band, and
    // nothing distinguished the two cases. A certbot lineage also carries all
    // its SANs, so deleting the one named `example.com` took `www.` and `mail.`
    // with it, plus the renewal config that would have brought it back. The
    // panel does not read these certificates and cannot re-issue them; the only
    // correct action is to say so and leave them alone.
    let renewal_conf = format!("/etc/letsencrypt/renewal/{domain}.conf");
    if std::path::Path::new(&renewal_conf).exists() {
        tracing::warn!(
            "A certbot lineage named {domain} exists at {renewal_conf} and was NOT \
             touched — the panel did not issue it. Remove it with `certbot delete \
             --cert-name {domain}` if it really is stale."
        );
    }

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
    let mut canonical = None;
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

            canonical = Some(
                ssl::enable_ssl_for_site(&state.templates, &domain, &site_config)
                    .await
                    .map_err(|e| {
                        (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e })))
                    })?,
            );
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
        "canonical_url": canonical,
    })))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/ssl/provision/{domain}", post(provision))
        .route("/ssl/provision-dns01/{domain}", post(provision_dns01))
        // Registered BEFORE the `{domain}` routes it shares a prefix with, and
        // named `certificates` rather than a bare `/ssl` for the same reason the
        // db-backups router pins `/db-backups/dump` above `{db_name}`: a static
        // segment that could also be read as a parameter is a collision waiting
        // for a domain with that name.
        .route("/ssl/certificates", get(list_certificates))
        .route("/ssl/status/{domain}", get(status))
        .route("/ssl/profiles", get(profiles))
        .route("/ssl/{domain}/renewal-info", get(renewal_info))
        .route("/ssl/upload", post(upload_cert))
        .route("/ssl/{domain}/renew", post(renew))
        .route("/ssl/{domain}", delete(revoke))
}
