use crate::safe_cmd::safe_command;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
use serde::Deserialize;
use std::collections::HashMap;

use super::{is_valid_container_id, is_valid_domain, is_valid_name, AppState};
use crate::routes::nginx::SiteConfig;
use crate::services::compose;
use crate::services::docker_apps;
use crate::services::{nginx, ownership, ssl, traefik};

#[derive(Deserialize)]
struct DeployRequest {
    template_id: String,
    name: String,
    port: u16,
    #[serde(default)]
    env: HashMap<String, String>,
    /// Optional domain for auto reverse proxy
    domain: Option<String>,
    /// Email for Let's Encrypt SSL (requires domain)
    ssl_email: Option<String>,
    /// "none" | "acme" | "provided". Absent = legacy inference from ssl_email (an older panel).
    #[serde(default)]
    tls_mode: Option<String>,
    /// Registry alias; provided mode only.
    #[serde(default)]
    tls_certificate: Option<String>,
    /// Memory limit in MB (e.g., 512)
    memory_mb: Option<u64>,
    /// CPU limit as percentage (e.g., 50 = 50% of one core)
    cpu_percent: Option<u64>,
    /// When true, use Traefik file-based routing instead of nginx
    #[serde(default)]
    use_traefik: bool,
    /// User ID for container labeling and isolation
    user_id: Option<String>,
    /// Enable GPU passthrough (requires NVIDIA Container Toolkit)
    #[serde(default)]
    gpu_enabled: bool,
    /// Specific GPU device indices (e.g., [0, 2]) to assign to this container.
    /// When None or empty with gpu_enabled=true, all GPUs are assigned (legacy
    /// behavior). When Some(non-empty), only the listed indices are passed
    /// through via Docker's device_ids field. Ignored when gpu_enabled=false.
    gpu_indices: Option<Vec<u32>>,
}

/// Reject the request unless `container_id` names a dockpanel-managed container. WRITE-scope parity
/// with the `list` READ path (which filters on `dockpanel.managed=true`): prevents an admin from
/// acting on the panel's OWN infrastructure containers (postgres/api/agent) by id. Returns 403.
async fn ensure_managed(container_id: &str) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    docker_apps::require_managed(container_id).await.map_err(|_| {
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": "Not a dockpanel-managed container" })),
        )
    })
}

/// GET /apps/templates — List all available app templates.
async fn templates() -> Json<Vec<docker_apps::AppTemplate>> {
    Json(docker_apps::list_templates())
}

/// POST /apps/deploy — Deploy an app from a template, optionally with reverse proxy + SSL.
async fn deploy(
    State(state): State<AppState>,
    Json(body): Json<DeployRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_name(&body.name) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid app name" })),
        ));
    }

    if let Some(ref domain) = body.domain {
        if !is_valid_domain(domain) {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "Invalid domain format" })),
            ));
        }
    }

    // Decide the TLS shape BEFORE the container exists: a request that names a
    // mode it cannot honour is refused with nothing deployed, rather than
    // answered with a running container and a warning.
    let tls = TlsIntent::from_request(
        body.tls_mode.as_deref(),
        body.ssl_email.as_deref(),
        body.tls_certificate.as_deref(),
        body.use_traefik,
    )
    .map_err(|e| (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": e }))))?;

    let result =
        docker_apps::deploy_app(&body.template_id, &body.name, body.port, body.env, body.domain.as_deref(), body.memory_mb, body.cpu_percent, body.user_id.as_deref(), body.gpu_enabled, body.gpu_indices.clone())
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e })),
                )
            })?;

    let mut response = serde_json::json!({
        "success": true,
        "container_id": result.container_id,
        "name": result.name,
        "port": result.port,
    });

    // Auto reverse proxy: Traefik (file-based dynamic config) or nginx
    if let Some(ref domain) = body.domain {
        expose_domain(
            &state.templates,
            domain,
            body.port,
            tls,
            body.use_traefik,
            &mut response,
        )
        .await;
    }

    Ok(Json(response))
}

/// A reverse-proxy vhost pointing at a local port. Built in one place because
/// the same shape is needed to render the plain-HTTP config and again to
/// re-render it with SSL, and two literals drift.
fn proxy_site_config(port: u16) -> SiteConfig {
    SiteConfig {
        runtime: "proxy".to_string(),
        root: None,
        proxy_port: Some(port),
        php_socket: None,
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
    }
}

/// How a domain claimed by a deploy is to be secured — the request's TLS
/// fields resolved into one decision before anything runs.
///
/// Until v2.160.0 the only signal was whether `ssl_email` was present, and a
/// redeploy that omitted it silently rewrote the vhost without its `:443`
/// block (the breadcrumb on `expose_domain` tells the whole story). The mode
/// is now STORED by the panel and sent on every deploy, so an absent address
/// stops being the only signal. `None` for the mode keeps the old rule
/// byte-for-byte, which is what an older panel still sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TlsIntent<'a> {
    /// Plain HTTP, or TLS terminated upstream. The reporter's interim on #104.
    None,
    /// Order a Let's Encrypt certificate under this ACME account address.
    Acme { email: &'a str },
    /// Serve a certificate the operator registered under this alias.
    Provided { alias: &'a str },
}

impl<'a> TlsIntent<'a> {
    /// Resolve the wire fields. `Err` is the sentence a handler answers 400
    /// with, and it is asked BEFORE any container runs.
    /// `use_traefik` refuses `provided` HERE, before any container runs — the
    /// same front-door discipline this function already applies to a bad
    /// alias or a missing `ssl_email`. Until now the combination was refused
    /// only INSIDE `expose_domain`'s Traefik branch, after `deploy_app` had
    /// already created the container — exactly the outcome this function's
    /// own doc comment (on the `deploy` handler above) says a request naming
    /// an unhonourable mode must never produce: "a running container and a
    /// warning" instead of nothing deployed. Traefik's file provider has no
    /// per-route certificate form, so a registered certificate can never be
    /// served through it regardless of which door the request came in by.
    pub(crate) fn from_request(
        tls_mode: Option<&str>,
        ssl_email: Option<&'a str>,
        alias: Option<&'a str>,
        use_traefik: bool,
    ) -> Result<TlsIntent<'a>, String> {
        match tls_mode {
            // Legacy inference — an older panel that knows no mode. Kept
            // exactly as it was, presence and not content, so nothing an old
            // panel sends changes meaning.
            None => Ok(match ssl_email {
                Some(email) => TlsIntent::Acme { email },
                None => TlsIntent::None,
            }),
            Some("none") => Ok(TlsIntent::None),
            Some("acme") => match ssl_email {
                Some(email) if !email.trim().is_empty() => Ok(TlsIntent::Acme { email }),
                _ => Err(
                    "tls_mode acme needs an ssl_email — the address the Let's Encrypt account \
                     is registered under"
                        .to_string(),
                ),
            },
            Some("provided") => match alias {
                Some(alias) if use_traefik => Err(format!(
                    "tls_mode provided cannot be served through Traefik: its file provider has no \
                     per-route certificate form for '{alias}'. Switch the reverse proxy to nginx, \
                     or use tls_mode acme or none."
                )),
                Some(alias) if ssl::is_valid_cert_alias(alias) => Ok(TlsIntent::Provided { alias }),
                Some(alias) => Err(format!(
                    "'{alias}' is not a certificate alias — 1 to 64 lowercase letters, digits or \
                     hyphens, starting and ending with a letter or digit"
                )),
                None => Err(
                    "tls_mode provided needs tls_certificate — the alias of a registered certificate"
                        .to_string(),
                ),
            },
            Some(other) => Err(format!(
                "unknown tls_mode '{other}': expected none, acme or provided"
            )),
        }
    }

    /// The vocabulary word this intent answers as.
    pub(crate) fn mode(self) -> &'static str {
        match self {
            TlsIntent::None => "none",
            TlsIntent::Acme { .. } => "acme",
            TlsIntent::Provided { .. } => "provided",
        }
    }
}

