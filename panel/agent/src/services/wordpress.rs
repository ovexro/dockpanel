use std::process::Stdio;
use tokio::process::Command;

const WP_CLI: &str = "/usr/local/bin/wp";
const WP_ROOT: &str = "/var/www";

fn site_path(domain: &str) -> Result<String, String> {
    if domain.is_empty() || domain.contains("..") || domain.contains('/')
        || !domain.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return Err("Invalid domain".to_string());
    }
    Ok(format!("{WP_ROOT}/{domain}/public"))
}

/// Ensure wp-cli is installed at /usr/local/bin/wp.
pub async fn ensure_cli() -> Result<(), String> {
    if std::path::Path::new(WP_CLI).exists() {
        return Ok(());
    }
    let out = Command::new("curl")
        .args([
            "-sS",
            "-L",
            "-o",
            WP_CLI,
            "https://raw.githubusercontent.com/wp-cli/builds/gh-pages/phar/wp-cli.phar",
        ])
        .output()
        .await
        .map_err(|e| format!("Download failed: {e}"))?;
    if !out.status.success() {
        return Err("Failed to download wp-cli".into());
    }
    Command::new("chmod")
        .args(["+x", WP_CLI])
        .output()
        .await
        .ok();
    Ok(())
}

/// Run a wp-cli command, return stdout on success.
async fn wp(domain: &str, args: &[&str]) -> Result<String, String> {
    ensure_cli().await?;
    let path = site_path(domain)?;
    let out = Command::new(WP_CLI)
        .args(args)
        .arg("--allow-root")
        .arg(format!("--path={path}"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("wp-cli error: {e}"))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(stderr.trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Check if WordPress is installed at the site's document root.
pub fn detect(domain: &str) -> bool {
    match site_path(domain) {
        Ok(path) => std::path::Path::new(&format!("{path}/wp-config.php")).exists(),
        Err(_) => false,
    }
}

/// Get WP version and update availability.
pub async fn info(domain: &str) -> Result<serde_json::Value, String> {
    let version = wp(domain, &["core", "version"]).await?;

    // Check for available updates
    let update_check = wp(domain, &["core", "check-update", "--format=json"])
        .await
        .unwrap_or_default();
    let updates: Vec<serde_json::Value> =
        serde_json::from_str(&update_check).unwrap_or_default();
    let update_available = updates
        .first()
        .and_then(|u| u.get("version").and_then(|v| v.as_str()))
        .map(String::from);

    Ok(serde_json::json!({
        "installed": true,
        "version": version,
        "update_available": update_available,
    }))
}

/// List plugins with status and update info.
pub async fn plugins(domain: &str) -> Result<serde_json::Value, String> {
    let out = wp(domain, &["plugin", "list", "--format=json"]).await?;
    serde_json::from_str(&out).map_err(|e| format!("Parse error: {e}"))
}

/// List themes with status and update info.
pub async fn themes(domain: &str) -> Result<serde_json::Value, String> {
    let out = wp(domain, &["theme", "list", "--format=json"]).await?;
    serde_json::from_str(&out).map_err(|e| format!("Parse error: {e}"))
}

/// Update WordPress core.
pub async fn update_core(domain: &str) -> Result<String, String> {
    let result = wp(domain, &["core", "update"]).await?;
    // Fix ownership after update
    Command::new("chown")
        .args(["-R", "www-data:www-data", &site_path(domain)?])
        .output()
        .await
        .ok();
    Ok(result)
}

/// Update all plugins.
pub async fn update_all_plugins(domain: &str) -> Result<String, String> {
    let result = wp(domain, &["plugin", "update", "--all"]).await?;
    Command::new("chown")
        .args(["-R", "www-data:www-data", &site_path(domain)?])
        .output()
        .await
        .ok();
    Ok(result)
}

/// Update all themes.
pub async fn update_all_themes(domain: &str) -> Result<String, String> {
    let result = wp(domain, &["theme", "update", "--all"]).await?;
    Command::new("chown")
        .args(["-R", "www-data:www-data", &site_path(domain)?])
        .output()
        .await
        .ok();
    Ok(result)
}

/// Plugin action: activate, deactivate, update, delete, install.
pub async fn plugin_action(domain: &str, name: &str, action: &str) -> Result<String, String> {
    let result = match action {
        "activate" | "deactivate" | "update" | "delete" => {
            wp(domain, &["plugin", action, name]).await?
        }
        "install" => wp(domain, &["plugin", "install", name]).await?,
        _ => return Err(format!("Unknown action: {action}")),
    };
    if matches!(action, "install" | "update") {
        Command::new("chown")
            .args(["-R", "www-data:www-data", &site_path(domain)?])
            .output()
            .await
            .ok();
    }
    Ok(result)
}

/// Theme action: activate, update, delete, install.
pub async fn theme_action(domain: &str, name: &str, action: &str) -> Result<String, String> {
    let result = match action {
        "activate" | "update" | "delete" => wp(domain, &["theme", action, name]).await?,
        "install" => wp(domain, &["theme", "install", name]).await?,
        _ => return Err(format!("Unknown action: {action}")),
    };
    if matches!(action, "install" | "update") {
        Command::new("chown")
            .args(["-R", "www-data:www-data", &site_path(domain)?])
            .output()
            .await
            .ok();
    }
    Ok(result)
}

/// Install WordPress from scratch.
pub async fn install(
    domain: &str,
    url: &str,
    title: &str,
    admin_user: &str,
    admin_pass: &str,
    admin_email: &str,
    db_name: &str,
    db_user: &str,
    db_pass: &str,
    db_host: &str,
) -> Result<String, String> {
    ensure_cli().await?;
    let path = site_path(domain)?;

    // Ensure document root exists before wp-cli tries to write
    tokio::fs::create_dir_all(&path)
        .await
        .map_err(|e| format!("Failed to create site directory {path}: {e}"))?;

    // Download WordPress core files
    wp(domain, &["core", "download", "--force"]).await?;

    // Create wp-config.php
    let out = Command::new(WP_CLI)
        .args([
            "config",
            "create",
            &format!("--dbname={db_name}"),
            &format!("--dbuser={db_user}"),
            &format!("--dbpass={db_pass}"),
            &format!("--dbhost={db_host}"),
            "--allow-root",
            &format!("--path={path}"),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("wp config create: {e}"))?;

    if !out.status.success() {
        return Err(format!(
            "Config create failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    // Install WordPress
    let out = Command::new(WP_CLI)
        .args([
            "core",
            "install",
            &format!("--url={url}"),
            &format!("--title={title}"),
            &format!("--admin_user={admin_user}"),
            &format!("--admin_password={admin_pass}"),
            &format!("--admin_email={admin_email}"),
            "--allow-root",
            &format!("--path={path}"),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("wp core install: {e}"))?;

    if !out.status.success() {
        return Err(format!(
            "Core install failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    // Fix ownership
    Command::new("chown")
        .args(["-R", "www-data:www-data", &path])
        .output()
        .await
        .ok();

    Ok("WordPress installed successfully".into())
}

/// Set or remove auto-update cron.
pub async fn set_auto_update(domain: &str, enabled: bool) -> Result<(), String> {
    let path = site_path(domain)?;
    let marker = format!("# wp-auto-update-{domain}");
    let cron_line = format!(
        "0 3 * * * {WP_CLI} core update --allow-root --path={path} > /dev/null 2>&1 && \
         {WP_CLI} plugin update --all --allow-root --path={path} > /dev/null 2>&1 && \
         {WP_CLI} theme update --all --allow-root --path={path} > /dev/null 2>&1 \
         {marker}"
    );

    // Get current crontab
    let current = Command::new("crontab")
        .args(["-l", "-u", "root"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();

    // Remove existing auto-update line for this domain
    let filtered: Vec<&str> = current
        .lines()
        .filter(|l| !l.contains(&marker))
        .collect();

    let mut new_crontab = filtered.join("\n");
    if !new_crontab.ends_with('\n') && !new_crontab.is_empty() {
        new_crontab.push('\n');
    }

    if enabled {
        new_crontab.push_str(&cron_line);
        new_crontab.push('\n');
    }

    // Write crontab via stdin pipe
    let mut child = Command::new("crontab")
        .args(["-u", "root", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("crontab spawn: {e}"))?;

    if let Some(ref mut stdin) = child.stdin {
        use tokio::io::AsyncWriteExt;
        stdin
            .write_all(new_crontab.as_bytes())
            .await
            .map_err(|e| format!("crontab write: {e}"))?;
    }

    let out = child
        .wait_with_output()
        .await
        .map_err(|e| format!("crontab wait: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "crontab failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    Ok(())
}

/// Check if auto-update cron is enabled for a domain.
pub fn is_auto_update_enabled(domain: &str) -> bool {
    let marker = format!("wp-auto-update-{domain}");
    std::process::Command::new("crontab")
        .args(["-l", "-u", "root"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(&marker))
        .unwrap_or(false)
}
