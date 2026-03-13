use crate::routes::nginx::SiteConfig;
use std::sync::Arc;
use tera::{Context, Tera};
use tokio::process::Command;

pub struct CmdOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Initialize Tera templates with embedded nginx templates.
pub fn init_templates() -> Arc<Tera> {
    let mut tera = Tera::default();

    tera.add_raw_template("http.conf", include_str!("../templates/nginx/http.conf"))
        .expect("Failed to load http.conf template");

    tera.add_raw_template("https.conf", include_str!("../templates/nginx/https.conf"))
        .expect("Failed to load https.conf template");

    tera.add_raw_template("proxy.conf", include_str!("../templates/nginx/proxy.conf"))
        .expect("Failed to load proxy.conf template");

    Arc::new(tera)
}

/// Validate a filesystem path value used in nginx config (root, ssl_cert, ssl_key).
fn is_safe_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains("..")
        && !path.contains('\0')
        && path.starts_with('/')
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/-_.".contains(c))
}

/// Validate a PHP-FPM socket path.
fn is_safe_php_socket(socket: &str) -> bool {
    socket.starts_with("unix:/")
        && socket.ends_with(".sock")
        && !socket.contains("..")
        && !socket.contains('\0')
        && socket[5..] // skip "unix:" prefix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/-_.".contains(c))
}

/// Render the appropriate nginx config for a site.
pub fn render_site_config(
    templates: &Tera,
    domain: &str,
    config: &SiteConfig,
) -> Result<String, tera::Error> {
    // Validate fields that get inserted into nginx config
    if let Some(ref root) = config.root {
        if !is_safe_path(root) {
            return Err(tera::Error::msg("Invalid root path"));
        }
    }
    if let Some(ref socket) = config.php_socket {
        if !is_safe_php_socket(socket) {
            return Err(tera::Error::msg("Invalid PHP socket path"));
        }
    }
    if let Some(ref cert) = config.ssl_cert {
        if !is_safe_path(cert) {
            return Err(tera::Error::msg("Invalid SSL certificate path"));
        }
    }
    if let Some(ref key) = config.ssl_key {
        if !is_safe_path(key) {
            return Err(tera::Error::msg("Invalid SSL key path"));
        }
    }

    let mut ctx = Context::new();
    ctx.insert("domain", domain);
    ctx.insert("root", config.root.as_deref().unwrap_or("/var/www"));
    ctx.insert("runtime", &config.runtime);

    // Resource limits
    let rate_limit = config.rate_limit.unwrap_or(0);
    ctx.insert("rate_limit", &rate_limit);
    let max_upload_mb = config.max_upload_mb.unwrap_or(64);
    ctx.insert("max_upload_mb", &max_upload_mb);

    let ssl = config.ssl.unwrap_or(false);
    ctx.insert("ssl", &ssl);

    if ssl {
        ctx.insert(
            "ssl_cert",
            config
                .ssl_cert
                .as_deref()
                .unwrap_or(&format!("/etc/dockpanel/ssl/{domain}/fullchain.pem")),
        );
        ctx.insert(
            "ssl_key",
            config
                .ssl_key
                .as_deref()
                .unwrap_or(&format!("/etc/dockpanel/ssl/{domain}/privkey.pem")),
        );
    }

    match config.runtime.as_str() {
        "proxy" => {
            ctx.insert("proxy_port", &config.proxy_port.unwrap_or(3000));
            if ssl {
                templates.render("https.conf", &ctx)
            } else {
                templates.render("proxy.conf", &ctx)
            }
        }
        "php" => {
            ctx.insert(
                "php_socket",
                config
                    .php_socket
                    .as_deref()
                    .unwrap_or("unix:/run/php/php-fpm.sock"),
            );
            if ssl {
                templates.render("https.conf", &ctx)
            } else {
                templates.render("http.conf", &ctx)
            }
        }
        _ => {
            // Static site
            if ssl {
                templates.render("https.conf", &ctx)
            } else {
                templates.render("http.conf", &ctx)
            }
        }
    }
}

/// Write a per-site PHP-FPM pool config with resource limits.
pub fn write_php_pool_config(
    domain: &str,
    php_version: &str,
    memory_mb: u32,
    max_workers: u32,
) -> Result<(), String> {
    let pool_dir = format!("/etc/php/{php_version}/fpm/pool.d");
    if !std::path::Path::new(&pool_dir).exists() {
        return Ok(()); // PHP not installed — skip silently
    }

    // Sanitize domain for use as pool name (replace dots with underscores)
    let pool_name = domain.replace('.', "_");

    let config = format!(
        r#"[{pool_name}]
user = www-data
group = www-data
listen = /run/php/php{php_version}-fpm-{pool_name}.sock
listen.owner = www-data
listen.group = www-data
listen.mode = 0660

pm = dynamic
pm.max_children = {max_workers}
pm.start_servers = {start}
pm.min_spare_servers = 1
pm.max_spare_servers = {spare}
pm.max_requests = 500

php_admin_value[memory_limit] = {memory_mb}M
php_admin_value[upload_max_filesize] = 64M
php_admin_value[post_max_size] = 64M
php_admin_value[max_execution_time] = 300
"#,
        start = std::cmp::min(2, max_workers),
        spare = std::cmp::min(3, max_workers),
    );

    let pool_path = format!("{pool_dir}/{pool_name}.conf");
    std::fs::write(&pool_path, &config)
        .map_err(|e| format!("Failed to write FPM pool config: {e}"))?;

    tracing::info!("PHP-FPM pool config written: {pool_path} (workers={max_workers}, memory={memory_mb}M)");
    Ok(())
}

/// Reload PHP-FPM for a given version.
pub async fn reload_php_fpm(php_version: &str) -> Result<(), String> {
    let service = format!("php{php_version}-fpm");
    let output = Command::new("systemctl")
        .args(["reload", &service])
        .output()
        .await
        .map_err(|e| format!("Failed to reload {service}: {e}"))?;

    if output.status.success() {
        tracing::info!("{service} reloaded");
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Not fatal — pool config will apply on next restart
        tracing::warn!("{service} reload failed (will apply on restart): {stderr}");
        Ok(())
    }
}

/// Run `nginx -t` to test configuration.
pub async fn test_config() -> Result<CmdOutput, std::io::Error> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        Command::new("nginx").arg("-t").output(),
    )
    .await
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "nginx -t timed out"))??;

    Ok(CmdOutput {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

/// Reload nginx gracefully.
pub async fn reload() -> Result<(), String> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        Command::new("nginx").args(["-s", "reload"]).output(),
    )
    .await
    .map_err(|_| "Nginx reload timed out".to_string())?
    .map_err(|e| format!("Failed to execute nginx: {e}"))?;

    if output.status.success() {
        tracing::info!("Nginx reloaded successfully");
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::error!("Nginx reload failed: {stderr}");
        Err(format!("Nginx reload failed: {stderr}"))
    }
}