/// Put a domain in front of a local port: write the vhost, then secure it the
/// way the request asked.
///
/// Extracted from `deploy` so compose stacks reach the same code rather than a
/// second copy of it. Until v2.54.0 a stack had no route to any of this — the
/// compose deploy request carried no domain field at all, so a stack could only
/// ever be reached on `127.0.0.1:{port}` and "SSL doesn't work" was the
/// accurate report of a feature that did not exist.
///
/// Failures are reported into `response` as warnings rather than returned: the
/// container is already up, and losing the whole deploy over a certificate that
/// can be retried from the panel would be the worse trade.
///
/// ⚠ The trap this function used to hold, kept here because the shape that
/// caused it is still the shape of the None/Acme flow below. Before v2.160.0
/// the mode was INFERRED from whether an address was present, and the reason
/// that could not be a new request field alone was the REDEPLOY rather than the
/// first deploy. `stacks::update` sends the domain on every edit and tears the
/// vhost down only when the domain actually changes — so an ordinary YAML edit
/// arrived here with the same domain and no address. This function rewrote the
/// vhost from `proxy_site_config`, which carries no certificate, and returned at
/// the address check. The `:443` server block was gone, on an edit that never
/// mentioned TLS, and `https.conf` had already sent a year of
/// `Strict-Transport-Security` — so every browser that had seen the site refused
/// the plain-HTTP replacement. A hard outage with no way back for the visitor,
/// not a downgrade anyone can click past.
///
/// The mode is now STORED by the panel (`docker_stacks.tls_mode`, with the
/// alias beside it) and sent on every deploy, arriving here as [`TlsIntent`].
/// The two repairs that were named: the OMISSION half is closed because an
/// absent address is no longer the signal — a stored mode is; the MODE half is
/// the `Provided` arm, which short-circuits BEFORE the HTTP-first write below,
/// because that write is the one that strips `:443` from behind HSTS. A
/// provided domain whose certificate cannot be served is left exactly as it
/// was and reported — never degraded to HTTP. `external_tls` remains the shape
/// that was copied FROM and not the precedent: it declines TLS on this box, so
/// there is no `:443` block for a redeploy to lose.
///
/// Reported as #104; the design was accepted in writing on 2026-08-12 and its
/// three points are binding:
///   1. Multiple registered certificates from the start, each named at upload
///      time — not a single default slot. Migrating a one-slot design after
///      domains have been claimed against it is the expensive version. Built:
///      the registry under `ssl::SSL_REGISTRY_DIR`, one directory per alias,
///      served by `routes/ssl_registry.rs`.
///   2. Referenced by alias, never by filesystem path. A claim naming a path
///      breaks the first time anything moves. Built: `TlsIntent::Provided`
///      carries the alias and `ssl::registry_paths` resolves it here.
///   3. SAN validation AT CLAIM TIME — the panel checks the claimed domain
///      actually falls under the supplied certificate's SAN, rather than
///      trusting the operator and failing at TLS handshake time later. The
///      reporter called this the most valuable of the three, for the reason
///      that it shares the quiet failure mode of the trap above: operator
///      error goes undetected until a browser complains. ⭐ DO NOT BUILD THIS
///      — it already exists and is unit-tested: `ssl::cert_covers_domain`
///      (`services/ssl.rs`), called from the site upload path at
///      `routes/ssl.rs`, from the registry's covers door, and re-asked in the
///      `Provided` arm below as defence in depth. It handles wildcards, the CN
///      fallback for a certificate carrying no SAN, case and trailing-dot
///      normalisation, and refuses partial wildcards. It lives in the AGENT
///      because this crate depends on an X.509 parser and the panel does not —
///      which is where the reporter said the check belonged.
///
/// ⚠ A fourth consideration is OURS, not the reporter's, and is NOT binding:
/// renewal suppression made explicit for a provided certificate (already the
/// product's stated position on three other surfaces). It is worth doing; it
/// was never put to the reporter and must not be presented as agreed. Until
/// v2.157.0 this comment listed it as accepted point 3 and omitted SAN
/// validation entirely — inverting which of the two a contributor would treat
/// as settled. The thread is the record: read it before building. (Since
/// v2.161.0 a stack's ACME certificate IS renewed on a schedule — the panel's
/// `security_scanner::renew_stack_certificate` drives it — so the suppression
/// this paragraph calls unbuilt now has something real to suppress. The
/// registry root still sits outside every renewal path by construction, see
/// `ssl::SSL_REGISTRY_DIR`, and the panel-side guard declines any stack whose
/// effective mode is not `acme`.)
async fn expose_domain(
    templates: &tera::Tera,
    domain: &str,
    port: u16,
    tls: TlsIntent<'_>,
    use_traefik: bool,
    response: &mut serde_json::Value,
) {
    if use_traefik {
        // --- Traefik mode: write a dynamic route config file ---
        if let TlsIntent::Provided { alias } = tls {
            // Traefik's file provider has no per-route certificate form the
            // agent writes, so a registered certificate cannot reach it. Refuse
            // in words and write nothing — a route silently written without
            // TLS would be the HSTS outage above, one proxy over.
            tracing::warn!("Auto-proxy (Traefik): {domain} asked for registered certificate {alias}, which only nginx can serve");
            response["proxy_warning"] = serde_json::json!(
                "registered certificates are served by nginx; this server proxies through Traefik"
            );
            response["tls_refused"] = serde_json::json!(true);
            return;
        }
        let ssl = matches!(tls, TlsIntent::Acme { .. });
        match traefik::write_route_config(domain, port, ssl) {
            Ok(()) => {
                response["domain"] = serde_json::json!(domain);
                response["proxy"] = serde_json::json!("traefik");
                response["tls_mode"] = serde_json::json!(tls.mode());
                if ssl {
                    response["ssl"] = serde_json::json!(true);
                }
                tracing::info!("Auto-proxy (Traefik): {domain} → 127.0.0.1:{port} (ssl={ssl})");
            }
            Err(e) => {
                tracing::warn!("Auto-proxy (Traefik): failed to write route config for {domain}: {e}");
                response["proxy_warning"] = serde_json::json!(format!("Traefik config failed: {e}"));
            }
        }
        return;
    }

    // --- nginx mode, registered certificate: render HTTPS directly ---
    //
    // This arm runs BEFORE the HTTP-first write below and never reaches it. The
    // HTTP-first write is what strips the `:443` block from behind a year of
    // HSTS, so a provided domain is either rendered with its certificate or
    // left exactly as it was — never rewritten to plain HTTP and never handed
    // to the ACME path.
    if let TlsIntent::Provided { alias } = tls {
        let (cert_path, key_path) = ssl::registry_paths(alias);
        let pem = match std::fs::read_to_string(&cert_path) {
            Ok(pem) => pem,
            Err(e) => {
                tracing::warn!("Auto-proxy: {domain} names registered certificate {alias}, which is not on this server ({e}); nothing written");
                response["proxy_warning"] = serde_json::json!(format!(
                    "no certificate named {alias} is registered on this server, so the vhost for \
                     {domain} was left as it was rather than served over plain HTTP. Register the \
                     certificate and redeploy."
                ));
                response["tls_refused"] = serde_json::json!(true);
                return;
            }
        };
        // Binding point 3, re-asked here: the panel already checked at claim
        // time, but the pair may have been replaced since, and a vhost that
        // serves the wrong name is the quiet failure this whole feature exists
        // to stop.
        if let Err(reason) = ssl::cert_covers_domain(&pem, domain) {
            tracing::warn!("Auto-proxy: registered certificate {alias} does not cover {domain}: {reason}");
            response["proxy_warning"] = serde_json::json!(format!(
                "the registered certificate {alias} cannot serve {domain}: {reason} The vhost was \
                 left as it was."
            ));
            response["tls_refused"] = serde_json::json!(true);
            return;
        }
        // Rendered through the ordinary renderer with the registry paths named
        // outright — NOT through `enable_ssl_for_site`, which hardcodes the
        // per-domain tree and would point the vhost at a directory this
        // certificate is deliberately not in.
        let mut site_config = proxy_site_config(port);
        site_config.ssl = Some(true);
        site_config.ssl_cert = Some(cert_path);
        site_config.ssl_key = Some(key_path);
        let rendered = match nginx::render_site_config(templates, domain, &site_config) {
            Ok(rendered) => rendered,
            Err(e) => {
                tracing::warn!("Auto-proxy: failed to render HTTPS config for {domain}: {e}");
                response["proxy_warning"] =
                    serde_json::json!(format!("Failed to render nginx config: {e}"));
                return;
            }
        };
        let target = nginx::vhost_target(domain);
        let config_path = target.path().to_string();
        let previous = std::fs::read_to_string(&config_path).ok();
        let tmp_path = format!("{config_path}.tmp");
        let write_result = std::fs::write(&tmp_path, &rendered)
            .and_then(|_| std::fs::rename(&tmp_path, &config_path));
        if let Err(e) = write_result {
            std::fs::remove_file(&tmp_path).ok();
            tracing::warn!("Auto-proxy: failed to write nginx config for {domain}: {e}");
            response["proxy_warning"] =
                serde_json::json!(format!("Failed to write nginx config: {e}"));
            return;
        }
        if !target.is_live() {
            tracing::info!("Auto-proxy: {domain} is disabled, saved the HTTPS route to its parked configuration and left the maintenance response in service");
            response["proxy_warning"] = serde_json::json!(format!(
                "{domain} is disabled, so the proxy configuration was saved but is not \
                 serving. Enable the site to bring it up."
            ));
            // The certificate IS bound — the parked body carries it and serves it
            // the moment the site is enabled — so this is not a refused TLS leg,
            // and the panel must not read it as one.
            response["domain"] = serde_json::json!(domain);
            response["ssl"] = serde_json::json!(true);
            response["parked"] = serde_json::json!(true);
            response["tls_mode"] = serde_json::json!("provided");
            response["tls_certificate"] = serde_json::json!(alias);
            return;
        }
        match nginx::test_config().await {
            Ok(output) if output.success => {
                if let Err(e) = nginx::reload().await {
                    // The vhost is on disk and passes the test, but nothing is
                    // serving it yet. Answering `ssl: true` here would have the
                    // panel record a success over a domain still answering as
                    // it did before.
                    tracing::warn!("Auto-proxy: nginx reload failed after deploy for {domain}: {e}");
                    response["proxy_warning"] = serde_json::json!(format!(
                        "the HTTPS vhost for {domain} was written but nginx did not reload: {e}. \
                         Redeploy the stack, or reload nginx, to bring it into service."
                    ));
                    response["tls_refused"] = serde_json::json!(true);
                    return;
                }
                response["domain"] = serde_json::json!(domain);
                response["proxy"] = serde_json::json!(true);
                response["ssl"] = serde_json::json!(true);
                response["tls_mode"] = serde_json::json!("provided");
                response["tls_certificate"] = serde_json::json!(alias);
                tracing::info!("Auto-proxy: {domain} → 127.0.0.1:{port} over registered certificate {alias}");
            }
            Ok(output) => {
                let restored = nginx::restore_or_remove(&config_path, previous.as_deref());
                tracing::warn!("Auto-proxy: nginx config test failed for {domain}: {}", output.stderr);
                response["proxy_warning"] = serde_json::json!(format!(
                    "Nginx config test failed: {}{}",
                    output.stderr,
                    nginx::restore_note(restored)
                ));
            }
            Err(e) => {
                let restored = nginx::restore_or_remove(&config_path, previous.as_deref());
                tracing::warn!("Auto-proxy: nginx test error for {domain}: {e}");
                response["proxy_warning"] = serde_json::json!(format!(
                    "Nginx test error: {e}{}",
                    nginx::restore_note(restored)
                ));
            }
        }
        return;
    }

    // --- nginx mode: create nginx config pointing to the app's port ---
    // HTTP first; the ACME arm below re-renders with the certificate once it
    // has one. A provided certificate never reaches this write (see above).
    let site_config = proxy_site_config(port);

    match nginx::render_site_config(templates, domain, &site_config) {
        Ok(rendered) => {
            // Exposing an app on a domain the operator took offline updates that
            // domain's parked body instead of putting it back into service.
            let target = nginx::vhost_target(domain);
            let config_path = target.path().to_string();
            // Snapshot first: this path may already belong to a site or a
            // git deploy, and `nginx -t` below is a whole-server check that
            // an unrelated broken vhost is enough to fail.
            let previous = std::fs::read_to_string(&config_path).ok();
            let tmp_path = format!("{config_path}.tmp");
            let write_result = std::fs::write(&tmp_path, &rendered)
                .and_then(|_| std::fs::rename(&tmp_path, &config_path));
            if let Err(e) = write_result {
                // Clean up tmp file on failure
                std::fs::remove_file(&tmp_path).ok();
                tracing::warn!("Auto-proxy: failed to write nginx config for {domain}: {e}");
                response["proxy_warning"] =
                    serde_json::json!(format!("Failed to write nginx config: {e}"));
            } else if !target.is_live() {
                tracing::info!("Auto-proxy: {domain} is disabled, saved the route to its parked configuration and left the maintenance response in service");
                response["proxy_warning"] = serde_json::json!(format!(
                    "{domain} is disabled, so the proxy configuration was saved but is not \
                     serving. Enable the site to bring it up."
                ));
            } else {
                match nginx::test_config().await {
                    Ok(output) if output.success => {
                        if let Err(e) = nginx::reload().await {
                            tracing::warn!("Auto-proxy: nginx reload failed after deploy for {domain}: {e}");
                        }
                        response["domain"] = serde_json::json!(domain);
                        response["proxy"] = serde_json::json!(true);
                        tracing::info!("Auto-proxy: {domain} → 127.0.0.1:{port}");
                    }
                    Ok(output) => {
                        let restored = nginx::restore_or_remove(&config_path, previous.as_deref());
                        tracing::warn!("Auto-proxy: nginx config test failed for {domain}: {}", output.stderr);
                        response["proxy_warning"] = serde_json::json!(format!(
                            "Nginx config test failed: {}{}",
                            output.stderr,
                            nginx::restore_note(restored)
                        ));
                    }
                    Err(e) => {
                        let restored = nginx::restore_or_remove(&config_path, previous.as_deref());
                        tracing::warn!("Auto-proxy: nginx test error for {domain}: {e}");
                        response["proxy_warning"] = serde_json::json!(format!(
                            "Nginx test error: {e}{}",
                            nginx::restore_note(restored)
                        ));
                    }
                }
            }
        }
        Err(e) => {
            tracing::warn!("Auto-proxy: failed to render config for {domain}: {e}");
            response["proxy_warning"] =
                serde_json::json!(format!("Failed to render nginx config: {e}"));
        }
    }

    // SSL provisioning (only if proxy was set up successfully, nginx mode only)
    if response.get("proxy").is_none() {
        return;
    }
    response["tls_mode"] = serde_json::json!(tls.mode());
    let email = match tls {
        TlsIntent::Acme { email } => email,
        _ => return,
    };

    // Wait for DNS propagation before attempting SSL (up to 30 seconds)
    for i in 0..6u32 {
        if i > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
        match tokio::net::lookup_host(format!("{}:80", domain)).await {
            Ok(_) => {
                tracing::info!("DNS resolved for {domain} (attempt {}/6)", i + 1);
                break;
            }
            Err(_) if i < 5 => {
                tracing::info!("Waiting for DNS propagation for {}... ({}/6)", domain, i + 1);
                continue;
            }
            Err(e) => {
                tracing::warn!("DNS not propagated for {}: {} — trying SSL anyway", domain, e);
                break;
            }
        }
    }

    match ssl::load_or_create_account(email).await {
        Ok(account) => match ssl::provision_cert(&account, domain, None).await {
            Ok(_cert_info) => {
                match ssl::enable_ssl_for_site(templates, domain, &site_config).await {
                    Ok(_) => {
                        response["ssl"] = serde_json::json!(true);
                        tracing::info!("Auto-SSL: certificate provisioned for {domain}");
                    }
                    Err(e) => {
                        tracing::warn!("Auto-SSL: enable_ssl_for_site failed for {domain}: {e}");
                        response["ssl_warning"] =
                            serde_json::json!(format!("SSL enable failed: {e} — retry from panel"));
                        response["ssl_pending"] = serde_json::json!(true);
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Auto-SSL: cert provisioning failed for {domain}: {e}");
                response["ssl_warning"] =
                    serde_json::json!(format!("SSL provisioning failed: {e} — retry from panel"));
                response["ssl_pending"] = serde_json::json!(true);
            }
        },
        Err(e) => {
            tracing::warn!("Auto-SSL: ACME account failed: {e}");
            response["ssl_warning"] =
                serde_json::json!(format!("ACME account failed: {e} — retry from panel"));
            response["ssl_pending"] = serde_json::json!(true);
        }
    }
}

/// Take a domain back down: remove the vhost, the certificates and the logs —
/// but only the ones that can still be proved to belong to the caller.
///
/// Every delete here is conditional on the resource saying so. Before v2.53.0
/// they all fired on a label alone, so removing an app whose domain a site had
/// since taken over destroyed the SITE's vhost, certificates and logs. Shared
/// with the Compose-stack teardown so a stack cannot grow a second, unguarded
/// copy of the same deletes.
async fn unexpose_domain(domain: &str, host_port: Option<u16>, response: &mut serde_json::Value) {

    // Remove Traefik dynamic route config (if it exists). The legacy-name
    // leg inside checks ownership itself.
    traefik::remove_route_config(domain);

    // Remove nginx config — only if it is still fronting THIS container.
    // The vhost is rendered from the same template a site's is, so the only
    // thing in it identifying the app is the `proxy_pass` to the port the
    // container published.
    let config_path = format!("{}/{domain}.conf", nginx::sites_dir());
    let mut removed_vhost = false;
    if std::path::Path::new(&config_path).exists() {
        if ownership::app_vhost(&config_path, host_port).may_delete() {
            std::fs::remove_file(&config_path).ok();
            removed_vhost = true;
            if let Err(e) = nginx::reload().await {
                tracing::warn!("Auto-proxy cleanup: nginx reload failed after removing config for {domain}: {e}");
            }
            tracing::info!("Auto-proxy cleanup: removed nginx config for {domain}");
        } else {
            tracing::warn!(
                "Auto-proxy cleanup: LEAVING {config_path} in place — it does not \
                 proxy to this container's port. Another site or app now serves \
                 {domain}; removing this app must not take it down."
            );
            response["proxy_warning"] = serde_json::json!(format!(
                "The nginx configuration for {domain} is serving something else and was left in place."
            ));
        }
    }

    // Remove SSL certificates (panel-provisioned). A DNS-01 wildcard is
    // provisioned once under the zone apex and SHARED by every site in the
    // zone, so this directory is not necessarily this app's to delete.
    //
    // Also gated on `removed_vhost`, not just `cert_dir_in_use_elsewhere` — a
    // STOPPED (parked) container reports no port in Docker's live listing, so
    // the vhost check above cannot prove ownership and correctly leaves the
    // vhost standing; `cert_dir_in_use_elsewhere` explicitly excludes that
    // same still-standing vhost when it scans for other references (a solo
    // domain's own vhost never counts as "in use elsewhere"), so without this
    // gate the certificate files were deleted out from under a vhost that
    // still named them — breaking `nginx -t` for the WHOLE server at the next
    // reload for any reason. Only delete the certs when the vhost naming them
    // was ALSO just removed this call, matching the log-removal gate below.
    let ssl_dir = format!("/etc/dockpanel/ssl/{domain}");
    if removed_vhost && std::path::Path::new(&ssl_dir).exists() {
        if ownership::cert_dir_in_use_elsewhere(domain) {
            tracing::warn!(
                "SSL cleanup: LEAVING {ssl_dir} in place — another vhost still \
                 points at it. Deleting it would break that site at the next \
                 nginx reload."
            );
        } else {
            std::fs::remove_dir_all(&ssl_dir).ok();
            tracing::info!("SSL cleanup: removed certs for {domain}");
        }
    } else if std::path::Path::new(&ssl_dir).exists() {
        tracing::warn!(
            "SSL cleanup: LEAVING {ssl_dir} in place — its vhost was not removed \
             this call (often a stopped/parked container, whose port Docker no \
             longer reports), so deleting the certs would orphan a vhost that \
             still names them."
        );
    }

    // Let's Encrypt is NOT the panel's namespace and this code never had any
    // business in it. Neither tree issues through certbot — provisioning is
    // instant_acme into /etc/dockpanel/ssl above — so every lineage this
    // could reach was created by the operator, out of band. Deleting
    // live/ + archive/ + renewal/ destroyed the certificate, its whole
    // history, and the automation that would have renewed it, for a
    // certificate the panel did not issue and does not read. On a box whose
    // mail stack is configured by the panel that includes the lineage
    // Postfix and Dovecot are pointed at (`routes/mail.rs`
    // `panel_tls_paths`), whose documented fallback is the distro snakeoil.
    // There is no marker distinguishing a panel-era lineage from an
    // operator's, so there is no safe narrowing — the delete is gone.

    // Remove nginx logs — only ours to remove if the vhost was ours. When
    // another site now serves this domain, these are that site's logs.
    if removed_vhost {
        let access_log = format!("/var/log/nginx/{domain}.access.log");
        let error_log = format!("/var/log/nginx/{domain}.error.log");
        std::fs::remove_file(&access_log).ok();
        std::fs::remove_file(&error_log).ok();
    }
}

/// GET /apps — List all deployed apps.
async fn list() -> Result<Json<Vec<docker_apps::DeployedApp>>, (StatusCode, Json<serde_json::Value>)>
{
    let apps = docker_apps::list_deployed_apps().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
    })?;

    Ok(Json(apps))
}

