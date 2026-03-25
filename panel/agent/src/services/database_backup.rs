use std::path::PathBuf;
use tokio::io::AsyncWriteExt;
use crate::safe_cmd::safe_command;

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

/// Validate container/db/user names to prevent argument injection.
/// These must be alphanumeric + underscore/hyphen only.
fn is_safe_db_identifier(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
        && !name.starts_with('-')
}

/// Dump a MySQL/MariaDB database from its Docker container.
///
/// Uses piped `docker exec` → `gzip` to avoid shell interpolation entirely.
pub async fn dump_mysql(
    container_name: &str,
    db_name: &str,
    user: &str,
    password: &str,
) -> Result<BackupInfo, String> {
    if !is_safe_db_identifier(container_name) {
        return Err("Invalid container name".into());
    }
    if !is_safe_db_identifier(db_name) {
        return Err("Invalid database name".into());
    }
    if !is_safe_db_identifier(user) {
        return Err("Invalid username".into());
    }

    let dest_dir = backup_dir(db_name);
    std::fs::create_dir_all(&dest_dir)
        .map_err(|e| format!("Failed to create backup dir: {e}"))?;

    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let filename = format!("{db_name}-{timestamp}.sql.gz");
    let filepath = dest_dir.join(&filename);
    let filepath_str = filepath.to_str().ok_or("Invalid path encoding")?;

    // docker exec outputs to stdout → pipe to gzip → write to file
    let mut docker_child = safe_command("docker")
        .args([
            "exec",
            "-e", &format!("MYSQL_PWD={password}"),
            container_name,
            "mariadb-dump",
            "-u", user,
            "--single-transaction", "--routines", "--triggers",
            db_name,
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn docker exec: {e}"))?;

    let docker_stdout = docker_child.stdout.take()
        .ok_or("Failed to capture docker stdout")?;

    let mut gzip_child = safe_command("gzip")
        .stdin(docker_stdout.into_owned_fd().map_err(|_| "Failed to get fd")?)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn gzip: {e}"))?;

    let gzip_stdout = gzip_child.stdout.take()
        .ok_or("Failed to capture gzip stdout")?;

    // Write gzip output to file
    let filepath_clone = filepath.clone();
    let write_handle = tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut reader = gzip_stdout;
        let mut file = tokio::fs::File::create(&filepath_clone).await?;
        tokio::io::copy(&mut reader, &mut file).await?;
        file.flush().await?;
        Ok::<_, std::io::Error>(())
    });

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(600),
        async {
            let docker_status = docker_child.wait().await
                .map_err(|e| format!("docker exec wait error: {e}"))?;
            let _gzip_status = gzip_child.wait().await
                .map_err(|e| format!("gzip wait error: {e}"))?;
            write_handle.await
                .map_err(|e| format!("write task error: {e}"))?
                .map_err(|e| format!("file write error: {e}"))?;
            if !docker_status.success() {
                return Err("MySQL dump failed (docker exec returned non-zero)".to_string());
            }
            Ok(())
        }
    )
    .await
    .map_err(|_| "Database dump timed out (10 minutes)".to_string())?;

    if let Err(e) = result {
        std::fs::remove_file(&filepath).ok();
        return Err(e);
    }

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
        sha256: None,
    })
}

