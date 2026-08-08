use std::path::{Path, PathBuf};
use crate::safe_cmd::safe_command;
use crate::services::database_backup;

const BACKUP_DIR: &str = "/var/backups/dockpanel";
const WEBROOT: &str = "/var/www";

/// Where database dumps are staged while an archive is being built or unpacked.
/// Must live under a path the agent unit lists in `ReadWritePaths`.
const STAGING_ROOT: &str = "/var/backups/dockpanel/.staging";

// The three roots below are indirected through functions ONLY so the unit tests
// can run hermetically under a temp directory: the real paths need root and a
// provisioned box, which CI is neither. The override exists exclusively in
// `cfg(test)` builds — a release binary has no way to relocate these.
#[cfg(not(test))]
fn webroot_root() -> PathBuf { PathBuf::from(WEBROOT) }
#[cfg(not(test))]
fn backups_root() -> PathBuf { PathBuf::from(BACKUP_DIR) }
#[cfg(not(test))]
fn staging_root() -> PathBuf { PathBuf::from(STAGING_ROOT) }

#[cfg(test)]
fn test_root() -> PathBuf { std::env::temp_dir().join("dockpanel-agent-tests") }
#[cfg(test)]
fn webroot_root() -> PathBuf { test_root().join("www") }
#[cfg(test)]
fn backups_root() -> PathBuf { test_root().join("backups") }
#[cfg(test)]
fn staging_root() -> PathBuf { test_root().join("backups/.staging") }

/// Directory inside the archive that holds everything DockPanel adds to a site
/// backup beyond the webroot itself. Deliberately NOT under `./` so it is a
/// distinct top-level member and can be extracted (and excluded) on its own.
const PAYLOAD_DIR: &str = ".dockpanel-backup";

/// Make the whole backup tree root-only, repairing what is already on disk.
///
/// Everything under `/var/backups/dockpanel` is a restorable copy of something
/// secret: the panel's own database — which carries `servers.agent_token` in
/// CLEARTEXT, and that token is both the Bearer credential for every
/// authenticated agent endpoint and the HMAC key the agent verifies its own
/// root-PTY tickets with (`routes/terminal.rs`) — plus every tenant's database
/// dump and every site archive, `wp-config.php` included.
///
/// The tree was created `0755` and each writer used a plain shell redirect or
/// `fs::write`, so files landed `0644`. Both panel services run as root, so
/// nothing legitimate ever needed the world bits — but on a panel host that
/// also serves sites, `www-data` is the uid EVERY hosted site's PHP runs as,
/// and `services/command_filter.rs` already records that every site resolves to
/// that one uid. Measured on the demo box at v2.87.0: **17 of 17 files readable
/// as www-data**, the newest being a full `pg_dump` of the panel database.
///
/// The directory bit is the load-bearing half: at `0700` nothing under the tree
/// is reachable by a non-root uid whatever the individual files say. The file
/// bits are defence in depth for anything that gets moved out.
///
/// Repairing on every start rather than only fixing the writers is deliberate.
/// Three writers produce these files — this agent, the panel's auto-healer, and
/// a root crontab line `setup.sh` installs — and a fix that only corrects new
/// writes would leave every install that already holds a backup exposed for
/// ever. That is the same delivery shape as the s280 install-template lesson.
/// Idempotent, cheap, and it walks a directory that holds tens of files.
pub fn secure_backup_tree() {
    secure_tree_at(&backups_root());
}