/// POST /apps/{container_id}/stop — Stop a running app.
async fn stop(
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container ID" })),
        ));
    }
    ensure_managed(&container_id).await?;

    docker_apps::stop_app(&container_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
        })?;

    Ok(Json(serde_json::json!({ "success": true })))
}

/// POST /apps/{container_id}/start — Start a stopped app.
async fn start(
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container ID" })),
        ));
    }
    ensure_managed(&container_id).await?;

    docker_apps::start_app(&container_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
        })?;

    Ok(Json(serde_json::json!({ "success": true })))
}

/// POST /apps/{container_id}/restart — Restart an app.
async fn restart(
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container ID" })),
        ));
    }
    ensure_managed(&container_id).await?;

    docker_apps::restart_app(&container_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
        })?;

    Ok(Json(serde_json::json!({ "success": true })))
}

/// GET /apps/{container_id}/logs — Get app container logs.
async fn logs(
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container ID" })),
        ));
    }
    ensure_managed(&container_id).await?;

    let output = docker_apps::get_app_logs(&container_id, 200)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
        })?;

    Ok(Json(serde_json::json!({ "logs": output })))
}

/// POST /apps/{container_id}/update — Pull latest image and recreate container.
/// Uses blue-green deployment (zero-downtime) when the app has a domain with nginx reverse proxy.
async fn update(
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container ID" })),
        ));
    }
    ensure_managed(&container_id).await?;

    let result = docker_apps::update_app(&container_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
        })?;

    Ok(Json(serde_json::json!({
        "success": true,
        "container_id": result.container_id,
        "blue_green": result.blue_green,
        "migrated_volumes": result.migrated_volumes,
        "repaired_volumes": result.repaired_volumes,
    })))
}