/// Dump a PostgreSQL database from its Docker container.
pub async fn dump_postgres(
    container_name: &str,
    db_name: &str,
    user: &str,
    password: &str,
) -> Result<BackupInfo, String> {
    if !is_safe_db_identifier(container_name) {
        return Err("Invalid container name".into());
    }
    if !is_safe_db_identifier(db_name) {
        return Err("Invalid database name".into());
    }
    if !is_safe_db_identifier(user) {
        return Err("Invalid username".into());
    }

    let dest_dir = backup_dir(db_name);
    std::fs::create_dir_all(&dest_dir)
        .map_err(|e| format!("Failed to create backup dir: {e}"))?;

    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let filename = format!("{db_name}-{timestamp}.sql.gz");
    let filepath = dest_dir.join(&filename);
    let filepath_str = filepath.to_str().ok_or("Invalid path encoding")?;

    let mut docker_child = safe_command("docker")
        .args([
            "exec",
            "-e", &format!("PGPASSWORD={password}"),
            container_name,
            "pg_dump",
            "-U", user,
            "-d", db_name,
            "--no-owner", "--no-acl",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn docker exec: {e}"))?;

    let docker_stdout = docker_child.stdout.take()
        .ok_or("Failed to capture docker stdout")?;

    let mut gzip_child = safe_command("gzip")
        .stdin(docker_stdout.into_owned_fd().map_err(|_| "Failed to get fd")?)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn gzip: {e}"))?;

    let gzip_stdout = gzip_child.stdout.take()
        .ok_or("Failed to capture gzip stdout")?;

    let filepath_clone = filepath.clone();
    let write_handle = tokio::spawn(async move {
        let mut reader = gzip_stdout;
        let mut file = tokio::fs::File::create(&filepath_clone).await?;
        tokio::io::copy(&mut reader, &mut file).await?;
        file.flush().await?;
        Ok::<_, std::io::Error>(())
    });

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(600),
        async {
            let docker_status = docker_child.wait().await
                .map_err(|e| format!("docker exec wait error: {e}"))?;
            let _gzip_status = gzip_child.wait().await
                .map_err(|e| format!("gzip wait error: {e}"))?;
            write_handle.await
                .map_err(|e| format!("write task error: {e}"))?
                .map_err(|e| format!("file write error: {e}"))?;
            if !docker_status.success() {
                return Err("PostgreSQL dump failed (docker exec returned non-zero)".to_string());
            }
            Ok(())
        }
    )
    .await
    .map_err(|_| "Database dump timed out (10 minutes)".to_string())?;

    if let Err(e) = result {
        std::fs::remove_file(&filepath).ok();
        return Err(e);
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
        sha256: None,
    })
}

/// Dump a MongoDB database from its Docker container.
pub async fn dump_mongo(
    container_name: &str,
    db_name: &str,
) -> Result<BackupInfo, String> {
    if !is_safe_db_identifier(container_name) {
        return Err("Invalid container name".into());
    }
    if !is_safe_db_identifier(db_name) {
        return Err("Invalid database name".into());
    }

    let dest_dir = backup_dir(db_name);
    std::fs::create_dir_all(&dest_dir)
        .map_err(|e| format!("Failed to create backup dir: {e}"))?;

    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let filename = format!("{db_name}-{timestamp}.archive.gz");
    let filepath = dest_dir.join(&filename);
    let filepath_str = filepath.to_str().ok_or("Invalid path encoding")?;

    // mongodump --archive --gzip outputs directly, no need for separate gzip
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(600),
        async {
            let child_output = safe_command("docker")
                .args([
                    "exec", container_name,
                    "mongodump", "--db", db_name, "--archive", "--gzip",
                ])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .output()
                .await
                .map_err(|e| format!("Failed to run mongodump: {e}"))?;

            // Write stdout to file
            tokio::fs::write(&filepath, &child_output.stdout).await
                .map_err(|e| format!("Failed to write dump file: {e}"))?;

            if !child_output.status.success() {
                let stderr = String::from_utf8_lossy(&child_output.stderr);
                return Err(format!("MongoDB dump failed: {stderr}"));
            }
            Ok(())
        }
    )
    .await
    .map_err(|_| "Database dump timed out (10 minutes)".to_string())?;

    if let Err(e) = output {
        std::fs::remove_file(&filepath).ok();
        return Err(e);
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
        sha256: None,
    })
}

/// Restore a MySQL/MariaDB database from a backup file.
pub async fn restore_mysql(
    container_name: &str,
    db_name: &str,
    user: &str,
    password: &str,
    filepath: &str,
) -> Result<(), String> {
    if !is_safe_db_identifier(container_name) {
        return Err("Invalid container name".into());
    }
    if !is_safe_db_identifier(db_name) {
        return Err("Invalid database name".into());
    }
    if !is_safe_db_identifier(user) {
        return Err("Invalid username".into());
    }

    // gunzip → pipe to docker exec mysql
    let mut gunzip_child = safe_command("gunzip")
        .args(["-c", filepath])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn gunzip: {e}"))?;

    let gunzip_stdout = gunzip_child.stdout.take()
        .ok_or("Failed to capture gunzip stdout")?;

    let docker_child = safe_command("docker")
        .args([
            "exec", "-i",
            "-e", &format!("MYSQL_PWD={password}"),
            container_name,
            "mysql", "-u", user, db_name,
        ])
        .stdin(gunzip_stdout.into_owned_fd().map_err(|_| "Failed to get fd")?)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn docker exec: {e}"))?;

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(600),
        async {
            let _gunzip_status = gunzip_child.wait().await
                .map_err(|e| format!("gunzip wait error: {e}"))?;
            let docker_output = docker_child.wait_with_output().await
                .map_err(|e| format!("docker exec wait error: {e}"))?;
            if !docker_output.status.success() {
                let stderr = String::from_utf8_lossy(&docker_output.stderr);
                return Err(format!("MySQL restore failed: {stderr}"));
            }
            Ok(())
        }
    )
    .await
    .map_err(|_| "Database restore timed out (10 minutes)".to_string())?;

    result?;
    tracing::info!("MySQL database {db_name} restored from {filepath}");
    Ok(())
}

