use crate::safe_cmd::{safe_command, safe_command_sync};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use super::{is_valid_domain, AppState};
use crate::services;
use crate::services::nginx::{restore_note, restore_or_remove};
use crate::services::ssl;

#[derive(Deserialize)]
pub struct SiteConfig {
    /// Site runtime: "static", "php", "proxy"
    pub runtime: String,
    /// Document root (for static/PHP) relative to site dir
    pub root: Option<String>,
    /// Upstream port (for proxy/Docker sites)
    pub proxy_port: Option<u16>,
    /// PHP-FPM socket path (for PHP sites)
    pub php_socket: Option<String>,
    /// Whether SSL is enabled
    pub ssl: Option<bool>,
    /// SSL certificate path
    pub ssl_cert: Option<String>,
    /// SSL key path
    pub ssl_key: Option<String>,
    /// Rate limit: requests per second per IP (None = no limit)
    pub rate_limit: Option<u32>,
    /// Max upload body size in MB
    pub max_upload_mb: Option<u32>,
    /// PHP memory_limit in MB (for PHP-FPM pool config)
    pub php_memory_mb: Option<u32>,
    /// PHP-FPM pm.max_children
    pub php_max_workers: Option<u32>,
    /// Custom nginx directives injected into server block
    pub custom_nginx: Option<String>,
    /// PHP framework preset: "laravel", "wordpress", "drupal", "joomla", "symfony", "codeigniter", "magento", "generic"
    pub php_preset: Option<String>,
    /// App start command (for node/python runtimes)
    #[serde(default)]
    pub app_command: Option<String>,
    /// Enable FastCGI cache for PHP sites
    #[serde(default)]
    pub fastcgi_cache: Option<bool>,
    /// Enable Redis object cache for PHP sites
    #[serde(default)]
    pub redis_cache: Option<bool>,
    /// Redis DB number (0-15) for per-site isolation
    #[serde(default)]
    pub redis_db: Option<i32>,
    /// Enable WAF (ModSecurity) for this site
    #[serde(default)]
    pub waf_enabled: Option<bool>,
    /// WAF mode: "detection" (log only) or "prevention" (block)
    #[serde(default)]
    pub waf_mode: Option<String>,
    /// Content-Security-Policy header value
    #[serde(default)]
    pub csp_policy: Option<String>,
    /// Permissions-Policy header value
    #[serde(default)]
    pub permissions_policy: Option<String>,
    /// Bot protection mode: "off", "rate-limit", "challenge", "block"
    #[serde(default)]
    pub bot_protection: Option<String>,
}

#[derive(Serialize)]
struct NginxResponse {
    success: bool,
    message: String,
}

#[derive(Serialize)]
struct NginxTestResponse {
    success: bool,
    output: String,
}

#[derive(Serialize)]
struct SiteStatusResponse {
    domain: String,
    config_exists: bool,
    ssl_enabled: bool,
    ssl_cert_path: Option<String>,
    ssl_expiry: Option<String>,
}

/// PUT /nginx/sites/:domain — Create or update site nginx config.
async fn put_site(
    State(state): State<AppState>,
    Path(domain): Path<String>,
    Json(mut config): Json<SiteConfig>,
) -> Result<Json<NginxResponse>, (StatusCode, Json<NginxResponse>)> {
    // Validate domain format
    if !is_valid_domain(&domain) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(NginxResponse {
                success: false,
                message: "Invalid domain format".into(),
            }),
        ));
    }

    // Create app service for node/python runtimes
    if config.runtime == "node" || config.runtime == "python" {
        if let (Some(cmd), Some(port)) = (&config.app_command, config.proxy_port) {
            if let Err(e) = services::app_process::create_app_service(
                &domain, cmd, port, &config.runtime,
            ) {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(NginxResponse {
                        success: false,
                        message: format!("Failed to create app service: {e}"),
                    }),
                ));
            }
        }
    }

    // Validate php_socket path
    if let Some(ref socket) = config.php_socket {
        let socket_path = socket.strip_prefix("unix:").unwrap_or(socket);
        if !socket_path.starts_with("/run/php/") || socket_path.contains("..") {
            return Err((StatusCode::BAD_REQUEST, Json(NginxResponse {
                success: false,
                message: "PHP socket must be under /run/php/".into(),
            })));
        }
    }

    // Check PHP-FPM socket exists before creating a PHP site
    if config.runtime == "php" {
        if let Some(ref socket) = config.php_socket {
            // Extract socket path (e.g., "unix:/run/php/php8.4-fpm.sock" → "/run/php/php8.4-fpm.sock")
            let socket_path = socket.strip_prefix("unix:").unwrap_or(socket);
            // Extract version to resolve against, not to check literally: the panel
            // always sends the Debian-shaped path above as the wire format, but the
            // REAL socket on a RHEL-family host is a different, unversioned path
            // (`/run/php-fpm/www.sock`) that this literal `Path::exists()` could
            // never see — every PHP site creation on RHEL was rejected as "not
            // installed" even with PHP-FPM correctly installed and running.
            // `resolve_php_fpm_socket` is the same resolver `php.rs::settle()`
            // already uses, so the two routes can't disagree on what's real.
            let version = socket_path
                .strip_prefix("/run/php/php")
                .and_then(|s| s.strip_suffix("-fpm.sock"))
                .unwrap_or("unknown");
            match crate::services::pkg::resolve_php_fpm_socket(version).await {
                Some(real_socket) => config.php_socket = Some(format!("unix:{real_socket}")),
                None => {
                    // Where this used to send people was a dead end. Settings →
                    // Services has one PHP tile with no version on it, and it
                    // reports installed if ANY version is — so on a box with 8.4
                    // the operator was told to go and install 8.3 on a screen
                    // showing "Installed / Uninstall" and no install button at all.
                    // The PHP Version control is the one place that can do this,
                    // and since v2.49.0 it offers the install itself.
                    return Err((
                        StatusCode::BAD_REQUEST,
                        Json(NginxResponse {
                            success: false,
                            message: format!(
                                "PHP {version} is not installed, or its PHP-FPM service is not \
                                 running. Install it from the PHP Version control on the site — \
                                 it offers any version this server does not have yet."
                            ),
                        }),
                    ));
                }
            }
        }

        // Write PHP-FPM pool config if PHP site with resource limits
        let mut per_site_socket: Option<String> = None;
        if let Some(ref socket) = config.php_socket {
            // Extract PHP version from socket path (e.g., "unix:/run/php/php8.4-fpm.sock" → "8.4")
            if let Some(ver) = socket.strip_prefix("unix:/run/php/php").and_then(|s| s.strip_suffix("-fpm.sock")) {
                let memory = config.php_memory_mb.unwrap_or(256);
                let workers = config.php_max_workers.unwrap_or(5);
                if let Err(e) = services::nginx::write_php_pool_config(&domain, ver, memory, workers) {
                    tracing::warn!("Failed to write PHP pool config for {domain}: {e}");
                } else {
                    // Reload PHP-FPM so the new per-site pool is actually picked up.
                    // Non-fatal: reload_php_fpm already swallows reload failures.
                    let _ = services::nginx::reload_php_fpm(ver).await;

                    // ...and then actually POINT the vhost at that pool. Writing the
                    // pool was never enough on its own: the rendered config kept the
                    // SHARED php{ver}-fpm.sock, so every request bypassed the per-site
                    // pool and the "PHP Memory" / "PHP Workers" limits the UI
                    // advertises did nothing — the pool just idled beside the site.
                    // (v2.28.0 fixed the sandbox so the pool gets WRITTEN; the fresh-box
                    // verification then showed nginx still on the shared socket.)
                    //
                    // Fail-safe: only switch once the socket really exists. A reload can
                    // fail or the pool can be rejected, and pointing nginx at a missing
                    // socket would 502 the entire site. If it never appears we keep the
                    // shared socket — degraded exactly as before, rather than broken.
                    let pool_name = domain.replace('.', "_");
                    let candidate = format!("/run/php/php{ver}-fpm-{pool_name}.sock");
                    for _ in 0..15 {
                        if std::path::Path::new(&candidate).exists() {
                            per_site_socket = Some(format!("unix:{candidate}"));
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    }
                    if per_site_socket.is_none() {
                        tracing::warn!(
                            "Per-site PHP-FPM socket {candidate} did not appear after reload; \
                             {domain} stays on the shared pool (per-site PHP limits inactive)"
                        );
                    }
                }
            }
        }
        if let Some(sock) = per_site_socket {
            tracing::info!("{domain} now uses its own PHP-FPM pool ({sock})");
            config.php_socket = Some(sock);
        }
    }

    // Create FastCGI cache directory if enabled.
    // Must succeed: nginx -t calls mkdir() on this path at config-test time and
    // fails the whole reload if the parent doesn't exist. Returning here
    // prevents writing a config we know nginx can't validate (#54-B, v2.8.14).
    if config.fastcgi_cache.unwrap_or(false) && config.runtime == "php" {
        let cache_dir = format!(
            "/var/cache/nginx/fastcgi/{}",
            domain.replace('.', "_")
        );
        if let Err(e) = std::fs::create_dir_all(&cache_dir) {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(NginxResponse {
                    success: false,
                    message: format!(
                        "Failed to create FastCGI cache dir {cache_dir}: {e}. \
                         Ensure /var/cache/nginx is in the agent's ReadWritePaths."
                    ),
                }),
            ));
        }
        // Ensure nginx can write to it
        let _ = safe_command_sync("chown")
            .args(["www-data:www-data", &cache_dir])
            .output();
    }

    // Render nginx config from template
    let rendered = match services::nginx::render_site_config(&state.templates, &domain, &config) {
        Ok(c) => c,
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(NginxResponse {
                    success: false,
                    message: format!("Template render error: {e}"),
                }),
            ));
        }
    };

    // Write config file atomically (write to .tmp, then rename).
    //
    // `rename` REPLACES whatever is already at this path, and `nginx -t` below is
    // a whole-server check — an unrelated broken vhost anywhere on the box fails
    // it. So the error paths must be able to put back exactly what was here: for
    // four years they deleted instead, under a comment promising a restore, which
    // meant an ordinary settings toggle (PHP version, WAF, headers) on a healthy
    // site could remove that site's config while some *other* vhost was broken.
    // Nothing reloads nginx on that path, so the box kept serving from memory and
    // the loss only surfaced at the next reload.
    //
    // Where it lands is not a constant: a site the operator took offline keeps a
    // stub in service and its real body parked beside it, and writing the render
    // into service would put that site back on the internet. See
    // `services::nginx::vhost_target_between`.
    let target = services::nginx::vhost_target(&domain);
    let config_path = target.path().to_string();
    let previous = std::fs::read_to_string(&config_path).ok();
    let tmp_path = format!("{config_path}.tmp");
    let write_result = std::fs::write(&tmp_path, &rendered)
        .and_then(|_| std::fs::rename(&tmp_path, &config_path));
    if let Err(e) = write_result {
        std::fs::remove_file(&tmp_path).ok();
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(NginxResponse {
                success: false,
                message: format!("Failed to write config: {e}"),
            }),
        ));
    }

    // Create document root with default index.html
    // Nginx templates use: root {{ root }}/{{ domain }}/public (for static/PHP)
    // So we need to create the /public subdirectory for non-proxy runtimes
    // Validate document root
    if let Some(ref root) = config.root {
        if !root.starts_with("/var/www/") || root.contains("..") {
            return Err((StatusCode::BAD_REQUEST, Json(NginxResponse {
                success: false,
                message: "Document root must be under /var/www/".into(),
            })));
        }
    }

    let default_root = format!("/var/www/{domain}");
    let doc_root = config.root.as_deref().unwrap_or(&default_root);
    let actual_root =
        crate::services::nginx::document_root_for(doc_root, &config.runtime);
    if let Err(e) = std::fs::create_dir_all(&actual_root) {
        tracing::warn!("Failed to create document root {actual_root}: {e}");
    } else {
        let index_path = format!("{actual_root}/index.html");
        if !std::path::Path::new(&index_path).exists() {
            let _ = std::fs::write(
                &index_path,
                crate::services::nginx::placeholder_page(&domain),
            );
        }
    }

    // A parked body is not in service, so there is nothing for `nginx -t` to read
    // and nothing to reload. Returning the ordinary "configured and nginx
    // reloaded" here would be the sentence that kept this invisible: the panel
    // would report success for a change the operator cannot see, on a site it
    // simultaneously displays as disabled.
    if !target.is_live() {
        tracing::info!("Site {domain} is disabled: saved its configuration to {config_path} and left the maintenance response in service");
        return Ok(Json(NginxResponse {
            success: true,
            message: format!(
                "Site {domain} is disabled — its configuration was saved and takes effect \
                 when the site is enabled. Nginx was not reloaded."
            ),
        }));
    }

    // Test nginx config
    match services::nginx::test_config().await {
        Ok(output) if output.success => {
            // Reload nginx
            if let Err(e) = services::nginx::reload().await {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(NginxResponse {
                        success: false,
                        message: format!("Config valid but reload failed: {e}"),
                    }),
                ));
            }
            // Auto-configure Fail2Ban jail for this site's access log
            let jail_name = format!("nginx-{}", domain.replace('.', "-"));
            let jail_config = format!(
                "[{jail_name}]\n\
                 enabled = true\n\
                 port = http,https\n\
                 filter = nginx-http-auth\n\
                 logpath = /var/log/nginx/{domain}.access.log\n\
                 maxretry = 10\n\
                 findtime = 300\n\
                 bantime = 3600\n"
            );

            // `jail_name` collapses '.' to '-', which is not injective over the
            // domains `is_valid_domain` accepts, so `a.b.com` and `a-b.com` share
            // one jail file. Writing it unconditionally repointed the existing
            // jail's `logpath` at the other tenant's access log — leaving the
            // first site unprotected while its jail appeared to still exist.
            let jail_path = format!("/etc/fail2ban/jail.d/{jail_name}.conf");
            let jail_is_someone_elses = std::path::Path::new(&jail_path).exists()
                && services::ownership::fail2ban_jail(&jail_path, &domain)
                    == services::ownership::Owner::Theirs;
            if jail_is_someone_elses {
                tracing::warn!(
                    "Not writing {jail_path} for {domain}: it already watches another \
                     domain's access log. Jail names collapse '.' to '-', so these two \
                     domains share one jail file and only the first keeps protection."
                );
            } else if let Ok(()) = std::fs::write(&jail_path, &jail_config) {
                // Reload fail2ban (best-effort)
                let _ = safe_command_sync("systemctl")
                    .args(["reload", "fail2ban"])
                    .output();
                tracing::info!("Auto-configured Fail2Ban jail for {domain}");
            }

            Ok(Json(NginxResponse {
                success: true,
                message: format!("Site {domain} configured and nginx reloaded"),
            }))
        }
        Ok(output) => {
            // Invalid config — put back exactly what was here before.
            let restored = restore_or_remove(&config_path, previous.as_deref());
            Err((
                StatusCode::BAD_REQUEST,
                Json(NginxResponse {
                    success: false,
                    message: format!(
                        "Nginx config test failed: {}{}",
                        output.stderr,
                        restore_note(restored)
                    ),
                }),
            ))
        }
        Err(e) => {
            // `nginx -t` did not answer (timeout, missing binary). The rendered
            // config is unproven, so it does not stay — but neither does the
            // previous owner's config get destroyed by our inability to test.
            let restored = restore_or_remove(&config_path, previous.as_deref());
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(NginxResponse {
                    success: false,
                    message: format!("Failed to test config: {e}{}", restore_note(restored)),
                }),
            ))
        }
    }
}

