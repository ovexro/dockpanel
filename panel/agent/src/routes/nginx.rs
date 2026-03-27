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
    Json(config): Json<SiteConfig>,
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
            if !std::path::Path::new(socket_path).exists() {
                // Extract version for a helpful error message
                let version = socket_path
                    .strip_prefix("/run/php/php")
                    .and_then(|s| s.strip_suffix("-fpm.sock"))
                    .unwrap_or("unknown");
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(NginxResponse {
                        success: false,
                        message: format!(
                            "PHP {version} is not installed or PHP-FPM is not running. \
                             Install it from Settings > Services before creating a PHP site."
                        ),
                    }),
                ));
            }
        }

        // Write PHP-FPM pool config if PHP site with resource limits
        if let Some(ref socket) = config.php_socket {
            // Extract PHP version from socket path (e.g., "unix:/run/php/php8.4-fpm.sock" → "8.4")
            if let Some(ver) = socket.strip_prefix("unix:/run/php/php").and_then(|s| s.strip_suffix("-fpm.sock")) {
                let memory = config.php_memory_mb.unwrap_or(256);
                let workers = config.php_max_workers.unwrap_or(5);
                if let Err(e) = services::nginx::write_php_pool_config(&domain, ver, memory, workers) {
                    tracing::warn!("Failed to write PHP pool config for {domain}: {e}");
                }
            }
        }
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

    // Write config file atomically (write to .tmp, then rename)
    let config_path = format!("/etc/nginx/sites-enabled/{domain}.conf");
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
    let actual_root = match config.runtime.as_str() {
        "proxy" | "node" | "python" => doc_root.to_string(),
        _ => format!("{doc_root}/public"),
    };
    if let Err(e) = std::fs::create_dir_all(&actual_root) {
        tracing::warn!("Failed to create document root {actual_root}: {e}");
    } else {
        let index_path = format!("{actual_root}/index.html");
        if !std::path::Path::new(&index_path).exists() {
            let _ = std::fs::write(&index_path, format!(
                "<!DOCTYPE html><html><head><title>{domain}</title></head>\
                 <body><h1>Welcome to {domain}</h1>\
                 <p>Site is ready. Upload your files to replace this page.</p></body></html>"
            ));
        }
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

            let jail_path = format!("/etc/fail2ban/jail.d/{jail_name}.conf");
            if let Ok(()) = std::fs::write(&jail_path, &jail_config) {
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
            // Invalid config — remove it and restore
            std::fs::remove_file(&config_path).ok();
            Err((
                StatusCode::BAD_REQUEST,
                Json(NginxResponse {
                    success: false,
                    message: format!("Nginx config test failed: {}", output.stderr),
                }),
            ))
        }
        Err(e) => {
            std::fs::remove_file(&config_path).ok();
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(NginxResponse {
                    success: false,
                    message: format!("Failed to test config: {e}"),
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

    let config_path = format!("/etc/nginx/sites-enabled/{domain}.conf");

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

    // SSL certificates
    let ssl_dir = format!("/etc/dockpanel/ssl/{domain}");
    if std::path::Path::new(&ssl_dir).exists() {
        if let Err(e) = std::fs::remove_dir_all(&ssl_dir) {
            tracing::warn!("Failed to remove SSL certs for {domain}: {e}");
        } else {
            tracing::info!("Removed SSL certs: {ssl_dir}");
        }
    }

    // PHP-FPM pool configs (all versions)
    for version in &["8.1", "8.2", "8.3", "8.4"] {
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

    // Fail2Ban jail
    let jail_name = format!("nginx-{}", domain.replace('.', "-"));
    let jail_path = format!("/etc/fail2ban/jail.d/{jail_name}.conf");
    if std::path::Path::new(&jail_path).exists() {
        let _ = std::fs::remove_file(&jail_path);
        let _ = safe_command_sync("systemctl")
            .args(["reload", "fail2ban"])
            .output();
        tracing::info!("Removed Fail2Ban jail for {domain}");
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

    let config_path = format!("/etc/nginx/sites-enabled/{domain}.conf");
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

    let old_conf = format!("/etc/nginx/sites-enabled/{old_domain}.conf");
    if !std::path::Path::new(&old_conf).exists() {
        return Err((StatusCode::NOT_FOUND, Json(NginxResponse {
            success: false, message: format!("No nginx config for {old_domain}"),
        })));
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

    // 3. Write new nginx config
    let new_conf = format!("/etc/nginx/sites-enabled/{new_domain}.conf");
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

    // 5. Rename SSL certificates directory
    let old_ssl = format!("/etc/dockpanel/ssl/{old_domain}");
    let new_ssl = format!("/etc/dockpanel/ssl/{new_domain}");
    if std::path::Path::new(&old_ssl).exists() {
        std::fs::rename(&old_ssl, &new_ssl).ok();
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
    for version in &["8.1", "8.2", "8.3", "8.4"] {
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

    // 8. Rename Fail2Ban jail
    let old_jail = format!("nginx-{}", old_domain.replace('.', "-"));
    let new_jail = format!("nginx-{}", new_domain.replace('.', "-"));
    let old_jail_path = format!("/etc/fail2ban/jail.d/{old_jail}.conf");
    let new_jail_path = format!("/etc/fail2ban/jail.d/{new_jail}.conf");
    if std::path::Path::new(&old_jail_path).exists() {
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
            // Rollback: restore old config
            std::fs::write(&old_conf, &config_content).ok();
            std::fs::remove_file(&new_conf).ok();
            if std::path::Path::new(&new_dir).exists() {
                std::fs::rename(&new_dir, &old_dir).ok();
            }
            Err((StatusCode::BAD_REQUEST, Json(NginxResponse {
                success: false,
                message: format!("Nginx config test failed after rename: {}", output.stderr),
            })))
        }
        Err(e) => {
            std::fs::write(&old_conf, &config_content).ok();
            std::fs::remove_file(&new_conf).ok();
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
    let site_conf = format!("/etc/nginx/sites-enabled/{}.conf", body.domain);
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
    let site_conf = format!("/etc/nginx/sites-enabled/{}.conf", body.domain);
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

    let site_conf = format!("/etc/nginx/sites-enabled/{}.conf", body.domain);
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
    let site_conf = format!("/etc/nginx/sites-enabled/{domain}.conf");
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

    let site_conf = format!("/etc/nginx/sites-enabled/{domain}.conf");
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

    let source_dir = format!("/var/www/{}", body.source_domain);
    let target_dir = format!("/var/www/{}", body.target_domain);

    if !std::path::Path::new(&source_dir).exists() {
        return Err(api_err(StatusCode::NOT_FOUND, "Source site directory not found"));
    }

    // Create target directory
    tokio::fs::create_dir_all(&target_dir).await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to create directory: {e}")))?;

    // Copy files using rsync (preserves permissions, faster than cp -r)
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        safe_command("rsync")
            .args(["-a", "--delete", &format!("{source_dir}/"), &format!("{target_dir}/")])
            .output()
    ).await
        .map_err(|_| api_err(StatusCode::GATEWAY_TIMEOUT, "Clone timed out (300s)"))?
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, &format!("rsync failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(api_err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Clone failed: {stderr}")));
    }

    // Fix ownership
    let _ = safe_command("chown").args(["-R", "www-data:www-data", &target_dir]).output().await;

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
async fn get_env(Path(domain): Path<String>) -> Json<serde_json::Value> {
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

    Json(serde_json::json!({ "vars": vars, "raw": content }))
}

/// PUT /nginx/env/{domain} — Write .env file for a site.
async fn set_env(Path(domain): Path<String>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiErr> {
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

    // Restart the app service if it's a Node/Python site
    let service_name = format!("dockpanel-app-{}", domain.replace('.', "-"));
    let _ = safe_command("systemctl").args(["restart", &service_name]).output().await;

    tracing::info!("Environment variables updated for {domain}");
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /nginx/sites/{domain}/disable — Disable a site by replacing its nginx config with a 503 page.
async fn disable_site(Path(domain): Path<String>) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if !is_valid_domain(&domain) {
        return Err((StatusCode::BAD_REQUEST, "Invalid domain".into()));
    }
    let conf_path = format!("/etc/nginx/sites-enabled/{domain}.conf");
    let backup_path = format!("/etc/nginx/sites-available/{domain}.conf.disabled");

    // Check config exists
    if !std::path::Path::new(&conf_path).exists() {
        return Err((StatusCode::NOT_FOUND, format!("No config for {domain}")));
    }

    // Back up the current config
    std::fs::copy(&conf_path, &backup_path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Backup config: {e}")))?;

    // Write a 503 maintenance page config
    let disabled_conf = format!(
        r#"server {{
    listen 80;
    listen [::]:80;
    server_name {domain} www.{domain};
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

    // Stop app process if it exists (node/python)
    let service_name = format!("dockpanel-app-{}", domain.replace('.', "-"));
    let _ = safe_command("systemctl").args(["stop", &service_name]).output().await;

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
    let conf_path = format!("/etc/nginx/sites-enabled/{domain}.conf");
    let backup_path = format!("/etc/nginx/sites-available/{domain}.conf.disabled");

    // Restore the backed-up config
    if !std::path::Path::new(&backup_path).exists() {
        return Err((StatusCode::NOT_FOUND, format!("No disabled config backup for {domain}")));
    }

    std::fs::copy(&backup_path, &conf_path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Restore config: {e}")))?;
    std::fs::remove_file(&backup_path).ok();

    // Restart app process if it exists (node/python)
    let service_name = format!("dockpanel-app-{}", domain.replace('.', "-"));
    let _ = safe_command("systemctl").args(["restart", &service_name]).output().await;

    // Reload nginx
    services::nginx::reload().await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Nginx reload: {e}")))?;

    tracing::info!("Site enabled: {domain}");
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/nginx/sites/{domain}", put(put_site))
        .route("/nginx/sites/{domain}", delete(delete_site))
        .route("/nginx/sites/{domain}", get(get_site))
        .route("/nginx/sites/{domain}/rename", post(rename_site))
        .route("/nginx/sites/{domain}/disable", post(disable_site))
        .route("/nginx/sites/{domain}/enable", post(enable_site))
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
        .route("/nginx/php-errors/{domain}", get(php_errors))
        // Site Cloning
        .route("/nginx/clone-site", post(clone_site))
        // Environment Variables
        .route("/nginx/env/{domain}", get(get_env).put(set_env))
}