/// Restore a PostgreSQL database from a backup file.
pub async fn restore_postgres(
    container_name: &str,
    db_name: &str,
    user: &str,
    password: &str,
    filepath: &str,
) -> Result<(), String> {
    if !is_safe_db_identifier(container_name) {
        return Err("Invalid container name".into());
    }
    if !is_safe_db_identifier(db_name) {
        return Err("Invalid database name".into());
    }
    if !is_safe_db_identifier(user) {
        return Err("Invalid username".into());
    }

    let mut gunzip_child = safe_command("gunzip")
        .args(["-c", filepath])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn gunzip: {e}"))?;

    let gunzip_stdout = gunzip_child.stdout.take()
        .ok_or("Failed to capture gunzip stdout")?;

    let docker_child = safe_command("docker")
        .args([
            "exec", "-i",
            "-e", &format!("PGPASSWORD={password}"),
            container_name,
            "psql", "-U", user, "-d", db_name,
        ])
        .stdin(gunzip_stdout.into_owned_fd().map_err(|_| "Failed to get fd")?)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn docker exec: {e}"))?;

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(600),
        async {
            let _gunzip_status = gunzip_child.wait().await
                .map_err(|e| format!("gunzip wait error: {e}"))?;
            let docker_output = docker_child.wait_with_output().await
                .map_err(|e| format!("docker exec wait error: {e}"))?;
            if !docker_output.status.success() {
                let stderr = String::from_utf8_lossy(&docker_output.stderr);
                return Err(format!("PostgreSQL restore failed: {stderr}"));
            }
            Ok(())
        }
    )
    .await
    .map_err(|_| "Database restore timed out (10 minutes)".to_string())?;

    result?;
    tracing::info!("PostgreSQL database {db_name} restored from {filepath}");
    Ok(())
}

/// Restore a MongoDB database from a backup file.
pub async fn restore_mongo(
    container_name: &str,
    db_name: &str,
    filepath: &str,
) -> Result<(), String> {
    if !is_safe_db_identifier(container_name) {
        return Err("Invalid container name".into());
    }
    if !is_safe_db_identifier(db_name) {
        return Err("Invalid database name".into());
    }

    let file_data = tokio::fs::read(filepath).await
        .map_err(|e| format!("Failed to read backup file: {e}"))?;

    let mut docker_child = safe_command("docker")
        .args([
            "exec", "-i", container_name,
            "mongorestore", "--db", db_name, "--archive", "--gzip", "--drop",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn docker exec: {e}"))?;

    let mut stdin = docker_child.stdin.take()
        .ok_or("Failed to capture docker stdin")?;

    let write_handle = tokio::spawn(async move {
        stdin.write_all(&file_data).await?;
        stdin.shutdown().await?;
        Ok::<_, std::io::Error>(())
    });

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(600),
        async {
            write_handle.await
                .map_err(|e| format!("write task error: {e}"))?
                .map_err(|e| format!("stdin write error: {e}"))?;
            let docker_output = docker_child.wait_with_output().await
                .map_err(|e| format!("docker exec wait error: {e}"))?;
            if !docker_output.status.success() {
                let stderr = String::from_utf8_lossy(&docker_output.stderr);
                return Err(format!("MongoDB restore failed: {stderr}"));
            }
            Ok(())
        }
    )
    .await
    .map_err(|_| "Database restore timed out (10 minutes)".to_string())?;

    result?;
    tracing::info!("MongoDB database {db_name} restored from {filepath}");
    Ok(())
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
            sha256: None,
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