/// DELETE /nginx/sites/:domain — Remove site and all associated resources.
async fn delete_site(
    Path(domain): Path<String>,
) -> Result<Json<NginxResponse>, (StatusCode, Json<NginxResponse>)> {
    if !is_valid_domain(&domain) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(NginxResponse {
                success: false,
                message: "Invalid domain format".into(),
            }),
        ));
    }

    let config_path = format!("{}/{domain}.conf", services::nginx::sites_dir());

    if !std::path::Path::new(&config_path).exists() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(NginxResponse {
                success: false,
                message: format!("No config found for {domain}"),
            }),
        ));
    }

    if let Err(e) = std::fs::remove_file(&config_path) {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(NginxResponse {
                success: false,
                message: format!("Failed to remove config: {e}"),
            }),
        ));
    }

    // Reload nginx
    if let Err(e) = services::nginx::reload().await {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(NginxResponse {
                success: false,
                message: format!("Config removed but reload failed: {e}"),
            }),
        ));
    }

    // Clean up all associated resources (best-effort, don't fail the delete)
    let pool_name = domain.replace('.', "_");

    // FastCGI cache directory
    let cache_dir = format!(
        "/var/cache/nginx/fastcgi/{}",
        domain.replace('.', "_")
    );
    if std::path::Path::new(&cache_dir).exists() {
        std::fs::remove_dir_all(&cache_dir).ok();
        tracing::info!("Removed FastCGI cache: {cache_dir}");
    }

    // SSL certificates. A DNS-01 wildcard is provisioned ONCE under the zone
    // apex and every site in the zone points `ssl_certificate` at that one
    // directory, so deleting the apex site used to take the certificate out from
    // under its siblings. That does not fail now — nginx is already serving from
    // memory — it fails at the next `nginx -t` anyone triggers, and leaves nginx
    // down box-wide at the next full restart.
    let ssl_dir = format!("/etc/dockpanel/ssl/{domain}");
    if std::path::Path::new(&ssl_dir).exists() {
        if services::ownership::cert_dir_in_use_elsewhere(&domain) {
            tracing::warn!(
                "Leaving {ssl_dir} in place: another vhost still points at it \
                 (a wildcard shared across the zone)."
            );
        } else if let Err(e) = std::fs::remove_dir_all(&ssl_dir) {
            tracing::warn!("Failed to remove SSL certs for {domain}: {e}");
        } else {
            tracing::info!("Removed SSL certs: {ssl_dir}");
        }
    }

    // PHP-FPM pool configs (all versions)
    for version in &["8.1", "8.2", "8.3", "8.4", "8.5"] {
        let pool_path = format!("/etc/php/{version}/fpm/pool.d/{pool_name}.conf");
        if std::path::Path::new(&pool_path).exists() {
            std::fs::remove_file(&pool_path).ok();
            tracing::info!("Removed PHP pool: {pool_path}");
        }
    }

    // Site files
    let site_dir = format!("/var/www/{domain}");
    if std::path::Path::new(&site_dir).exists() {
        if let Err(e) = std::fs::remove_dir_all(&site_dir) {
            tracing::warn!("Failed to remove site files for {domain}: {e}");
        } else {
            tracing::info!("Removed site files: {site_dir}");
        }
    }

    // Nginx logs
    for suffix in &["access.log", "error.log"] {
        let log_path = format!("/var/log/nginx/{domain}.{suffix}");
        std::fs::remove_file(&log_path).ok();
    }

    // App process service (Node.js/Python)
    if let Err(e) = services::app_process::remove_app_service(&domain) {
        tracing::warn!("Failed to remove app service for {domain}: {e}");
    }

    // WordPress auto-update cron
    if crate::services::wordpress::is_auto_update_enabled(&domain) {
        crate::services::wordpress::set_auto_update(&domain, false).await.ok();
        tracing::info!("Removed WordPress auto-update cron for {domain}");
    }

    // Fail2Ban jail — same non-injective name as the create side above, so the
    // file at this path may be another domain's brute-force protection. The
    // jail's own `logpath` says which log it watches.
    let jail_name = format!("nginx-{}", domain.replace('.', "-"));
    let jail_path = format!("/etc/fail2ban/jail.d/{jail_name}.conf");
    if std::path::Path::new(&jail_path).exists() {
        if services::ownership::fail2ban_jail(&jail_path, &domain).may_delete() {
            let _ = std::fs::remove_file(&jail_path);
            let _ = safe_command_sync("systemctl")
                .args(["reload", "fail2ban"])
                .output();
            tracing::info!("Removed Fail2Ban jail for {domain}");
        } else {
            tracing::warn!(
                "Leaving {jail_path} in place: it does not watch {domain}'s access \
                 log. Jail names collapse '.' to '-', so this file protects a \
                 different domain."
            );
        }
    }

    Ok(Json(NginxResponse {
        success: true,
        message: format!("Site {domain} removed and nginx reloaded"),
    }))
}

/// POST /nginx/test — Test nginx configuration.
async fn test_nginx() -> Json<NginxTestResponse> {
    match services::nginx::test_config().await {
        Ok(output) => Json(NginxTestResponse {
            success: output.success,
            output: if output.success {
                output.stdout
            } else {
                output.stderr
            },
        }),
        Err(e) => Json(NginxTestResponse {
            success: false,
            output: format!("Error: {e}"),
        }),
    }
}

/// POST /nginx/reload — Reload nginx.
async fn reload_nginx() -> Result<Json<NginxResponse>, (StatusCode, Json<NginxResponse>)> {
    match services::nginx::reload().await {
        Ok(_) => Ok(Json(NginxResponse {
            success: true,
            message: "Nginx reloaded".into(),
        })),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(NginxResponse {
                success: false,
                message: format!("Reload failed: {e}"),
            }),
        )),
    }
}

/// GET /nginx/sites/:domain — Get site status.
async fn get_site(
    Path(domain): Path<String>,
) -> Result<Json<SiteStatusResponse>, (StatusCode, Json<NginxResponse>)> {
    if !is_valid_domain(&domain) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(NginxResponse {
                success: false,
                message: "Invalid domain format".into(),
            }),
        ));
    }

    let config_path = format!("{}/{domain}.conf", services::nginx::sites_dir());
    let ssl_cert_path = format!("/etc/dockpanel/ssl/{domain}/fullchain.pem");
    let config_exists = std::path::Path::new(&config_path).exists();
    let ssl_enabled = std::path::Path::new(&ssl_cert_path).exists();

    let ssl_expiry = if ssl_enabled {
        let status = ssl::get_cert_status(&domain).await;
        status.not_after
    } else {
        None
    };

    Ok(Json(SiteStatusResponse {
        domain,
        config_exists,
        ssl_enabled,
        ssl_cert_path: if ssl_enabled {
            Some(ssl_cert_path)
        } else {
            None
        },
        ssl_expiry,
    }))
}

/// POST /nginx/sites/{domain}/rename — Rename a site's domain.
/// Copy the files directly inside `src` into `dst`, creating `dst`.
///
/// Shallow on purpose: a certificate directory is a flat set of PEMs, and a
/// recursive copy here would be reach this function does not need.
fn copy_dir_shallow(src: &str, dst: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            std::fs::copy(entry.path(), std::path::Path::new(dst).join(entry.file_name()))?;
        }
    }
    Ok(())
}