/// The body of [`secure_backup_tree`], parameterized on the root.
///
/// Split out for ONE reason: the tests must not share `backups_root()`. Nine
/// tests in this module use that single fixed path and cargo runs them
/// concurrently, so an arm written against it races another test's
/// `remove_dir_all` — which is exactly what happened. The first symlink arm
/// written here PASSED against a deliberately broken `tighten()`, twice, and
/// only a mutation revealed it was measuring an empty directory.
fn secure_tree_at(root: &Path) {
    use std::os::unix::fs::PermissionsExt;

    fn tighten(path: &Path, want: u32) {
        // Compare before writing so a correct tree costs one stat per entry and
        // the log line below only ever fires on a box that was actually exposed.
        let Ok(meta) = std::fs::symlink_metadata(path) else { return };
        // Never follow a symlink here: chmod would apply to its target, which is
        // an arbitrary path a writer could have aimed anywhere.
        if meta.file_type().is_symlink() {
            return;
        }
        if meta.permissions().mode() & 0o777 != want {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(want));
        }
    }

    fn walk(dir: &Path, loosened: &mut u32) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                if meta.permissions().mode() & 0o777 != 0o700 {
                    *loosened += 1;
                }
                tighten(&path, 0o700);
                walk(&path, loosened);
            } else {
                if meta.permissions().mode() & 0o007 != 0 {
                    *loosened += 1;
                }
                tighten(&path, 0o600);
            }
        }
    }

    if !root.exists() {
        return;
    }
    let mut loosened = 0u32;
    tighten(root, 0o700);
    walk(root, &mut loosened);
    if loosened > 0 {
        tracing::warn!(
            "Tightened {loosened} world-readable entries under {} — a backup there \
             carries the panel database, which holds agent tokens in cleartext",
            root.display()
        );
    }
}

/// Bump when the payload layout changes in a way a reader must know about.
const MANIFEST_SCHEMA: u32 = 1;

