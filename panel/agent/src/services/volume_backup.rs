use std::path::PathBuf;
use crate::safe_cmd::safe_command;

use super::backups::{BackupInfo, compute_file_sha256};

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
    let _filepath_str = filepath.to_str().ok_or("Invalid path encoding")?;

    // Use a minimal alpine container to tar the volume contents
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(600),
        safe_command("docker")
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

    let filepath_str = filepath.to_str().ok_or("Invalid path encoding")?;
    let sha256 = compute_file_sha256(filepath_str).await;

    tracing::info!("Volume backup created: {filename} ({} bytes, hash: {})", meta.len(), sha256.as_deref().unwrap_or("N/A"));

    Ok(BackupInfo {
        filename,
        size_bytes: meta.len(),
        created_at: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        sha256,
        ..Default::default()
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
        safe_command("docker")
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
                // Validate the archive can be read in full BEFORE wiping the live volume.
                // Previously the `rm -rf` ran unconditionally, so a truncated/corrupt/empty
                // backup destroyed the volume and THEN failed the extract — an irreversible
                // drop-then-fail data loss with no rollback. `tar tzf` decompresses and walks
                // the whole archive (catches gzip-CRC/truncation) without touching /volume;
                // only on success do we clear + extract. `filename` is is_safe_filename-
                // validated (alnum + -_. only), so it is safe to interpolate into the shell.
                //
                // The corruption check above catches a bad archive but NOT a syntactically
                // valid, gzip-CRC-clean archive with zero members — `tar tzf` exits 0 on an
                // empty tar, so the wipe below would still run and leave the volume empty
                // with no rollback. A second, separate `tar tzf | wc -l` re-run (kept apart
                // from the corruption check rather than folded into one pipeline, so an
                // early SIGPIPE on `grep -q`-style short-circuiting can never mask a
                // mid-listing failure as "found a line") requires at least one archive
                // member before the wipe is allowed to proceed.
                &format!("tar tzf /backup/{filename} >/dev/null 2>&1 || {{ echo 'backup archive is corrupt or truncated — volume left untouched' >&2; exit 3; }}; n=$(tar tzf /backup/{filename} 2>/dev/null | wc -l); [ $n -gt 0 ] || {{ echo 'backup archive is empty — volume left untouched' >&2; exit 3; }}; rm -rf /volume/* /volume/..?* /volume/.[!.]* 2>/dev/null; tar xzf /backup/{filename} -C /volume"),
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
            sha256: None,
        ..Default::default()
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