/// The backup trees this agent keys by site domain.
///
/// Three trees, three spellings of the same site, and every one of them is
/// rebuilt from the LIVE domain by whoever reads it — the panel's `backups`
/// table stores a filename and no path at all. So a rename that does not carry
/// these leaves every archive on disk under a name nothing will look up again,
/// while the panel keeps listing the rows and answers a restore with a path that
/// does not exist.
///
/// The restic key substitutes `_` for `.`, and that substitution is injective
/// here: `is_valid_domain` permits only ASCII alphanumerics and `-` inside a
/// label, so a `_` in the key can only have come from a `.`. That is what
/// separates it from the fail2ban jail key in step 8, which maps `.` onto `-`
/// where `-` is already legal, and therefore needs an ownership guard.
fn domain_backup_trees(old_domain: &str, new_domain: &str) -> Vec<(String, String)> {
    let root = "/var/backups/dockpanel";
    vec![
        (format!("{root}/{old_domain}"), format!("{root}/{new_domain}")),
        (
            format!("{root}/restic/{}", old_domain.replace('.', "_")),
            format!("{root}/restic/{}", new_domain.replace('.', "_")),
        ),
        (
            format!("{root}/wp-snapshots/{old_domain}"),
            format!("{root}/wp-snapshots/{new_domain}"),
        ),
    ]
}

