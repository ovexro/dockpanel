//! Phase 4 W4: persistent panel snapshots.
//!
//! Builds tar.gz triplets containing:
//!   binaries/   — agent + api + cli binary copies
//!   db/         — gzipped pg_dump of the DockPanel database
//!   etc/        — copy of /etc/dockpanel
//!   metadata.json — provenance (from_version, trigger, operator)
//!
//! Stored in `/var/backups/dockpanel/snapshots/`. The orchestrator creates
//! one BEFORE invoking `update.sh`; the resulting file outlives the
//! `.bak` triplet that `update.sh:432-499` deletes on successful health
//! check, giving operators an after-the-fact rollback path.
//!
//! This module is the IO layer only. systemctl stop/start choreography
//! around restores lives in the orchestrator so this service stays usable
//! for one-shot "snapshot now" admin actions without state-machine
//! coupling.

use chrono::Utc;
use sqlx::PgPool;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;
use uuid::Uuid;

use crate::models::PanelSnapshot;

const SNAPSHOT_DIR: &str = "/var/backups/dockpanel/snapshots";
const STAGING_DIR_PARENT: &str = "/var/backups/dockpanel/.snapshot-staging";
/// Refuse to create a snapshot if the target partition has less than this
/// many bytes free. A typical snapshot is ~150-300 MB; 2 GiB keeps the
/// partition healthy even if a sweep is lagging.
const MIN_FREE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Retention: never delete the most-recent N snapshots, even if all are
/// older than `RETENTION_DAYS`. Protects against "broke weeks ago, only
/// just noticed" scenarios.
pub const RETENTION_MIN: i64 = 3;
pub const RETENTION_DAYS: i64 = 7;

#[derive(Debug, Clone)]
#[allow(dead_code)] // Fleet variant reserved for future per-server fleet snapshot tagging
pub enum SnapshotTrigger {
    Manual,
    PreUpdate { target_version: String },
    Fleet { server_id: Uuid },
}

impl SnapshotTrigger {
    pub fn as_str(&self) -> String {
        match self {
            SnapshotTrigger::Manual => "manual".to_string(),
            SnapshotTrigger::PreUpdate { target_version } => {
                format!("pre-update:{target_version}")
            }
            SnapshotTrigger::Fleet { server_id } => format!("fleet:{server_id}"),
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct SnapshotMeta {
    pub id: Uuid,
    pub file_path: PathBuf,
    pub size_bytes: u64,
    pub sha256: String,
    pub from_version: String,
}

#[derive(Debug)]
pub enum SnapshotError {
    DirInit(String),
    InsufficientDisk { available: u64, required: u64 },
    Subprocess { cmd: String, stderr: String },
    Io(std::io::Error),
    Db(sqlx::Error),
    NotFound(Uuid),
    FileMissing(PathBuf),
    Sha256Mismatch { expected: String, actual: String },
}

impl std::fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SnapshotError::DirInit(s) => write!(f, "snapshot dir cannot be initialized: {s}"),
            SnapshotError::InsufficientDisk { available, required } => write!(
                f,
                "insufficient disk space: {available} bytes available, {required} required"
            ),
            SnapshotError::Subprocess { cmd, stderr } => {
                write!(f, "subprocess `{cmd}` failed: {stderr}")
            }
            SnapshotError::Io(e) => write!(f, "io error: {e}"),
            SnapshotError::Db(e) => write!(f, "db error: {e}"),
            SnapshotError::NotFound(id) => write!(f, "snapshot {id} not found"),
            SnapshotError::FileMissing(p) => {
                write!(f, "snapshot file missing on disk: {}", p.display())
            }
            SnapshotError::Sha256Mismatch { expected, actual } => {
                write!(f, "sha256 mismatch: expected {expected}, got {actual}")
            }
        }
    }
}

impl std::error::Error for SnapshotError {}

impl From<std::io::Error> for SnapshotError {
    fn from(e: std::io::Error) -> Self {
        SnapshotError::Io(e)
    }
}

impl From<sqlx::Error> for SnapshotError {
    fn from(e: sqlx::Error) -> Self {
        SnapshotError::Db(e)
    }
}