/// GET /apps/{container_id}/env — Get container environment variables.
async fn get_env(
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container ID" })),
        ));
    }
    ensure_managed(&container_id).await?;

    let env = docker_apps::get_app_env(&container_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
        })?;

    // Sensitive env var name patterns — mask values containing these substrings.
    //
    // Substring and not a whole token, deliberately: an unknown key is safer
    // over-masked than under-masked, and `APIKEY` has no separator to anchor on.
    // The cost of that choice is paid by `catalogue_non_secret_env`, which
    // exempts the names our OWN templates declare non-secret — otherwise
    // `KEYCLOAK_ADMIN`, `NEXTAUTH_URL` and `AUTHENTIK_POSTGRESQL__HOST` all read
    // as `********`.
    const SENSITIVE_PATTERNS: &[&str] = &[
        "PASSWORD", "SECRET", "KEY", "TOKEN", "CREDENTIAL", "AUTH",
    ];

    let env_map: Vec<serde_json::Value> = env
        .into_iter()
        .map(|(k, v)| {
            let upper = k.to_uppercase();
            let is_sensitive = SENSITIVE_PATTERNS.iter().any(|pat| upper.contains(pat))
                && !docker_apps::catalogue_non_secret_env(&k);
            let masked_value = if is_sensitive {
                docker_apps::ENV_MASK.to_string()
            } else {
                v
            };
            serde_json::json!({ "key": k, "value": masked_value })
        })
        .collect();

    Ok(Json(serde_json::json!({ "env": env_map })))
}

#[derive(Deserialize)]
struct UpdateEnvRequest {
    env: HashMap<String, String>,
}

/// PUT /apps/{container_id}/env — Update environment variables and recreate container.
async fn update_env(
    Path(container_id): Path<String>,
    Json(body): Json<UpdateEnvRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container ID" })),
        ));
    }
    ensure_managed(&container_id).await?;

    let new_id = docker_apps::update_env(&container_id, body.env)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
        })?;

    Ok(Json(serde_json::json!({ "success": true, "container_id": new_id })))
}

/// GET /apps/{container_id}/stats — Get live resource usage for a container.
async fn container_stats(
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container ID" })),
        ));
    }
    ensure_managed(&container_id).await?;

    // Use docker stats --no-stream for a single snapshot
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        safe_command("docker")
            .args(["stats", "--no-stream", "--format", "{{.CPUPerc}}|{{.MemUsage}}|{{.MemPerc}}|{{.NetIO}}|{{.BlockIO}}|{{.PIDs}}", &container_id])
            .output(),
    )
    .await
    .map_err(|_| (StatusCode::GATEWAY_TIMEOUT, Json(serde_json::json!({"error": "Timeout"}))))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parts: Vec<&str> = stdout.trim().split('|').collect();

    if parts.len() >= 6 {
        Ok(Json(serde_json::json!({
            "cpu_percent": parts[0].trim_end_matches('%').trim(),
            "memory_usage": parts[1].trim(),
            "memory_percent": parts[2].trim_end_matches('%').trim(),
            "network_io": parts[3].trim(),
            "block_io": parts[4].trim(),
            "pids": parts[5].trim(),
        })))
    } else {
        Ok(Json(serde_json::json!({ "error": "Container not running or stats unavailable" })))
    }
}

/// GET /apps/{container_id}/shell-info — Get shell availability for a container.
async fn shell_info(
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container ID" })),
        ));
    }
    ensure_managed(&container_id).await?;

    let name_output = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        safe_command("docker")
            .args(["inspect", "--format", "{{.Name}}", &container_id])
            .output(),
    ).await;
    let name = name_output
        .ok()
        .and_then(|r| r.ok())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .trim_start_matches('/')
                .to_string()
        })
        .unwrap_or_default();

    let bash = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        safe_command("docker")
            .args(["exec", &container_id, "which", "bash"])
            .output(),
    ).await;
    let has_bash = bash.ok().and_then(|r| r.ok()).map(|o| o.status.success()).unwrap_or(false);

    let sh = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        safe_command("docker")
            .args(["exec", &container_id, "which", "sh"])
            .output(),
    ).await;
    let has_sh = sh.ok().and_then(|r| r.ok()).map(|o| o.status.success()).unwrap_or(false);

    Ok(Json(serde_json::json!({
        "name": name,
        "has_bash": has_bash,
        "has_sh": has_sh,
        "shell": if has_bash { "/bin/bash" } else if has_sh { "/bin/sh" } else { "" },
    })))
}

/// POST /apps/{container_id}/exec — Execute a command inside a container.
async fn exec_command(
    Path(container_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container ID" })),
        ));
    }
    ensure_managed(&container_id).await?;
    let command = body
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("ls");
    if command.is_empty() || command.len() > 1000 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid command" })),
        ));
    }

    // Block dangerous commands that could escape the container
    const CONTAINER_BLOCKED: &[&str] = &[
        "mount", "nsenter", "chroot", "/proc/1/", "/proc/sysrq", "docker", "kubectl",
        "unshare", "pivot_root", "setns", "capsh", "mknod", "debugfs", "kexec",
    ];
    let cmd_lower = command.to_lowercase();
    for pattern in CONTAINER_BLOCKED {
        if cmd_lower.contains(pattern) {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("Blocked command: contains '{pattern}'") })),
            ));
        }
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        safe_command("docker")
            .args(["exec", &container_id, "sh", "-c", command])
            .output(),
    )
    .await
    .map_err(|_| {
        (
            StatusCode::GATEWAY_TIMEOUT,
            Json(serde_json::json!({"error": "Command timed out (30s)"})),
        )
    })?
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    Ok(Json(serde_json::json!({
        "success": output.status.success(),
        "stdout": stdout.chars().take(50000).collect::<String>(),
        "stderr": stderr.chars().take(10000).collect::<String>(),
        "exit_code": output.status.code(),
    })))
}