#[cfg(all(test, unix))]
mod backup_perm_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode_of(p: &Path) -> u32 {
        std::fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777
    }

    /// A root of this arm's OWN, never `backups_root()` — see `secure_tree_at`.
    fn isolated_root() -> PathBuf {
        let d = std::env::temp_dir().join(format!("dp-bkperm-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The defect, reproduced: a 0755 tree of 0644 dumps is readable by any uid
    /// on the box — and on a panel host that also serves sites, that uid is
    /// `www-data`, which runs every tenant's PHP. Measured on the demo box at
    /// v2.87.0: 17 of 17 files readable as www-data, the newest a full pg_dump
    /// carrying `servers.agent_token` in cleartext.
    #[test]
    fn a_world_readable_backup_tree_is_repaired() {
        let root = isolated_root();
        std::fs::create_dir_all(root.join("db")).unwrap();
        std::fs::write(root.join("dockpanel-db-x.sql.gz"), b"dump").unwrap();
        std::fs::write(root.join("db/nested.sql.gz"), b"dump").unwrap();
        // Exactly what the three writers leave behind today.
        for (p, m) in [
            (root.clone(), 0o755),
            (root.join("db"), 0o755),
            (root.join("dockpanel-db-x.sql.gz"), 0o644),
            (root.join("db/nested.sql.gz"), 0o644),
        ] {
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(m)).unwrap();
        }
        // The arm can only mean something if the tree really is exposed first.
        assert_eq!(mode_of(&root.join("dockpanel-db-x.sql.gz")), 0o644, "setup did not land");

        secure_tree_at(&root);

        assert_eq!(mode_of(&root), 0o700, "the backup root is still traversable");
        assert_eq!(mode_of(&root.join("db")), 0o700, "a nested backup dir is still traversable");
        assert_eq!(mode_of(&root.join("dockpanel-db-x.sql.gz")), 0o600, "the panel dump is still world-readable");
        assert_eq!(mode_of(&root.join("db/nested.sql.gz")), 0o600, "a nested dump is still world-readable");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// It must never chmod THROUGH a symlink: a writer that dropped one in the
    /// tree could otherwise aim a root-owned `set_permissions` at any path.
    #[test]
    fn it_does_not_follow_a_symlink_out_of_the_tree() {
        let root = isolated_root();
        let outside = std::env::temp_dir().join(format!("dp-outside-{}", uuid::Uuid::new_v4()));
        std::fs::write(&outside, b"not ours").unwrap();
        std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        // Non-vacuity: prove the link resolves before asserting it is not followed.
        assert_eq!(std::fs::read_to_string(root.join("link")).unwrap(), "not ours");

        secure_tree_at(&root);

        assert_eq!(mode_of(&outside), 0o644, "chmod followed a symlink out of the backup tree");
        let _ = std::fs::remove_file(&outside);
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// A database the panel wants dumped into (or restored from) a site backup.
///
/// The agent has no access to the panel's `databases` table, so the backend
/// resolves the site's databases and passes their decrypted credentials here.
#[derive(serde::Deserialize, Clone, Debug)]
pub struct DbSpec {
    pub container_name: String,
    pub db_name: String,
    pub db_type: String,
    pub user: String,
    #[serde(default)]
    pub password: String,
}

/// A database that could not be dumped or restored, and why. Never swallowed —
/// a backup that silently lost a database is the defect this whole path exists
/// to close.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct DbFailure {
    pub db_name: String,
    pub error: String,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct ManifestDb {
    pub db_name: String,
    pub db_type: String,
    /// File name inside `<PAYLOAD_DIR>/db/`.
    pub file: String,
    pub size_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct Manifest {
    pub schema: u32,
    pub domain: String,
    pub created_at: String,
    #[serde(default)]
    pub databases: Vec<ManifestDb>,
}

#[derive(serde::Serialize, Default)]
pub struct BackupInfo {
    pub filename: String,
    pub size_bytes: u64,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Databases whose dump is inside this archive. Empty (and omitted) for a
    /// files-only backup, which is what every archive made before v2.34.0 is.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub databases_included: Vec<String>,
    /// Databases the panel asked for that are NOT in the archive.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub databases_failed: Vec<DbFailure>,
}

/// What a restore actually did. `ok()` is the only thing callers should treat
/// as success — a restore that put the files back but not the database is a
/// failure with a partially-changed site, not a success with a footnote.
#[derive(serde::Serialize, Default)]
pub struct RestoreReport {
    pub files_restored: bool,
    pub databases_in_archive: usize,
    pub databases_restored: Vec<String>,
    pub databases_failed: Vec<DbFailure>,
}

impl RestoreReport {
    pub fn ok(&self) -> bool {
        self.files_restored && self.databases_failed.is_empty()
    }
}

/// Create a staging directory that only root can read. The dumps inside are the
/// site's entire content in plaintext.
fn make_staging(tag: &str) -> Result<PathBuf, String> {
    let dir = staging_root().join(tag);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create staging dir: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        // The parent is shared by every staged backup; keep it closed too.
        let _ = std::fs::set_permissions(staging_root(), std::fs::Permissions::from_mode(0o700));
    }
    Ok(dir)
}

/// Dump one database and move it into the archive payload, reusing the hardened
/// per-engine dump paths rather than re-implementing them.
async fn stage_one_dump(spec: &DbSpec, dest_dir: &Path) -> Result<ManifestDb, String> {
    let info = match spec.db_type.as_str() {
        "mysql" | "mariadb" => {
            database_backup::dump_mysql(&spec.container_name, &spec.db_name, &spec.user, &spec.password).await?
        }
        "postgres" | "postgresql" => {
            database_backup::dump_postgres(&spec.container_name, &spec.db_name, &spec.user, &spec.password).await?
        }
        "mongo" | "mongodb" => {
            database_backup::dump_mongo(&spec.container_name, &spec.db_name).await?
        }
        other => return Err(format!("Unsupported database engine '{other}'")),
    };

    // dump_* writes into the per-database backup tree; move it into the archive
    // payload so it does not also surface as a standalone database backup.
    let src = database_backup::get_backup_path(&spec.db_name, &info.filename)?;
    let dest = dest_dir.join(&info.filename);
    std::fs::rename(&src, &dest)
        .map_err(|e| format!("Failed to stage dump for {}: {e}", spec.db_name))?;

    Ok(ManifestDb {
        db_name: spec.db_name.clone(),
        db_type: spec.db_type.clone(),
        file: info.filename,
        size_bytes: info.size_bytes,
        sha256: info.sha256,
    })
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
    backups_root().join(domain)
}

/// Create a backup of the site's webroot, plus a dump of each database the panel
/// passes in.
///
/// With an empty `databases` slice this produces byte-for-byte the archive it
/// always did, so nothing about existing backups or callers changes.
pub async fn create_backup(domain: &str, databases: &[DbSpec]) -> Result<BackupInfo, String> {
    let site_root = webroot_root().join(domain);
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

    // ── Stage database dumps into the archive payload ───────────────────────
    let mut staging: Option<PathBuf> = None;
    let mut databases_included: Vec<String> = Vec::new();
    let mut databases_failed: Vec<DbFailure> = Vec::new();

    if !databases.is_empty() {
        if site_root.join(PAYLOAD_DIR).exists() {
            // The site itself owns a path we would have to shadow. Refuse the
            // payload rather than produce an archive whose restore is ambiguous.
            for spec in databases {
                databases_failed.push(DbFailure {
                    db_name: spec.db_name.clone(),
                    error: format!(
                        "site root contains a reserved '{PAYLOAD_DIR}' path; database not included"
                    ),
                });
            }
        } else {
            let stage = make_staging(&format!("{domain}-{timestamp}"))?;
            let db_dir = stage.join(PAYLOAD_DIR).join("db");
            std::fs::create_dir_all(&db_dir)
                .map_err(|e| format!("Failed to create payload dir: {e}"))?;

            let mut manifest_dbs: Vec<ManifestDb> = Vec::new();
            for spec in databases {
                match stage_one_dump(spec, &db_dir).await {
                    Ok(entry) => {
                        databases_included.push(spec.db_name.clone());
                        manifest_dbs.push(entry);
                    }
                    Err(e) => {
                        tracing::warn!("Backup for {domain}: database {} not included: {e}", spec.db_name);
                        databases_failed.push(DbFailure {
                            db_name: spec.db_name.clone(),
                            error: e,
                        });
                    }
                }
            }

            let manifest = Manifest {
                schema: MANIFEST_SCHEMA,
                domain: domain.to_string(),
                created_at: chrono::Utc::now().to_rfc3339(),
                databases: manifest_dbs,
            };
            let manifest_json = serde_json::to_string_pretty(&manifest)
                .map_err(|e| format!("Failed to serialize manifest: {e}"))?;
            std::fs::write(stage.join(PAYLOAD_DIR).join("manifest.json"), manifest_json)
                .map_err(|e| format!("Failed to write manifest: {e}"))?;

            staging = Some(stage);
        }
    }

    let mut tar_args: Vec<String> = vec![
        "czf".into(),
        filepath_str.into(),
        "-C".into(),
        site_root_str.into(),
        ".".into(),
    ];
    if let Some(stage) = &staging {
        let stage_str = stage
            .to_str()
            .ok_or_else(|| "Invalid staging path encoding".to_string())?;
        tar_args.push("-C".into());
        tar_args.push(stage_str.into());
        tar_args.push(PAYLOAD_DIR.into());
    }

    let tar_result = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        safe_command("tar").args(&tar_args).output(),
    )
    .await;

    // The staged dumps are a full plaintext copy of the site's data — remove
    // them whatever happened to tar.
    if let Some(stage) = &staging {
        let _ = std::fs::remove_dir_all(stage);
    }

    let output = tar_result
        .map_err(|_| "Backup timed out (5 minutes)".to_string())?
        .map_err(|e| format!("Failed to run tar: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        std::fs::remove_file(&filepath).ok();
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

    tracing::info!(
        "Backup created: {filename} ({} bytes, hash: {}, databases: {} included / {} failed)",
        meta.len(),
        sha256.as_deref().unwrap_or("N/A"),
        databases_included.len(),
        databases_failed.len()
    );

    Ok(BackupInfo {
        filename,
        size_bytes: meta.len(),
        created_at: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        sha256,
        databases_included,
        databases_failed,
    })
}

/// Compute SHA256 hash of a file (for backup integrity chain).
/// Shared by site / database / volume backup paths.
pub(crate) async fn compute_file_sha256(path: &str) -> Option<String> {
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
            databases_included: Vec::new(),
            databases_failed: Vec::new(),
        });
    }

    backups.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(backups)
}

/// Extract the DockPanel payload out of an archive into `stage`, if it has one.
///
/// Returns `Ok(None)` for an archive that predates the payload (every archive
/// made before v2.34.0) — that is a normal, expected shape, not an error.
async fn extract_payload(archive: &str, stage: &Path) -> Result<Option<Manifest>, String> {
    let stage_str = stage
        .to_str()
        .ok_or_else(|| "Invalid staging path encoding".to_string())?;

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        safe_command("tar")
            .args([
                "xzf", archive,
                "--no-same-owner", "--no-same-permissions",
                "-C", stage_str,
                PAYLOAD_DIR,
            ])
            .output(),
    )
    .await
    .map_err(|_| "Reading backup payload timed out (5 minutes)".to_string())?
    .map_err(|e| format!("Failed to run tar: {e}"))?;

    let manifest_path = stage.join(PAYLOAD_DIR).join("manifest.json");

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // GNU tar's way of saying "this archive simply doesn't have that member".
        if stderr.contains("Not found in archive") || stderr.contains("not found in archive") {
            return Ok(None);
        }
        return Err(format!("Failed to read backup payload: {stderr}"));
    }

    if !manifest_path.exists() {
        return Ok(None);
    }

    let raw = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("Failed to read backup manifest: {e}"))?;
    let manifest: Manifest = serde_json::from_str(&raw)
        .map_err(|e| format!("Backup manifest is unreadable: {e}"))?;

    if manifest.schema > MANIFEST_SCHEMA {
        return Err(format!(
            "Backup was made by a newer DockPanel (manifest schema {}, this agent understands {MANIFEST_SCHEMA}). Update this server before restoring it.",
            manifest.schema
        ));
    }

    Ok(Some(manifest))
}