/// Build a snapshot of the current panel state (binaries + DB dump + etc).
/// Writes to a `.tmp` file first, computes sha256, renames to final, then
/// inserts the DB row. The DB row + file are consistent: if any earlier
/// step fails, no row is written and the .tmp file is cleaned up.
pub async fn create_snapshot(
    pool: &PgPool,
    trigger: SnapshotTrigger,
    operator: Option<String>,
) -> Result<SnapshotMeta, SnapshotError> {
    ensure_dirs().await?;
    check_free_disk(SNAPSHOT_DIR, MIN_FREE_BYTES).await?;

    let snapshot_id = Uuid::new_v4();
    let timestamp = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let final_name = format!("panel-snapshot-{timestamp}.tar.gz");
    let final_path = PathBuf::from(SNAPSHOT_DIR).join(&final_name);
    let tmp_path = PathBuf::from(SNAPSHOT_DIR).join(format!("{final_name}.tmp"));
    let staging_dir = PathBuf::from(STAGING_DIR_PARENT).join(snapshot_id.to_string());

    // Best-effort cleanup of any stale .tmp from a prior crashed run before
    // we start writing. Same name pattern is improbable but cheap to guard.
    let _ = tokio::fs::remove_file(&tmp_path).await;
    let _ = tokio::fs::remove_dir_all(&staging_dir).await;

    let result = build_snapshot_inner(
        snapshot_id,
        &staging_dir,
        &tmp_path,
        &final_path,
        &trigger,
        operator.as_deref(),
    )
    .await;

    // Always sweep staging dir, success or fail.
    let _ = tokio::fs::remove_dir_all(&staging_dir).await;

    let (size_bytes, sha256, from_version) = match result {
        Ok(t) => t,
        Err(e) => {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(e);
        }
    };

    // Persist DB row only after the file is in its final location.
    let row_result = sqlx::query(
        "INSERT INTO panel_snapshots \
            (id, file_path, from_version, trigger, operator, size_bytes, sha256) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(snapshot_id)
    .bind(final_path.to_string_lossy().to_string())
    .bind(&from_version)
    .bind(trigger.as_str())
    .bind(&operator)
    .bind(size_bytes as i64)
    .bind(&sha256)
    .execute(pool)
    .await;

    if let Err(e) = row_result {
        // DB row didn't land — remove the file so we don't leak orphans.
        let _ = tokio::fs::remove_file(&final_path).await;
        return Err(SnapshotError::Db(e));
    }

    tracing::info!(
        "Created panel snapshot {snapshot_id} at {} ({} bytes, sha256: {})",
        final_path.display(),
        size_bytes,
        &sha256[..16.min(sha256.len())]
    );

    Ok(SnapshotMeta {
        id: snapshot_id,
        file_path: final_path,
        size_bytes,
        sha256,
        from_version,
    })
}

async fn build_snapshot_inner(
    snapshot_id: Uuid,
    staging_dir: &Path,
    tmp_path: &Path,
    final_path: &Path,
    trigger: &SnapshotTrigger,
    operator: Option<&str>,
) -> Result<(u64, String, String), SnapshotError> {
    // Layout staging dir: binaries/ db/ etc/ metadata.json
    tokio::fs::create_dir_all(staging_dir.join("binaries")).await?;
    tokio::fs::create_dir_all(staging_dir.join("db")).await?;
    tokio::fs::create_dir_all(staging_dir.join("etc")).await?;

    // Copy binaries. cp is fine; size + perms aren't security-critical
    // inside the tar (extraction restores from the tar entries).
    for bin in &["dockpanel-agent", "dockpanel-api", "dockpanel"] {
        let src = format!("/usr/local/bin/{bin}");
        let dst = staging_dir.join("binaries").join(bin);
        if Path::new(&src).exists() {
            tokio::fs::copy(&src, &dst).await?;
        } else {
            tracing::warn!("snapshot: binary {src} not found, skipping");
        }
    }

    // Dump DB via docker exec.
    //
    // `set -o pipefail` is load-bearing, not style. The exit status of
    // `pg_dump | gzip` is gzip's, and gzip cheerfully compresses a truncated
    // stream and exits 0 — so without it a pg_dump that died halfway (or never
    // connected) produced a short dump, this check passed, and the snapshot was
    // stored with a perfectly valid sha256 over perfectly incomplete contents.
    // A backup that cannot be restored is worse than no backup, because the
    // operator believes they are covered; the same one-character-class oversight
    // on the restore side is what let a rollback destroy a live database while
    // reporting success.
    let dump_path = staging_dir.join("db").join("dump.sql.gz");
    let dump_status = Command::new("bash")
        .arg("-c")
        .arg(format!(
            "set -o pipefail; docker exec dockpanel-postgres pg_dump -U dockpanel --clean --if-exists dockpanel | gzip > {}",
            shell_escape(&dump_path.to_string_lossy())
        ))
        .status()
        .await?;
    if !dump_status.success() {
        return Err(SnapshotError::Subprocess {
            cmd: "pg_dump".into(),
            stderr: format!("exit status {dump_status}"),
        });
    }

    // And prove what we just wrote is a whole dump before we let it become a
    // snapshot. pg_dump emits this marker as its last line; its absence means
    // the dump is short no matter what the exit statuses claimed.
    let dump_tail = Command::new("bash")
        .arg("-c")
        .arg(format!(
            // Wider than the marker's current position on purpose — see the same
            // check in restore-snapshot.sh. A trailing `\unrestrict` line added by
            // PostgreSQL's August-2025 minors left the marker 5 lines from the end
            // with zero margin; one more trailer and every snapshot would be
            // rejected as incomplete at creation time.
            "gunzip -c {} | tail -20",
            shell_escape(&dump_path.to_string_lossy())
        ))
        .output()
        .await?;
    if !String::from_utf8_lossy(&dump_tail.stdout).contains("PostgreSQL database dump complete") {
        return Err(SnapshotError::Subprocess {
            cmd: "pg_dump".into(),
            stderr: "database dump is incomplete (completion marker absent) — \
                     refusing to store a snapshot that could not be restored"
                .into(),
        });
    }

    // Copy /etc/dockpanel into staging. The tree is small (api.env, ssl/,
    // a handful of small text files) — recursive cp via tar piped is
    // simplest and preserves permissions cleanly.
    let etc_status = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "cp -a /etc/dockpanel/. {}",
            shell_escape(&staging_dir.join("etc").to_string_lossy())
        ))
        .status()
        .await?;
    if !etc_status.success() {
        return Err(SnapshotError::Subprocess {
            cmd: "cp etc".into(),
            stderr: format!("exit status {etc_status}"),
        });
    }

    let from_version = env!("CARGO_PKG_VERSION").to_string();
    let metadata = serde_json::json!({
        "snapshot_id": snapshot_id.to_string(),
        "from_version": from_version,
        "created_at": Utc::now().to_rfc3339(),
        "trigger": trigger.as_str(),
        "operator": operator,
    });
    tokio::fs::write(
        staging_dir.join("metadata.json"),
        serde_json::to_vec_pretty(&metadata).unwrap_or_default(),
    )
    .await?;

    // Build tarball: tar -C <staging> -czf <tmp> .
    // -C cd's into staging so tar entries are relative ("binaries/" not
    // "<staging>/binaries/").
    let tar_status = run_cmd_with_timeout(
        "tar",
        &[
            "-C",
            &staging_dir.to_string_lossy(),
            "-czf",
            &tmp_path.to_string_lossy(),
            ".",
        ],
        Duration::from_secs(300),
    )
    .await?;
    if !tar_status.success() {
        return Err(SnapshotError::Subprocess {
            cmd: "tar -czf".into(),
            stderr: format!("exit status {tar_status}"),
        });
    }

    let size_bytes = tokio::fs::metadata(&tmp_path).await?.len();
    let sha256 = sha256_of(tmp_path).await?;

    // Tighten BEFORE the rename, so the tarball is never observable at the
    // final path with a permissive mode. It carries api.env (JWT_SECRET, DB
    // password), agent.token and the agent TLS key — tar created it 0644.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o600))?;
    }

    // Atomic rename — the tar.gz only becomes "real" once this succeeds.
    tokio::fs::rename(&tmp_path, &final_path).await?;

    Ok((size_bytes, sha256, from_version))
}