/// GET /apps/{container_id}/volumes — Get volume info and sizes.
async fn container_volumes(
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container ID" })),
        ));
    }
    ensure_managed(&container_id).await?;

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        safe_command("docker")
            .args([
                "inspect",
                "--format",
                "{{range .Mounts}}{{.Source}}|{{.Destination}}|{{.Type}}\n{{end}}",
                &container_id,
            ])
            .output(),
    )
    .await
    .map_err(|_| (StatusCode::GATEWAY_TIMEOUT, Json(serde_json::json!({"error": "Timeout"}))))?
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut volumes = Vec::new();

    for line in stdout.lines() {
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() >= 3 {
            let source = parts[0];
            let dest = parts[1];
            let mount_type = parts[2];

            let du = tokio::time::timeout(
                std::time::Duration::from_secs(15),
                safe_command("du")
                    .args(["-sb", source])
                    .output(),
            ).await;
            let size: u64 = du
                .ok()
                .and_then(|r| r.ok())
                .map(|o| {
                    String::from_utf8_lossy(&o.stdout)
                        .split_whitespace()
                        .next()
                        .unwrap_or("0")
                        .parse()
                        .unwrap_or(0)
                })
                .unwrap_or(0);

            let ls = tokio::time::timeout(
                std::time::Duration::from_secs(15),
                safe_command("ls")
                    .args(["-la", source])
                    .output(),
            ).await;
            let listing = ls
                .ok()
                .and_then(|r| r.ok())
                .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                .unwrap_or_default();

            volumes.push(serde_json::json!({
                "source": source,
                "destination": dest,
                "type": mount_type,
                "size_bytes": size,
                "size_mb": (size as f64 / 1024.0 / 1024.0 * 10.0).round() / 10.0,
                "listing": listing.lines().take(20).collect::<Vec<_>>().join("\n"),
            }));
        }
    }

    Ok(Json(serde_json::json!({ "volumes": volumes })))
}

#[derive(Deserialize)]
struct RegistryLoginRequest {
    server: String,
    username: String,
    password: String,
}

/// POST /apps/registry-login — Login to a private Docker registry.
async fn registry_login(
    Json(body): Json<RegistryLoginRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if body.server.is_empty() || body.username.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Server and username required" })),
        ));
    }
    if !is_valid_image_ref(&body.server) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid registry server" })),
        ));
    }

    // Pass password via stdin to avoid leaking it in process args
    use tokio::io::AsyncWriteExt;
    let mut child = safe_command("docker")
        .args(["login", &body.server, "-u", &body.username, "--password-stdin"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(body.password.as_bytes()).await;
        drop(stdin);
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        child.wait_with_output(),
    )
    .await
    .map_err(|_| {
        (
            StatusCode::GATEWAY_TIMEOUT,
            Json(serde_json::json!({"error": "Login timed out"})),
        )
    })?
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;

    if output.status.success() {
        tracing::info!("Docker registry login: {} @ {}", body.username, body.server);
        Ok(Json(serde_json::json!({ "success": true, "server": body.server })))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": format!("Login failed: {}", stderr.chars().take(200).collect::<String>()) })),
        ))
    }
}

/// GET /apps/registries — List configured registries.
async fn list_registries() -> Json<serde_json::Value> {
    let config_path = "/root/.docker/config.json";
    let content = std::fs::read_to_string(config_path).unwrap_or_default();
    let config: serde_json::Value =
        serde_json::from_str(&content).unwrap_or(serde_json::json!({}));

    let auths = config.get("auths").and_then(|a| a.as_object());
    let servers: Vec<String> = auths
        .map(|a| a.keys().cloned().collect())
        .unwrap_or_default();

    Json(serde_json::json!({ "registries": servers }))
}

/// POST /apps/registry-logout — Logout from a registry.
async fn registry_logout(
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let server = body
        .get("server")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if server.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Server required"})),
        ));
    }
    if !is_valid_image_ref(server) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Invalid registry server"})),
        ));
    }

    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        safe_command("docker")
            .args(["logout", server])
            .output(),
    ).await;
    Ok(Json(serde_json::json!({ "success": true })))
}

/// DELETE /apps/{container_id} — Remove a deployed app and clean up its proxy.
async fn remove(
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container ID" })),
        ));
    }
    ensure_managed(&container_id).await?;

    // Everything the cleanup below needs to PROVE it owns what it deletes, read
    // while the container still exists. The domain and name labels alone are not
    // proof: they say what this app was called, not that the vhost, the cert
    // directory or the data tree of that name are still its own.
    let identity = docker_apps::removal_identity(&container_id).await;
    let domain = identity.domain.clone();

    docker_apps::remove_app(&container_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
        })?;

    let mut response = serde_json::json!({ "success": true });

    // Clean up proxy config (nginx + Traefik) + SSL certs if domain was set.
    //
    // Every delete here is now conditional on the resource saying it belongs to
    // this app. Before, all of them fired on the label alone — so removing an
    // app whose domain a site had since taken over destroyed the SITE's vhost,
    // certificates and logs, and nothing said so.
    if let Some(ref domain) = domain {
        response["domain_removed"] = serde_json::json!(domain);
        unexpose_domain(domain, identity.host_port, &mut response).await;
    }

    // Clean up persistent volume data — only when this container actually
    // bind-mounts out of that directory. The name label is not proof: a compose
    // service carries a caller-supplied `container_name` in the same label while
    // its own data lives under /var/lib/dockpanel/compose, so removing it used
    // to delete the identically-named template app's entire data tree.
    // The directory comes from the container's OWN binds, never from its name label.
    // Rebuilding it from the label missed anything the v2.111.0-v2.113.3 migration had
    // moved — that wrote under a directory named for the CONTAINER — so a migrated app's
    // data survived its own deletion, and silently: the label-derived path did not exist,
    // so even the warning below never printed.
    match (identity.app_dir.as_ref(), identity.name.as_ref()) {
        (Some(volume_dir), _) if std::path::Path::new(volume_dir).exists() => {
            std::fs::remove_dir_all(volume_dir).ok();
            tracing::info!("Volume cleanup: removed {volume_dir}");
        }
        (None, Some(name)) => {
            let label_dir = format!("{}/{}", docker_apps::APP_DATA_DIR, name);
            if std::path::Path::new(&label_dir).exists() {
                tracing::warn!(
                    "Volume cleanup: LEAVING {label_dir} in place — this container \
                     had no bind mount under it, so the directory belongs to a \
                     different app."
                );
            }
        }
        _ => {}
    }

    Ok(Json(response))
}

#[derive(Deserialize)]
struct ComposeParseRequest {
    yaml: String,
    stack_id: Option<String>,
    /// Optional domain to put in front of the stack (deploy only).
    domain: Option<String>,
    /// Email for Let's Encrypt SSL (requires domain).
    ssl_email: Option<String>,
    /// "none" | "acme" | "provided". Absent = legacy inference from ssl_email (an older panel).
    #[serde(default)]
    tls_mode: Option<String>,
    /// Registry alias; provided mode only.
    #[serde(default)]
    tls_certificate: Option<String>,
    /// Host port the domain proxies to. Defaults to the first published port
    /// in the stack, which is the right answer for every package we ship.
    expose_port: Option<u16>,
}

/// POST /apps/compose/parse — Parse docker-compose.yml and return services preview.
async fn compose_parse(
    Json(body): Json<ComposeParseRequest>,
) -> Result<Json<Vec<compose::ComposeService>>, (StatusCode, Json<serde_json::Value>)> {
    let services = compose::parse_compose(&body.yaml, body.stack_id.as_deref()).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e })),
        )
    })?;

    Ok(Json(services))
}

/// POST /apps/compose/validate — Validate compose YAML with detailed feedback.
async fn compose_validate(
    Json(body): Json<ComposeParseRequest>,
) -> Json<serde_json::Value> {
    let mut errors: Vec<serde_json::Value> = Vec::new();
    let mut warnings: Vec<serde_json::Value> = Vec::new();
    let mut info: Vec<serde_json::Value> = Vec::new();

    // Try to parse
    match compose::parse_compose(&body.yaml, body.stack_id.as_deref()) {
        Ok(services) => {
            info.push(serde_json::json!({
                "message": format!("{} service(s) found", services.len()),
            }));

            for svc in &services {
                // Check for latest tag
                if svc.image.ends_with(":latest") || !svc.image.contains(':') {
                    warnings.push(serde_json::json!({
                        "service": svc.key,
                        "message": "Using ':latest' tag — pin to a specific version for reproducible deploys",
                    }));
                }

                // Check for exposed privileged ports
                for port in &svc.ports {
                    if port.host < 1024 && port.host != 80 && port.host != 443 {
                        warnings.push(serde_json::json!({
                            "service": svc.key,
                            "message": format!("Privileged port {} — consider using a higher port", port.host),
                        }));
                    }
                }

                // Check for missing volumes on databases
                let db_images = ["postgres", "mysql", "mariadb", "mongo", "redis"];
                if db_images.iter().any(|db| svc.image.contains(db)) && svc.volumes.is_empty() {
                    warnings.push(serde_json::json!({
                        "service": svc.key,
                        "message": "Database service without volumes — data will be lost on container restart",
                    }));
                }

                // Check for missing restart policy
                if svc.restart.is_empty() || svc.restart == "no" {
                    info.push(serde_json::json!({
                        "service": svc.key,
                        "message": "No restart policy — container won't auto-restart. Consider 'unless-stopped'",
                    }));
                }

                // Check for missing health check env vars
                if svc.environment.is_empty() && db_images.iter().any(|db| svc.image.contains(db)) {
                    warnings.push(serde_json::json!({
                        "service": svc.key,
                        "message": "Database without environment variables — password/root may use defaults",
                    }));
                }
            }
        }
        Err(e) => {
            errors.push(serde_json::json!({
                "message": e,
            }));
        }
    }

    // YAML syntax check
    if body.yaml.contains('\t') {
        warnings.push(serde_json::json!({
            "message": "YAML contains tabs — use spaces for indentation to avoid parse errors",
        }));
    }

    Json(serde_json::json!({
        "valid": errors.is_empty(),
        "errors": errors,
        "warnings": warnings,
        "info": info,
    }))
}

