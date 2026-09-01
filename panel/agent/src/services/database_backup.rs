use std::path::PathBuf;
use tokio::io::AsyncWriteExt;
use crate::safe_cmd::{safe_command, DockerEnvFile};

use super::backups::{BackupInfo, compute_file_sha256};

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

/// Names this module will resolve to a directory under the dump root.
///
/// A destructive operation should not borrow its idea of a legal name from a
/// general-purpose route validator in another module: that validator exists to
/// keep shell arguments safe and can reasonably be widened for a reason that has
/// nothing to do with this, and the blast radius here is a directory that gets
/// removed. So the purge owns its charset, and the charset admits no dot at all —
/// `.` and `..` cannot be SPELLED, rather than being spelled and then refused.
///
/// ⚠ Deliberately a SUPERSET of the shared validator, never a narrowing: same
/// characters plus no rule about the first one, same length. A name that door
/// accepts, this one accepts. Making it narrower is how a dump door quietly stops
/// answering for some database nobody thought about — an imported one, most
/// likely, since a migration does not go through the panel's own create form.
pub fn is_db_dir_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Names this module MINTED, as opposed to names it merely accepts.
///
/// ⚠ The distinction is the whole safety of the purge, and the obvious predicate
/// gets it wrong. The one guarding the import door is an EXTENSION test — it says
/// whether a file can be read as a dump, which is exactly what an operator's
/// hand-placed `dump.sql.gz` is designed to satisfy. Deleting on that basis would
/// take the staged import the product's own screen tells them to put here, and
/// the set of files it would take is a superset of the set it would keep.
///
/// So this asks the narrower question: does the name have the shape this module
/// writes — the database's own name, a timestamp, and an extension it mints? A
/// file the operator named survives, and the directory survives with it.
fn is_minted_dump_name(db_name: &str, name: &str) -> bool {
    let Some(rest) = name.strip_prefix(&format!("{db_name}-")) else {
        return false;
    };
    // `{db_name}-%Y%m%d-%H%M%S.<ext>` — 15 characters of timestamp, then the
    // extension, optionally followed by the encryption suffix.
    let Some((stamp, ext)) = rest.split_once('.') else {
        return false;
    };
    let stamped = stamp.len() == 15
        && stamp.as_bytes()[8] == b'-'
        && stamp
            .char_indices()
            .all(|(i, c)| if i == 8 { c == '-' } else { c.is_ascii_digit() });
    stamped && matches!(ext, "sql.gz" | "archive.gz" | "sql.gz.enc" | "archive.gz.enc")
}

/// Remove a database's dump directory, once nothing owns it any more.
///
/// Deleting a database has always taken its container and its rows and left the
/// dumps behind — unreferenced, unreachable through any list, and counted by no
/// retention policy, because retention only ever unlinks paths named by rows
/// that still exist. They are simply there, and on a box that recycles
/// database names they accumulate for the life of the machine.
///
/// ⛔ The caller decides WHETHER, not this function. A dump directory is keyed by
/// database NAME alone while the name is unique only per site, so two live
/// databases on two different sites share one directory — purging on the first
/// delete would destroy the survivor's backups. That check needs the database,
/// so it lives in the panel; what lives here is the refusal to do anything
/// surprising once asked.
///
/// Deliberately not a recursive delete. It unlinks the files this module knows
/// how to write and then removes the directory only if that emptied it, so a
/// directory holding anything else — an operator's staged import, a file some
/// future version writes — survives with its contents, and no future bug in the
/// name guard can escalate into erasing a tree.
pub async fn purge_dir(db_name: &str) -> Result<PurgeReport, String> {
    if !is_db_dir_name(db_name) {
        return Err("Invalid database name".into());
    }
    let dir = backup_dir(db_name);
    if !dir.is_dir() {
        return Ok(PurgeReport { removed: 0, kept: 0, dir_removed: false });
    }

    let mut removed = 0usize;
    let mut kept = 0usize;
    let mut entries = tokio::fs::read_dir(&dir)
        .await
        .map_err(|e| format!("Failed to read dump directory: {e}"))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| format!("Failed to walk dump directory: {e}"))?
    {
        let name = entry.file_name().to_string_lossy().to_string();
        if entry.file_type().await.map(|t| t.is_file()).unwrap_or(false)
            && is_minted_dump_name(db_name, &name)
        {
            match tokio::fs::remove_file(entry.path()).await {
                Ok(()) => removed += 1,
                Err(e) => {
                    tracing::warn!("Could not remove dump {name}: {e}");
                    kept += 1;
                }
            }
        } else {
            kept += 1;
        }
    }

    let dir_removed = if kept == 0 {
        tokio::fs::remove_dir(&dir).await.is_ok()
    } else {
        tracing::info!(
            "Dump directory for {db_name} keeps {kept} item(s) this purge does not own; \
             leaving the directory in place"
        );
        false
    };

    Ok(PurgeReport { removed, kept, dir_removed })
}