/// Does anything already live at `path`?
///
/// `NotFound` is the only answer that means "free". Every other error — a
/// permission problem, a dangling mount — answers `true`, because the
/// alternative is moving one tenant's archives on top of something we could not
/// read.
fn dir_is_occupied(path: &str) -> bool {
    match std::fs::read_dir(path) {
        Ok(mut entries) => entries.next().is_some(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => true,
    }
}

/// Put carried trees back, most recent move first.
///
/// Best-effort by necessity — it runs on a path that is already failing — but
/// never silent. A tree that cannot be put back is an archive the panel will
/// look for under the old name and not find, which is the exact defect the
/// carry exists to prevent, so both paths go into the log at ERROR.
fn undo_backup_carry(carried: &[(String, String)], old_domain: &str) {
    for (old_tree, new_tree) in carried.iter().rev() {
        if let Err(e) = std::fs::rename(new_tree, old_tree) {
            tracing::error!(
                "Rolling back the rename of {old_domain}, but {new_tree} could not be put back                  at {old_tree}: {e}. Those archives are on disk under the new name while the                  panel will look for them under the old one."
            );
        }
    }
}

async fn rename_site(
    Path(old_domain): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<NginxResponse>, (StatusCode, Json<NginxResponse>)> {
    if !is_valid_domain(&old_domain) {
        return Err((StatusCode::BAD_REQUEST, Json(NginxResponse {
            success: false, message: "Invalid old domain format".into(),
        })));
    }

    let new_domain = body.get("new_domain")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if !is_valid_domain(new_domain) {
        return Err((StatusCode::BAD_REQUEST, Json(NginxResponse {
            success: false, message: "Invalid new domain format".into(),
        })));
    }

    let old_conf = format!("{}/{old_domain}.conf", services::nginx::sites_dir());
    if !std::path::Path::new(&old_conf).exists() {
        return Err((StatusCode::NOT_FOUND, Json(NginxResponse {
            success: false, message: format!("No nginx config for {old_domain}"),
        })));
    }

    // 0. Refuse, before anything moves, if a backup tree is already occupied at
    //    the destination.
    //
    //    This is not a hypothetical collision — every completed site delete
    //    manufactures one. The panel writes a pre-delete archive for a site it is
    //    about to remove and records no row for it, then the site row goes and the
    //    cascade takes the backup rows with it. The directory outlives every
    //    record of itself. Rename another site onto that domain and two tenants'
    //    archives become one listing, with no ownership filter between them and a
    //    restore one click away.
    //
    //    Refusing costs the operator one directory they can inspect and remove.
    //    Merging costs them somebody else's site restored over their webroot.
    let backup_trees = domain_backup_trees(&old_domain, new_domain);
    for (old_tree, new_tree) in &backup_trees {
        if std::path::Path::new(old_tree).exists() && dir_is_occupied(new_tree) {
            return Err((StatusCode::CONFLICT, Json(NginxResponse {
                success: false,
                message: format!(
                    "{new_tree} already holds files. {old_domain} has backups that must move \
                     there, and merging them would put two sites' archives in one listing. \
                     Move or delete that directory, then rename again."
                ),
            })));
        }
    }

    // 1. Rename site directory
    let old_dir = format!("/var/www/{old_domain}");
    let new_dir = format!("/var/www/{new_domain}");
    if std::path::Path::new(&old_dir).exists() {
        if let Err(e) = std::fs::rename(&old_dir, &new_dir) {
            return Err((StatusCode::INTERNAL_SERVER_ERROR, Json(NginxResponse {
                success: false, message: format!("Failed to rename site directory: {e}"),
            })));
        }
        tracing::info!("Renamed site dir: {old_dir} → {new_dir}");
    }

    // 2. Read nginx config and replace domain references
    let config_content = std::fs::read_to_string(&old_conf).unwrap_or_default();
    let new_content = config_content.replace(&old_domain, new_domain);

    // 3. Write new nginx config.
    //
    // Snapshot the destination first. This is the writer the v2.52.0
    // `restore_or_remove` retrofit missed, and it replaces a path that
    // `domain_claim` cannot vouch for: the claim guard clears a name against
    // `sites`, `git_deploys` and Docker-app labels, and never looks at the
    // filesystem — so a vhost the OPERATOR wrote by hand (`webmail.example.com`,
    // a legacy config predating the panel) is claimable, and was silently
    // overwritten here and then bare-deleted on rollback. The tenant's own
    // config came back; the operator's never existed anywhere else.
    let new_conf = format!("{}/{new_domain}.conf", services::nginx::sites_dir());
    let displaced = std::fs::read_to_string(&new_conf).ok();
    if let Some(ref bytes) = displaced {
        tracing::warn!(
            "Renaming {old_domain} onto {new_domain} is replacing an existing vhost \
             at {new_conf} ({} bytes) that no site, git deploy or Docker app claims.",
            bytes.len()
        );
    }
    if let Err(e) = std::fs::write(&new_conf, &new_content) {
        // Rollback directory rename
        if std::path::Path::new(&new_dir).exists() {
            std::fs::rename(&new_dir, &old_dir).ok();
        }
        return Err((StatusCode::INTERNAL_SERVER_ERROR, Json(NginxResponse {
            success: false, message: format!("Failed to write new config: {e}"),
        })));
    }

    // 4. Remove old config
    std::fs::remove_file(&old_conf).ok();

    // 4b. Carry the parked body across too, if this site is disabled.
    //
    // What step 3 just rewrote is whatever nginx has in service, and for a
    // disabled site that is the maintenance stub — which would leave the real
    // body parked under the OLD name, where enabling the site will never look.
    // The site becomes unenableable through the panel while its configuration
    // sits on disk under a name nothing reads.
    let (_, old_parked) = services::nginx::vhost_paths(&old_domain);
    let (_, new_parked) = services::nginx::vhost_paths(new_domain);
    if std::path::Path::new(&old_parked).exists() {
        match std::fs::read_to_string(&old_parked) {
            Ok(parked) => {
                let carried = parked.replace(&old_domain, new_domain);
                if std::fs::write(&new_parked, &carried).is_ok() {
                    std::fs::remove_file(&old_parked).ok();
                    tracing::info!(
                        "Carried the parked configuration for disabled site {old_domain} over to {new_domain}"
                    );
                } else {
                    tracing::warn!(
                        "Could not carry the parked configuration for disabled site {old_domain} \
                         over to {new_parked} — it can still be enabled under its old name"
                    );
                }
            }
            Err(e) => tracing::warn!(
                "Could not read the parked configuration for disabled site {old_domain}: {e}"
            ),
        }
    }

    // 5. Rename SSL certificates directory.
    //
    // A DNS-01 wildcard is issued once under the zone apex and every site in the
    // zone points its `ssl_certificate` at that one directory. MOVING it because
    // one of those sites was renamed leaves all the others pointing at a path
    // that no longer exists — which does not fail now, because nginx is already
    // serving from memory, but fails the next `nginx -t` anybody triggers and
    // leaves nginx down at the next restart, for every site on the box. v2.53.0
    // guarded the delete paths against exactly this and did not reach the rename.
    //
    // When it is shared we copy instead of moving: the renamed site's vhost was
    // rewritten to the new path two steps ago, so leaving the directory alone
    // would take the whole server's config invalid — the same outage from the
    // other side. A copy is a duplicate that renewal will not refresh, and that
    // is worth saying out loud rather than discovering at expiry.
    let old_ssl = format!("/etc/dockpanel/ssl/{old_domain}");
    let new_ssl = format!("/etc/dockpanel/ssl/{new_domain}");
    if std::path::Path::new(&old_ssl).exists() {
        if services::ownership::cert_dir_in_use_elsewhere(&old_domain) {
            tracing::warn!(
                "{old_ssl} is referenced by another vhost — copying it to {new_ssl} instead of \
                 moving it, so the sites still pointing at it keep working. The copy is a \
                 snapshot: reissue a certificate for {new_domain} before the original expires."
            );
            if let Err(e) = copy_dir_shallow(&old_ssl, &new_ssl) {
                tracing::warn!("Could not copy {old_ssl} to {new_ssl}: {e}");
            }
        } else {
            std::fs::rename(&old_ssl, &new_ssl).ok();
        }
    }

    // 6. Rename log files
    for suffix in &["access.log", "error.log"] {
        let old_log = format!("/var/log/nginx/{old_domain}.{suffix}");
        let new_log = format!("/var/log/nginx/{new_domain}.{suffix}");
        if std::path::Path::new(&old_log).exists() {
            std::fs::rename(&old_log, &new_log).ok();
        }
    }

    // 7. Rename PHP-FPM pool configs
    let old_pool = old_domain.replace('.', "_");
    let new_pool = new_domain.replace('.', "_");
    for version in &["8.1", "8.2", "8.3", "8.4", "8.5"] {
        let old_pool_path = format!("/etc/php/{version}/fpm/pool.d/{old_pool}.conf");
        let new_pool_path = format!("/etc/php/{version}/fpm/pool.d/{new_pool}.conf");
        if std::path::Path::new(&old_pool_path).exists() {
            if let Ok(pool_content) = std::fs::read_to_string(&old_pool_path) {
                let updated = pool_content.replace(&old_domain, new_domain);
                std::fs::write(&new_pool_path, updated).ok();
                std::fs::remove_file(&old_pool_path).ok();
            }
        }
    }

    // 8. Rename Fail2Ban jail.
    //
    // `nginx-{domain with '.' → '-'}` is the non-injective key v2.53.0 fixed on
    // the create and remove paths by reading each jail's own `logpath`. The
    // rename path was missed, and it does both halves of the damage at once:
    // it DELETES the file at the old name (which may be `a.b.com`'s jail while
    // the caller is renaming `a-b.com`) and WRITES the destination (which may be
    // a third domain's). Both are answered the same way — open the file and ask.
    let old_jail = format!("nginx-{}", old_domain.replace('.', "-"));
    let new_jail = format!("nginx-{}", new_domain.replace('.', "-"));
    let old_jail_path = format!("/etc/fail2ban/jail.d/{old_jail}.conf");
    let new_jail_path = format!("/etc/fail2ban/jail.d/{new_jail}.conf");
    let jail_is_ours =
        services::ownership::fail2ban_jail(&old_jail_path, &old_domain).may_delete();
    let destination_is_free = !std::path::Path::new(&new_jail_path).exists()
        || services::ownership::fail2ban_jail(&new_jail_path, new_domain).may_delete();
    if std::path::Path::new(&old_jail_path).exists() && !(jail_is_ours && destination_is_free) {
        tracing::warn!(
            "Not migrating {old_jail_path}: ours={jail_is_ours}, destination_free=\
             {destination_is_free}. The jail name collapses '.' to '-', so one of these files \
             protects a different domain."
        );
    } else if std::path::Path::new(&old_jail_path).exists() {
        if let Ok(jail_content) = std::fs::read_to_string(&old_jail_path) {
            let updated = jail_content
                .replace(&old_jail, &new_jail)
                .replace(&old_domain, new_domain);
            std::fs::write(&new_jail_path, updated).ok();
            std::fs::remove_file(&old_jail_path).ok();
            let _ = safe_command_sync("systemctl")
                .args(["reload", "fail2ban"])
                .output();
        }
    }

    // 9. Rename redirect/auth/htpasswd configs
    for dir in &["/etc/nginx/redirects", "/etc/nginx/auth", "/etc/nginx/htpasswd"] {
        let old_file = format!("{dir}/{old_domain}.conf");
        let new_file = format!("{dir}/{new_domain}.conf");
        if std::path::Path::new(&old_file).exists() {
            std::fs::rename(&old_file, &new_file).ok();
        }
        // Also handle files without .conf extension (htpasswd)
        let old_plain = format!("{dir}/{old_domain}");
        let new_plain = format!("{dir}/{new_domain}");
        if std::path::Path::new(&old_plain).exists() && !old_plain.ends_with(".conf") {
            std::fs::rename(&old_plain, &new_plain).ok();
        }
    }

    // 9c. Carry the backup trees.
    //
    // Last among the moves, deliberately. It is the heaviest and it is the only
    // one whose failure aborts the rename, so putting it here keeps the undo a
    // single self-contained loop and means a cheap failure earlier never has to
    // unwind gigabytes.
    //
    // The result of each move is matched, never discarded. A move that lands on
    // a destination which filled up since step 0 answers ENOTEMPTY, and
    // swallowing that answer is precisely the "reports success while leaking"
    // defect this whole step exists to close.
    let mut carried: Vec<(String, String)> = Vec::new();
    let mut files_renamed = 0usize;
    for (old_tree, new_tree) in &backup_trees {
        if !std::path::Path::new(old_tree).exists() {
            continue;
        }
        if let Some(parent) = std::path::Path::new(new_tree).parent() {
            std::fs::create_dir_all(parent).ok();
        }
        match std::fs::rename(old_tree, new_tree) {
            Ok(()) => {
                carried.push((old_tree.clone(), new_tree.clone()));
                tracing::info!("Carried backups for {old_domain}: {old_tree} to {new_tree}");
            }
            Err(e) => {
                undo_backup_carry(&carried, &old_domain);
                std::fs::write(&old_conf, &config_content).ok();
                services::nginx::restore_or_remove(&new_conf, displaced.as_deref());
                if std::path::Path::new(&new_dir).exists() {
                    std::fs::rename(&new_dir, &old_dir).ok();
                }
                // A destination that filled up is the operator's to clear, so it
                // gets a status that carries this sentence to them. Anything else
                // is this machine's fault and keeps its incident id.
                let status = match e.kind() {
                    std::io::ErrorKind::DirectoryNotEmpty
                    | std::io::ErrorKind::AlreadyExists => StatusCode::CONFLICT,
                    _ => StatusCode::INTERNAL_SERVER_ERROR,
                };
                return Err((status, Json(NginxResponse {
                    success: false,
                    message: format!(
                        "Could not move {old_tree} to {new_tree}: {e}. The rename was rolled \
                         back and {old_domain} is unchanged."
                    ),
                })));
            }
        }
    }

    // The archive names carry the old domain as a prefix, and that prefix is load
    // bearing rather than decorative: off-site retention is handed the site's
    // live domain and uses it to choose which remote objects it may delete. Left
    // alone, remote pruning silently stops matching this site's archives — and
    // starts matching the previous occupant's if the old name is ever reused.
    if !carried.is_empty() {
        let moved_dir = format!("/var/backups/dockpanel/{new_domain}");
        let old_prefix = format!("{old_domain}-");
        let new_prefix = format!("{new_domain}-");
        if let Ok(entries) = std::fs::read_dir(&moved_dir) {
            for entry in entries.flatten() {
                let Some(name) = entry.file_name().to_str().map(|n| n.to_string()) else {
                    continue;
                };
                let Some(rest) = name.strip_prefix(&old_prefix) else {
                    continue;
                };
                let renamed = format!("{new_prefix}{rest}");
                if let Err(e) = std::fs::rename(
                    std::path::Path::new(&moved_dir).join(&name),
                    std::path::Path::new(&moved_dir).join(&renamed),
                ) {
                    tracing::warn!("Could not rename archive {name} to {renamed}: {e}");
                } else {
                    files_renamed += 1;
                }
            }
        }

        // A moved directory keeps the mode it was created with, and archives are
        // created with the process umask rather than the tightened one. The
        // repair otherwise runs only at agent start, so a tree created since the
        // last restart would ride to its new name still world-traversable.
        services::backups::secure_backup_tree();
        tracing::info!(
            "Backup carry for {old_domain} to {new_domain}: {} trees, {files_renamed} archives renamed",
            carried.len()
        );
    }

    // 10. Test and reload nginx
    match services::nginx::test_config().await {
        Ok(output) if output.success => {
            if let Err(e) = services::nginx::reload().await {
                return Err((StatusCode::INTERNAL_SERVER_ERROR, Json(NginxResponse {
                    success: false, message: format!("Config valid but reload failed: {e}"),
                })));
            }
            tracing::info!("Domain renamed: {old_domain} → {new_domain}");
            Ok(Json(NginxResponse {
                success: true,
                message: format!("Domain renamed from {old_domain} to {new_domain}"),
            }))
        }
        Ok(output) => {
            // Rollback: restore old config, and put back whatever we displaced
            // at the destination rather than deleting it.
            std::fs::write(&old_conf, &config_content).ok();
            undo_backup_carry(&carried, &old_domain);
            let restored = services::nginx::restore_or_remove(&new_conf, displaced.as_deref());
            if std::path::Path::new(&new_dir).exists() {
                std::fs::rename(&new_dir, &old_dir).ok();
            }
            Err((StatusCode::BAD_REQUEST, Json(NginxResponse {
                success: false,
                message: format!(
                    "Nginx config test failed after rename: {}{}",
                    output.stderr,
                    services::nginx::restore_note(restored)
                ),
            })))
        }
        Err(e) => {
            std::fs::write(&old_conf, &config_content).ok();
            undo_backup_carry(&carried, &old_domain);
            services::nginx::restore_or_remove(&new_conf, displaced.as_deref());
            if std::path::Path::new(&new_dir).exists() {
                std::fs::rename(&new_dir, &old_dir).ok();
            }
            Err((StatusCode::INTERNAL_SERVER_ERROR, Json(NginxResponse {
                success: false,
                message: format!("Failed to test config: {e}"),
            })))
        }
    }
}

// ──────────────────────────────────────────────────────────────
// Redirect Rules
// ──────────────────────────────────────────────────────────────

type ApiErr = (StatusCode, Json<serde_json::Value>);

fn api_err(status: StatusCode, msg: &str) -> ApiErr {
    (status, Json(serde_json::json!({ "error": msg })))
}

#[derive(Deserialize)]
struct RedirectRequest {
    domain: String,
    source: String,
    target: String,
    redirect_type: String,
}

/// POST /nginx/redirects/add — Add a redirect rule.
async fn add_redirect(
    Json(body): Json<RedirectRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if body.domain.is_empty() || body.source.is_empty() || body.target.is_empty() {
        return Err(api_err(StatusCode::BAD_REQUEST, "Missing fields"));
    }
    if !super::is_valid_domain(&body.domain) {
        return Err(api_err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    if body.redirect_type != "301" && body.redirect_type != "302" {
        return Err(api_err(StatusCode::BAD_REQUEST, "Type must be 301 or 302"));
    }
    if !body.source.starts_with('/') {
        return Err(api_err(
            StatusCode::BAD_REQUEST,
            "Source must start with /",
        ));
    }
    if body.source.contains(|c: char| c.is_whitespace() || c == ';' || c == '{' || c == '}' || c == '\n' || c == '\r') {
        return Err(api_err(StatusCode::BAD_REQUEST, "Invalid source path characters"));
    }
    if body.target.contains(|c: char| c == '\n' || c == '\r' || c == ';' || c == '{' || c == '}') {
        return Err(api_err(StatusCode::BAD_REQUEST, "Invalid target URL characters"));
    }

    let redirects_file = format!("/etc/nginx/redirects/{}.conf", body.domain);
    std::fs::create_dir_all("/etc/nginx/redirects").ok();

    let rule = format!(
        "location = {} {{ return {} {}; }}\n",
        body.source, body.redirect_type, body.target
    );
    let existing = std::fs::read_to_string(&redirects_file).unwrap_or_default();
    std::fs::write(&redirects_file, format!("{existing}{rule}"))
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")))?;

    // Include in nginx site config if not already
    let site_conf = format!("{}/{}.conf", services::nginx::sites_dir(), body.domain);
    let site_content = std::fs::read_to_string(&site_conf).unwrap_or_default();
    if !site_content.contains(&format!("include /etc/nginx/redirects/{}.conf", body.domain)) {
        let include_line = format!(
            "    include /etc/nginx/redirects/{}.conf;\n",
            body.domain
        );
        if let Some(pos) = site_content.rfind('}') {
            let new_content = format!(
                "{}{}{}",
                &site_content[..pos],
                include_line,
                &site_content[pos..]
            );
            std::fs::write(&site_conf, new_content).ok();
        }
    }

    // Test and reload
    match services::nginx::test_config().await {
        Ok(output) if output.success => {
            if let Err(e) = services::nginx::reload().await {
                return Err(api_err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("Config saved but nginx reload failed: {e}"),
                ));
            }
        }
        _ => {
            // Rollback
            std::fs::write(&redirects_file, &existing).ok();
            return Err(api_err(
                StatusCode::BAD_REQUEST,
                "Nginx config test failed — redirect reverted",
            ));
        }
    }

    tracing::info!(
        "Redirect added: {} → {} ({})",
        body.source,
        body.target,
        body.redirect_type
    );
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /nginx/redirects/{domain} — List redirects for a domain.
async fn list_redirects(Path(domain): Path<String>) -> Result<Json<serde_json::Value>, ApiErr> {
    if !super::is_valid_domain(&domain) {
        return Err(api_err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    let redirects_file = format!("/etc/nginx/redirects/{domain}.conf");
    let content = std::fs::read_to_string(&redirects_file).unwrap_or_default();

    let rules: Vec<serde_json::Value> = content
        .lines()
        .filter(|l| l.contains("return "))
        .filter_map(|l| {
            let source = l.split("location = ").nth(1)?.split(' ').next()?;
            let after_return = l.split("return ").nth(1)?;
            let parts: Vec<&str> = after_return.split_whitespace().collect();
            if parts.len() >= 2 {
                let rtype = parts[0];
                let target = parts[1].trim_end_matches(';');
                Some(serde_json::json!({
                    "source": source,
                    "target": target,
                    "type": rtype
                }))
            } else {
                None
            }
        })
        .collect();

    Ok(Json(serde_json::json!({ "redirects": rules })))
}

/// POST /nginx/redirects/{domain}/remove — Remove a specific redirect.
async fn remove_redirect(
    Path(domain): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !super::is_valid_domain(&domain) {
        return Err(api_err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    let source = body
        .get("source")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if source.is_empty() {
        return Err(api_err(StatusCode::BAD_REQUEST, "Source required"));
    }

    let redirects_file = format!("/etc/nginx/redirects/{domain}.conf");
    let content = std::fs::read_to_string(&redirects_file).unwrap_or_default();
    let cleaned: String = content
        .lines()
        .filter(|l| !l.contains(&format!("location = {source} ")))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&redirects_file, format!("{cleaned}\n")).ok();

    match services::nginx::test_config().await {
        Ok(output) if output.success => {
            if let Err(e) = services::nginx::reload().await {
                return Err(api_err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("Config saved but nginx reload failed: {e}"),
                ));
            }
        }
        _ => {}
    }

    tracing::info!("Redirect removed: {source} from {domain}");
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ──────────────────────────────────────────────────────────────
// Password Protection (htpasswd)
// ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct PasswordProtectRequest {
    domain: String,
    path: String,
    username: String,
    password: String,
}

/// POST /nginx/password-protect — Enable basic auth on a path.
async fn password_protect(
    Json(body): Json<PasswordProtectRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if body.domain.is_empty() || body.username.is_empty() || body.password.is_empty() {
        return Err(api_err(StatusCode::BAD_REQUEST, "Missing fields"));
    }
    if !super::is_valid_domain(&body.domain) {
        return Err(api_err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }

    let htpasswd_dir = "/etc/nginx/htpasswd";
    std::fs::create_dir_all(htpasswd_dir).ok();
    let htpasswd_file = format!("{htpasswd_dir}/{}", body.domain);

    // Generate htpasswd entry using openssl
    let output = safe_command("openssl")
        .args(["passwd", "-apr1", &body.password])
        .output()
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")))?;

    let hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if hash.is_empty() {
        return Err(api_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to generate password hash",
        ));
    }
    let entry = format!("{}:{}", body.username, hash);

    // Append or create htpasswd file, removing existing entry for this username
    let existing = std::fs::read_to_string(&htpasswd_file).unwrap_or_default();
    let mut lines: Vec<&str> = existing
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with(&format!("{}:", body.username)))
        .collect();
    lines.push(&entry);
    std::fs::write(&htpasswd_file, lines.join("\n") + "\n")
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")))?;

    // Add auth_basic to nginx config via include
    let auth_conf_dir = "/etc/nginx/auth";
    std::fs::create_dir_all(auth_conf_dir).ok();
    let auth_file = format!("{auth_conf_dir}/{}.conf", body.domain);

    let path = if body.path.is_empty() { "/" } else { &body.path };
    if !path.starts_with('/') || path.contains(|c: char| c.is_whitespace() || c == ';' || c == '{' || c == '}' || c == '\n' || c == '\r') {
        return Err(api_err(StatusCode::BAD_REQUEST, "Invalid path format"));
    }
    let auth_block = format!(
        "location {} {{\n    auth_basic \"Restricted\";\n    auth_basic_user_file {};\n    try_files $uri $uri/ =404;\n}}\n",
        path, htpasswd_file
    );

    // Check if auth block for this path already exists
    let existing_auth = std::fs::read_to_string(&auth_file).unwrap_or_default();
    if !existing_auth.contains(&format!("location {} ", path)) {
        std::fs::write(&auth_file, format!("{existing_auth}{auth_block}")).ok();
    }

    // Include in site config if not already
    let site_conf = format!("{}/{}.conf", services::nginx::sites_dir(), body.domain);
    let site_content = std::fs::read_to_string(&site_conf).unwrap_or_default();
    if !site_content.contains(&format!("include /etc/nginx/auth/{}.conf", body.domain)) {
        if let Some(pos) = site_content.rfind('}') {
            let include_line = format!(
                "    include /etc/nginx/auth/{}.conf;\n",
                body.domain
            );
            let new_content = format!(
                "{}{}{}",
                &site_content[..pos],
                include_line,
                &site_content[pos..]
            );
            std::fs::write(&site_conf, new_content).ok();
        }
    }

    match services::nginx::test_config().await {
        Ok(output) if output.success => {
            if let Err(e) = services::nginx::reload().await {
                return Err(api_err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("Config saved but nginx reload failed: {e}"),
                ));
            }
        }
        _ => {
            return Err(api_err(
                StatusCode::BAD_REQUEST,
                "Nginx config test failed",
            ));
        }
    }

    tracing::info!(
        "Password protection enabled on {} path {} for user {}",
        body.domain,
        path,
        body.username
    );
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /nginx/password-protect/{domain} — List protected paths and users.
async fn list_protected(Path(domain): Path<String>) -> Result<Json<serde_json::Value>, ApiErr> {
    if !super::is_valid_domain(&domain) {
        return Err(api_err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    let auth_file = format!("/etc/nginx/auth/{domain}.conf");
    let content = std::fs::read_to_string(&auth_file).unwrap_or_default();
    let htpasswd_file = format!("/etc/nginx/htpasswd/{domain}");
    let users_content = std::fs::read_to_string(&htpasswd_file).unwrap_or_default();

    let paths: Vec<String> = content
        .lines()
        .filter(|l| l.contains("location "))
        .filter_map(|l| {
            l.split("location ")
                .nth(1)?
                .split(' ')
                .next()
                .map(|s| s.to_string())
        })
        .collect();

    let users: Vec<String> = users_content
        .lines()
        .filter(|l| !l.is_empty())
        .filter_map(|l| l.split(':').next().map(|s| s.to_string()))
        .collect();

    Ok(Json(serde_json::json!({ "paths": paths, "users": users })))
}

/// POST /nginx/password-protect/{domain}/remove — Remove password protection from a path.
async fn remove_protection(
    Path(domain): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !super::is_valid_domain(&domain) {
        return Err(api_err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    let path = body
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("/");
    let auth_file = format!("/etc/nginx/auth/{domain}.conf");
    let content = std::fs::read_to_string(&auth_file).unwrap_or_default();

    // Remove the location block for this path
    let mut cleaned = String::new();
    let mut skip = false;
    let mut brace_count: i32 = 0;
    for line in content.lines() {
        if line.contains(&format!("location {} ", path)) || line.contains(&format!("location {path} ")) {
            skip = true;
            brace_count = 0;
        }
        if skip {
            brace_count += line.matches('{').count() as i32;
            brace_count -= line.matches('}').count() as i32;
            if brace_count <= 0 && line.contains('}') {
                skip = false;
                continue;
            }
            continue;
        }
        cleaned.push_str(line);
        cleaned.push('\n');
    }

    std::fs::write(&auth_file, cleaned).ok();
    match services::nginx::test_config().await {
        Ok(output) if output.success => {
            if let Err(e) = services::nginx::reload().await {
                return Err(api_err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("Config saved but nginx reload failed: {e}"),
                ));
            }
        }
        _ => {}
    }

    tracing::info!("Password protection removed from {domain} path {path}");
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ──────────────────────────────────────────────────────────────
// Domain Aliases
// ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct AliasRequest {
    domain: String,
    alias: String,
}

/// POST /nginx/aliases/add — Add a domain alias to a site.
async fn add_alias(
    Json(body): Json<AliasRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if body.domain.is_empty() || body.alias.is_empty() {
        return Err(api_err(StatusCode::BAD_REQUEST, "Missing fields"));
    }
    if !super::is_valid_domain(&body.domain) {
        return Err(api_err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    if !super::is_valid_domain(&body.alias) {
        return Err(api_err(StatusCode::BAD_REQUEST, "Invalid alias domain format"));
    }

    let site_conf = format!("{}/{}.conf", services::nginx::sites_dir(), body.domain);
    let content = std::fs::read_to_string(&site_conf)
        .map_err(|_| api_err(StatusCode::NOT_FOUND, "Site config not found"))?;

    // Add alias to server_name line
    let new_content = content.replace(
        &format!("server_name {};", body.domain),
        &format!("server_name {} {};", body.domain, body.alias),
    );

    if new_content == content {
        return Err(api_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not find server_name directive",
        ));
    }

    let tmp = format!("{site_conf}.tmp");
    std::fs::write(&tmp, &new_content)
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")))?;
    std::fs::rename(&tmp, &site_conf).map_err(|e| {
        std::fs::remove_file(&tmp).ok();
        api_err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}"))
    })?;

    match services::nginx::test_config().await {
        Ok(output) if output.success => {
            if let Err(e) = services::nginx::reload().await {
                return Err(api_err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("Config saved but nginx reload failed: {e}"),
                ));
            }
        }
        _ => {
            // Revert
            std::fs::write(&site_conf, &content).ok();
            if let Err(e) = services::nginx::reload().await {
                tracing::warn!("Nginx reload failed during alias rollback for {}: {e}", body.domain);
            }
            return Err(api_err(
                StatusCode::BAD_REQUEST,
                "Nginx test failed — alias reverted",
            ));
        }
    }

    tracing::info!("Domain alias added: {} → {}", body.alias, body.domain);
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /nginx/aliases/{domain} — List domain aliases.
async fn list_aliases(Path(domain): Path<String>) -> Result<Json<serde_json::Value>, ApiErr> {
    if !super::is_valid_domain(&domain) {
        return Err(api_err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    let site_conf = format!("{}/{domain}.conf", services::nginx::sites_dir());
    let content = std::fs::read_to_string(&site_conf).unwrap_or_default();

    let mut aliases: Vec<String> = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("server_name ") {
            let names_part = trimmed
                .strip_prefix("server_name ")
                .unwrap_or("")
                .trim_end_matches(';')
                .trim();
            for name in names_part.split_whitespace() {
                if name != domain {
                    aliases.push(name.to_string());
                }
            }
        }
    }

    // Deduplicate
    aliases.sort();
    aliases.dedup();

    Ok(Json(serde_json::json!({ "aliases": aliases })))
}

/// POST /nginx/aliases/{domain}/remove — Remove a domain alias.
async fn remove_alias(
    Path(domain): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !super::is_valid_domain(&domain) {
        return Err(api_err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    let alias = body
        .get("alias")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if alias.is_empty() {
        return Err(api_err(StatusCode::BAD_REQUEST, "Alias required"));
    }

    let site_conf = format!("{}/{domain}.conf", services::nginx::sites_dir());
    let content = std::fs::read_to_string(&site_conf)
        .map_err(|_| api_err(StatusCode::NOT_FOUND, "Site config not found"))?;

    // Remove alias from server_name lines
    let new_content = content
        .replace(&format!(" {alias}"), "")
        .replace(&format!("{alias} "), "");

    std::fs::write(&site_conf, &new_content).ok();

    match services::nginx::test_config().await {
        Ok(output) if output.success => {
            if let Err(e) = services::nginx::reload().await {
                return Err(api_err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("Config saved but nginx reload failed: {e}"),
                ));
            }
        }
        _ => {
            std::fs::write(&site_conf, &content).ok();
            if let Err(e) = services::nginx::reload().await {
                tracing::warn!("Nginx reload failed during alias removal rollback for {domain}: {e}");
            }
        }
    }

    tracing::info!("Domain alias removed: {alias} from {domain}");
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ──────────────────────────────────────────────────────────────
// Site Logs & Stats
// ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct LogQuery {
    lines: Option<usize>,
    log_type: Option<String>, // "access" or "error"
}

/// GET /nginx/site-logs/{domain} — Get nginx access or error logs for a site.
async fn site_logs(
    Path(domain): Path<String>,
    axum::extract::Query(params): axum::extract::Query<LogQuery>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !super::is_valid_domain(&domain) {
        return Err(api_err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    let lines = params.lines.unwrap_or(200).min(1000);
    let log_type = params.log_type.as_deref().unwrap_or("access");

    let log_file = match log_type {
        "error" => format!("/var/log/nginx/{domain}.error.log"),
        _ => format!("/var/log/nginx/{domain}.access.log"),
    };

    if !std::path::Path::new(&log_file).exists() {
        return Ok(Json(serde_json::json!({ "logs": "", "lines": 0, "file": log_file })));
    }

    let output = safe_command("tail")
        .args(["-n", &lines.to_string(), &log_file])
        .output()
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")))?;

    let content = String::from_utf8_lossy(&output.stdout).to_string();
    let line_count = content.lines().count();

    Ok(Json(serde_json::json!({ "logs": content, "lines": line_count, "file": log_file })))
}

/// GET /nginx/last-activity/{domain} — When this domain last served a request.
///
/// The access log's mtime, as seconds since the epoch. That is deliberately not
/// a parse of the log: the only consumer is the auto-sleep timer, which asks
/// "has anyone visited recently?" every two minutes for every configured
/// container, and reading the last write time of a file answers that exactly,
/// in constant time, no matter how large the log has grown.
///
/// `seconds_ago` is `null` when the domain has no access log at all — a vhost
/// that has never been written, or an app with no domain. That is "unknown",
/// not "idle", and callers must treat it as such: reporting a silent zero here
/// is what would let a timer stop a container it knows nothing about.
async fn last_activity(Path(domain): Path<String>) -> Result<Json<serde_json::Value>, ApiErr> {
    if !super::is_valid_domain(&domain) {
        return Err(api_err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    let log_file = format!("/var/log/nginx/{domain}.access.log");

    let seconds_ago = std::fs::metadata(&log_file)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .map(|d| d.as_secs());

    Ok(Json(serde_json::json!({
        "domain": domain,
        "seconds_ago": seconds_ago,
        "log_file": log_file,
    })))
}

/// GET /nginx/site-stats/{domain} — Basic traffic stats from access log.
async fn site_stats(Path(domain): Path<String>) -> Result<Json<serde_json::Value>, ApiErr> {
    if !super::is_valid_domain(&domain) {
        return Err(api_err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    let log_file = format!("/var/log/nginx/{domain}.access.log");

    if !std::path::Path::new(&log_file).exists() {
        return Ok(Json(serde_json::json!({ "requests": 0, "bandwidth": 0, "unique_ips": 0, "top_pages": [], "status_codes": {} })));
    }

    // Read last 10000 lines for stats
    let output = safe_command("tail")
        .args(["-n", "10000", &log_file])
        .output()
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")))?;

    let content = String::from_utf8_lossy(&output.stdout);
    let mut requests = 0u32;
    let mut bandwidth: u64 = 0;
    let mut ips = std::collections::HashSet::new();
    let mut pages: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut status_codes: std::collections::HashMap<String, u32> = std::collections::HashMap::new();

    for line in content.lines() {
        if line.is_empty() {
            continue;
        }
        requests += 1;

        // Parse nginx combined log format:
        // IP - - [date] "METHOD /path HTTP/x.x" STATUS SIZE "referrer" "user-agent"
        let parts: Vec<&str> = line.splitn(2, ' ').collect();
        if let Some(ip) = parts.first() {
            ips.insert(ip.to_string());
        }

        // Extract status code and size
        if let Some(rest) = line.split("\" ").nth(1) {
            let fields: Vec<&str> = rest.split_whitespace().collect();
            if let Some(status) = fields.first() {
                *status_codes.entry(status.to_string()).or_insert(0) += 1;
            }
            if let Some(size) = fields.get(1) {
                if let Ok(s) = size.parse::<u64>() {
                    bandwidth += s;
                }
            }
        }

        // Extract path
        if let Some(request_line) = line.split('"').nth(1) {
            let req_parts: Vec<&str> = request_line.split_whitespace().collect();
            if let Some(path) = req_parts.get(1) {
                let clean_path = path.split('?').next().unwrap_or(path);
                if clean_path != "/favicon.ico" && !clean_path.starts_with("/api/") {
                    *pages.entry(clean_path.to_string()).or_insert(0) += 1;
                }
            }
        }
    }

    // Top 10 pages
    let mut top_pages: Vec<(&String, &u32)> = pages.iter().collect();
    top_pages.sort_by(|a, b| b.1.cmp(a.1));
    let top: Vec<serde_json::Value> = top_pages
        .iter()
        .take(10)
        .map(|(path, count)| serde_json::json!({ "path": path, "count": count }))
        .collect();

    Ok(Json(serde_json::json!({
        "requests": requests,
        "bandwidth": bandwidth,
        "bandwidth_mb": (bandwidth as f64 / 1024.0 / 1024.0 * 10.0).round() / 10.0,
        "unique_ips": ips.len(),
        "top_pages": top,
        "status_codes": status_codes,
    })))
}

/// GET /nginx/php-errors/{domain} — Get PHP-FPM error log for a site.
async fn php_errors(
    Path(domain): Path<String>,
    axum::extract::Query(params): axum::extract::Query<LogQuery>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    let lines = params.lines.unwrap_or(100).min(500);

    // PHP error log locations to check
    let pool_name = domain.replace('.', "_");
    let log_candidates = [
        format!("/var/log/php-fpm/{pool_name}.error.log"),
        format!("/var/log/php-fpm/{domain}.error.log"),
        format!("/var/www/{domain}/storage/logs/laravel.log"),
        "/var/log/php8.3-fpm.log".to_string(),
        "/var/log/php8.2-fpm.log".to_string(),
    ];

    let mut content = String::new();
    let mut found_file = String::new();

    for candidate in &log_candidates {
        if std::path::Path::new(candidate).exists() {
            let output = safe_command("tail")
                .args(["-n", &lines.to_string(), candidate])
                .output()
                .await;
            if let Ok(out) = output {
                let text = String::from_utf8_lossy(&out.stdout).to_string();
                if !text.trim().is_empty() {
                    content = text;
                    found_file = candidate.clone();
                    break;
                }
            }
        }
    }

    Ok(Json(serde_json::json!({ "logs": content, "file": found_file, "lines": content.lines().count() })))
}

// ──────────────────────────────────────────────────────────────
// Site Cloning
// ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct CloneRequest {
    source_domain: String,
    target_domain: String,
}

/// POST /nginx/clone-site — Clone site files from one domain to another.
async fn clone_site(Json(body): Json<CloneRequest>) -> Result<Json<serde_json::Value>, ApiErr> {
    if body.source_domain.is_empty() || body.target_domain.is_empty() {
        return Err(api_err(StatusCode::BAD_REQUEST, "Source and target domains required"));
    }
    if !is_valid_domain(&body.source_domain) || !is_valid_domain(&body.target_domain) {
        return Err(api_err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }

    let target_dir = format!("/var/www/{}", body.target_domain);

    // The actual copy — source-existence check, target dir creation, `rsync -a
    // --delete`, and the www-data/`.git` ownership fixup — lives in
    // `services::staging::clone_files`, which also backs `POST /staging/clone`.
    // This used to be a second, inline copy of that same logic; the 300s cap
    // stays here as the request-level SLA guard it's always been. `is_valid_domain`
    // above is strictly narrower than `clone_files`'s own internal domain check
    // (it requires dotted, length-capped labels), so anything that reaches this
    // point already satisfies `clone_files`'s validation too.
    tokio::time::timeout(
        std::time::Duration::from_secs(300),
        services::staging::clone_files(&body.source_domain, &body.target_domain),
    ).await
        .map_err(|_| api_err(StatusCode::GATEWAY_TIMEOUT, "Clone timed out (300s)"))?
        .map_err(|e| {
            let status = if e.contains("directory not found") {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            api_err(status, &e)
        })?;

    // Get size
    let du_output = safe_command("du").args(["-sb", &target_dir]).output().await;
    let size: u64 = du_output.ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().next().unwrap_or("0").parse().unwrap_or(0))
        .unwrap_or(0);

    tracing::info!("Site cloned: {} → {} ({} bytes)", body.source_domain, body.target_domain, size);
    Ok(Json(serde_json::json!({ "ok": true, "size": size })))
}

// ──────────────────────────────────────────────────────────────
// Environment Variables
// ──────────────────────────────────────────────────────────────

/// GET /nginx/env/{domain} — Read .env file for a site.
async fn get_env(Path(domain): Path<String>) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(api_err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    let env_path = format!("/var/www/{domain}/.env");
    let content = std::fs::read_to_string(&env_path).unwrap_or_default();

    let vars: Vec<serde_json::Value> = content.lines()
        .filter(|l| !l.trim().is_empty() && !l.trim().starts_with('#'))
        .filter_map(|l| {
            let eq = l.find('=')?;
            let key = l[..eq].trim().to_string();
            let value = l[eq+1..].trim().trim_matches('"').trim_matches('\'').to_string();
            Some(serde_json::json!({ "key": key, "value": value }))
        })
        .collect();

    Ok(Json(serde_json::json!({ "vars": vars, "raw": content })))
}

/// PUT /nginx/env/{domain} — Write .env file for a site.
async fn set_env(Path(domain): Path<String>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(api_err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    let vars = body.get("vars").and_then(|v| v.as_array());

    let content = match vars {
        Some(vars) => {
            vars.iter().filter_map(|v| {
                let key = v.get("key")?.as_str()?;
                let value = v.get("value")?.as_str()?;
                if key.is_empty() { return None; }
                // Quote values with spaces
                if value.contains(' ') || value.contains('"') {
                    Some(format!("{key}=\"{value}\""))
                } else {
                    Some(format!("{key}={value}"))
                }
            }).collect::<Vec<_>>().join("\n") + "\n"
        }
        None => body.get("raw").and_then(|v| v.as_str()).unwrap_or("").to_string(),
    };

    let env_path = format!("/var/www/{domain}/.env");
    std::fs::write(&env_path, &content)
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")))?;

    // Fix ownership
    let _ = safe_command("chown").args(["www-data:www-data", &env_path]).output().await;

    // Restart the app service if it's a Node/Python site — if the unit at that
    // name is in fact this site's. The name is not injective.
    if let Some(svc) = services::app_process::owned_service_name(&domain) {
        let _ = safe_command("systemctl").args(["restart", &svc]).output().await;
    }

    tracing::info!("Environment variables updated for {domain}");
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /nginx/sites/{domain}/disable — Disable a site by replacing its nginx config with a 503 page.
async fn disable_site(Path(domain): Path<String>) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if !is_valid_domain(&domain) {
        return Err((StatusCode::BAD_REQUEST, "Invalid domain".into()));
    }
    let (conf_path, backup_path) = services::nginx::vhost_paths(&domain);

    // Check config exists
    if !std::path::Path::new(&conf_path).exists() {
        return Err((StatusCode::NOT_FOUND, format!("No config for {domain}")));
    }

    // Back up the current config
    let live = std::fs::read_to_string(&conf_path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Read config: {e}")))?;
    std::fs::copy(&conf_path, &backup_path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Backup config: {e}")))?;

    // The maintenance response has to answer where the SITE answered. Naming the
    // addresses here instead of taking them from the body being parked is what
    // made disabling a site do nothing at all on any install whose panel knows
    // its own address — see `services::nginx::listener_lines`.
    let mut listeners = services::nginx::listener_lines(&live);
    if listeners.trim().is_empty() {
        listeners = "    listen 80;\n    listen [::]:80;\n".to_string();
    }

    // Write a 503 maintenance page config
    let disabled_conf = format!(
        r#"server {{
{listeners}    server_name {domain} www.{domain};
    return 503;
    error_page 503 @maintenance;
    location @maintenance {{
        default_type text/html;
        return 503 '<html><body style="font-family:sans-serif;display:flex;justify-content:center;align-items:center;height:100vh;margin:0;background:#1a1a2e;color:#e0e0e0"><div style="text-align:center"><h1>Site Disabled</h1><p>{domain} is currently disabled by the administrator.</p></div></body></html>';
    }}
}}
"#
    );

    std::fs::write(&conf_path, &disabled_conf)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Write disabled config: {e}")))?;

    // Stop app process if it exists (node/python) — and is this site's.
    if let Some(svc) = services::app_process::owned_service_name(&domain) {
        let _ = safe_command("systemctl").args(["stop", &svc]).output().await;
    }

    // Reload nginx
    services::nginx::reload().await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Nginx reload: {e}")))?;

    tracing::info!("Site disabled: {domain}");
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /nginx/sites/{domain}/enable — Re-enable a disabled site by restoring its nginx config.
async fn enable_site(Path(domain): Path<String>) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if !is_valid_domain(&domain) {
        return Err((StatusCode::BAD_REQUEST, "Invalid domain".into()));
    }
    let (conf_path, backup_path) = services::nginx::vhost_paths(&domain);

    // Restore the backed-up config
    if !std::path::Path::new(&backup_path).exists() {
        return Err((StatusCode::NOT_FOUND, format!("No disabled config backup for {domain}")));
    }

    // The backup holds the body as it was when the site was disabled. For a site
    // switched php→static under v2.67.0 and disabled before the operator upgraded,
    // that is the UNPATCHED body — and the startup retrofit cannot reach it: it
    // scans sites-enabled for `*.conf`, while this is `*.conf.disabled` in
    // sites-available, failing both tests. A plain copy therefore re-arms the
    // disclosure v2.68.0 shipped for, at the exact moment the body goes back into
    // service. Patch on the way in: restoring is the only path by which a disabled
    // body is ever served, so this is where that population is reachable.
    let backup = std::fs::read_to_string(&backup_path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Read disabled config: {e}")))?;
    let patched = services::nginx::patch_static_vhost(&backup);
    if patched.is_some() {
        tracing::info!("Retrofitted static denies onto {domain} while re-enabling it");
    }
    std::fs::write(&conf_path, patched.as_deref().unwrap_or(&backup))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Restore config: {e}")))?;
    std::fs::remove_file(&backup_path).ok();

    // Restart app process if it exists (node/python) — and is this site's.
    if let Some(svc) = services::app_process::owned_service_name(&domain) {
        let _ = safe_command("systemctl").args(["restart", &svc]).output().await;
    }

    // Reload nginx
    services::nginx::reload().await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Nginx reload: {e}")))?;

    tracing::info!("Site enabled: {domain}");
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /nginx/sites/{domain}/cache/purge — Purge FastCGI cache for a site.
async fn purge_cache(
    Path(domain): Path<String>,
) -> Result<Json<NginxResponse>, (StatusCode, Json<NginxResponse>)> {
    if !is_valid_domain(&domain) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(NginxResponse {
                success: false,
                message: "Invalid domain format".into(),
            }),
        ));
    }

    let cache_dir = format!(
        "/var/cache/nginx/fastcgi/{}",
        domain.replace('.', "_")
    );

    if !std::path::Path::new(&cache_dir).exists() {
        return Ok(Json(NginxResponse {
            success: true,
            message: "No cache directory found (cache may not be enabled)".into(),
        }));
    }

    // Remove all files in the cache directory
    match std::fs::remove_dir_all(&cache_dir) {
        Ok(_) => {
            // Recreate the empty directory
            std::fs::create_dir_all(&cache_dir).ok();
            let _ = safe_command_sync("chown")
                .args(["www-data:www-data", &cache_dir])
                .output();
            tracing::info!("FastCGI cache purged for {domain}");
            Ok(Json(NginxResponse {
                success: true,
                message: format!("FastCGI cache purged for {domain}"),
            }))
        }
        Err(e) => {
            tracing::warn!("Failed to purge cache for {domain}: {e}");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(NginxResponse {
                    success: false,
                    message: format!("Failed to purge cache: {e}"),
                }),
            ))
        }
    }
}

// ── Redis object cache per site ─────────────────────────────────────

#[derive(Deserialize)]
struct RedisEnableBody {
    redis_db: i32,
    php_preset: Option<String>,
}

/// POST /nginx/sites/{domain}/redis/enable — Configure Redis object cache for a PHP site.
async fn redis_enable(
    Path(domain): Path<String>,
    Json(body): Json<RedisEnableBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_domain(&domain) {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Invalid domain"}))));
    }

    let redis_db = body.redis_db.clamp(0, 15);

    // Verify Redis is running
    let check = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        safe_command("redis-cli").args(["ping"]).output(),
    ).await;

    match check {
        Ok(Ok(out)) if String::from_utf8_lossy(&out.stdout).trim() == "PONG" => {}
        _ => {
            return Err((StatusCode::PRECONDITION_FAILED, Json(serde_json::json!({
                "error": "Redis is not running. Install Redis first from the Services page."
            }))));
        }
    }

    // For WordPress sites, configure wp-config.php
    if body.php_preset.as_deref() == Some("wordpress") {
        let wp_config = format!("/var/www/{domain}/public/wp-config.php");
        if std::path::Path::new(&wp_config).exists() {
            // Read current wp-config
            let content = std::fs::read_to_string(&wp_config)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": format!("Read wp-config: {e}")}))))?;

            // Remove any existing Redis config lines
            let cleaned: Vec<&str> = content.lines()
                .filter(|l| !l.contains("WP_REDIS_") && !l.contains("WP_CACHE_KEY_SALT"))
                .collect();

            // Insert Redis config before "That's all, stop editing!" or before first require
            let redis_config = format!(
                "// Redis Object Cache (managed by DockPanel)\n\
                 define('WP_REDIS_HOST', '127.0.0.1');\n\
                 define('WP_REDIS_PORT', 6379);\n\
                 define('WP_REDIS_DATABASE', {redis_db});\n\
                 define('WP_CACHE_KEY_SALT', '{domain}:');\n\
                 define('WP_REDIS_TIMEOUT', 1);\n\
                 define('WP_REDIS_READ_TIMEOUT', 1);\n"
            );

            let joined = cleaned.join("\n");
            let new_content = if let Some(pos) = joined.find("/* That's all, stop editing!") {
                format!("{}\n{}\n{}", &joined[..pos], redis_config, &joined[pos..])
            } else if let Some(pos) = joined.find("require_once") {
                format!("{}\n{}\n{}", &joined[..pos], redis_config, &joined[pos..])
            } else {
                format!("{redis_config}\n{joined}")
            };

            std::fs::write(&wp_config, new_content)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": format!("Write wp-config: {e}")}))))?;

            // Install Redis Object Cache drop-in if not present
            let dropin = format!("/var/www/{domain}/public/wp-content/object-cache.php");
            if !std::path::Path::new(&dropin).exists() {
                // Use wp-cli to install the Redis Object Cache plugin
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(30),
                    safe_command("sudo")
                        .args(["-u", "www-data", "wp", "plugin", "install", "redis-cache",
                               "--activate", "--skip-plugins", "--skip-themes",
                               &format!("--path=/var/www/{domain}/public")])
                        .output(),
                ).await;

                // Enable the Redis drop-in
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(15),
                    safe_command("sudo")
                        .args(["-u", "www-data", "wp", "redis", "enable",
                               "--skip-plugins", "--skip-themes",
                               &format!("--path=/var/www/{domain}/public")])
                        .output(),
                ).await;
            }

            tracing::info!("WordPress Redis configured for {domain} (db: {redis_db})");
        }
    }

    tracing::info!("Redis object cache enabled for {domain} (db: {redis_db})");
    Ok(Json(serde_json::json!({
        "ok": true,
        "redis_db": redis_db,
    })))
}

/// POST /nginx/sites/{domain}/redis/disable — Remove Redis object cache config.
async fn redis_disable(
    Path(domain): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_domain(&domain) {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Invalid domain"}))));
    }

    // For WordPress, remove Redis config from wp-config.php
    let wp_config = format!("/var/www/{domain}/public/wp-config.php");
    if std::path::Path::new(&wp_config).exists() {
        if let Ok(content) = std::fs::read_to_string(&wp_config) {
            let cleaned: Vec<&str> = content.lines()
                .filter(|l| {
                    !l.contains("WP_REDIS_") &&
                    !l.contains("WP_CACHE_KEY_SALT") &&
                    !l.contains("// Redis Object Cache (managed by DockPanel)")
                })
                .collect();
            let _ = std::fs::write(&wp_config, cleaned.join("\n"));
        }

        // Disable the Redis drop-in
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            safe_command("sudo")
                .args(["-u", "www-data", "wp", "redis", "disable",
                       "--skip-plugins", "--skip-themes",
                       &format!("--path=/var/www/{domain}/public")])
                .output(),
        ).await;
    }

    tracing::info!("Redis object cache disabled for {domain}");
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /nginx/sites/{domain}/redis/purge — Flush the Redis DB for a site.
async fn redis_purge(
    Path(domain): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_domain(&domain) {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Invalid domain"}))));
    }

    let redis_db = body.get("redis_db").and_then(|v| v.as_i64()).unwrap_or(0);
    if !(0..=15).contains(&redis_db) {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Invalid Redis DB (0-15)"}))));
    }

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        safe_command("redis-cli")
            .args(["-n", &redis_db.to_string(), "FLUSHDB"])
            .output(),
    ).await;

    match result {
        Ok(Ok(out)) if out.status.success() => {
            tracing::info!("Redis DB {redis_db} flushed for {domain}");
            Ok(Json(serde_json::json!({
                "ok": true,
                "message": format!("Redis DB {redis_db} flushed for {domain}"),
            })))
        }
        Ok(Ok(out)) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": format!("redis-cli failed: {stderr}")}))))
        }
        Ok(Err(e)) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": format!("redis-cli: {e}")})))),
        Err(_) => Err((StatusCode::GATEWAY_TIMEOUT, Json(serde_json::json!({"error": "Redis flush timed out"})))),
    }
}

// ── WAF (ModSecurity) per-site configuration ───────────────────────

/// POST /nginx/sites/{domain}/waf/configure — Generate per-site ModSecurity config.
async fn waf_configure(
    Path(domain): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_domain(&domain) {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Invalid domain"}))));
    }

    // Verify ModSecurity is installed
    if !std::path::Path::new("/etc/modsecurity/modsecurity.conf").exists() {
        return Err((StatusCode::PRECONDITION_FAILED, Json(serde_json::json!({
            "error": "WAF not installed. Install it from the Services page first."
        }))));
    }

    let mode = body.get("mode").and_then(|v| v.as_str()).unwrap_or("detection");
    if mode != "detection" && mode != "prevention" {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Mode must be 'detection' or 'prevention'"}))));
    }
    let engine = if mode == "prevention" { "On" } else { "DetectionOnly" };

    let safe_domain = domain.replace('.', "_");
    let log_dir = "/var/log/modsecurity";
    std::fs::create_dir_all(log_dir).ok();
    std::fs::create_dir_all("/etc/modsecurity/sites").ok();

    // Generate per-site config
    let crs_dir = "/etc/modsecurity/crs";
    let has_crs = std::path::Path::new(&format!("{crs_dir}/crs-setup.conf")).exists();

    let config = format!(
        "# WAF config for {domain} (managed by DockPanel)\n\
         Include /etc/modsecurity/modsecurity.conf\n\
         SecRuleEngine {engine}\n\
         SecAuditLog {log_dir}/{safe_domain}_audit.log\n\
         SecAuditLogType Serial\n\
         {crs_include}\n\
         # Allow larger request bodies for file uploads\n\
         SecRequestBodyLimit 52428800\n",
        crs_include = if has_crs {
            format!(
                "Include {crs_dir}/crs-setup.conf\n\
                 Include {crs_dir}/rules/*.conf"
            )
        } else {
            "# OWASP CRS not found — basic protection only".to_string()
        }
    );

    let config_path = format!("/etc/modsecurity/sites/{safe_domain}.conf");
    std::fs::write(&config_path, &config)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": format!("Write config: {e}")}))))?;

    tracing::info!("WAF configured for {domain} (mode: {mode})");
    Ok(Json(serde_json::json!({
        "ok": true,
        "mode": mode,
        "config_path": config_path,
    })))
}

/// GET /nginx/sites/{domain}/waf/logs — Get recent WAF events.
async fn waf_logs(
    Path(domain): Path<String>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_domain(&domain) {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Invalid domain"}))));
    }

    let safe_domain = domain.replace('.', "_");
    let log_path = format!("/var/log/modsecurity/{safe_domain}_audit.log");
    let limit: usize = params.get("limit").and_then(|v| v.parse().ok()).unwrap_or(50);

    if !std::path::Path::new(&log_path).exists() {
        return Ok(Json(serde_json::json!({
            "events": [],
            "total": 0,
        })));
    }

    // Read last N KB of the log and parse events
    let content = match tokio::fs::read_to_string(&log_path).await {
        Ok(c) => c,
        Err(_) => return Ok(Json(serde_json::json!({ "events": [], "total": 0 }))),
    };

    // ModSecurity audit log format: sections separated by "---xxx---"
    // Parse the last `limit` entries
    let mut events: Vec<serde_json::Value> = Vec::new();
    let mut current_event = String::new();
    let mut in_event = false;

    for line in content.lines().rev().take(10000) {
        if line.starts_with("--") && line.ends_with("-A--") {
            // Start of a new event boundary (reading backwards)
            if !current_event.is_empty() {
                if let Some(evt) = parse_modsec_event(&current_event) {
                    events.push(evt);
                    if events.len() >= limit {
                        break;
                    }
                }
                current_event.clear();
            }
            in_event = true;
        }
        if in_event {
            current_event = format!("{line}\n{current_event}");
        }
        if line.starts_with("--") && line.ends_with("-Z--") {
            in_event = false;
        }
    }

    // Handle last event
    if !current_event.is_empty() {
        if let Some(evt) = parse_modsec_event(&current_event) {
            events.push(evt);
        }
    }

    let total = events.len();
    Ok(Json(serde_json::json!({
        "events": events,
        "total": total,
    })))
}

fn parse_modsec_event(raw: &str) -> Option<serde_json::Value> {
    let mut timestamp = "";
    let mut request_line = "";
    let mut uri = "";
    let mut client_ip = "";
    let mut rule_msg = "";
    let mut severity = "";
    let mut action = "";

    for line in raw.lines() {
        if line.starts_with('[') && timestamp.is_empty() {
            timestamp = line;
        }
        if let Some(rest) = line.strip_prefix("GET ").or(line.strip_prefix("POST ").or(line.strip_prefix("PUT ").or(line.strip_prefix("DELETE ")))) {
            request_line = line;
            uri = rest.split_whitespace().next().unwrap_or("");
        }
        if line.contains("client:") || line.contains("client ") {
            if let Some(ip) = line.split("client:").nth(1).or(line.split("client ").nth(1)) {
                client_ip = ip.trim().split(',').next().unwrap_or("").trim();
            }
        }
        if line.contains("[msg \"") {
            if let Some(msg) = line.split("[msg \"").nth(1) {
                rule_msg = msg.split('"').next().unwrap_or("");
            }
        }
        if line.contains("[severity \"") {
            if let Some(sev) = line.split("[severity \"").nth(1) {
                severity = sev.split('"').next().unwrap_or("");
            }
        }
        if line.contains("Action:") {
            action = if line.contains("Intercepted") { "blocked" } else { "logged" };
        }
    }

    if rule_msg.is_empty() && request_line.is_empty() {
        return None;
    }

    Some(serde_json::json!({
        "timestamp": timestamp,
        "client_ip": client_ip,
        "request": request_line,
        "uri": uri,
        "rule_message": rule_msg,
        "severity": severity,
        "action": if action.is_empty() { "logged" } else { action },
    }))
}

// ── Image Optimization ──────────────────────────────────────────────

/// POST /nginx/sites/{domain}/optimize-images — Convert images to WebP/AVIF.
async fn optimize_images(
    Path(domain): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_domain(&domain) {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Invalid domain"}))));
    }

    // Where this site's files actually are. node/python/proxy serve from the site
    // root; static and php nest under `public/`. This hardcoded `public/`, which
    // for a node or python site is not where the images are — and before v2.123.0
    // that directory existed and was empty (`create_app_service` made it), so the
    // WebP/AVIF buttons reported a clean run over zero files rather than failing.
    // An absent `runtime` means an older backend, and keeps the previous behaviour.
    let runtime = body.get("runtime").and_then(|v| v.as_str());
    let site_dir = crate::services::nginx::document_root_for(
        &format!("/var/www/{domain}"),
        runtime.unwrap_or("static"),
    );
    if !std::path::Path::new(&site_dir).exists() {
        return Err((StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "Site directory not found"}))));
    }

    let format = body.get("format").and_then(|v| v.as_str()).unwrap_or("webp");
    if format != "webp" && format != "avif" {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Format must be 'webp' or 'avif'"}))));
    }
    let quality = body.get("quality").and_then(|v| v.as_u64()).unwrap_or(80);
    let quality = quality.clamp(1, 100);

    // Check if tools are installed
    let tool = match format {
        "avif" => "avifenc",
        _ => "cwebp",
    };

    let has_tool = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        safe_command("which").arg(tool).output()
    ).await.ok().and_then(|r| r.ok()).map(|o| o.status.success()).unwrap_or(false);

    if !has_tool {
        // Try to install
        let pkg = match format {
            "avif" => "libavif-bin",
            _ => "webp",
        };
        let install = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            safe_command("sh")
                .args(["-c", &format!("DEBIAN_FRONTEND=noninteractive apt-get install -y {pkg}")])
                .output()
        ).await;

        if install.ok().and_then(|r| r.ok()).map(|o| o.status.success()).unwrap_or(false) {
            tracing::info!("Installed {pkg} for image optimization");
        } else {
            return Err((StatusCode::PRECONDITION_FAILED, Json(serde_json::json!({
                "error": format!("{tool} not available. Install '{pkg}' package.")
            }))));
        }
    }

    // Find images and convert
    let _ext_pattern = "jpg,jpeg,png";
    let find_cmd = format!(
        "find {site_dir} -type f \\( -name '*.jpg' -o -name '*.jpeg' -o -name '*.png' \\) -size +1k 2>/dev/null | head -500"
    );

    let find_output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        safe_command("sh").args(["-c", &find_cmd]).output()
    ).await
        .map_err(|_| (StatusCode::GATEWAY_TIMEOUT, Json(serde_json::json!({"error": "Find timed out"}))))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": format!("Find: {e}")}))))?;

    let files: Vec<&str> = std::str::from_utf8(&find_output.stdout)
        .unwrap_or("")
        .lines()
        .filter(|l| !l.is_empty())
        .collect();

    let total = files.len();
    let mut converted = 0u32;
    let mut skipped = 0u32;
    let mut saved_bytes: i64 = 0;

    for file in &files {
        let output_file = match format {
            "avif" => format!("{file}.avif"),
            _ => format!("{file}.webp"),
        };

        // Skip if already converted
        if std::path::Path::new(&output_file).exists() {
            skipped += 1;
            continue;
        }

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            match format {
                "avif" => safe_command("avifenc")
                    .args(["--min", "0", "--max", "63",
                           "-a", "end-usage=q",
                           "-a", &format!("cq-level={}", 100 - quality),
                           "-a", "tune=ssim",
                           file, &output_file])
                    .output(),
                _ => safe_command("cwebp")
                    .args(["-q", &quality.to_string(), file, "-o", &output_file])
                    .output(),
            }
        ).await;

        if result.ok().and_then(|r| r.ok()).map(|o| o.status.success()).unwrap_or(false) {
            // Calculate savings
            if let (Ok(orig), Ok(opt)) = (std::fs::metadata(file), std::fs::metadata(&output_file)) {
                saved_bytes += orig.len() as i64 - opt.len() as i64;
            }
            converted += 1;
        }
    }

    let saved_mb = saved_bytes as f64 / 1048576.0;
    tracing::info!("Image optimization for {domain}: {converted}/{total} converted ({format}), {saved_mb:.1}MB saved");

    Ok(Json(serde_json::json!({
        "ok": true,
        "format": format,
        "quality": quality,
        "total_images": total,
        "converted": converted,
        "skipped": skipped,
        "saved_bytes": saved_bytes,
        "saved_mb": format!("{saved_mb:.1}"),
    })))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/nginx/sites/{domain}", put(put_site))
        .route("/nginx/sites/{domain}", delete(delete_site))
        .route("/nginx/sites/{domain}", get(get_site))
        .route("/nginx/sites/{domain}/rename", post(rename_site))
        .route("/nginx/sites/{domain}/disable", post(disable_site))
        .route("/nginx/sites/{domain}/enable", post(enable_site))
        .route("/nginx/sites/{domain}/cache/purge", post(purge_cache))
        .route("/nginx/sites/{domain}/redis/enable", post(redis_enable))
        .route("/nginx/sites/{domain}/redis/disable", post(redis_disable))
        .route("/nginx/sites/{domain}/redis/purge", post(redis_purge))
        .route("/nginx/sites/{domain}/waf/configure", post(waf_configure))
        .route("/nginx/sites/{domain}/waf/logs", get(waf_logs))
        .route("/nginx/sites/{domain}/optimize-images", post(optimize_images))
        .route("/nginx/test", post(test_nginx))
        .route("/nginx/reload", post(reload_nginx))
        // Redirects
        .route("/nginx/redirects/add", post(add_redirect))
        .route("/nginx/redirects/{domain}", get(list_redirects))
        .route(
            "/nginx/redirects/{domain}/remove",
            post(remove_redirect),
        )
        // Password Protection
        .route("/nginx/password-protect", post(password_protect))
        .route(
            "/nginx/password-protect/{domain}",
            get(list_protected),
        )
        .route(
            "/nginx/password-protect/{domain}/remove",
            post(remove_protection),
        )
        // Domain Aliases
        .route("/nginx/aliases/add", post(add_alias))
        .route("/nginx/aliases/{domain}", get(list_aliases))
        .route("/nginx/aliases/{domain}/remove", post(remove_alias))
        // Site Logs & Stats
        .route("/nginx/site-logs/{domain}", get(site_logs))
        .route("/nginx/site-stats/{domain}", get(site_stats))
        .route("/nginx/last-activity/{domain}", get(last_activity))
        .route("/nginx/php-errors/{domain}", get(php_errors))
        // Site Cloning
        .route("/nginx/clone-site", post(clone_site))
        // Environment Variables
        .route("/nginx/env/{domain}", get(get_env).put(set_env))
}
