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

    // Fix ownership to www-data — everything except `.git`, which stays root's.
    // rsync just copied the source tree verbatim, `.git` included if the source
    // is git-deployed; handing it to www-data gives the app `config`/`hooks/`
    // that `deploy.rs::clone_or_pull` runs as root on the next deploy. Mirrors
    // `deploy.rs::hand_tree_to_web_user`.
    if let Ok(mut entries) = tokio::fs::read_dir(&target).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry.file_name() == std::ffi::OsStr::new(".git") {
                continue;
            }
            let _ = safe_command("chown")
                .args(["-R", "www-data:www-data", &entry.path().to_string_lossy()])
                .output()
                .await;
        }
    }
    let git_dir = format!("{target}.git");
    if Path::new(&git_dir).exists() {
        let _ = safe_command("chown").args(["-R", "root:root", &git_dir]).output().await;
        let _ = safe_command("chmod").args(["-R", "go-rwx", &git_dir]).output().await;
    }

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

    // Fix ownership — everything except `.git`. See `clone_files` above for why.
    if let Ok(mut entries) = tokio::fs::read_dir(&target).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry.file_name() == std::ffi::OsStr::new(".git") {
                continue;
            }
            let _ = safe_command("chown")
                .args(["-R", "www-data:www-data", &entry.path().to_string_lossy()])
                .output()
                .await;
        }
    }
    let git_dir = format!("{target}.git");
    if Path::new(&git_dir).exists() {
        let _ = safe_command("chown").args(["-R", "root:root", &git_dir]).output().await;
        let _ = safe_command("chmod").args(["-R", "go-rwx", &git_dir]).output().await;
    }

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
