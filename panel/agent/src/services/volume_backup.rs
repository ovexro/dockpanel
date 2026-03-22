use std::path::PathBuf;
use tokio::process::Command;

use super::backups::BackupInfo;

const BACKUP_DIR: &str = "/var/backups/dockpanel/volumes";

/// Validate backup filename.
fn is_safe_filename(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains("..")
        && name.ends_with(".tar.gz")
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

fn backup_dir(container_name: &str) -> PathBuf {
    PathBuf::from(format!("{BACKUP_DIR}/{container_name}"))
}

/// Backup a Docker volume by running a temporary container that mounts
/// the volume and tars its contents.
pub async fn backup_volume(
    volume_name: &str,
    container_name: &str,
) -> Result<BackupInfo, String> {
    let dest_dir = backup_dir(container_name);
    std::fs::create_dir_all(&dest_dir)
        .map_err(|e| format!("Failed to create backup dir: {e}"))?;

    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let filename = format!("{container_name}-{volume_name}-{timestamp}.tar.gz");
    let filepath = dest_dir.join(&filename);
    let filepath_str = filepath.to_str().ok_or("Invalid path encoding")?;

    // Use a minimal alpine container to tar the volume contents
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(600),
        Command::new("docker")
            .args([
                "run",
                "--rm",
                "-v",
                &format!("{volume_name}:/volume:ro"),
                "-v",
                &format!("{}:/backup", dest_dir.to_str().ok_or("Invalid dir")?),
                "alpine:3.19",
                "tar",
                "czf",
                &format!("/backup/{filename}"),
                "-C",
                "/volume",
                ".",
            ])
            .output(),
    )
    .await
    .map_err(|_| "Volume backup timed out (10 minutes)".to_string())?
    .map_err(|e| format!("Failed to run volume backup: {e}"))?;

    if !output.status.success() {
        std::fs::remove_file(&filepath).ok();
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Volume backup failed: {stderr}"));
    }

    let meta = std::fs::metadata(&filepath)
        .map_err(|e| format!("Failed to read backup metadata: {e}"))?;

    tracing::info!("Volume backup created: {filename} ({} bytes)", meta.len());

    Ok(BackupInfo {
        filename,
        size_bytes: meta.len(),
        created_at: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    })
}

/// Restore a Docker volume from a backup.
pub async fn restore_volume(
    volume_name: &str,
    container_name: &str,
    filename: &str,
) -> Result<(), String> {
    if !is_safe_filename(filename) {
        return Err("Invalid backup filename".into());
    }

    let dir = backup_dir(container_name);
    let filepath = dir.join(filename);
    if !filepath.exists() {
        return Err("Backup file not found".into());
    }

    let dir_str = dir.to_str().ok_or("Invalid dir")?;

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(600),
        Command::new("docker")
            .args([
                "run",
                "--rm",
                "-v",
                &format!("{volume_name}:/volume"),
                "-v",
                &format!("{dir_str}:/backup:ro"),
                "alpine:3.19",
                "sh",
                "-c",
                &format!("rm -rf /volume/* /volume/..?* /volume/.[!.]* 2>/dev/null; tar xzf /backup/{filename} -C /volume"),
            ])
            .output(),
    )
    .await
    .map_err(|_| "Volume restore timed out (10 minutes)".to_string())?
    .map_err(|e| format!("Failed to run volume restore: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Volume restore failed: {stderr}"));
    }

    tracing::info!("Volume restored: {filename} to {volume_name}");
    Ok(())
}

/// List volume backups for a container.
pub fn list_volume_backups(container_name: &str) -> Result<Vec<BackupInfo>, String> {
    let dir = backup_dir(container_name);
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
        });
    }

    backups.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(backups)
}

/// Delete a volume backup file.
pub fn delete_volume_backup(container_name: &str, filename: &str) -> Result<(), String> {
    if !is_safe_filename(filename) {
        return Err("Invalid backup filename".into());
    }

    let filepath = backup_dir(container_name).join(filename);
    if !filepath.exists() {
        return Err("Backup file not found".into());
    }

    std::fs::remove_file(&filepath)
        .map_err(|e| format!("Failed to delete backup: {e}"))?;

    tracing::info!("Volume backup deleted: {filename} for {container_name}");
    Ok(())
}

/// Get the full filesystem path for a volume backup file.
pub fn get_backup_path(container_name: &str, filename: &str) -> Result<String, String> {
    if !is_safe_filename(filename) {
        return Err("Invalid backup filename".into());
    }
    let filepath = backup_dir(container_name).join(filename);
    if !filepath.exists() {
        return Err("Backup file not found".into());
    }
    filepath
        .to_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "Invalid path encoding".to_string())
}
