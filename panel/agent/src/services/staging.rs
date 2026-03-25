use std::path::Path;
use crate::safe_cmd::safe_command;

const WEB_ROOT: &str = "/var/www";

fn validate_domain(domain: &str) -> Result<(), String> {
    if domain.is_empty() || domain.contains("..") || domain.contains('/')
        || domain.contains('\\') || domain.contains('\0')
        || !domain.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return Err("Invalid domain format".to_string());
    }
    Ok(())
}

/// Clone site files from source to target domain using rsync.
/// Creates the target directory if it doesn't exist.
pub async fn clone_files(source_domain: &str, target_domain: &str) -> Result<String, String> {
    validate_domain(source_domain)?;
    validate_domain(target_domain)?;
    let source = format!("{WEB_ROOT}/{source_domain}/");
    let target = format!("{WEB_ROOT}/{target_domain}/");

    if !Path::new(&source).exists() {
        return Err(format!("Source directory not found: {source}"));
    }

    // Create target directory
    tokio::fs::create_dir_all(&target)
        .await
        .map_err(|e| format!("Failed to create target directory: {e}"))?;

    // rsync -a preserves permissions, ownership, timestamps
    // --delete ensures target is an exact copy
    let output = safe_command("rsync")
        .args(["-a", "--delete", &source, &target])
        .output()
        .await
        .map_err(|e| format!("Failed to run rsync: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("rsync failed: {stderr}"));
    }

    // Fix ownership to www-data
    safe_command("chown")
        .args(["-R", "www-data:www-data", &target])
        .output()
        .await
        .ok();

    Ok(format!("Cloned {source} → {target}"))
}

/// Sync files between two site directories.
/// direction: "prod_to_staging" or "staging_to_prod"
pub async fn sync_files(source_domain: &str, target_domain: &str) -> Result<String, String> {
    validate_domain(source_domain)?;
    validate_domain(target_domain)?;
    let source = format!("{WEB_ROOT}/{source_domain}/");
    let target = format!("{WEB_ROOT}/{target_domain}/");

    if !Path::new(&source).exists() {
        return Err(format!("Source directory not found: {source}"));
    }
    if !Path::new(&target).exists() {
        return Err(format!("Target directory not found: {target}"));
    }

    let output = safe_command("rsync")
        .args(["-a", "--delete", &source, &target])
        .output()
        .await
        .map_err(|e| format!("Failed to run rsync: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("rsync failed: {stderr}"));
    }

    // Fix ownership
    safe_command("chown")
        .args(["-R", "www-data:www-data", &target])
        .output()
        .await
        .ok();

    Ok(format!("Synced {source} → {target}"))
}

/// Get disk usage of a site directory in bytes.
pub async fn site_disk_usage(domain: &str) -> Result<u64, String> {
    validate_domain(domain)?;
    let path = format!("{WEB_ROOT}/{domain}");
    if !Path::new(&path).exists() {
        return Ok(0);
    }

    let output = safe_command("du")
        .args(["-sb", &path])
        .output()
        .await
        .map_err(|e| format!("du failed: {e}"))?;

    if !output.status.success() {
        return Ok(0);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .split_whitespace()
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| "Failed to parse du output".to_string())
}

/// Delete a site's web directory.
pub async fn delete_site_files(domain: &str) -> Result<(), String> {
    validate_domain(domain)?;
    let path = format!("{WEB_ROOT}/{domain}");
    if Path::new(&path).exists() {
        tokio::fs::remove_dir_all(&path)
            .await
            .map_err(|e| format!("Failed to delete {path}: {e}"))?;
    }
    Ok(())
}