/// The restore procedure, embedded at compile time.
///
/// Kept as a real file under `scripts/` so it is reviewable and testable, but
/// compiled in rather than read from disk: `/opt/dockpanel` is not guaranteed to
/// exist (hand-built layouts have no repo at all), and a repo-side script that
/// the running binary merely *hopes* is in sync is the drift trap of lesson #35.
const RESTORE_SCRIPT: &str = include_str!("../../../../scripts/restore-snapshot.sh");

/// Where the restore writes its verdict. Read back by `last_restore_result`
/// after the api has been restarted by the restore itself.
pub const RESTORE_RESULT_PATH: &str = "/var/lib/dockpanel/last-restore.json";
const RESTORE_SCRIPT_PATH: &str = "/var/lib/dockpanel/restore-snapshot.sh";

/// Validate a snapshot and hand the restore to a detached, PID1-owned process.
///
/// Returns as soon as the restore has been *started* — it cannot be awaited,
/// because the very first thing it does is stop the api process this code runs
/// in. Everything that can be checked cheaply (row exists, file present, sha256
/// matches) is checked HERE, synchronously, so the operator gets a real 4xx
/// instead of a 202 followed by silence.
///
/// History, because the shape of this function is the fix: the restore used to
/// run inline in the HTTP handler. It outlives the panel's own 300s request
/// timeout (measured 394.9s on a lab box), so axum dropped the request future
/// mid-restore, which broke the `gunzip | psql` pipe; psql read that as a clean
/// end of input and exited 0, and the code recorded success while the database
/// had been reduced to 1 of its 92 tables. See `scripts/restore-snapshot.sh`.
pub async fn spawn_restore(pool: &PgPool, snapshot_id: Uuid) -> Result<(), SnapshotError> {
    let row: Option<PanelSnapshot> =
        sqlx::query_as("SELECT * FROM panel_snapshots WHERE id = $1")
            .bind(snapshot_id)
            .fetch_optional(pool)
            .await?;

    let snapshot = row.ok_or(SnapshotError::NotFound(snapshot_id))?;
    let file_path = PathBuf::from(&snapshot.file_path);
    if !file_path.exists() {
        return Err(SnapshotError::FileMissing(file_path));
    }

    // Verify before we hand off. The script re-verifies (it must — it is the one
    // that acts), but failing here turns a corrupt snapshot into a synchronous
    // error the operator sees immediately instead of a result file they have to
    // go looking for.
    let actual_sha = sha256_of(&file_path).await?;
    if actual_sha != snapshot.sha256 {
        return Err(SnapshotError::Sha256Mismatch {
            expected: snapshot.sha256,
            actual: actual_sha,
        });
    }

    // NB: `rolled_back_at` is deliberately NOT stamped here. The restore replaces
    // panel_snapshots with the snapshot's own copy of itself, so a stamp written
    // before it runs is overwritten and lost (observed on a lab box: the column
    // came back empty), and this process no longer exists afterwards to write
    // one. The restore records it itself, into the database it just restored.

    tokio::fs::create_dir_all("/var/lib/dockpanel").await?;
    tokio::fs::write(RESTORE_SCRIPT_PATH, RESTORE_SCRIPT).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(RESTORE_SCRIPT_PATH, std::fs::Permissions::from_mode(0o700))?;
    }

    // A PID1-owned transient SERVICE, not a scope and not a bare child: this
    // process is about to stop dockpanel-api.service, and dockpanel-api.service
    // is KillMode=control-group, so anything still inside that cgroup dies with
    // it. `--scope` is not a substitute — it is created in the caller's context
    // and dies with the invoking session (lesson #47).
    let status = Command::new("systemd-run")
        .args([
            "--quiet",
            "--collect",
            &format!("--unit=dockpanel-snapshot-restore-{snapshot_id}"),
            "--setenv=DOCKPANEL_RESTORE_DETACHED=1",
            &format!("--setenv=DOCKPANEL_SNAPSHOT_ID={snapshot_id}"),
            &format!(
                "--setenv=DOCKPANEL_SNAPSHOT_TARBALL={}",
                file_path.to_string_lossy()
            ),
            &format!("--setenv=DOCKPANEL_SNAPSHOT_SHA256={}", snapshot.sha256),
            "bash",
            RESTORE_SCRIPT_PATH,
        ])
        .status()
        .await?;

    if !status.success() {
        return Err(SnapshotError::Subprocess {
            cmd: "systemd-run (snapshot restore)".into(),
            stderr: format!(
                "could not start the detached restore unit: exit status {status}"
            ),
        });
    }

    tracing::info!(
        "Snapshot restore {snapshot_id} handed to transient unit \
         dockpanel-snapshot-restore-{snapshot_id}; this process will be stopped by it"
    );
    Ok(())
}

