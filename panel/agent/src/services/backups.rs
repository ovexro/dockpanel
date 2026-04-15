use std::path::PathBuf;
use crate::safe_cmd::safe_command;

const BACKUP_DIR: &str = "/var/backups/dockpanel";
const WEBROOT: &str = "/var/www";

#[derive(serde::Serialize, Default)]
pub struct BackupInfo {
    pub filename: String,
    pub size_bytes: u64,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// Validate backup filename (prevent path traversal).
fn is_safe_filename(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains("..")
        && name.ends_with(".tar.gz")
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

fn backup_dir(domain: &str) -> PathBuf {
    PathBuf::from(format!("{BACKUP_DIR}/{domain}"))
}

/// Create a backup of the site's webroot.
pub async fn create_backup(domain: &str) -> Result<BackupInfo, String> {
    let site_root = PathBuf::from(format!("{WEBROOT}/{domain}"));
    if !site_root.exists() {
        return Err(format!("Site root does not exist: {}", site_root.display()));
    }

    let dest_dir = backup_dir(domain);
    std::fs::create_dir_all(&dest_dir)
        .map_err(|e| format!("Failed to create backup dir: {e}"))?;

    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let filename = format!("{domain}-{timestamp}.tar.gz");
    let filepath = dest_dir.join(&filename);

    let filepath_str = filepath
        .to_str()
        .ok_or_else(|| "Invalid backup path encoding".to_string())?;
    let site_root_str = site_root
        .to_str()
        .ok_or_else(|| "Invalid site root path encoding".to_string())?;

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        safe_command("tar")
            .args(["czf", filepath_str, "-C", site_root_str, "."])
            .output(),
    )
    .await
    .map_err(|_| "Backup timed out (5 minutes)".to_string())?
    .map_err(|e| format!("Failed to run tar: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Backup failed: {stderr}"));
    }

    let meta = std::fs::metadata(&filepath)
        .map_err(|e| format!("Failed to read backup metadata: {e}"))?;

    // Warn if backup exceeds 5GB — may indicate bloated site or logs in webroot
    const BACKUP_SIZE_WARN: u64 = 5 * 1024 * 1024 * 1024;
    if meta.len() > BACKUP_SIZE_WARN {
        tracing::warn!(
            "Backup for {domain} is very large: {:.2} GB ({filename}). Consider cleaning up the site directory.",
            meta.len() as f64 / (1024.0 * 1024.0 * 1024.0)
        );
    }

    // Feature 13: Compute SHA256 hash of the backup file for integrity chain
    let sha256 = compute_file_sha256(filepath_str).await;

    tracing::info!("Backup created: {filename} ({} bytes, hash: {})", meta.len(), sha256.as_deref().unwrap_or("N/A"));

    Ok(BackupInfo {
        filename,
        size_bytes: meta.len(),
        created_at: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        sha256,
    })
}

/// Compute SHA256 hash of a file (for backup integrity chain).
async fn compute_file_sha256(path: &str) -> Option<String> {
    let output = safe_command("sha256sum")
        .arg(path)
        .output()
        .await
        .ok()?;
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        Some(stdout.split_whitespace().next()?.to_string())
    } else {
        None
    }
}

/// List backups for a domain.
pub fn list_backups(domain: &str) -> Result<Vec<BackupInfo>, String> {
    let dir = backup_dir(domain);
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut backups = Vec::new();
    for entry in std::fs::read_dir(&dir).map_err(|e| format!("Read dir error: {e}"))? {
        let entry = entry.map_err(|e| format!("Entry error: {e}"))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".tar.gz") {
            continue;
        }
        let meta = entry.metadata().ok();
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let created = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .map(|t| {
                let dt: chrono::DateTime<chrono::Utc> = t.into();
                dt.format("%Y-%m-%d %H:%M:%S").to_string()
            })
            .unwrap_or_default();

        backups.push(BackupInfo {
            filename: name,
            size_bytes: size,
            created_at: created,
            sha256: None,
        });
    }

    backups.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(backups)
}