/// POST /apps/compose/deploy — Deploy services from parsed compose file,
/// optionally behind a domain with a certificate.
async fn compose_deploy(
    State(state): State<AppState>,
    Json(body): Json<ComposeParseRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let services = compose::parse_compose(&body.yaml, body.stack_id.as_deref()).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e })),
        )
    })?;

    if let Some(ref domain) = body.domain {
        if !is_valid_domain(domain) {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "Invalid domain format" })),
            ));
        }
    }

    // The TLS shape is decided here for the same reason the port is resolved
    // below: a request the agent cannot honour is refused BEFORE any container
    // runs, not reported as a warning beside a running one.
    // Compose stacks have no `use_traefik` field of their own (matching the
    // literal `false` this handler already passes to `expose_domain` below) —
    // a stack can never be Traefik-routed, so this is always the nginx arm.
    let tls = TlsIntent::from_request(
        body.tls_mode.as_deref(),
        body.ssl_email.as_deref(),
        body.tls_certificate.as_deref(),
        false,
    )
    .map_err(|e| (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": e }))))?;

    // Resolve which published port the domain should point at before deploying,
    // so a stack that publishes nothing is refused up front rather than after
    // its containers are running.
    let expose_port = match body.domain.as_ref() {
        None => None,
        Some(_) => {
            let port = body
                .expose_port
                .or_else(|| services.iter().find_map(|s| s.ports.first().map(|p| p.host)));
            match port {
                Some(p) => Some(p),
                None => {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({
                            "error": "This stack publishes no host port, so there is nothing for the domain to point at. \
                                      Add a 'ports:' entry to the service you want reachable."
                        })),
                    ))
                }
            }
        }
    };

    let result = compose::deploy_compose(&services, body.stack_id.as_deref()).await;
    let mut response = serde_json::to_value(&result).unwrap_or_else(|_| serde_json::json!({}));

    // Only front a stack that actually came up. Writing a vhost to a dead
    // upstream would hand the operator a 502 and a certificate for it.
    if let (Some(domain), Some(port)) = (body.domain.as_ref(), expose_port) {
        let running = result.services.iter().any(|s| s.status == "running");
        if running {
            expose_domain(
                &state.templates,
                domain,
                port,
                tls,
                false,
                &mut response,
            )
            .await;
        } else {
            response["proxy_warning"] = serde_json::json!(
                "No service in the stack stayed running, so no vhost was written. \
                 Fix the stack and re-apply the domain."
            );
        }
    }

    Ok(Json(response))
}

#[derive(Deserialize)]
struct StackActionRequest {
    stack_id: String,
    action: String,
    /// The domain this stack was exposed on, so `remove` can take the vhost and
    /// certificates down with it. Absent for a stack that was never exposed.
    domain: Option<String>,
}

/// POST /apps/stack/action — Perform a lifecycle action on all containers in a stack.
async fn stack_action(
    Json(body): Json<StackActionRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !["start", "stop", "restart", "remove"].contains(&body.action.as_str()) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Action must be start, stop, restart, or remove" })),
        ));
    }

    // Find all containers with this stack_id
    let apps = docker_apps::list_deployed_apps().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
    })?;

    let stack_containers: Vec<&docker_apps::DeployedApp> = apps
        .iter()
        .filter(|a| a.stack_id.as_deref() == Some(&body.stack_id))
        .collect();

    if stack_containers.is_empty() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "No containers found for this stack" })),
        ));
    }

    // Read the published port while the containers still exist — it is what
    // proves the vhost is this stack's before anything deletes it. Sourced
    // from `inspect_container`'s HostConfig (via `inspect_host_port`), the
    // same primitive `removal_identity` already uses for single-app removal —
    // NOT `DeployedApp.port` (`stack_containers`' own field), which reflects
    // Docker's LIVE container listing and reads empty the moment a stack is
    // stopped ("parked"), permanently stranding its vhost and certificate the
    // first time `remove` runs against it.
    let mut stack_port = None;
    for app in &stack_containers {
        if let Some(p) = docker_apps::inspect_host_port(&app.container_id).await {
            stack_port = Some(p);
            break;
        }
    }

    let mut results = Vec::new();
    for app in &stack_containers {
        let cid = &app.container_id;
        let result = match body.action.as_str() {
            "start" => docker_apps::start_app(cid).await.map(|_| "started"),
            "stop" => docker_apps::stop_app(cid).await.map(|_| "stopped"),
            "restart" => docker_apps::restart_app(cid).await.map(|_| "restarted"),
            "remove" => docker_apps::remove_app(cid).await.map(|_| "removed"),
            _ => unreachable!(),
        };
        results.push(serde_json::json!({
            "container_id": cid,
            "name": app.name,
            "status": match &result {
                Ok(s) => *s,
                Err(_) => "failed",
            },
            "error": result.err(),
        }));
    }

    let mut response = serde_json::json!({
        "stack_id": body.stack_id,
        "action": body.action,
        "results": results,
    });

    if body.action == "remove" {
        // The stack's own bridge outlives its last container, so tear it down
        // with them. `remove_stack_network` refuses anything it cannot prove
        // is ours.
        compose::remove_stack_network(Some(&body.stack_id)).await;

        // And the vhost, certificates and logs — through the same guarded
        // teardown an app removal uses, so a stack cannot delete a domain that
        // has since been taken over by a site.
        if let Some(ref domain) = body.domain {
            if is_valid_domain(domain) {
                response["domain_removed"] = serde_json::json!(domain);
                unexpose_domain(domain, stack_port, &mut response).await;
            }
        }
    }

    Ok(Json(response))
}

/// GET /apps/images — List Docker images.
async fn list_images() -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        safe_command("docker")
            .args(["images", "--format", "{{.Repository}}|{{.Tag}}|{{.ID}}|{{.Size}}|{{.CreatedSince}}", "--no-trunc"])
            .output(),
    ).await
        .map_err(|_| (StatusCode::GATEWAY_TIMEOUT, Json(serde_json::json!({"error": "Timeout listing images"}))))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let images: Vec<serde_json::Value> = stdout.lines().filter(|l| !l.is_empty()).map(|l| {
        let parts: Vec<&str> = l.split('|').collect();
        serde_json::json!({
            "repository": parts.first().unwrap_or(&""),
            "tag": parts.get(1).unwrap_or(&""),
            "id": parts.get(2).unwrap_or(&""),
            "size": parts.get(3).unwrap_or(&""),
            "created": parts.get(4).unwrap_or(&""),
        })
    }).collect();

    Ok(Json(serde_json::json!({ "images": images })))
}

/// POST /apps/images/prune — Remove unused Docker images.
async fn prune_images_all() -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        safe_command("docker")
            .args(["image", "prune", "-af"])
            .output(),
    ).await
        .map_err(|_| (StatusCode::GATEWAY_TIMEOUT, Json(serde_json::json!({"error": "Image prune timed out (120s)"}))))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(Json(serde_json::json!({ "success": true, "output": stdout.trim() })))
}

/// DELETE /apps/images/{id} — Remove a specific Docker image.
async fn remove_image(Path(id): Path<String>) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // Validate image ID: alphanumeric + : / . - _ only
    let is_valid = !id.is_empty()
        && id.len() <= 256
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == ':' || c == '/' || c == '.' || c == '-' || c == '_');
    if !is_valid {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Invalid image ID"}))));
    }

    // Strip sha256: prefix if present (docker images --no-trunc includes it)
    let image_ref = if id.starts_with("sha256:") { &id } else { &id };
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        safe_command("docker")
            .args(["rmi", image_ref])
            .output(),
    ).await
        .map_err(|_| (StatusCode::GATEWAY_TIMEOUT, Json(serde_json::json!({"error": "Image removal timed out (60s)"}))))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err((StatusCode::CONFLICT, Json(serde_json::json!({"error": format!("Cannot remove: {}", stderr.chars().take(200).collect::<String>())}))));
    }
    Ok(Json(serde_json::json!({ "success": true })))
}

#[derive(Deserialize)]
struct SnapshotRequest {
    tag: Option<String>,
}