/// Restore a backup to the site's webroot, and restore any databases the
/// archive carries.
///
/// Files are restored first and databases last: the per-engine restore paths
/// roll back on failure, so a database that fails to load leaves the live data
/// as it was rather than half-overwritten.
pub async fn restore_backup(
    domain: &str,
    filename: &str,
    databases: &[DbSpec],
) -> Result<RestoreReport, String> {
    if !is_safe_filename(filename) {
        return Err("Invalid backup filename".into());
    }

    let filepath = backup_dir(domain).join(filename);
    if !filepath.exists() {
        return Err("Backup file not found".into());
    }

    let site_root = webroot_root().join(domain);
    std::fs::create_dir_all(&site_root)
        .map_err(|e| format!("Failed to create site root: {e}"))?;

    let filepath_str = filepath
        .to_str()
        .ok_or_else(|| "Invalid backup path encoding".to_string())?;
    let site_root_str = site_root
        .to_str()
        .ok_or_else(|| "Invalid site root path encoding".to_string())?;

    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let stage = make_staging(&format!("restore-{domain}-{timestamp}"))?;

    let result = restore_inner(
        domain, filename, filepath_str, site_root_str, &site_root, &stage, databases,
    )
    .await;

    // The staged dumps are the site's data in plaintext — never leave them behind.
    let _ = std::fs::remove_dir_all(&stage);

    result
}