/// Restore a backup to the site's webroot.
pub async fn restore_backup(domain: &str, filename: &str) -> Result<(), String> {
    if !is_safe_filename(filename) {
        return Err("Invalid backup filename".into());
    }

    let filepath = backup_dir(domain).join(filename);
    if !filepath.exists() {
        return Err("Backup file not found".into());
    }

    let site_root = PathBuf::from(format!("{WEBROOT}/{domain}"));
    std::fs::create_dir_all(&site_root)
        .map_err(|e| format!("Failed to create site root: {e}"))?;

    let filepath_str = filepath
        .to_str()
        .ok_or_else(|| "Invalid backup path encoding".to_string())?;
    let site_root_str = site_root
        .to_str()
        .ok_or_else(|| "Invalid site root path encoding".to_string())?;

    // Full overwrite ensures a clean restore; --no-same-owner prevents uid/gid hijacking
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        safe_command("tar")
            .args(["xzf", filepath_str, "--no-same-owner", "--no-same-permissions", "-C", site_root_str])
            .output(),
    )
    .await
    .map_err(|_| "Restore timed out (5 minutes)".to_string())?
    .map_err(|e| format!("Failed to run tar: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Restore failed: {stderr}"));
    }

    // Verify the restore produced a non-empty site directory
    let entries = std::fs::read_dir(&site_root).map(|d| d.count()).unwrap_or(0);
    if entries == 0 {
        return Err("Restore completed but site directory is empty".to_string());
    }

    tracing::info!("Backup restored: {filename} for {domain} ({entries} entries)");
    Ok(())
}

/// List files in a backup archive (max 500 entries).
pub async fn list_backup_files(domain: &str, filename: &str) -> Result<Vec<String>, String> {
    if !is_safe_filename(filename) {
        return Err("Invalid backup filename".into());
    }

    let filepath = backup_dir(domain).join(filename);
    if !filepath.exists() {
        return Err("Backup file not found".into());
    }

    let filepath_str = filepath
        .to_str()
        .ok_or_else(|| "Invalid backup path encoding".to_string())?;

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        safe_command("tar")
            .args(["tzf", filepath_str])
            .output(),
    )
    .await
    .map_err(|_| "Listing timed out (30s)".to_string())?
    .map_err(|e| format!("Failed to run tar: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Failed to list archive: {stderr}"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let files: Vec<String> = stdout
        .lines()
        .take(500)
        .map(|l| l.to_string())
        .collect();

    Ok(files)
}

/// Restore a single file from a backup archive.
pub async fn restore_single_file(domain: &str, filename: &str, file_path: &str) -> Result<(), String> {
    if !is_safe_filename(filename) {
        return Err("Invalid backup filename".into());
    }

    // Validate file_path: no path traversal, no leading /
    if file_path.is_empty() {
        return Err("File path cannot be empty".into());
    }
    if file_path.contains("..") {
        return Err("File path must not contain '..'".into());
    }
    if file_path.starts_with('/') {
        return Err("File path must not start with '/'".into());
    }

    let backup_filepath = backup_dir(domain).join(filename);
    if !backup_filepath.exists() {
        return Err("Backup file not found".into());
    }

    let target = format!("{WEBROOT}/{domain}");
    let backup_str = backup_filepath
        .to_str()
        .ok_or_else(|| "Invalid backup path encoding".to_string())?;

    // Normalize: ensure it starts with ./
    let extract_path = if file_path.starts_with("./") {
        file_path.to_string()
    } else {
        format!("./{file_path}")
    };

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        safe_command("tar")
            .args(["xzf", backup_str, "--no-same-owner", "--no-same-permissions", "-C", &target, &extract_path])
            .output(),
    )
    .await
    .map_err(|_| "Restore timed out (60s)".to_string())?
    .map_err(|e| format!("Failed to run tar: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Single-file restore failed: {stderr}"));
    }

    tracing::info!("Single file restored from {filename}: {file_path} for {domain}");
    Ok(())
}

/// Delete a backup file.
pub fn delete_backup(domain: &str, filename: &str) -> Result<(), String> {
    if !is_safe_filename(filename) {
        return Err("Invalid backup filename".into());
    }

    let filepath = backup_dir(domain).join(filename);
    if !filepath.exists() {
        return Err("Backup file not found".into());
    }

    std::fs::remove_file(&filepath)
        .map_err(|e| format!("Failed to delete backup: {e}"))?;

    tracing::info!("Backup deleted: {filename} for {domain}");
    Ok(())
}
