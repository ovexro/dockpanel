use std::process::Stdio;
use crate::safe_cmd::{safe_command, safe_command_sync};

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
    let out = safe_command("curl")
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
    safe_command("chmod")
        .args(["+x", WP_CLI])
        .output()
        .await
        .ok();
    Ok(())
}

/// Run a wp-cli command, return stdout on success.
/// Uses --skip-plugins --skip-themes by default to prevent RCE from compromised
/// plugins loading PHP during admin operations. Pass skip_safety=false only for
/// commands that explicitly need to interact with plugins/themes (list, activate).
async fn wp(domain: &str, args: &[&str]) -> Result<String, String> {
    wp_inner(domain, args, true).await
}

/// Run a wp-cli command that needs plugin/theme loading (e.g., plugin list, theme list).
async fn wp_with_plugins(domain: &str, args: &[&str]) -> Result<String, String> {
    wp_inner(domain, args, false).await
}

async fn wp_inner(domain: &str, args: &[&str], skip_plugins: bool) -> Result<String, String> {
    ensure_cli().await?;
    let path = site_path(domain)?;
    let mut cmd = safe_command(WP_CLI);
    cmd.args(args)
        .arg("--allow-root")
        .arg(format!("--path={path}"));
    if skip_plugins {
        cmd.arg("--skip-plugins").arg("--skip-themes");
    }
    let out = cmd
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
/// Note: plugin list requires loading plugins to get accurate status.
pub async fn plugins(domain: &str) -> Result<serde_json::Value, String> {
    let out = wp_with_plugins(domain, &["plugin", "list", "--format=json"]).await?;
    serde_json::from_str(&out).map_err(|e| format!("Parse error: {e}"))
}

/// List themes with status and update info.
/// Note: theme list requires loading themes to get accurate status.
pub async fn themes(domain: &str) -> Result<serde_json::Value, String> {
    let out = wp_with_plugins(domain, &["theme", "list", "--format=json"]).await?;
    serde_json::from_str(&out).map_err(|e| format!("Parse error: {e}"))
}

/// Update WordPress core.
pub async fn update_core(domain: &str) -> Result<String, String> {
    let result = wp(domain, &["core", "update"]).await?;
    // Fix ownership after update
    safe_command("chown")
        .args(["-R", "www-data:www-data", &site_path(domain)?])
        .output()
        .await
        .ok();
    Ok(result)
}

/// Update all plugins.
pub async fn update_all_plugins(domain: &str) -> Result<String, String> {
    let result = wp(domain, &["plugin", "update", "--all"]).await?;
    safe_command("chown")
        .args(["-R", "www-data:www-data", &site_path(domain)?])
        .output()
        .await
        .ok();
    Ok(result)
}

/// Update all themes.
pub async fn update_all_themes(domain: &str) -> Result<String, String> {
    let result = wp(domain, &["theme", "update", "--all"]).await?;
    safe_command("chown")
        .args(["-R", "www-data:www-data", &site_path(domain)?])
        .output()
        .await
        .ok();
    Ok(result)
}

/// Validate a WordPress plugin/theme slug: alphanumeric, hyphens, underscores only.
/// Rejects URLs, flags, and shell metacharacters.
fn is_valid_wp_slug(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && !name.starts_with('-')
        && !name.contains("://")
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Plugin action: activate, deactivate, update, delete, install.
pub async fn plugin_action(domain: &str, name: &str, action: &str) -> Result<String, String> {
    if !is_valid_wp_slug(name) {
        return Err("Invalid plugin name. Only alphanumeric, hyphens, and underscores allowed.".into());
    }
    let result = match action {
        "activate" | "deactivate" | "update" | "delete" => {
            wp(domain, &["plugin", action, name]).await?
        }
        "install" => wp(domain, &["plugin", "install", name]).await?,
        _ => return Err(format!("Unknown action: {action}")),
    };
    if matches!(action, "install" | "update") {
        safe_command("chown")
            .args(["-R", "www-data:www-data", &site_path(domain)?])
            .output()
            .await
            .ok();
    }
    Ok(result)
}

/// Theme action: activate, update, delete, install.
pub async fn theme_action(domain: &str, name: &str, action: &str) -> Result<String, String> {
    if !is_valid_wp_slug(name) {
        return Err("Invalid theme name. Only alphanumeric, hyphens, and underscores allowed.".into());
    }
    let result = match action {
        "activate" | "update" | "delete" => wp(domain, &["theme", action, name]).await?,
        "install" => wp(domain, &["theme", "install", name]).await?,
        _ => return Err(format!("Unknown action: {action}")),
    };
    if matches!(action, "install" | "update") {
        safe_command("chown")
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

    // Create wp-config.php (--skip-plugins --skip-themes for safety)
    let out = safe_command(WP_CLI)
        .args([
            "config",
            "create",
            &format!("--dbname={db_name}"),
            &format!("--dbuser={db_user}"),
            &format!("--dbpass={db_pass}"),
            &format!("--dbhost={db_host}"),
            "--skip-plugins",
            "--skip-themes",
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

    // Install WordPress (--skip-plugins --skip-themes for safety)
    let out = safe_command(WP_CLI)
        .args([
            "core",
            "install",
            &format!("--url={url}"),
            &format!("--title={title}"),
            &format!("--admin_user={admin_user}"),
            &format!("--admin_password={admin_pass}"),
            &format!("--admin_email={admin_email}"),
            "--skip-plugins",
            "--skip-themes",
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
    safe_command("chown")
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
        "0 3 * * * {WP_CLI} core update --skip-plugins --skip-themes --allow-root --path={path} > /dev/null 2>&1 && \
         {WP_CLI} plugin update --all --skip-plugins --skip-themes --allow-root --path={path} > /dev/null 2>&1 && \
         {WP_CLI} theme update --all --skip-plugins --skip-themes --allow-root --path={path} > /dev/null 2>&1 \
         {marker}"
    );

    // Get current crontab
    let current = safe_command("crontab")
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
    let mut child = safe_command("crontab")
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
    safe_command_sync("crontab")
        .args(["-l", "-u", "root"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(&marker))
        .unwrap_or(false)
}

/// Create a pre-update snapshot (files + DB) for rollback.
pub async fn create_update_snapshot(domain: &str) -> Result<String, String> {
    let _path = site_path(domain)?;
    let snapshot_dir = format!("/var/backups/dockpanel/wp-snapshots/{domain}");
    std::fs::create_dir_all(&snapshot_dir)
        .map_err(|e| format!("Create snapshot dir: {e}"))?;

    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let snapshot_path = format!("{snapshot_dir}/pre-update-{timestamp}.tar.gz");

    // Tar the site directory
    let tar = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        safe_command("tar")
            .args(["czf", &snapshot_path, "-C", "/var/www", &format!("{domain}/public")])
            .output()
    ).await
        .map_err(|_| "Snapshot tar timed out".to_string())?
        .map_err(|e| format!("Snapshot tar: {e}"))?;

    if !tar.status.success() {
        let stderr = String::from_utf8_lossy(&tar.stderr);
        return Err(format!("Snapshot failed: {}", stderr.chars().take(200).collect::<String>()));
    }

    // DB dump if WordPress has a database
    let db_name_output = wp(domain, &["config", "get", "DB_NAME"]).await.unwrap_or_default();
    let db_name = db_name_output.trim();
    if !db_name.is_empty() {
        let db_path = format!("{snapshot_dir}/pre-update-{timestamp}.sql");
        let _ = wp(domain, &["db", "export", &db_path, "--quiet"]).await;
    }

    // Cleanup old snapshots (keep last 5)
    if let Ok(entries) = std::fs::read_dir(&snapshot_dir) {
        let mut files: Vec<_> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tar.gz"))
            .collect();
        files.sort_by_key(|e| std::cmp::Reverse(e.file_name()));
        for old in files.iter().skip(5) {
            std::fs::remove_file(old.path()).ok();
            // Also remove matching .sql
            let sql = old.path().with_extension("").with_extension("sql");
            std::fs::remove_file(sql).ok();
        }
    }

    tracing::info!("WP update snapshot created for {domain}: {snapshot_path}");
    Ok(snapshot_path)
}

/// Rollback a WordPress site to a snapshot.
pub async fn rollback_from_snapshot(domain: &str, snapshot_path: &str) -> Result<(), String> {
    if !std::path::Path::new(snapshot_path).exists() {
        return Err("Snapshot file not found".to_string());
    }

    // Extract tar over site directory
    let restore = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        safe_command("tar")
            .args(["xzf", snapshot_path, "-C", "/var/www"])
            .output()
    ).await
        .map_err(|_| "Rollback timed out".to_string())?
        .map_err(|e| format!("Rollback tar: {e}"))?;

    if !restore.status.success() {
        return Err("Rollback tar extraction failed".to_string());
    }

    // Restore DB if SQL dump exists
    let sql_path = snapshot_path.replace(".tar.gz", ".sql");
    if std::path::Path::new(&sql_path).exists() {
        let _ = wp(domain, &["db", "import", &sql_path, "--quiet"]).await;
    }

    // Fix ownership
    let _ = safe_command("chown")
        .args(["-R", "www-data:www-data", &format!("/var/www/{domain}/public")])
        .output()
        .await;

    tracing::info!("WP rollback completed for {domain} from {snapshot_path}");
    Ok(())
}

/// Run a health check on a WordPress site after update.
pub async fn health_check(domain: &str) -> bool {
    // Check if WordPress responds to wp-cli
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        safe_command("sudo")
            .args(["-u", "www-data", WP_CLI, "eval", "echo 'OK';",
                   "--skip-plugins", "--skip-themes", "--allow-root",
                   &format!("--path={}", site_path(domain).unwrap_or_default())])
            .output()
    ).await;

    match result {
        Ok(Ok(out)) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            out.status.success() && stdout.contains("OK")
        }
        _ => false,
    }
}

/// Update WordPress with snapshot + rollback on failure.
pub async fn update_with_rollback(domain: &str) -> Result<serde_json::Value, String> {
    let mut log: Vec<String> = Vec::new();

    // 1. Health check before update
    if !health_check(domain).await {
        return Err("Site failed pre-update health check — skipping update".to_string());
    }
    log.push("Pre-update health check: passed".into());

    // 2. Create snapshot
    let snapshot = create_update_snapshot(domain).await?;
    log.push("Snapshot created".into());

    // 3. Get current versions
    let core_before = wp(domain, &["core", "version"]).await.unwrap_or_default().trim().to_string();

    // 4. Run updates
    let core_ok = wp(domain, &["core", "update"]).await.is_ok();
    let plugins_ok = wp(domain, &["plugin", "update", "--all"]).await.is_ok();
    let themes_ok = wp(domain, &["theme", "update", "--all"]).await.is_ok();
    log.push(format!("Updates: core={}, plugins={}, themes={}",
        if core_ok { "ok" } else { "failed" },
        if plugins_ok { "ok" } else { "failed" },
        if themes_ok { "ok" } else { "failed" }));

    // 5. Fix ownership
    let _ = safe_command("chown")
        .args(["-R", "www-data:www-data", &format!("/var/www/{domain}/public")])
        .output()
        .await;

    // 6. Post-update health check
    let healthy = health_check(domain).await;

    if !healthy {
        log.push("Post-update health check: FAILED — rolling back".into());
        match rollback_from_snapshot(domain, &snapshot).await {
            Ok(()) => {
                log.push("Rollback completed successfully".into());
                tracing::warn!("WP update for {domain} rolled back due to health check failure");
            }
            Err(e) => {
                log.push(format!("Rollback failed: {e}"));
                tracing::error!("WP rollback failed for {domain}: {e}");
            }
        }
    } else {
        log.push("Post-update health check: passed".into());
    }

    let core_after = wp(domain, &["core", "version"]).await.unwrap_or_default().trim().to_string();

    Ok(serde_json::json!({
        "domain": domain,
        "healthy": healthy,
        "rolled_back": !healthy,
        "core_before": core_before,
        "core_after": core_after,
        "snapshot": snapshot,
        "log": log,
    }))
}