#[allow(clippy::too_many_arguments)]
async fn restore_inner(
    domain: &str,
    filename: &str,
    filepath_str: &str,
    site_root_str: &str,
    site_root: &Path,
    stage: &Path,
    databases: &[DbSpec],
) -> Result<RestoreReport, String> {
    let mut report = RestoreReport::default();

    // 1. Pull the database payload out FIRST, so it never lands in the webroot.
    let manifest = extract_payload(filepath_str, stage).await?;
    let manifest_dbs = manifest.map(|m| m.databases).unwrap_or_default();
    report.databases_in_archive = manifest_dbs.len();

    // 2. Restore the files. Full overwrite ensures a clean restore;
    //    --no-same-owner prevents uid/gid hijacking. The payload is excluded so
    //    a site restored from a v1 archive does not gain a .dockpanel-backup
    //    directory full of database dumps under its document root.
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        safe_command("tar")
            .args([
                "xzf", filepath_str,
                "--no-same-owner", "--no-same-permissions",
                "--exclude", PAYLOAD_DIR,
                "--exclude", &format!("{PAYLOAD_DIR}/*"),
                "-C", site_root_str,
            ])
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
    let entries = std::fs::read_dir(site_root).map(|d| d.count()).unwrap_or(0);
    if entries == 0 {
        return Err("Restore completed but site directory is empty".to_string());
    }
    report.files_restored = true;

    // 3. Restore each database the archive carries.
    let db_dir = stage.join(PAYLOAD_DIR).join("db");
    for entry in &manifest_dbs {
        let Some(spec) = databases.iter().find(|s| s.db_name == entry.db_name) else {
            // Two different situations, and telling them apart matters to whoever
            // reads the message: the caller sent no credentials at all (the CLI
            // authenticates to this agent and cannot resolve them), or the site
            // genuinely no longer has that database.
            let error = if databases.is_empty() {
                format!(
                    "this archive contains a dump of '{}', but the caller supplied no database credentials, so it could not be restored",
                    entry.db_name
                )
            } else {
                format!(
                    "the archive contains a dump of '{}' but no such database is attached to this site now; restore it from the Databases page",
                    entry.db_name
                )
            };
            report.databases_failed.push(DbFailure { db_name: entry.db_name.clone(), error });
            continue;
        };

        let dump_path = db_dir.join(&entry.file);
        if !dump_path.exists() {
            report.databases_failed.push(DbFailure {
                db_name: entry.db_name.clone(),
                error: "the manifest lists this database but its dump is missing from the archive".into(),
            });
            continue;
        }
        let Some(dump_str) = dump_path.to_str() else {
            report.databases_failed.push(DbFailure {
                db_name: entry.db_name.clone(),
                error: "Invalid dump path encoding".into(),
            });
            continue;
        };

        let outcome = match spec.db_type.as_str() {
            "mysql" | "mariadb" => {
                database_backup::restore_mysql(
                    &spec.container_name, &spec.db_name, &spec.user, &spec.password, dump_str,
                ).await
            }
            "postgres" | "postgresql" => {
                database_backup::restore_postgres(
                    &spec.container_name, &spec.db_name, &spec.user, &spec.password, dump_str,
                ).await
            }
            "mongo" | "mongodb" => {
                database_backup::restore_mongo(&spec.container_name, &spec.db_name, dump_str).await
            }
            other => Err(format!("Unsupported database engine '{other}'")),
        };

        match outcome {
            Ok(()) => report.databases_restored.push(entry.db_name.clone()),
            Err(e) => {
                tracing::error!("Restore for {domain}: database {} failed: {e}", entry.db_name);
                report.databases_failed.push(DbFailure {
                    db_name: entry.db_name.clone(),
                    error: e,
                });
            }
        }
    }

    tracing::info!(
        "Backup restored: {filename} for {domain} ({entries} entries, databases: {} restored / {} failed of {} in archive)",
        report.databases_restored.len(),
        report.databases_failed.len(),
        report.databases_in_archive
    );
    Ok(report)
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
    // Never hand back the database payload through the single-file path: those
    // dumps are the site's entire content in plaintext and the target is a
    // publicly-served document root. Today the archive's payload members carry
    // no `./` prefix while this function always adds one, so tar would not match
    // them anyway — but that is an accident of member naming, and the guard
    // should not depend on it.
    let normalized = file_path.strip_prefix("./").unwrap_or(file_path);
    if normalized == PAYLOAD_DIR || normalized.starts_with(&format!("{PAYLOAD_DIR}/")) {
        return Err(format!(
            "'{PAYLOAD_DIR}' holds this backup's database dumps and cannot be restored into the site directory; restore the whole backup instead"
        ));
    }

    let backup_filepath = backup_dir(domain).join(filename);
    if !backup_filepath.exists() {
        return Err("Backup file not found".into());
    }

    let target = webroot_root().join(domain).to_string_lossy().to_string();
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

#[cfg(test)]
mod db_payload_tests {
    use super::*;

    /// Creates `/var/www/<domain>` and removes both it and the backup dir on drop,
    /// so a failing assertion cannot leave state behind on a real box.
    struct TestSite(String);

    impl TestSite {
        fn new(tag: &str) -> Self {
            let domain = format!("dp-pin-{tag}.invalid");
            let root = webroot_root().join(&domain);
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("create test site root");
            std::fs::write(root.join("index.html"), b"<h1>marker</h1>").expect("write marker");
            TestSite(domain)
        }
        fn domain(&self) -> &str { &self.0 }
        fn root(&self) -> PathBuf { webroot_root().join(&self.0) }
    }

    impl Drop for TestSite {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.root());
            let _ = std::fs::remove_dir_all(backup_dir(&self.0));
        }
    }

    fn members(archive: &Path) -> String {
        let out = std::process::Command::new("tar")
            .args(["tzf", archive.to_str().unwrap()])
            .output()
            .expect("tar tzf");
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    fn spec(db_name: &str, db_type: &str) -> DbSpec {
        DbSpec {
            // A container that does not exist, so the dump fails without needing
            // a live database — what is under test is how the failure is REPORTED.
            container_name: format!("dp-pin-absent-{db_name}"),
            db_name: db_name.to_string(),
            db_type: db_type.to_string(),
            user: "pinuser".into(),
            password: "pinpass".into(),
        }
    }

    /// A site with no databases must produce exactly the archive it always did.
    #[tokio::test]
    async fn no_databases_produces_a_files_only_archive() {
        let site = TestSite::new("nodb");
        let info = create_backup(site.domain(), &[]).await.expect("backup");
        let archive = backup_dir(site.domain()).join(&info.filename);

        let listing = members(&archive);
        assert!(listing.contains("./index.html"), "site files must be in the archive: {listing}");
        assert!(
            !listing.contains(PAYLOAD_DIR),
            "an archive for a site with no databases must carry no payload dir: {listing}"
        );
        assert!(info.databases_included.is_empty());
        assert!(info.databases_failed.is_empty());
    }

    /// A database that cannot be dumped is REPORTED, never silently dropped.
    /// This is the whole thesis of the ship: a backup missing the site's content
    /// must not be indistinguishable from a complete one.
    #[tokio::test]
    async fn a_failed_dump_is_reported_not_swallowed() {
        let site = TestSite::new("faildump");
        let info = create_backup(site.domain(), &[spec("pinmissing", "postgres")])
            .await
            .expect("backup still succeeds for the files");

        assert!(
            info.databases_included.is_empty(),
            "a database that could not be dumped must not be listed as included"
        );
        assert_eq!(info.databases_failed.len(), 1, "the failure must be reported");
        assert_eq!(info.databases_failed[0].db_name, "pinmissing");
        assert!(
            !info.databases_failed[0].error.is_empty(),
            "the report must say WHY the database is missing"
        );
    }

    /// A site that already owns the reserved path is refused rather than shadowed.
    #[tokio::test]
    async fn reserved_path_collision_refuses_the_payload() {
        let site = TestSite::new("collide");
        std::fs::create_dir_all(site.root().join(PAYLOAD_DIR)).expect("collide dir");

        let info = create_backup(site.domain(), &[spec("pincollide", "postgres")])
            .await
            .expect("backup");

        assert_eq!(info.databases_failed.len(), 1);
        assert!(
            info.databases_failed[0].error.contains("reserved"),
            "the collision must be named as the reason: {}",
            info.databases_failed[0].error
        );
    }

    /// Restoring a pre-v2.34.0 archive must still work, and report honestly that
    /// it carried no database.
    #[tokio::test]
    async fn restoring_a_files_only_archive_still_works() {
        let site = TestSite::new("oldarchive");
        let info = create_backup(site.domain(), &[]).await.expect("backup");
        std::fs::remove_file(site.root().join("index.html")).expect("delete marker");

        let report = restore_backup(site.domain(), &info.filename, &[])
            .await
            .expect("restore");

        assert!(report.files_restored);
        assert_eq!(report.databases_in_archive, 0);
        assert!(report.ok(), "a files-only archive restores cleanly");
        assert!(site.root().join("index.html").exists(), "the file marker must come back");
    }

    /// The payload must never be extracted into the document root — the dumps are
    /// the site's entire content in plaintext and the webroot is served publicly.
    /// It must also count as a failure when its database cannot be restored.
    #[tokio::test]
    async fn payload_never_lands_in_the_webroot_and_a_missing_target_fails() {
        let site = TestSite::new("payload");

        // Hand-build a v1 archive: site files plus a payload naming one database.
        let stage = std::env::temp_dir().join("dp-pin-payload-stage");
        let _ = std::fs::remove_dir_all(&stage);
        std::fs::create_dir_all(stage.join(PAYLOAD_DIR).join("db")).expect("stage");
        std::fs::write(stage.join(PAYLOAD_DIR).join("db").join("pinwp.sql.gz"), b"not-a-real-dump")
            .expect("dump");
        let manifest = serde_json::json!({
            "schema": 1, "domain": site.domain(), "created_at": "2026-07-26T00:00:00Z",
            "databases": [{
                "db_name": "pinwp", "db_type": "postgres",
                "file": "pinwp.sql.gz", "size_bytes": 15
            }]
        });
        std::fs::write(
            stage.join(PAYLOAD_DIR).join("manifest.json"),
            serde_json::to_string(&manifest).unwrap(),
        ).expect("manifest");

        std::fs::create_dir_all(backup_dir(site.domain())).expect("backup dir");
        let archive = backup_dir(site.domain()).join("handmade-20260726-000000.tar.gz");
        let status = std::process::Command::new("tar")
            .args([
                "czf", archive.to_str().unwrap(),
                "-C", site.root().to_str().unwrap(), ".",
                "-C", stage.to_str().unwrap(), PAYLOAD_DIR,
            ])
            .status()
            .expect("tar");
        assert!(status.success());
        let _ = std::fs::remove_dir_all(&stage);

        // Restore with NO matching database configured on the site.
        let report = restore_backup(site.domain(), "handmade-20260726-000000.tar.gz", &[])
            .await
            .expect("restore runs");

        assert!(report.files_restored, "files must still be restored");
        assert_eq!(report.databases_in_archive, 1, "the manifest must be read");
        assert_eq!(report.databases_failed.len(), 1, "an unrestorable database is a failure");
        assert!(
            !report.ok(),
            "a restore that could not put the database back is NOT a success"
        );
        assert!(
            !site.root().join(PAYLOAD_DIR).exists(),
            "database dumps must never be extracted into the document root"
        );
    }

    /// The single-file restore extracts an arbitrary member into the document
    /// root. It must refuse the database payload explicitly.
    #[tokio::test]
    async fn single_file_restore_refuses_the_database_payload() {
        let site = TestSite::new("singlefile");
        let info = create_backup(site.domain(), &[]).await.expect("backup");

        for attempt in [
            ".dockpanel-backup/db/x.sql.gz",
            "./.dockpanel-backup/db/x.sql.gz",
            ".dockpanel-backup",
        ] {
            let err = restore_single_file(site.domain(), &info.filename, attempt)
                .await
                .expect_err(&format!("must refuse '{attempt}'"));
            assert!(
                err.contains("database dumps"),
                "refusal must say why, got: {err}"
            );
        }

        // An ordinary file is still restorable.
        restore_single_file(site.domain(), &info.filename, "index.html")
            .await
            .expect("a normal file still restores");
    }

    #[test]
    fn report_ok_requires_both_halves() {
        let mut r = RestoreReport { files_restored: true, ..Default::default() };
        assert!(r.ok());
        r.databases_failed.push(DbFailure { db_name: "x".into(), error: "boom".into() });
        assert!(!r.ok(), "files restored + database failed is a FAILURE");
        let r2 = RestoreReport { files_restored: false, ..Default::default() };
        assert!(!r2.ok());
    }
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
