use std::path::PathBuf;
use tokio::process::Command;

use super::backups::BackupInfo;

const BACKUP_DIR: &str = "/var/backups/dockpanel/databases";

/// Validate backup filename (prevent path traversal).
fn is_safe_filename(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains("..")
        && (name.ends_with(".sql.gz") || name.ends_with(".archive.gz") || name.ends_with(".sql.gz.enc") || name.ends_with(".archive.gz.enc"))
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

fn backup_dir(db_name: &str) -> PathBuf {
    PathBuf::from(format!("{BACKUP_DIR}/{db_name}"))
}

/// Dump a MySQL/MariaDB database from its Docker container.
pub async fn dump_mysql(
    container_name: &str,
    db_name: &str,
    user: &str,
    password: &str,
) -> Result<BackupInfo, String> {
    let dest_dir = backup_dir(db_name);
    std::fs::create_dir_all(&dest_dir)
        .map_err(|e| format!("Failed to create backup dir: {e}"))?;

    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let filename = format!("{db_name}-{timestamp}.sql.gz");
    let filepath = dest_dir.join(&filename);
    let filepath_str = filepath.to_str().ok_or("Invalid path encoding")?;

    // mysqldump inside container, pipe through gzip, write to host filesystem
    let shell_cmd = format!(
        "docker exec -e MYSQL_PWD='{}' {} mariadb-dump -u {} --single-transaction --routines --triggers {} | gzip > '{}'",
        password.replace('\'', "'\\''"),
        container_name,
        user,
        db_name,
        filepath_str.replace('\'', "'\\''"),
    );

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(600),
        Command::new("bash")
            .args(["-c", &shell_cmd])
            .output(),
    )
    .await
    .map_err(|_| "Database dump timed out (10 minutes)".to_string())?
    .map_err(|e| format!("Failed to run dump: {e}"))?;

    if !output.status.success() {
        // Clean up partial file
        std::fs::remove_file(&filepath).ok();
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("MySQL dump failed: {stderr}"));
    }

    // Verify the file is non-empty (gzip of empty stream is ~20 bytes)
    let meta = std::fs::metadata(&filepath)
        .map_err(|e| format!("Failed to read dump metadata: {e}"))?;
    if meta.len() < 30 {
        std::fs::remove_file(&filepath).ok();
        return Err("Database dump produced empty output".to_string());
    }

    tracing::info!("MySQL dump created: {filename} ({} bytes)", meta.len());

    Ok(BackupInfo {
        filename,
        size_bytes: meta.len(),
        created_at: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    })
}

/// Dump a PostgreSQL database from its Docker container.
pub async fn dump_postgres(
    container_name: &str,
    db_name: &str,
    user: &str,
    password: &str,
) -> Result<BackupInfo, String> {
    let dest_dir = backup_dir(db_name);
    std::fs::create_dir_all(&dest_dir)
        .map_err(|e| format!("Failed to create backup dir: {e}"))?;

    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let filename = format!("{db_name}-{timestamp}.sql.gz");
    let filepath = dest_dir.join(&filename);
    let filepath_str = filepath.to_str().ok_or("Invalid path encoding")?;

    let shell_cmd = format!(
        "docker exec -e PGPASSWORD='{}' {} pg_dump -U {} -d {} --no-owner --no-acl | gzip > '{}'",
        password.replace('\'', "'\\''"),
        container_name,
        user,
        db_name,
        filepath_str.replace('\'', "'\\''"),
    );

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(600),
        Command::new("bash")
            .args(["-c", &shell_cmd])
            .output(),
    )
    .await
    .map_err(|_| "Database dump timed out (10 minutes)".to_string())?
    .map_err(|e| format!("Failed to run dump: {e}"))?;

    if !output.status.success() {
        std::fs::remove_file(&filepath).ok();
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("PostgreSQL dump failed: {stderr}"));
    }

    let meta = std::fs::metadata(&filepath)
        .map_err(|e| format!("Failed to read dump metadata: {e}"))?;
    if meta.len() < 30 {
        std::fs::remove_file(&filepath).ok();
        return Err("Database dump produced empty output".to_string());
    }

    tracing::info!("PostgreSQL dump created: {filename} ({} bytes)", meta.len());

    Ok(BackupInfo {
        filename,
        size_bytes: meta.len(),
        created_at: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    })
}

/// Dump a MongoDB database from its Docker container.
pub async fn dump_mongo(
    container_name: &str,
    db_name: &str,
) -> Result<BackupInfo, String> {
    let dest_dir = backup_dir(db_name);
    std::fs::create_dir_all(&dest_dir)
        .map_err(|e| format!("Failed to create backup dir: {e}"))?;

    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let filename = format!("{db_name}-{timestamp}.archive.gz");
    let filepath = dest_dir.join(&filename);
    let filepath_str = filepath.to_str().ok_or("Invalid path encoding")?;

    let shell_cmd = format!(
        "docker exec {} mongodump --db {} --archive --gzip > '{}'",
        container_name,
        db_name,
        filepath_str.replace('\'', "'\\''"),
    );

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(600),
        Command::new("bash")
            .args(["-c", &shell_cmd])
            .output(),
    )
    .await
    .map_err(|_| "Database dump timed out (10 minutes)".to_string())?
    .map_err(|e| format!("Failed to run dump: {e}"))?;

    if !output.status.success() {
        std::fs::remove_file(&filepath).ok();
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("MongoDB dump failed: {stderr}"));
    }

    let meta = std::fs::metadata(&filepath)
        .map_err(|e| format!("Failed to read dump metadata: {e}"))?;
    if meta.len() < 30 {
        std::fs::remove_file(&filepath).ok();
        return Err("Database dump produced empty output".to_string());
    }

    tracing::info!("MongoDB dump created: {filename} ({} bytes)", meta.len());

    Ok(BackupInfo {
        filename,
        size_bytes: meta.len(),
        created_at: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    })
}

/// List database backups for a given database name.
pub fn list_db_backups(db_name: &str) -> Result<Vec<BackupInfo>, String> {
    let dir = backup_dir(db_name);
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut backups = Vec::new();
    for entry in std::fs::read_dir(&dir).map_err(|e| format!("Read dir error: {e}"))? {
        let entry = entry.map_err(|e| format!("Entry error: {e}"))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".sql.gz") && !name.ends_with(".archive.gz")
            && !name.ends_with(".sql.gz.enc") && !name.ends_with(".archive.gz.enc")
        {
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

/// Delete a database backup file.
pub fn delete_db_backup(db_name: &str, filename: &str) -> Result<(), String> {
    if !is_safe_filename(filename) {
        return Err("Invalid backup filename".into());
    }

    let filepath = backup_dir(db_name).join(filename);
    if !filepath.exists() {
        return Err("Backup file not found".into());
    }

    std::fs::remove_file(&filepath)
        .map_err(|e| format!("Failed to delete backup: {e}"))?;

    tracing::info!("Database backup deleted: {filename} for {db_name}");
    Ok(())
}

/// Get the full filesystem path for a database backup file.
pub fn get_backup_path(db_name: &str, filename: &str) -> Result<String, String> {
    if !is_safe_filename(filename) {
        return Err("Invalid backup filename".into());
    }
    let filepath = backup_dir(db_name).join(filename);
    if !filepath.exists() {
        return Err("Backup file not found".into());
    }
    filepath
        .to_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "Invalid path encoding".to_string())
}