/// POST /apps/{container_id}/snapshot — Commit container to image.
async fn snapshot_container(
    Path(container_id): Path<String>,
    Json(body): Json<SnapshotRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "Invalid container ID" }))));
    }
    ensure_managed(&container_id).await?;

    let tag = {
        let raw = body.tag.unwrap_or_else(|| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            now.to_string()
        });
        // Force-namespace with dockpanel-snapshot: prefix to prevent overwriting system images
        let suffix = raw.strip_prefix("dockpanel-snapshot:").unwrap_or(&raw);
        // Sanitise the suffix: only allow alphanumeric, -, _, .
        let safe_suffix: String = suffix.chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
            .take(128)
            .collect();
        if safe_suffix.is_empty() {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            format!("dockpanel-snapshot:{}", now)
        } else {
            format!("dockpanel-snapshot:{}", safe_suffix)
        }
    };

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        safe_command("docker")
            .args(["commit", &container_id, &tag])
            .output()
    ).await
        .map_err(|_| (StatusCode::GATEWAY_TIMEOUT, Json(serde_json::json!({"error": "Snapshot timed out"}))))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": format!("Snapshot failed: {stderr}")}))));
    }

    let image_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    tracing::info!("Container snapshot: {container_id} → {tag} ({image_id})");

    Ok(Json(serde_json::json!({ "success": true, "tag": tag, "image_id": image_id })))
}

/// Validate that an image reference contains only safe characters.
fn is_valid_image_ref(image: &str) -> bool {
    !image.is_empty()
        && image.len() <= 256
        && image.chars().all(|c| c.is_ascii_alphanumeric() || c == '/' || c == ':' || c == '.' || c == '-' || c == '_' || c == '@')
        && !image.starts_with('-')
}

/// POST /apps/{container_id}/change-image — Change a container's image tag.
/// Recreates the container with the new image while PRESERVING its full runtime config
/// (caps, security_opt, port bindings, restart, resource limits, GPU, labels, env, volumes)
/// via `docker_apps::change_container_image` — see that fn for the s237 rationale.
async fn change_image(
    Path(container_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Invalid container ID"}))));
    }
    ensure_managed(&container_id).await?;

    let image = body.get("image").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if image.is_empty() {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "image is required"}))));
    }

    if !is_valid_image_ref(&image) {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Invalid image reference: must be <= 256 chars, alphanumeric with / : . - _ @ only, and not start with -"}))));
    }

    let new_id = docker_apps::change_container_image(&container_id, &image)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e}))))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "container_id": new_id,
        "image": image,
    })))
}

/// POST /apps/{container_id}/update-limits — Update CPU/memory limits on a running container.
async fn update_container_limits(
    Path(container_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Invalid container ID"}))));
    }
    ensure_managed(&container_id).await?;

    let memory_mb = body.get("memory_mb").and_then(|v| v.as_u64());
    let cpu_percent = body.get("cpu_percent").and_then(|v| v.as_u64());

    let mut args = vec!["update".to_string()];

    if let Some(mem) = memory_mb {
        args.push(format!("--memory={}m", mem));
        args.push(format!("--memory-swap={}m", mem * 2)); // swap = 2x memory
    }

    if let Some(cpu) = cpu_percent {
        // cpu_percent maps to --cpus (100% = 1.0 CPU)
        let cpus = cpu as f64 / 100.0;
        args.push(format!("--cpus={:.2}", cpus));
    }

    args.push(container_id.clone());

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        safe_command("docker")
            .args(&args)
            .output(),
    ).await
        .map_err(|_| (StatusCode::GATEWAY_TIMEOUT, Json(serde_json::json!({"error": "Timeout"}))))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": format!("docker update failed: {stderr}")}))));
    }

    tracing::info!("Container limits updated: {container_id} (mem: {:?}MB, cpu: {:?}%)", memory_mb, cpu_percent);

    Ok(Json(serde_json::json!({
        "success": true,
        "memory_mb": memory_mb,
        "cpu_percent": cpu_percent,
    })))
}

/// GET /apps/update-check — Check all managed containers for available image updates.
async fn update_check() -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    match docker_apps::check_image_updates().await {
        Ok(results) => {
            let count = results.iter().filter(|r| r.update_available).count();
            Ok(Json(serde_json::json!({
                "updates": results,
                "updates_available": count,
            })))
        }
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e})))),
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/apps/templates", get(templates))
        .route("/apps/update-check", get(update_check))
        .route("/apps/deploy", post(deploy))
        .route("/apps/compose/parse", post(compose_parse))
        .route("/apps/compose/validate", post(compose_validate))
        .route("/apps/compose/deploy", post(compose_deploy))
        .route("/apps/stack/action", post(stack_action))
        .route("/apps/registries", get(list_registries))
        .route("/apps/registry-login", post(registry_login))
        .route("/apps/registry-logout", post(registry_logout))
        .route("/apps/images", get(list_images))
        .route("/apps/images/prune", post(prune_images_all))
        .route("/apps/images/{id}", delete(remove_image))
        .route("/apps", get(list))
        .route("/apps/{container_id}", delete(remove))
        .route("/apps/{container_id}/stop", post(stop))
        .route("/apps/{container_id}/start", post(start))
        .route("/apps/{container_id}/restart", post(restart))
        .route("/apps/{container_id}/logs", get(logs))
        .route("/apps/{container_id}/env", get(get_env).put(update_env))
        .route("/apps/{container_id}/update", post(update))
        .route("/apps/{container_id}/stats", get(container_stats))
        .route("/apps/{container_id}/shell-info", get(shell_info))
        .route("/apps/{container_id}/exec", post(exec_command))
        .route("/apps/{container_id}/volumes", get(container_volumes))
        .route("/apps/{container_id}/snapshot", post(snapshot_container))
        .route("/apps/{container_id}/change-image", post(change_image))
        .route("/apps/{container_id}/update-limits", post(update_container_limits))
        .route("/apps/gpu-info", get(gpu_info))
        .route("/apps/{container_id}/ollama/models", get(ollama_list_models))
        .route("/apps/{container_id}/ollama/pull", post(ollama_pull_model))
        .route("/apps/{container_id}/ollama/delete", post(ollama_delete_model))
}

// ─── Ollama Model Management ────────────────────────────────────────────

/// GET /apps/{container_id}/ollama/models — List models installed in an Ollama container.
async fn ollama_list_models(
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "Invalid container ID" }))));
    }
    ensure_managed(&container_id).await?;

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        safe_command("docker")
            .args(["exec", &container_id, "ollama", "list"])
            .output(),
    )
    .await
    .map_err(|_| (StatusCode::GATEWAY_TIMEOUT, Json(serde_json::json!({"error": "Timed out listing models"}))))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Ok(Json(serde_json::json!({ "models": [], "error": stderr.trim() })));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let models: Vec<serde_json::Value> = stdout
        .lines()
        .skip(1) // skip header row
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            serde_json::json!({
                "name": parts.first().unwrap_or(&""),
                "id": parts.get(1).unwrap_or(&""),
                "size": parts.get(2).map(|s| format!("{} {}", s, parts.get(3).unwrap_or(&""))).unwrap_or_default(),
                "modified": parts.get(4..).map(|p| p.join(" ")).unwrap_or_default(),
            })
        })
        .collect();

    Ok(Json(serde_json::json!({ "models": models })))
}

/// POST /apps/{container_id}/ollama/pull — Pull a model into an Ollama container.
async fn ollama_pull_model(
    Path(container_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "Invalid container ID" }))));
    }
    ensure_managed(&container_id).await?;

    let model = body.get("model").and_then(|v| v.as_str()).unwrap_or("").trim();
    if model.is_empty() || model.len() > 200 || model.starts_with('-') {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "Invalid model name" }))));
    }

    // Validate model name: alphanumeric, hyphens, underscores, colons, slashes, dots
    if !model.chars().all(|c| c.is_alphanumeric() || "-_:/.".contains(c)) {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "Invalid model name characters" }))));
    }

    // ollama pull can take a long time for large models — 10 minute timeout
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(600),
        safe_command("docker")
            .args(["exec", &container_id, "ollama", "pull", model])
            .output(),
    )
    .await
    .map_err(|_| (StatusCode::GATEWAY_TIMEOUT, Json(serde_json::json!({"error": "Model pull timed out (10m). Try a smaller model or pull manually."}))))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    Ok(Json(serde_json::json!({
        "success": output.status.success(),
        "stdout": stdout.chars().take(50000).collect::<String>(),
        "stderr": stderr.chars().take(10000).collect::<String>(),
    })))
}

/// POST /apps/{container_id}/ollama/delete — Remove a model from an Ollama container.
async fn ollama_delete_model(
    Path(container_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "Invalid container ID" }))));
    }
    ensure_managed(&container_id).await?;

    let model = body.get("model").and_then(|v| v.as_str()).unwrap_or("").trim();
    if model.is_empty() || model.len() > 200 || model.starts_with('-') {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "Invalid model name" }))));
    }
    if !model.chars().all(|c| c.is_alphanumeric() || "-_:/.".contains(c)) {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "Invalid model name characters" }))));
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        safe_command("docker")
            .args(["exec", &container_id, "ollama", "rm", model])
            .output(),
    )
    .await
    .map_err(|_| (StatusCode::GATEWAY_TIMEOUT, Json(serde_json::json!({"error": "Timed out"}))))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    Ok(Json(serde_json::json!({
        "success": output.status.success(),
        "message": if output.status.success() { format!("Deleted {model}") } else { String::from_utf8_lossy(&output.stderr).to_string() },
    })))
}