/// What a purge actually did, so the panel can log something true about it.
pub struct PurgeReport {
    pub removed: usize,
    pub kept: usize,
    pub dir_removed: bool,
}

/// Validate container/db/user names to prevent argument injection.
/// These must be alphanumeric + underscore/hyphen only.
fn is_safe_db_identifier(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.contains("..")
        && !name.contains('/')
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
    let _filepath_str = filepath.to_str().ok_or("Invalid path encoding")?;

    // docker exec outputs to stdout → pipe to gzip → write to file
    //
    // Credential via --env-file, not `-e KEY=value`: see `DockerEnvFile`.
    let env_file = DockerEnvFile::new(&[("MYSQL_PWD", password)])
        .map_err(|e| format!("Failed to prepare credentials: {e}"))?;
    let mut docker_child = safe_command("docker")
        .arg("exec")
        .arg("--env-file").arg(env_file.path())
        .args([
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

    let filepath_str = filepath.to_str().ok_or("Invalid path encoding")?;
    let sha256 = compute_file_sha256(filepath_str).await;

    tracing::info!("MySQL dump created: {filename} ({} bytes, hash: {})", meta.len(), sha256.as_deref().unwrap_or("N/A"));

    Ok(BackupInfo {
        filename,
        size_bytes: meta.len(),
        created_at: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        sha256,
        ..Default::default()
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
    let _filepath_str = filepath.to_str().ok_or("Invalid path encoding")?;

    let env_file = DockerEnvFile::new(&[("PGPASSWORD", password)])
        .map_err(|e| format!("Failed to prepare credentials: {e}"))?;
    let mut docker_child = safe_command("docker")
        .arg("exec")
        .arg("--env-file").arg(env_file.path())
        .args([
            container_name,
            "pg_dump",
            "-U", user,
            "-d", db_name,
            // --clean --if-exists so a restore OVERWRITES the target rather than merging
            // into it (without --clean, restoring into a non-empty DB appends/errors per
            // object and silently yields a merge). Pairs with the ON_ERROR_STOP +
            // --single-transaction restore below.
            "--no-owner", "--no-acl", "--clean", "--if-exists",
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

    let filepath_str = filepath.to_str().ok_or("Invalid path encoding")?;
    let sha256 = compute_file_sha256(filepath_str).await;

    tracing::info!("PostgreSQL dump created: {filename} ({} bytes, hash: {})", meta.len(), sha256.as_deref().unwrap_or("N/A"));

    Ok(BackupInfo {
        filename,
        size_bytes: meta.len(),
        created_at: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        sha256,
        ..Default::default()
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
    let _filepath_str = filepath.to_str().ok_or("Invalid path encoding")?;

    // mongodump --archive --gzip streams the archive on stdout. Stream it straight to the
    // dump file rather than buffering the ENTIRE archive in the agent's RAM via .output() —
    // a multi-GB Mongo DB would OOM-kill the shared root agent (a tenant-triggerable
    // cross-tenant DoS). The mysql/pg dump paths already stream via fd pipes; do the same
    // here. stderr is still captured for the failure message; stdout goes to the file fd.
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(600),
        async {
            let file = std::fs::File::create(&filepath)
                .map_err(|e| format!("Failed to create dump file: {e}"))?;
            let child = safe_command("docker")
                .args([
                    "exec", container_name,
                    "mongodump", "--db", db_name, "--archive", "--gzip",
                ])
                .stdout(std::process::Stdio::from(file))
                .stderr(std::process::Stdio::piped())
                // Kill mongodump if this future is dropped (e.g. the outer timeout fires), so it
                // stops streaming into the dump file the moment we give up on it.
                .kill_on_drop(true)
                .spawn()
                .map_err(|e| format!("Failed to run mongodump: {e}"))?;

            let child_output = child.wait_with_output().await
                .map_err(|e| format!("mongodump wait error: {e}"))?;

            if !child_output.status.success() {
                let stderr = String::from_utf8_lossy(&child_output.stderr);
                return Err(format!("MongoDB dump failed: {stderr}"));
            }
            Ok(())
        }
    )
    .await;

    // On timeout the mongodump child is dropped (kill_on_drop=true) and killed, but the
    // partially-streamed dump file remains on disk — remove it so it can't later surface as a
    // "restorable" backup (the old .output() path never created a file until after success, so
    // this cleanup is new-code-specific). Clean up on a normal dump error too.
    let output = match output {
        Ok(inner) => inner,
        Err(_) => {
            std::fs::remove_file(&filepath).ok();
            return Err("Database dump timed out (10 minutes)".to_string());
        }
    };
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

    let filepath_str = filepath.to_str().ok_or("Invalid path encoding")?;
    let sha256 = compute_file_sha256(filepath_str).await;

    tracing::info!("MongoDB dump created: {filename} ({} bytes, hash: {})", meta.len(), sha256.as_deref().unwrap_or("N/A"));

    Ok(BackupInfo {
        filename,
        size_bytes: meta.len(),
        created_at: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        sha256,
        ..Default::default()
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

    let env_file = DockerEnvFile::new(&[("MYSQL_PWD", password)])
        .map_err(|e| format!("Failed to prepare credentials: {e}"))?;
    let docker_child = safe_command("docker")
        .arg("exec").arg("-i")
        .arg("--env-file").arg(env_file.path())
        .args([
            container_name,
            // `mariadb`, NOT `mysql`: the panel provisions `mariadb:11`, and
            // MariaDB 11 dropped the mysql-named client symlinks, so `mysql`
            // does not exist in the container at all. Every sibling call site
            // (database.rs, backup_drill.rs, backup_verify.rs) already invokes
            // `mariadb`; this one did not, so restoring a MySQL/MariaDB dump
            // failed on every install with "executable file not found" while
            // the DUMP half — which correctly calls `mariadb-dump` — worked.
            "mariadb", "-u", user, db_name,
        ])
        .stdin(gunzip_stdout.into_owned_fd().map_err(|_| "Failed to get fd")?)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn docker exec: {e}"))?;

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(600),
        async {
            // Fail the restore if decompression did not complete cleanly — a truncated
            // .gz that ends on a statement boundary otherwise imports partially and the
            // mysql client exits 0 (EOF-as-success).
            let gunzip_status = gunzip_child.wait().await
                .map_err(|e| format!("gunzip wait error: {e}"))?;
            let docker_output = docker_child.wait_with_output().await
                .map_err(|e| format!("docker exec wait error: {e}"))?;
            if !gunzip_status.success() {
                return Err("MySQL restore failed: backup decompression error (truncated/corrupt archive)".to_string());
            }
            if !docker_output.status.success() {
                let stderr = String::from_utf8_lossy(&docker_output.stderr);
                let stderr = stderr.trim();
                // Never report a bare "restore failed:" with nothing after it —
                // a failure with no reason is unactionable, and this path can
                // fail with empty stderr (e.g. the client binary is missing and
                // the runtime writes nothing we captured).
                if stderr.is_empty() {
                    return Err(format!(
                        "MySQL restore failed: the mariadb client exited with {} and produced no error output",
                        docker_output.status
                    ));
                }
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

    let env_file = DockerEnvFile::new(&[("PGPASSWORD", password)])
        .map_err(|e| format!("Failed to prepare credentials: {e}"))?;
    let docker_child = safe_command("docker")
        .arg("exec").arg("-i")
        .arg("--env-file").arg(env_file.path())
        .args([
            container_name,
            // ON_ERROR_STOP=1 + --single-transaction make the restore fail-and-rollback on
            // ANY statement error instead of psql's default (continue-on-error, exit 0),
            // which reported partial/failed restores as success.
            "psql", "-v", "ON_ERROR_STOP=1", "--single-transaction", "-U", user, "-d", db_name,
        ])
        .stdin(gunzip_stdout.into_owned_fd().map_err(|_| "Failed to get fd")?)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn docker exec: {e}"))?;

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(600),
        async {
            // A truncated/corrupt .gz makes gunzip exit non-zero while psql sees a clean
            // EOF at a statement boundary and exits 0 — the classic EOF-as-success trap.
            // Fail the restore if decompression did not complete cleanly.
            let gunzip_status = gunzip_child.wait().await
                .map_err(|e| format!("gunzip wait error: {e}"))?;
            let docker_output = docker_child.wait_with_output().await
                .map_err(|e| format!("docker exec wait error: {e}"))?;
            if !gunzip_status.success() {
                return Err("PostgreSQL restore failed: backup decompression error (truncated/corrupt archive)".to_string());
            }
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

    // Stream the archive from disk to mongorestore's stdin instead of reading the ENTIRE
    // file into RAM (tokio::fs::read) — a multi-GB archive would OOM the shared root agent
    // (same cross-tenant DoS class as the dump path). tokio::io::copy pipes in bounded chunks.
    let mut file = tokio::fs::File::open(filepath).await
        .map_err(|e| format!("Failed to open backup file: {e}"))?;

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
        tokio::io::copy(&mut file, &mut stdin).await?;
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
        // A stray subdirectory is not a dump and must not be reported as a rejected one.
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        // Anything the restore path cannot OPEN is REPORTED, not skipped. This loop used
        // to `continue` here, which is why an operator could copy a dump into this
        // directory, see an empty list, and have nowhere to learn why.
        //
        // The test is `is_safe_filename` itself — the same predicate `get_backup_path`
        // enforces — so this listing cannot offer a name the opener would refuse. That
        // matters for more than tidiness: a filename with a space passes every check the
        // panel makes and then fails when it is put on a request line, which produced a
        // failure the panel could not describe.
        //
        // ⚠ This says only "is this a backup file at all". Whether a given backup can be
        // imported into a given DATABASE is a different question with a different answer
        // per engine and per encryption state, and it is decided by the panel, which
        // knows both. Encoding it here would mislabel every encrypted backup as broken in
        // `dockpanel backup db-list`, which is a backup listing, not an import listing.
        let unsupported = if is_safe_filename(&name) {
            None
        } else {
            Some(unusable_reason(db_name, &name))
        };
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
            unsupported,
            ..Default::default()
        });
    }

    backups.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(backups)
}

/// What to tell an operator about a file that is in the directory but is not a backup
/// this agent can open.
///
/// A dump that is not gzipped is by far the likeliest thing to find here — plain
/// `mysqldump > dump.sql` produces one, and it is exactly what someone reaching for
/// database import already has. `restore_mysql` and `restore_postgres` pipe through
/// `gunzip -c` unconditionally (and never capture gunzip's stderr), so the honest answer
/// is to compress it rather than to widen the gate and find out at the end of a pipe.
///
/// The second case is subtler and cost a real diagnosis: a name that LOOKS fine but
/// carries a character `is_safe_filename` refuses — a space, most often, because
/// `scp "my dump.sql.gz" …` is a natural thing to type. Such a name used to be listed
/// as importable and then failed while the HTTP request line was being built, which is
/// about as far from the operator's action as a failure can happen.
///
/// ⚠ This answers "is this a backup at all", NOT "can it go into this database". The
/// second question depends on the engine and on whether the file is encrypted, and it
/// is answered by the panel, which knows both.
fn unusable_reason(db_name: &str, name: &str) -> String {
    let full = format!("{BACKUP_DIR}/{db_name}/{name}");
    let ext_ok = name.ends_with(".sql.gz")
        || name.ends_with(".archive.gz")
        || name.ends_with(".sql.gz.enc")
        || name.ends_with(".archive.gz.enc");

    if ext_ok {
        // The extension is right, so it was the charset that failed.
        format!(
            "The name contains a character backups cannot use — spaces most often. \
             Rename it with: mv '{full}' {BACKUP_DIR}/{db_name}/{}",
            sanitise_suggestion(name)
        )
    } else if name.ends_with(".sql") || name.ends_with(".archive") || name.ends_with(".dump") {
        format!(
            "Not compressed. Backups are read as .sql.gz — compress it in place with: gzip {full}"
        )
    } else if name.ends_with(".zip") {
        format!(
            "ZIP is not supported. Unpack it and gzip the .sql inside: unzip {full} && gzip <name>.sql"
        )
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") || name.ends_with(".tar") {
        "This looks like a site archive, not a database dump. A full cPanel/Plesk archive \
         goes through Migration instead."
            .to_string()
    } else {
        "Not a database dump. Backups are read as .sql.gz or .archive.gz.".to_string()
    }
}

/// A pasteable replacement for a name the charset refuses.
///
/// Only ever embedded in advice — nothing opens the result — but it is built with the
/// same charset `is_safe_filename` enforces, so following the advice actually works.
fn sanitise_suggestion(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_the_certificate_authority_said_is_marked() {
        use crate::services::ssl::CA_DECLINED;
        // ⭐ The classification the panel's 422 rests on. A CA refusal carries the
        // marker; a fault on this machine's own disk does not, and so keeps the
        // reference number the operator cannot act on anyway.
        let ca = format!("{CA_DECLINED}The CA did not validate the challenge: timeout");
        assert!(ca.strip_prefix(CA_DECLINED).is_some());
        let local = "Failed to write cert: No space left on device (os error 28)".to_string();
        assert!(local.strip_prefix(CA_DECLINED).is_none());
    }

    #[test]
    fn dump_dir_name_never_refuses_what_the_shared_validator_accepts() {
        // ⭐ THE PROPERTY THAT MATTERS: superset, never narrower. The purge owning
        // its charset is only safe if no name loses a door by the swap — and the
        // length cap is where that nearly went wrong, since the two differed by
        // one and nothing in a database IMPORT applies the create form's limit.
        for n in ["wp_main", "db-legacy", "a", &"x".repeat(64)] {
            assert!(
                is_db_dir_name(n),
                "{n} is accepted by the shared validator and must not lose a door"
            );
        }
        assert!(is_db_dir_name("_wp"), "no rule about the first character");
    }

    #[test]
    fn dump_dir_name_cannot_spell_a_traversal_at_all() {
        // Not "blocks traversal" — cannot express it. A purge resolves this name
        // to a directory it then removes, so the charset admitting no dot and no
        // separator is the property that matters, not a list of refusals.
        assert!(!is_db_dir_name("."));
        assert!(!is_db_dir_name(".."));
        assert!(!is_db_dir_name("../etc"));
        assert!(!is_db_dir_name("a/b"));
        assert!(!is_db_dir_name("a.b"));
        assert!(!is_db_dir_name(""));
        assert!(!is_db_dir_name(&"x".repeat(65)));
        assert!(is_db_dir_name(&"x".repeat(64)));
    }

    #[test]
    fn a_purge_takes_only_the_dumps_this_module_minted() {
        // ⭐ THE ASSERTION THAT KEEPS THE PROMISE. The product's own screen tells
        // operators to place an import here by hand, and the extension test that
        // guards the import door would have called that file a dump and deleted
        // it — the exact opposite of what the purge documents about itself.
        assert!(is_minted_dump_name("wp", "wp-20260821-140302.sql.gz"));
        assert!(is_minted_dump_name("wp", "wp-20260821-140302.sql.gz.enc"));
        assert!(is_minted_dump_name("wp", "wp-20260821-140302.archive.gz"));
        // Hand-placed, and every one of these passes the import door's test.
        assert!(!is_minted_dump_name("wp", "dump.sql.gz"));
        assert!(!is_minted_dump_name("wp", "wp.sql.gz"));
        assert!(!is_minted_dump_name("wp", "wp-before-upgrade.sql.gz"));
        assert!(!is_minted_dump_name("wp", "wp-2026.sql.gz"));
        // Another database's dump never belongs to this one's purge.
        assert!(!is_minted_dump_name("wp", "other-20260821-140302.sql.gz"));
        // A minted stem with an extension we do not write is not ours either.
        assert!(!is_minted_dump_name("wp", "wp-20260821-140302.tar"));
    }

    #[tokio::test]
    async fn purge_refuses_a_name_it_cannot_validate() {
        // The dot is the one that matters: the older identifier check in this very
        // file accepts it, and resolving it would name the dump root itself.
        assert!(purge_dir(".").await.is_err());
        assert!(purge_dir("..").await.is_err());
        assert!(purge_dir("a/b").await.is_err());
    }

    #[tokio::test]
    async fn purge_of_an_absent_directory_is_a_quiet_success() {
        // Deleting a database that never had a dump must not fail the delete.
        let r = purge_dir("nosuchdatabase_forpurgetest").await.expect("no error");
        assert_eq!(r.removed, 0);
        assert!(!r.dir_removed);
    }

    #[test]
    fn safe_db_identifier_rejects_traversal() {
        // Traversal sequences must be rejected even though '.' is allowed in the charset.
        assert!(!is_safe_db_identifier(".."));
        assert!(!is_safe_db_identifier("../etc"));
        assert!(!is_safe_db_identifier("a/../b"));
        assert!(!is_safe_db_identifier("foo/bar"));
        assert!(!is_safe_db_identifier("-leadingdash"));
        assert!(!is_safe_db_identifier(""));
        // Legitimate identifiers still pass.
        assert!(is_safe_db_identifier("wordpress"));
        assert!(is_safe_db_identifier("my_db-01"));
        assert!(is_safe_db_identifier("dockpanel-db-wordpress"));
    }

    #[test]
    fn safe_filename_rejects_traversal_and_bad_ext() {
        assert!(!is_safe_filename("../evil.sql.gz"));
        assert!(!is_safe_filename("a/b.sql.gz"));
        assert!(!is_safe_filename("dump.txt"));
        assert!(is_safe_filename("wordpress-20260722-120000.sql.gz"));
        assert!(is_safe_filename("db-20260722-120000.archive.gz.enc"));
    }

    /// The listing must offer exactly what `get_backup_path` will open — no more, or
    /// it advertises a file the opener refuses; no less, or a real backup disappears.
    #[test]
    fn the_listing_offers_exactly_what_the_opener_accepts() {
        for ok in [
            "wordpress-20260722-120000.sql.gz",
            "wordpress-20260722-120000.archive.gz",
            "wordpress-20260722-120000.sql.gz.enc",
            "wordpress-20260722-120000.archive.gz.enc",
        ] {
            assert!(is_safe_filename(ok), "{ok} must be listed and openable");
        }
        for bad in ["dump.sql", "dump.zip", "backup.tar.gz", "notes.txt", "dump"] {
            assert!(!is_safe_filename(bad), "{bad} must not be listed as a backup");
        }
    }

    /// ⛔ The bug this predicate was widened to catch. `scp "my dump.sql.gz" …` is a
    /// natural thing to type, and the name it produces has the right extension. It used
    /// to be listed as importable and then failed while the HTTP request line was being
    /// built — a failure with no explanation available anywhere near the operator.
    #[test]
    fn a_name_with_a_space_is_reported_rather_than_offered() {
        let name = "my dump.sql.gz";
        assert!(!is_safe_filename(name), "a space must keep it out of the listing");
        let why = unusable_reason("wordpress", name);
        assert!(why.contains("character"), "must say it is the NAME that is wrong: {why}");
        assert!(why.contains("my_dump.sql.gz"), "must offer a pasteable fix: {why}");
        // The advice has to be advice that works.
        assert!(
            is_safe_filename(&sanitise_suggestion(name)),
            "the suggested name must itself be acceptable"
        );
    }

    /// The point of the reason string is that the operator can act on it without
    /// leaving the page, so it has to name the file and the command.
    #[test]
    fn unusable_reason_names_the_file_and_the_fix() {
        let plain = unusable_reason("wordpress", "dump.sql");
        assert!(plain.contains("gzip"), "a plain .sql must be told to gzip: {plain}");
        assert!(
            plain.contains("/var/backups/dockpanel/databases/wordpress/dump.sql"),
            "the fix must name the full path so it can be pasted: {plain}"
        );

        let zip = unusable_reason("wordpress", "backup.zip");
        assert!(zip.contains("unzip"), "a .zip must be told to unpack: {zip}");

        // A site archive is a different product surface, and "gzip it" would send the
        // operator down a road that ends in a failed import.
        let archive = unusable_reason("wordpress", "cpanel-full.tar.gz");
        assert!(archive.contains("Migration"), "must route to Migration: {archive}");
        assert!(!archive.contains("gzip "), "must not say gzip: {archive}");

        let other = unusable_reason("wordpress", "notes.txt");
        assert!(other.contains(".sql.gz"), "must name the accepted form: {other}");
        // ⚠ It must NOT claim .enc forms are importable — the panel refuses those, and
        // two sentences on one screen disagreeing is worse than either alone.
        assert!(!other.contains(".enc"), "must not advertise .enc as importable: {other}");
    }
}
