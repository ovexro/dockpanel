use crate::routes::nginx::SiteConfig;
use std::sync::Arc;
use tera::{Context, Tera};
use crate::safe_cmd::safe_command;

pub struct CmdOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// The directory nginx will actually serve for a site, given its runtime.
///
/// This is not the site directory. Every template renders
/// `root {{ root }}/{{ domain }}/public` for the static and PHP runtimes, and
/// only the runtimes that proxy to a process are served from the site directory
/// itself. It lives here because two callers need the same answer and used to
/// disagree: `put_site` created the `public` subdirectory, while the migration
/// importer copied the imported site's files one level above it and then
/// deleted `public` on its way past — so every import produced a vhost whose
/// document root did not exist.
/// The one sentence that identifies the filler page as ours.
///
/// It is a constant rather than a literal at each site because the page is
/// WRITTEN when a site is created and READ back when a git deploy has to decide
/// whether a document root holds a real site or only our own filler. A deploy
/// that adopted a page it merely guessed was ours would delete an operator's
/// file, so the writer and the reader have to be the same string by
/// construction rather than by anyone remembering to update both.
pub const PLACEHOLDER_MARKER: &str = "Site is ready. Upload your files to replace this page.";

/// The filler page written into a new site's document root.
pub fn placeholder_page(domain: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><title>{domain}</title></head>\
         <body><h1>Welcome to {domain}</h1>\
         <p>{PLACEHOLDER_MARKER}</p></body></html>"
    )
}

/// Whether `path` is a file the panel itself wrote as filler.
///
/// Keyed on content, never on the name. `index.html` is the most ordinary
/// filename there is and an operator's own upload must never be mistaken for
/// ours; a file that cannot be read is, deliberately, not ours.
pub fn is_placeholder_page(path: &std::path::Path) -> bool {
    std::fs::read_to_string(path)
        .map(|body| body.contains(PLACEHOLDER_MARKER))
        .unwrap_or(false)
}

pub fn document_root_for(site_dir: &str, runtime: &str) -> String {
    match runtime {
        "proxy" | "node" | "python" => site_dir.to_string(),
        _ => format!("{site_dir}/public"),
    }
}

/// Whether this box actually serves `domain` over HTTPS.
///
/// The panel used to assume it always did, and printed `https://` links and
/// monitor URLs on that assumption. That is wrong in two ordinary situations:
/// an operator terminating TLS at an external reverse proxy, whose origin
/// serves plain HTTP (issue #96), and any app or site whose certificate step
/// was skipped — including one where the deploy said so on screen.
///
/// The answer is not a guess. The agent wrote the vhost, and its own template
/// emits a `listen …443 ssl` line only when a certificate is in place, so the
/// file on disk is the record of what is being served. Traefik keeps the same
/// record in its dynamic route config, and is asked first because when it is
/// the proxy for a domain there is no nginx vhost to read.
pub fn domain_is_https(domain: &str) -> bool {
    if let Some(tls) = crate::services::traefik::route_is_tls(domain) {
        return tls;
    }
    let config_path = format!("/etc/nginx/sites-enabled/{domain}.conf");
    let Ok(config) = std::fs::read_to_string(&config_path) else {
        return false;
    };
    config.lines().any(|line| {
        let line = line.trim();
        line.starts_with("listen ") && line.contains("443") && line.contains("ssl")
    })
}

/// Undo a vhost write: put `previous` back if there was one, otherwise remove the
/// file we just created. Returns whether a previous config was restored.
///
/// Every writer that replaces `/etc/nginx/sites-enabled/{domain}.conf` and then
/// validates must funnel its failure path through here. A bare `remove_file` is
/// only correct when the writer knows the path held nothing beforehand — and none
/// of the three writers knew that. `nginx -t` is a whole-server check, so an
/// unrelated broken vhost anywhere on the box fails it, and the site being edited
/// paid for it with its own configuration file.
pub fn restore_or_remove(config_path: &str, previous: Option<&str>) -> bool {
    match previous {
        Some(prev) => {
            // Restore through the same tmp+rename dance the write used, so a crash
            // mid-restore cannot leave a truncated vhost behind.
            let tmp_path = format!("{config_path}.restore");
            let ok = std::fs::write(&tmp_path, prev)
                .and_then(|_| std::fs::rename(&tmp_path, config_path))
                .is_ok();
            if !ok {
                std::fs::remove_file(&tmp_path).ok();
                tracing::error!(
                    "Failed to restore the previous nginx config at {config_path} \
                     ({} bytes) — this domain may now have no vhost",
                    prev.len()
                );
            }
            ok
        }
        None => {
            std::fs::remove_file(config_path).ok();
            false
        }
    }
}