/// The verdict written by the last detached restore, if any.
///
/// The restore stops and restarts the api, so its outcome cannot be returned
/// through the request that started it — this file is how the operator finds out
/// what happened, and it is written on EVERY exit path including aborts.
pub async fn last_restore_result() -> Option<serde_json::Value> {
    let raw = tokio::fs::read_to_string(RESTORE_RESULT_PATH).await.ok()?;
    serde_json::from_str(&raw).ok()
}

/// List snapshots newest-first. Operator-facing read; raw model rows.
pub async fn list_snapshots(pool: &PgPool) -> Result<Vec<PanelSnapshot>, SnapshotError> {
    let rows = sqlx::query_as::<_, PanelSnapshot>(
        "SELECT * FROM panel_snapshots ORDER BY created_at DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Delete one snapshot — removes file then DB row. File-first so the row
/// always reflects on-disk reality (no row-without-file states linger).
pub async fn delete_snapshot(pool: &PgPool, snapshot_id: Uuid) -> Result<(), SnapshotError> {
    let row: Option<PanelSnapshot> =
        sqlx::query_as("SELECT * FROM panel_snapshots WHERE id = $1")
            .bind(snapshot_id)
            .fetch_optional(pool)
            .await?;
    let snapshot = row.ok_or(SnapshotError::NotFound(snapshot_id))?;

    let _ = tokio::fs::remove_file(&snapshot.file_path).await;
    sqlx::query("DELETE FROM panel_snapshots WHERE id = $1")
        .bind(snapshot_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Retention sweep: keep most-recent `RETENTION_MIN` regardless of age;
/// delete older than `RETENTION_DAYS` beyond that floor.
/// Returns count of snapshots removed.
pub async fn retention_sweep(pool: &PgPool) -> Result<u32, SnapshotError> {
    let rows: Vec<PanelSnapshot> = sqlx::query_as(
        "SELECT * FROM panel_snapshots ORDER BY created_at DESC",
    )
    .fetch_all(pool)
    .await?;

    let mut removed = 0u32;
    let cutoff = Utc::now() - chrono::Duration::days(RETENTION_DAYS);

    for snap in rows.iter().skip(RETENTION_MIN as usize) {
        if snap.created_at < cutoff {
            // file-first; if file delete fails, leave the row for retry.
            if let Err(e) = tokio::fs::remove_file(&snap.file_path).await {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(
                        "retention sweep: failed to delete {}: {e} — keeping row",
                        &snap.file_path
                    );
                    continue;
                }
            }
            if sqlx::query("DELETE FROM panel_snapshots WHERE id = $1")
                .bind(snap.id)
                .execute(pool)
                .await
                .is_ok()
            {
                removed += 1;
            }
        }
    }

    if removed > 0 {
        tracing::info!("Snapshot retention sweep removed {removed} aged snapshot(s)");
    }
    Ok(removed)
}

// ── Helpers ──────────────────────────────────────────────────────────────

async fn ensure_dirs() -> Result<(), SnapshotError> {
    tokio::fs::create_dir_all(SNAPSHOT_DIR)
        .await
        .map_err(|e| SnapshotError::DirInit(format!("{SNAPSHOT_DIR}: {e}")))?;
    tokio::fs::create_dir_all(STAGING_DIR_PARENT)
        .await
        .map_err(|e| SnapshotError::DirInit(format!("{STAGING_DIR_PARENT}: {e}")))?;

    // A panel snapshot bundles /etc/dockpanel: api.env (JWT_SECRET + the
    // Postgres password), agent.token, and the agent's TLS private key. The
    // directories defaulted to 0755 and the tarballs to 0644, so on a box with
    // any untrusted local user — the normal case for a hosting panel — the
    // panel's signing secret was world-readable, and anyone who read it could
    // mint an admin JWT. Lock the directories down here and each tarball at
    // creation; harden retroactively so boxes that already took snapshots under
    // the old mode are fixed on the next snapshot rather than staying exposed.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for dir in [SNAPSHOT_DIR, STAGING_DIR_PARENT] {
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
        if let Ok(mut rd) = tokio::fs::read_dir(SNAPSHOT_DIR).await {
            while let Ok(Some(entry)) = rd.next_entry().await {
                let p = entry.path();
                if p.extension().is_some_and(|e| e == "gz") {
                    let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
                }
            }
        }
    }
    Ok(())
}

/// Refuse if the partition holding `path` has fewer than `required` bytes
/// free. Shells out to `df -B1 --output=avail`; if df is unavailable, the
/// check fails open (warns but allows) so the panel doesn't refuse
/// snapshots on a stripped-down install.
async fn check_free_disk(path: &str, required: u64) -> Result<(), SnapshotError> {
    let output = match Command::new("df")
        .args(["-B1", "--output=avail", path])
        .output()
        .await
    {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!("df failed: {e} — skipping free-disk check");
            return Ok(());
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let available: Option<u64> = stdout.lines().nth(1).and_then(|l| l.trim().parse().ok());

    match available {
        Some(bytes) if bytes < required => Err(SnapshotError::InsufficientDisk {
            available: bytes,
            required,
        }),
        Some(_) => Ok(()),
        None => {
            tracing::warn!("df output unparseable — skipping free-disk check");
            Ok(())
        }
    }
}

async fn sha256_of(path: &Path) -> Result<String, SnapshotError> {
    let output = Command::new("sha256sum")
        .arg(path)
        .output()
        .await
        .map_err(SnapshotError::Io)?;
    if !output.status.success() {
        return Err(SnapshotError::Subprocess {
            cmd: "sha256sum".into(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let digest = stdout
        .split_whitespace()
        .next()
        .ok_or_else(|| SnapshotError::Subprocess {
            cmd: "sha256sum".into(),
            stderr: "no digest in stdout".into(),
        })?;
    Ok(digest.to_string())
}

async fn run_cmd_with_timeout(
    binary: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<std::process::ExitStatus, SnapshotError> {
    let fut = Command::new(binary).args(args).status();
    match tokio::time::timeout(timeout, fut).await {
        Ok(Ok(status)) => Ok(status),
        Ok(Err(e)) => Err(SnapshotError::Io(e)),
        Err(_) => Err(SnapshotError::Subprocess {
            cmd: binary.into(),
            stderr: format!("timed out after {}s", timeout.as_secs()),
        }),
    }
}

/// Minimal shell-escape for filesystem paths. Paths are constructed under
/// our own roots (no user input), but quoting prevents accidental
/// whitespace problems.
fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_string_forms() {
        assert_eq!(SnapshotTrigger::Manual.as_str(), "manual");
        assert_eq!(
            SnapshotTrigger::PreUpdate {
                target_version: "v2.10.0".into()
            }
            .as_str(),
            "pre-update:v2.10.0"
        );
        let id = Uuid::new_v4();
        assert_eq!(
            SnapshotTrigger::Fleet { server_id: id }.as_str(),
            format!("fleet:{id}")
        );
    }

    #[test]
    fn shell_escape_quotes_single_quotes() {
        assert_eq!(shell_escape("plain"), "'plain'");
        assert_eq!(shell_escape("with space"), "'with space'");
        assert_eq!(shell_escape("o'brien"), "'o'\\''brien'");
    }

    #[tokio::test]
    async fn sha256_of_known_content_is_stable() {
        let tmp = std::env::temp_dir().join(format!("dp-snap-sha-{}", Uuid::new_v4()));
        tokio::fs::write(&tmp, b"hello world").await.unwrap();
        let hash = sha256_of(&tmp).await.unwrap();
        // sha256("hello world") = b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
        let _ = tokio::fs::remove_file(&tmp).await;
    }

    #[tokio::test]
    async fn check_free_disk_passes_with_low_requirement() {
        // 1 byte requirement against any path that exists — should pass.
        let res = check_free_disk("/tmp", 1).await;
        assert!(res.is_ok(), "expected free-disk check to pass: {res:?}");
    }

    /// The database stage must be atomic and must stop on the first error.
    ///
    /// Without both of these, a restore stream that is cut short applies the
    /// dump's leading DROP block, skips the rest, and exits 0 — measured on a
    /// lab box as 92 tables reduced to 1 while the caller recorded success.
    /// With both, the same truncated stream exits non-zero and leaves all 92
    /// tables intact. This test exists so that neither flag can be dropped
    /// without a failing build.
    #[test]
    fn restore_script_applies_the_database_atomically() {
        assert!(
            RESTORE_SCRIPT.contains("ON_ERROR_STOP=1"),
            "restore script must stop on the first failed statement"
        );
        assert!(
            RESTORE_SCRIPT.contains("--single-transaction"),
            "restore script must apply the dump in one transaction so a failure changes nothing"
        );
    }

    /// A rollback must REPLACE the database, not merge into it.
    ///
    /// `pg_dump --clean` drops only what the dump contains, so without an
    /// explicit teardown a table created by a newer version's migration outlived
    /// a rollback while `_sqlx_migrations` was rewound past it. The next forward
    /// update then re-ran that migration against objects that already existed,
    /// the api panicked at startup and crash-looped into a permanent 502.
    /// Reproduced end to end on a lab box before this teardown was added.
    #[test]
    fn restore_script_rebuilds_the_schema_instead_of_merging_into_it() {
        assert!(
            RESTORE_SCRIPT.contains("DROP SCHEMA IF EXISTS public CASCADE;"),
            "a restore that cannot remove post-snapshot objects is a merge, not a rollback"
        );
        assert!(
            RESTORE_SCRIPT.contains("CREATE SCHEMA public;"),
            "the schema must be recreated after being dropped"
        );
    }

    /// The teardown carries the schema's owner and ACL back with it. The dump
    /// contains no GRANT/REVOKE and no CREATE SCHEMA, so a bare recreate would
    /// hand `public` to the restoring role and silently drop PUBLIC's USAGE
    /// grant — changing the database's permissions on every rollback.
    #[test]
    fn restore_script_preserves_schema_ownership_and_grants() {
        assert!(
            RESTORE_SCRIPT.contains("ALTER SCHEMA public OWNER TO pg_database_owner;"),
            "recreating public without restoring its owner changes it on every rollback"
        );
        assert!(
            RESTORE_SCRIPT.contains("GRANT USAGE ON SCHEMA public TO PUBLIC;"),
            "recreating public without restoring PUBLIC's USAGE grant revokes it silently"
        );
    }

    /// psql must never be fed by a pipe.
    ///
    /// A live producer in front of psql is the original defect in a new costume:
    /// if it dies mid-stream psql sees a clean EOF, and `--single-transaction`
    /// then COMMITS whatever arrived — with the teardown that means a dropped
    /// schema plus half a dump, while the pipeline's non-zero status makes the
    /// script report "nothing changed". This was written as a pipe first and
    /// caught in review; the pin is here so it cannot come back.
    #[test]
    fn restore_script_never_feeds_psql_from_a_pipe() {
        for (i, line) in RESTORE_SCRIPT.lines().enumerate() {
            let l = line.trim_start();
            if l.starts_with('#') {
                continue;
            }
            let Some(psql_at) = l.find("psql") else { continue };
            // Only a pipe that FEEDS psql matters. Piping psql's own output into
            // `tr` is how several checks below read a scalar, and is fine.
            let piped_into = l
                .match_indices('|')
                .filter(|(at, _)| !l[*at..].starts_with("||"))
                .any(|(at, _)| at < psql_at);
            assert!(
                !piped_into,
                "line {} pipes into psql; feed it from a file by redirection: {l}",
                i + 1
            );
        }
        assert!(
            RESTORE_SCRIPT.contains("< \"$APPLY_SQL\""),
            "the assembled restore stream must reach psql by redirection"
        );
    }

    /// Destroying the schema is only safe because the dump has already been
    /// proven whole. If these ever swap order, a truncated dump would take the
    /// database with it.
    #[test]
    fn restore_script_proves_the_dump_before_it_drops_the_schema() {
        let verified = RESTORE_SCRIPT
            .find("PostgreSQL database dump complete")
            .expect("dump completion marker must be checked");
        let dropped = RESTORE_SCRIPT
            .find("DROP SCHEMA IF EXISTS public CASCADE;")
            .expect("schema teardown must exist");
        assert!(
            verified < dropped,
            "the dump must be proven complete before the schema is torn down"
        );
    }

    /// One transaction, not two. The teardown and the dump must commit or roll
    /// back together — a separate psql for the DROP would leave a dropped schema
    /// behind if the dump then failed, turning a safe failure into total loss.
    ///
    /// Counts only executable lines: this file explains itself at length, and a
    /// pin that counts prose fails the moment someone documents the thing it
    /// pins.
    #[test]
    fn restore_script_tears_down_and_reloads_in_a_single_transaction() {
        let executable_uses = RESTORE_SCRIPT
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .filter(|l| l.contains("--single-transaction"))
            .count();
        assert_eq!(
            executable_uses, 1,
            "the schema teardown must share the dump's transaction, not run in its own"
        );
    }

    /// The restore must outlive the service it stops. A transient *service* is
    /// PID1-owned; a scope is created in the caller's context and dies with the
    /// invoking session, and a plain child dies with the api's control group.
    #[test]
    fn restore_script_detaches_via_a_transient_unit() {
        assert!(RESTORE_SCRIPT.contains("systemd-run"));
        assert!(
            !RESTORE_SCRIPT.contains("--scope"),
            "a scope dies with the caller's session — this must be a transient service"
        );
    }

    /// A restore that cannot restore has to say so. `|| true` on a restore step
    /// is what let the previous rollback path print success while having changed
    /// nothing, so the script must not silence any of its own failures.
    #[test]
    fn restore_script_never_swallows_a_restore_failure() {
        for (i, line) in RESTORE_SCRIPT.lines().enumerate() {
            let l = line.trim_start();
            if l.starts_with('#') {
                continue;
            }
            if l.contains("|| true") {
                // Only tolerated where failing is genuinely the safe outcome:
                // best-effort cleanup and the start-services safety net.
                let tolerated = l.contains("chmod")
                    || l.contains("rm -rf")
                    || l.contains("systemctl start")
                    || l.contains("systemd-cat")
                    || l.contains("daemon-reload")
                    || l.contains("systemctl stop")
                    // The pre-restore copy of the config directory. Matched on
                    // `.prerestore.` rather than on a literal path, because the
                    // path became a variable and this pin then failed on a
                    // rename that changed nothing about why the line is safe.
                    || l.contains(".prerestore.")
                    || l.contains("grep -c");
                assert!(
                    tolerated,
                    "line {} silences a failure in the restore path: {l}",
                    i + 1
                );
            }
        }
    }

    /// Every exit path must leave a verdict behind: the restore restarts the
    /// api, so a result file is the only channel the operator has.
    #[test]
    fn restore_script_always_writes_a_result() {
        assert!(RESTORE_SCRIPT.contains("trap on_exit EXIT INT TERM"));
        assert!(RESTORE_SCRIPT.contains("write_result"));
        assert!(RESTORE_SCRIPT.contains(RESTORE_RESULT_PATH));
    }

    /// A rollback must not be able to undo itself (s231).
    ///
    /// `database-verify` and `record-rollback` both run AFTER the restore
    /// transaction has committed and BEFORE the binaries are swapped back. The
    /// exit trap restarted `dockpanel-api` unconditionally, so a failure in
    /// either one restarted the still-installed NEWER api against a database
    /// that had just been reverted — and that api migrated it forward again,
    /// quietly undoing the rollback while the result file said FAILED.
    ///
    /// Two independent guarantees, both required:
    ///   1. the trap consults the window before restarting the api, and
    ///   2. nothing between the commit and the swap raises in the first place.
    #[test]
    fn restore_script_never_restarts_the_api_over_a_reverted_database() {
        assert!(
            RESTORE_SCRIPT.contains("rollback_would_be_undone"),
            "the window between the database commit and the binary swap must be named"
        );
        let trap_at = RESTORE_SCRIPT
            .find("on_exit() {")
            .expect("the exit trap must exist");
        let trap = &RESTORE_SCRIPT[trap_at..];
        let trap_end = trap.find("\ntrap ").unwrap_or(trap.len());
        let trap = &trap[..trap_end];
        assert!(
            trap.contains("rollback_would_be_undone"),
            "the exit trap must consult the window before restarting the api"
        );
        // The agent may still come back — it holds no database. The api may not.
        let restarts_api = trap
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .filter(|l| l.contains("systemctl start dockpanel-api"))
            .count();
        assert_eq!(
            restarts_api, 1,
            "the api must be restarted from exactly one branch of the trap — the \
             one that has established the database and the binaries still agree"
        );
    }

    /// After the commit, aborting cannot undo anything and can only strand the
    /// box between two versions. So the post-commit checks REPORT.
    #[test]
    fn restore_script_reports_rather_than_aborts_after_the_database_commits() {
        let committed = RESTORE_SCRIPT
            .find("db_committed=1")
            .expect("the commit point must be marked");
        let swap = RESTORE_SCRIPT
            .find("stage=\"binaries\"")
            .expect("the binary swap must be a named stage");
        assert!(committed < swap, "the commit must precede the swap");

        // Everything the script does in that window, minus comments.
        let window = &RESTORE_SCRIPT[committed..swap];
        for (i, line) in window.lines().enumerate() {
            let l = line.trim_start();
            if l.starts_with('#') {
                continue;
            }
            assert!(
                !l.contains("fail \""),
                "line {} of the post-commit window raises instead of degrading, \
                 which strands the box between two versions: {l}",
                i + 1
            );
        }
        assert!(
            window.contains("degrade \""),
            "the post-commit window must still REPORT what went wrong"
        );
    }

    /// Refuse a dump that is already short before anything is destroyed.
    #[test]
    fn restore_script_verifies_dump_completeness_before_destroying_anything() {
        let verify = RESTORE_SCRIPT
            .find("PostgreSQL database dump complete")
            .expect("dump completion marker must be checked");
        let stop = RESTORE_SCRIPT
            .find("stage=\"stop-services\"")
            .expect("services must be stopped in a named stage");
        assert!(
            verify < stop,
            "the dump must be proven complete before the panel is taken down"
        );
    }

    #[tokio::test]
    async fn check_free_disk_refuses_when_requirement_exceeds_partition() {
        // 100 PiB on /tmp — should refuse (or fall through on weird envs).
        let res = check_free_disk("/tmp", u64::MAX / 2).await;
        match res {
            Err(SnapshotError::InsufficientDisk { .. }) => {}
            Ok(()) => {
                // Acceptable fallthrough on environments where df output
                // is unparseable; the function fails open by design.
            }
            Err(e) => panic!("unexpected error: {e}"),
        }
    }
}