/// GET /apps/gpu-info — Full GPU monitoring: utilization, VRAM, temperature, power, per-process usage.
async fn gpu_info() -> Json<serde_json::Value> {
    // Query comprehensive GPU metrics in one call
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        crate::safe_cmd::safe_command("nvidia-smi")
            .args([
                "--query-gpu=index,name,memory.total,memory.used,memory.free,utilization.gpu,utilization.memory,temperature.gpu,power.draw,power.limit,fan.speed,driver_version,pstate",
                "--format=csv,noheader,nounits"
            ])
            .output()
    ).await;

    let gpu_output = match output {
        Ok(Ok(out)) if out.status.success() => Some(out),
        _ => None,
    };

    let Some(gpu_out) = gpu_output else {
        return Json(serde_json::json!({
            "available": false,
            "gpus": [],
            "gpu_count": 0,
            "nvidia_toolkit_installed": false,
            "processes": [],
        }));
    };

    let stdout = String::from_utf8_lossy(&gpu_out.stdout);
    let gpus: Vec<serde_json::Value> = stdout.lines().filter(|l| !l.trim().is_empty()).map(|line| {
        // Split on ", " — GPU names can theoretically contain commas, so we parse
        // index (first field) and the 11 numeric/string fields from the right,
        // treating everything in between as the GPU name.
        let p: Vec<&str> = line.split(", ").collect();
        if p.len() >= 13 {
            // Normal case: exactly 13 fields (index + name + 11 metrics)
            let parse_u64 = |idx: usize| p.get(idx).and_then(|v| v.trim().parse::<u64>().ok());
            let _parse_f64 = |idx: usize| p.get(idx).and_then(|v| v.trim().parse::<f64>().ok());
            let _str_val = |idx: usize| p.get(idx).map(|v| v.trim()).unwrap_or("");
            // If there are extra commas (in GPU name), join the excess back into the name
            let name_end = p.len() - 11; // 11 fields after name
            let name = p[1..name_end].join(", ");
            serde_json::json!({
                "index": parse_u64(0).unwrap_or(0),
                "name": name.trim(),
                "memory_total_mb": p.get(name_end).and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(0),
                "memory_used_mb": p.get(name_end + 1).and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(0),
                "memory_free_mb": p.get(name_end + 2).and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(0),
                "utilization_gpu_pct": p.get(name_end + 3).and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(0),
                "utilization_memory_pct": p.get(name_end + 4).and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(0),
                "temperature_c": p.get(name_end + 5).and_then(|v| v.trim().parse::<u64>().ok()),
                "power_draw_w": p.get(name_end + 6).and_then(|v| v.trim().parse::<f64>().ok()),
                "power_limit_w": p.get(name_end + 7).and_then(|v| v.trim().parse::<f64>().ok()),
                "fan_speed_pct": p.get(name_end + 8).and_then(|v| v.trim().parse::<u64>().ok()),
                "driver_version": p.get(name_end + 9).map(|v| v.trim()).unwrap_or(""),
                "performance_state": p.get(name_end + 10).map(|v| v.trim()).unwrap_or(""),
            })
        } else {
            // Fallback: fewer fields than expected
            serde_json::json!({
                "index": p.first().and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(0),
                "name": p.get(1).unwrap_or(&"Unknown").trim(),
                "memory_total_mb": 0, "memory_used_mb": 0, "memory_free_mb": 0,
                "utilization_gpu_pct": 0, "utilization_memory_pct": 0,
                "temperature_c": null, "power_draw_w": null, "power_limit_w": null,
                "fan_speed_pct": null, "driver_version": "", "performance_state": "",
            })
        }
    }).collect();

    // Query per-process GPU usage (which PIDs are using which GPU and how much VRAM)
    let proc_output = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        crate::safe_cmd::safe_command("nvidia-smi")
            .args([
                "--query-compute-apps=pid,gpu_uuid,used_gpu_memory,name",
                "--format=csv,noheader,nounits"
            ])
            .output()
    ).await;

    let mut processes: Vec<serde_json::Value> = Vec::new();
    if let Ok(Ok(out)) = proc_output {
        if out.status.success() {
            let stdout = String::from_utf8_lossy(&out.stdout);
            for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
                let p: Vec<&str> = line.split(", ").collect();
                let pid = p.first().and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(0);

                // Try to resolve PID to a Docker container name
                let container_name = resolve_pid_to_container(pid).await;

                processes.push(serde_json::json!({
                    "pid": pid,
                    "gpu_uuid": p.get(1).map(|v| v.trim()).unwrap_or(""),
                    "vram_used_mb": p.get(2).and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(0),
                    "process_name": p.get(3).map(|v| v.trim()).unwrap_or(""),
                    "container_name": container_name,
                }));
            }
        }
    }

    // Check if NVIDIA Container Toolkit is installed
    let toolkit = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        crate::safe_cmd::safe_command("nvidia-container-cli")
            .arg("--version")
            .output()
    ).await
        .ok()
        .and_then(|r| r.ok())
        .map(|o| o.status.success())
        .unwrap_or(false);

    Json(serde_json::json!({
        "available": true,
        "gpus": gpus,
        "gpu_count": gpus.len(),
        "nvidia_toolkit_installed": toolkit,
        "processes": processes,
    }))
}

/// Resolve a host PID to a Docker container name (if it belongs to one).
async fn resolve_pid_to_container(pid: u64) -> Option<String> {
    // Read the cgroup of the process to find its container ID
    let cgroup = tokio::fs::read_to_string(format!("/proc/{pid}/cgroup")).await.ok()?;
    // Docker cgroup paths contain the container ID (64-char hex)
    let container_id = cgroup.lines()
        .filter_map(|line| {
            // Format: "0::/docker/<container_id>" or "0::/system.slice/docker-<id>.scope"
            let after_docker = line.split("/docker/").nth(1)
                .or_else(|| line.split("/docker-").nth(1));
            after_docker.map(|s| s.trim_end_matches(".scope").chars().take(12).collect::<String>())
        })
        .find(|id| id.len() >= 12)?;

    // Use docker inspect to get the container name
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        crate::safe_cmd::safe_command("docker")
            .args(["inspect", "--format", "{{.Name}}", &container_id])
            .output()
    ).await.ok()?.ok()?;

    if output.status.success() {
        let name = String::from_utf8_lossy(&output.stdout).trim().trim_start_matches('/').to_string();
        if !name.is_empty() { Some(name) } else { None }
    } else {
        None
    }
}

#[cfg(test)]
mod tls_intent_tests {
    use super::TlsIntent;

    #[test]
    fn an_older_panel_that_sends_no_mode_keeps_the_address_rule() {
        // Presence, not content: exactly what the inference did before.
        assert_eq!(
            TlsIntent::from_request(None, Some("ops@example.com"), None, false),
            Ok(TlsIntent::Acme { email: "ops@example.com" })
        );
        assert_eq!(TlsIntent::from_request(None, None, None, false), Ok(TlsIntent::None));
        // An alias without a mode is ignored, as an older panel could never send one.
        assert_eq!(TlsIntent::from_request(None, None, Some("wild"), false), Ok(TlsIntent::None));
    }

    #[test]
    fn each_mode_needs_its_own_field() {
        assert_eq!(TlsIntent::from_request(Some("none"), Some("x@y.z"), None, false), Ok(TlsIntent::None));
        assert_eq!(
            TlsIntent::from_request(Some("acme"), Some("x@y.z"), None, false),
            Ok(TlsIntent::Acme { email: "x@y.z" })
        );
        assert!(TlsIntent::from_request(Some("acme"), None, None, false).is_err());
        assert!(TlsIntent::from_request(Some("acme"), Some("   "), None, false).is_err());
        assert_eq!(
            TlsIntent::from_request(Some("provided"), None, Some("wildcard-2026"), false),
            Ok(TlsIntent::Provided { alias: "wildcard-2026" })
        );
        assert!(TlsIntent::from_request(Some("provided"), None, None, false).is_err());
        // The alias is a directory name on this box; the grammar is enforced here too.
        assert!(TlsIntent::from_request(Some("provided"), None, Some("../ssl"), false).is_err());
        assert!(TlsIntent::from_request(Some("provided"), None, Some("Upper"), false).is_err());
        assert!(TlsIntent::from_request(Some("letsencrypt"), Some("x@y.z"), None, false).is_err());
    }

    /// Traefik's file provider has no per-route certificate form — refused at
    /// the front door now, before `deploy_app` ever creates a container,
    /// rather than discovered afterward inside `expose_domain`'s Traefik
    /// branch (a running container plus a warning, the exact outcome this
    /// function exists to prevent for every other bad combination).
    #[test]
    fn provided_mode_is_refused_through_traefik_before_any_container_exists() {
        assert!(TlsIntent::from_request(Some("provided"), None, Some("wildcard-2026"), true).is_err());
        // The control: the identical alias, nginx-routed, still succeeds — this
        // is a Traefik-specific refusal, not a blanket break of "provided".
        assert_eq!(
            TlsIntent::from_request(Some("provided"), None, Some("wildcard-2026"), false),
            Ok(TlsIntent::Provided { alias: "wildcard-2026" })
        );
        // Other modes are unaffected by use_traefik.
        assert_eq!(TlsIntent::from_request(Some("none"), None, None, true), Ok(TlsIntent::None));
        assert_eq!(
            TlsIntent::from_request(Some("acme"), Some("x@y.z"), None, true),
            Ok(TlsIntent::Acme { email: "x@y.z" })
        );
    }

    #[test]
    fn the_answer_uses_the_shared_vocabulary() {
        assert_eq!(TlsIntent::None.mode(), "none");
        assert_eq!(TlsIntent::Acme { email: "x@y.z" }.mode(), "acme");
        assert_eq!(TlsIntent::Provided { alias: "a" }.mode(), "provided");
    }
}