/// Sentence appended to an error so the operator learns whether the previous
/// configuration survived. Saying nothing is what let the old behaviour stay
/// invisible for as long as it did.
pub fn restore_note(restored: bool) -> &'static str {
    if restored {
        " (the previous configuration for this domain was restored)"
    } else {
        ""
    }
}

/// The two denies the static branch of both templates carries from v2.68.0. Kept as
/// one block so the retrofit below writes exactly what a fresh render would.
const STATIC_DENIES: &str = "\n\
    \x20   # Retrofitted: a site switched from php to static keeps its application\n\
    \x20   # source in the same docroot. Both denies exist in every php preset.\n\
    \x20   location ~ /\\.(?!well-known) {\n\
    \x20       deny all;\n\
    \x20   }\n\
    \n\
    \x20   location ~ \\.php$ {\n\
    \x20       deny all;\n\
    \x20   }\n";

/// Decide whether a vhost needs the denies, and return the patched body if so.
///
/// Pure, so the decision is testable without a box: the caller does the I/O and
/// the `nginx -t` gate. A vhost qualifies when it renders the STATIC branch
/// (`index index.html index.htm;`) and carries no FastCGI handler — a php vhost
/// already has these denies inside its preset branch. Returns `None` for anything
/// already patched, which is what makes the migration idempotent.
pub fn patch_static_vhost(body: &str) -> Option<String> {
    if !body.contains("index index.html index.htm;")
        || body.contains("fastcgi_pass")
        || body.contains("location ~ /\\.(?!well-known)")
    {
        return None;
    }
    Some(
        body.lines()
            .map(|line| {
                if line.trim() == "index index.html index.htm;" {
                    format!("{line}\n{STATIC_DENIES}")
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// Retrofit the static branch's denies onto site vhosts already on disk.
///
/// v2.68.0 fixed the TEMPLATE. A template reaches a site only when something
/// re-renders its vhost, and nothing re-renders on upgrade — so a site already
/// switched from php to static keeps serving `wp-config.php`, `.env` and
/// `.git/config` as plain text after its operator has updated. v2.68.1 put the
/// retrofit in `scripts/update.sh`, and that reaches the PANEL host only: the
/// script's own header records that an agent-only box never runs it (it syncs a
/// git repo, dumps a postgres container, replaces the API — an agent box has
/// none of that), so a fleet member's sites stayed exposed. The agent owns
/// `/etc/nginx/sites-enabled` on every box it runs on, panel and member alike,
/// which makes this the only place the fix reaches the whole population.
///
/// Scoped by what the vhost IS: the static branch's `index index.html
/// index.htm;` and no FastCGI handler. A php vhost already carries these denies
/// inside its preset branch. Idempotent, and it declines to run at all if the
/// server's config is ALREADY failing `nginx -t` — that check is whole-server,
/// so one unrelated broken vhost would otherwise make this revert edits it made
/// correctly and blame itself for a fault it did not cause.
pub async fn retrofit_static_denies() {
    let dir = std::path::Path::new("/etc/nginx/sites-enabled");
    let Ok(entries) = std::fs::read_dir(dir) else { return };

    let mut candidates: Vec<(std::path::PathBuf, String, String)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("conf") {
            continue;
        }
        if path.file_name().and_then(|n| n.to_str()) == Some("dockpanel-panel.conf") {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(&path) else { continue };
        let Some(patched) = patch_static_vhost(&body) else { continue };
        candidates.push((path, body, patched));
    }

    if candidates.is_empty() {
        return;
    }

    // Refuse to touch anything on a box whose nginx is already unhappy.
    match test_config().await {
        Ok(out) if out.success => {}
        _ => {
            tracing::warn!(
                "Skipping the static-vhost deny retrofit on {} vhost(s): nginx -t already fails, \
                 so a post-edit check could not tell my change from the existing fault",
                candidates.len()
            );
            return;
        }
    }

    let mut written: Vec<(std::path::PathBuf, String)> = Vec::new();
    for (path, previous, patched) in &candidates {
        let tmp = path.with_extension("conf.retrofit");
        if std::fs::write(&tmp, patched)
            .and_then(|_| std::fs::rename(&tmp, path))
            .is_ok()
        {
            written.push((path.clone(), previous.clone()));
        } else {
            std::fs::remove_file(&tmp).ok();
            tracing::warn!("Could not retrofit denies onto {}", path.display());
        }
    }
    if written.is_empty() {
        return;
    }

    match test_config().await {
        Ok(out) if out.success => {
            let names: Vec<String> = written
                .iter()
                .map(|(p, _)| p.file_name().unwrap_or_default().to_string_lossy().to_string())
                .collect();
            tracing::info!(
                "Retrofitted dotfile/.php denies onto {} static vhost(s): {}",
                written.len(),
                names.join(", ")
            );
            reload().await.ok();
        }
        _ => {
            for (path, previous) in &written {
                restore_or_remove(&path.to_string_lossy(), Some(previous));
            }
            tracing::error!(
                "nginx -t rejected the static-vhost deny retrofit — all {} file(s) reverted, \
                 sites untouched",
                written.len()
            );
        }
    }
}

/// Detect the server's primary (non-loopback, non-docker) interface IP for nginx listen directives.
/// Returns empty string if detection fails (templates fall back to wildcard listen).
fn detect_bind_ip() -> String {
    use std::net::UdpSocket;
    // Connect to a public IP (doesn't actually send data) to find the outbound interface
    if let Ok(sock) = UdpSocket::bind("0.0.0.0:0") {
        if sock.connect("8.8.8.8:53").is_ok() {
            if let Ok(addr) = sock.local_addr() {
                let ip = addr.ip().to_string();
                if ip != "127.0.0.1" {
                    return ip;
                }
            }
        }
    }
    String::new()
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

/// Allowed nginx directives that can appear in custom_nginx config.
const ALLOWED_NGINX_DIRECTIVES: &[&str] = &[
    "client_max_body_size",
    "gzip",
    "gzip_types",
    "gzip_min_length",
    "proxy_read_timeout",
    "proxy_connect_timeout",
    "proxy_send_timeout",
    "send_timeout",
    "keepalive_timeout",
    "server_tokens",
    "add_header",
    "expires",
    "tcp_nodelay",
    "tcp_nopush",
    "sendfile",
    "types_hash_max_size",
];

/// Dangerous nginx directive prefixes/keywords (case-insensitive).
const DANGEROUS_DIRECTIVES: &[&str] = &[
    "lua_",
    "perl_",
    "proxy_pass",
    "return",
    "rewrite",
    "access_log",
    "error_log",
    "load_module",
    "include",
    "alias",
];

/// Validate custom nginx directives. Rejects dangerous content and only allows whitelisted directives.
fn validate_custom_nginx(custom: &str) -> Result<(), String> {
    if custom.trim().is_empty() {
        return Ok(());
    }

    // Reject block-escape characters
    if custom.contains('{') || custom.contains('}') {
        return Err("Custom nginx config must not contain '{' or '}' (server block escape)".into());
    }

    for line in custom.lines() {
        let line_trimmed = line.trim();

        // Skip empty lines
        if line_trimmed.is_empty() {
            continue;
        }

        // Reject comment-only lines (comment injection)
        if line_trimmed.starts_with('#') {
            return Err("Custom nginx config must not contain comment lines starting with '#'".into());
        }

        // nginx separates statements with ';', so a single LINE can carry many
        // directives. Validating only the first token let an attacker smuggle a
        // dangerous directive past the whitelist, e.g.
        //   "sendfile on; access_log /var/www/victim/public/shell.php;"
        // (leading token 'sendfile' is allowed, the chained access_log was never
        // inspected → cross-tenant log-poisoning file-write/RCE). Split on ';'
        // and validate EVERY statement independently.
        for stmt in line_trimmed.split(';') {
            let trimmed = stmt.trim();

            if trimmed.is_empty() {
                continue;
            }

            // A statement must not start a comment either.
            if trimmed.starts_with('#') {
                return Err("Custom nginx config must not contain comment lines starting with '#'".into());
            }

            // Check for dangerous directives (case-insensitive)
            let lower = trimmed.to_lowercase();
            for dangerous in DANGEROUS_DIRECTIVES {
                if lower.starts_with(dangerous) {
                    return Err(format!(
                        "Custom nginx config contains forbidden directive: {dangerous}"
                    ));
                }
            }

            // Extract the directive name (first word of the statement)
            let directive = trimmed
                .split(|c: char| c.is_whitespace())
                .next()
                .unwrap_or("")
                .to_lowercase();

            if directive.is_empty() {
                continue;
            }

            if !ALLOWED_NGINX_DIRECTIVES.contains(&directive.as_str()) {
                return Err(format!(
                    "Custom nginx directive '{}' is not in the allowed list. Allowed: {}",
                    directive,
                    ALLOWED_NGINX_DIRECTIVES.join(", ")
                ));
            }
        }
    }

    Ok(())
}

/// Defense-in-depth: strip characters that would break out of an
/// `add_header Name "<value>" always;` directive. The backend already rejects
/// these (routes/mod.rs::is_safe_header_value), but the agent renders the value
/// into a root-owned nginx config, so it sanitizes again at this render choke
/// point — any future/legacy caller that forwards an unvalidated CSP /
/// Permissions-Policy value cannot inject nginx directives.
fn sanitize_header_value(v: &str) -> String {
    v.chars()
        .filter(|c| !matches!(c, '"' | '\\' | '\n' | '\r' | '\0' | '{' | '}'))
        .collect()
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
    ctx.insert("bind_ip", &detect_bind_ip());
    ctx.insert("runtime", &config.runtime);

    // Resource limits
    let rate_limit = config.rate_limit.unwrap_or(0);
    ctx.insert("rate_limit", &rate_limit);
    let max_upload_mb = config.max_upload_mb.unwrap_or(64);
    ctx.insert("max_upload_mb", &max_upload_mb);

    // Custom nginx directives — validate before inserting
    if let Some(ref custom) = config.custom_nginx {
        validate_custom_nginx(custom).map_err(tera::Error::msg)?;
    }
    ctx.insert("custom_nginx", &config.custom_nginx.as_deref().unwrap_or(""));

    // FastCGI cache (PHP sites only)
    let fastcgi_cache = config.fastcgi_cache.unwrap_or(false) && config.runtime == "php";
    ctx.insert("fastcgi_cache", &fastcgi_cache);

    // Redis object cache (PHP sites only)
    let redis_cache = config.redis_cache.unwrap_or(false) && config.runtime == "php";
    ctx.insert("redis_cache", &redis_cache);
    ctx.insert("redis_db", &config.redis_db.unwrap_or(0));

    // WAF (ModSecurity)
    let waf_enabled = config.waf_enabled.unwrap_or(false);
    ctx.insert("waf_enabled", &waf_enabled);
    ctx.insert("waf_mode", &config.waf_mode.as_deref().unwrap_or("detection"));

    // CSP and security headers
    // Rendered verbatim into `add_header ... "<value>" always;` — sanitize so a
    // stray `"`/`\`/brace cannot break out of the quoted argument and inject
    // directives (backend also validates via is_safe_header_value).
    let csp_safe = sanitize_header_value(config.csp_policy.as_deref().unwrap_or(""));
    let perms_safe = sanitize_header_value(
        config.permissions_policy.as_deref().unwrap_or("camera=(), microphone=(), geolocation=()"),
    );
    ctx.insert("csp_policy", &csp_safe);
    ctx.insert("permissions_policy", &perms_safe);

    // Bot protection
    ctx.insert("bot_protection", &config.bot_protection.as_deref().unwrap_or("off"));

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
        "proxy" | "node" | "python" => {
            ctx.insert("proxy_port", &config.proxy_port.unwrap_or(3000));
            // Node/Python use same proxy template as "proxy" runtime
            ctx.insert("runtime", &"proxy");
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
            ctx.insert("php_preset", &config.php_preset.as_deref().unwrap_or("generic"));
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
    let output = safe_command("systemctl")
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
        safe_command("nginx").arg("-t").output(),
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
        safe_command("nginx").args(["-s", "reload"]).output(),
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── static-vhost deny retrofit — s311 / v2.68.2 ─────────────────────
    //
    // The defect these guard against is not "the denies are missing from the
    // template" (v2.68.0 fixed that and a pin suite covers it). It is that a
    // template fix does not reach a vhost already written, and the site that
    // most needs it — one switched from php to static — is exactly a vhost
    // already written.

    const STATIC_VHOST: &str = "server {\n    listen 80;\n    server_name a.example;\n    root /var/www/a.example/public;\n    index index.html index.htm;\n\n    location / {\n        try_files $uri $uri/ =404;\n    }\n}\n";

    #[test]
    fn retrofit_patches_a_static_vhost_once_and_only_once() {
        let patched = patch_static_vhost(STATIC_VHOST).expect("a static vhost is a candidate");
        assert!(patched.contains("location ~ /\\.(?!well-known)"));
        assert!(patched.contains("location ~ \\.php$"));
        // The site must keep working: the original serving block survives.
        assert!(patched.contains("try_files $uri $uri/ =404;"));
        assert!(patched.contains("root /var/www/a.example/public;"));
        // Idempotent — a second pass is not a candidate at all.
        assert!(patch_static_vhost(&patched).is_none());
        // Exactly one insertion, not one per line.
        assert_eq!(patched.matches("deny all;").count(), 2);
    }

    #[test]
    fn retrofit_skips_a_php_vhost() {
        // A php vhost carries the denies in its preset branch already, and its
        // `.php` location is a HANDLER — denying it would stop the site dead.
        let php = "server {\n    index index.html index.htm;\n    location ~ \\.php$ {\n        fastcgi_pass unix:/run/php/php8.3-fpm.sock;\n    }\n}\n";
        assert!(patch_static_vhost(php).is_none());
    }

    #[test]
    fn retrofit_skips_a_vhost_that_is_not_static() {
        // A proxy vhost has no static index line; nothing to key on, nothing to do.
        let proxy = "server {\n    location / {\n        proxy_pass http://127.0.0.1:3000;\n    }\n}\n";
        assert!(patch_static_vhost(proxy).is_none());
    }

    // ── custom_nginx per-statement validation — s241 C2 ─────────────────

    #[test]
    fn custom_nginx_blocks_semicolon_chained_directive() {
        // The whole-line-token bypass: a whitelisted leading directive must not
        // let a dangerous directive ride along after ';'.
        assert!(validate_custom_nginx("sendfile on; access_log /var/www/victim/public/shell.php;").is_err());
        assert!(validate_custom_nginx("gzip on; return 301 https://evil$request_uri;").is_err());
        assert!(validate_custom_nginx("add_header X y; proxy_pass http://169.254.169.254/;").is_err());
        assert!(validate_custom_nginx("client_max_body_size 10m; error_log /tmp/x;").is_err());
    }

    #[test]
    fn custom_nginx_allows_benign_statements() {
        assert!(validate_custom_nginx("client_max_body_size 20m;").is_ok());
        assert!(validate_custom_nginx("gzip on; gzip_types text/css application/json;").is_ok());
        assert!(validate_custom_nginx("").is_ok());
        // Block-escape and comment injection still rejected.
        assert!(validate_custom_nginx("location / { root /etc; }").is_err());
        assert!(validate_custom_nginx("# sneaky").is_err());
    }

    // ── header-value sanitization — s241 C1 (agent defense-in-depth) ────

    #[test]
    fn header_sanitize_strips_breakout_chars() {
        assert_eq!(sanitize_header_value("default-src 'self'; script-src 'self'"), "default-src 'self'; script-src 'self'");
        // Quote / backslash / braces removed so they can't close the add_header arg.
        assert_eq!(sanitize_header_value("x\" always; location /p { }"), "x always; location /p  ");
        assert!(!sanitize_header_value("a\"b\\c{d}").contains('"'));
        assert!(!sanitize_header_value("a\"b\\c{d}").contains('\\'));
    }
}
