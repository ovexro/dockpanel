use axum::{
    extract::Path,
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};

use super::{is_valid_domain, AppState};
use crate::services::backups;

type ApiErr = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: &str) -> ApiErr {
    (status, Json(serde_json::json!({ "error": msg })))
}

/// Optional body for the create/restore routes, naming the databases that
/// belong to the site. The agent cannot resolve these itself — it has no access
/// to the panel's `databases` table — so the backend passes them down with
/// decrypted credentials. Absent body ⇒ files only, exactly as before.
#[derive(serde::Deserialize, Default)]
struct SiteDbBody {
    #[serde(default)]
    databases: Vec<backups::DbSpec>,
}

/// POST /backups/{domain}/create — Create a backup.
async fn create(
    Path(domain): Path<String>,
    body: Option<Json<SiteDbBody>>,
) -> Result<Json<backups::BackupInfo>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }

    let databases = body.map(|Json(b)| b.databases).unwrap_or_default();

    let info = backups::create_backup(&domain, &databases)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(info))
}

/// GET /backups/{domain}/list — List backups.
async fn list(
    Path(domain): Path<String>,
) -> Result<Json<Vec<backups::BackupInfo>>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }

    let list = backups::list_backups(&domain)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(list))
}

/// POST /backups/{domain}/restore/{filename} — Restore from backup.
///
/// A restore that put the files back but could not load a database is reported
/// as a FAILURE (500) carrying the full report, not as a success with a
/// footnote — the site's files have already been replaced and the operator has
/// to know that.
async fn restore(
    Path((domain, filename)): Path<(String, String)>,
    body: Option<Json<SiteDbBody>>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }

    let databases = body.map(|Json(b)| b.databases).unwrap_or_default();

    let report = backups::restore_backup(&domain, &filename, &databases)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    // `restore_backup`'s tar extraction runs `--no-same-owner`, which leaves the
    // whole site root:root — unwritable by the www-data-running app until this
    // runs. Mirrors `restic_restore`'s own call to the same helper, below.
    chown_restored_tree(&format!("/var/www/{domain}")).await;

    let mut payload = serde_json::to_value(&report)
        .unwrap_or_else(|_| serde_json::json!({}));
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("success".into(), serde_json::Value::Bool(report.ok()));
        if !report.ok() {
            // Carry a plain-language summary under the key every client already
            // reads for errors, so a caller that only knows how to print
            // `error` still gets the reason instead of a bare HTTP 500.
            let detail = report.databases_failed.iter()
                .map(|f| format!("{}: {}", f.db_name, f.error))
                .collect::<Vec<_>>()
                .join("; ");
            let summary = if report.files_restored {
                format!(
                    "The site's files were restored, but {} database(s) were NOT: {detail}",
                    report.databases_failed.len()
                )
            } else {
                format!("Restore failed: {detail}")
            };
            obj.insert("error".into(), serde_json::Value::String(summary));
        }
    }

    if !report.ok() {
        return Err((StatusCode::INTERNAL_SERVER_ERROR, Json(payload)));
    }
    Ok(Json(payload))
}

/// GET /backups/{domain}/browse/{filename} — List files in a backup archive.
async fn browse(
    Path((domain, filename)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }

    let files = backups::list_backup_files(&domain, &filename)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(serde_json::json!({ "files": files, "count": files.len() })))
}

#[derive(serde::Deserialize)]
struct RestoreFileRequest {
    path: String,
}

/// POST /backups/{domain}/restore-file/{filename} — Restore a single file from backup.
async fn restore_file(
    Path((domain, filename)): Path<(String, String)>,
    Json(body): Json<RestoreFileRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }

    backups::restore_single_file(&domain, &filename, &body.path)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    // Same gap as `restore` above — the extracted member comes back root-owned.
    chown_restored_tree(&format!("/var/www/{domain}")).await;

    Ok(Json(serde_json::json!({ "success": true, "restored_path": body.path })))
}

/// DELETE /backups/{domain}/{filename} — Delete a backup.
async fn remove(
    Path((domain, filename)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }

    backups::delete_backup(&domain, &filename)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(serde_json::json!({ "success": true })))
}

// ── Restic incremental backups ──────────────────────────────────────

use crate::safe_cmd::safe_command;

/// POST /backups/{domain}/restic/backup — Run incremental backup with Restic.
/// Refuse before running restic when the binary is not on this box.
///
/// Shared by backup AND restore. It was inline in `restic_backup` and restore
/// simply did not have it, so a host whose repo survived a rebuild but whose
/// package did not answered the spawn failure as a 500 — which the panel
/// replaces with an incident id. The operator was told their agent was broken
/// in the middle of a restore, on a working agent, with the remedy being one
/// `apt install` they were never shown.
async fn ensure_restic() -> Result<(), ApiErr> {
    let has_restic = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        safe_command("which").arg("restic").kill_on_drop(true).output()
    ).await.ok().and_then(|r| r.ok()).map(|o| o.status.success()).unwrap_or(false);

    if !has_restic {
        return Err(err(StatusCode::PRECONDITION_FAILED,
            "Restic not installed. Install it with your system package manager \
             (apt install restic / dnf install restic)."));
    }
    Ok(())
}

async fn restic_backup(
    Path(domain): Path<String>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain"));
    }

    let repo = format!("/var/backups/dockpanel/restic/{}", domain.replace('.', "_"));
    let site_dir = format!("/var/www/{domain}");
    let password_file = "/etc/dockpanel/restic-password";

    if !std::path::Path::new(site_dir.as_str()).exists() {
        return Err(err(StatusCode::NOT_FOUND, "Site directory not found"));
    }

    ensure_restic().await?;

    // Ensure password file exists
    if !std::path::Path::new(password_file).exists() {
        // Generate random password and save it
        let password: String = (0..32).map(|_| format!("{:02x}", rand::random::<u8>())).collect();
        std::fs::write(password_file, &password)
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Write password: {e}")))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(password_file, std::fs::Permissions::from_mode(0o600));
        }
    }

    // Init repo if needed
    if !std::path::Path::new(&format!("{repo}/config")).exists() {
        std::fs::create_dir_all(&repo).ok();
        let init = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            safe_command("restic")
                .args(["-r", &repo, "--password-file", password_file, "init"])
                .kill_on_drop(true)
                .output()
        ).await;

        if init.ok().and_then(|r| r.ok()).map(|o| o.status.success()).unwrap_or(false) {
            tracing::info!("Restic repo initialized for {domain}");
        } else {
            return Err(err(StatusCode::INTERNAL_SERVER_ERROR, "Failed to init restic repo"));
        }
    }

    // Run incremental backup
    let backup = tokio::time::timeout(
        std::time::Duration::from_secs(600),
        safe_command("restic")
            .args(["-r", &repo, "--password-file", password_file,
                   "backup", &site_dir, "--tag", &domain, "--json"])
            .kill_on_drop(true)
            .output()
    ).await
        .map_err(|_| err(StatusCode::GATEWAY_TIMEOUT, "Backup timed out (10min)"))?
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Restic: {e}")))?;

    if !backup.status.success() {
        let stderr = String::from_utf8_lossy(&backup.stderr);
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Restic backup failed: {}", stderr.chars().take(300).collect::<String>())));
    }

    // Parse restic JSON output for summary
    let stdout = String::from_utf8_lossy(&backup.stdout);
    let summary: serde_json::Value = stdout.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .filter(|v: &serde_json::Value| v.get("message_type").and_then(|m| m.as_str()) == Some("summary"))
        .next()
        .unwrap_or(serde_json::json!({}));

    tracing::info!("Restic backup completed for {domain}");
    Ok(Json(serde_json::json!({
        "ok": true,
        "type": "restic",
        "files_new": summary.get("files_new"),
        "files_changed": summary.get("files_changed"),
        "data_added": summary.get("data_added"),
        "total_bytes_processed": summary.get("total_bytes_processed"),
        "snapshot_id": summary.get("snapshot_id"),
    })))
}

/// GET /backups/{domain}/restic/snapshots — List Restic snapshots.
async fn restic_snapshots(
    Path(domain): Path<String>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain"));
    }

    let repo = format!("/var/backups/dockpanel/restic/{}", domain.replace('.', "_"));
    let password_file = "/etc/dockpanel/restic-password";

    if !std::path::Path::new(&format!("{repo}/config")).exists() {
        return Ok(Json(serde_json::json!({ "snapshots": [], "total": 0 })));
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        safe_command("restic")
            .args(["-r", &repo, "--password-file", password_file, "snapshots", "--json"])
            .kill_on_drop(true)
            .output()
    ).await
        .map_err(|_| err(StatusCode::GATEWAY_TIMEOUT, "Timeout"))?
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Restic: {e}")))?;

    // Unlike `restic_backup`/`restic_restore`, this path used to skip the exit
    // check entirely: a wrong/rotated password or a locked/corrupted repo
    // produces empty stdout, which `serde_json::from_slice` fails to parse —
    // and `unwrap_or_default()` silently turned that failure into "0
    // snapshots", indistinguishable from a site that genuinely has none yet.
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to list restic snapshots: {}", stderr.chars().take(300).collect::<String>())));
    }

    let snapshots: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap_or_default();
    let total = snapshots.len();

    Ok(Json(serde_json::json!({
        "snapshots": snapshots,
        "total": total,
    })))
}

/// Hand a restored site tree back to the web server without exposing `.git`.
///
/// A restic restore run as root (this agent) reproduces each member's
/// original uid/gid, so `.git` — if this site uses Git Deploy — comes back
/// however `deploy.rs::hand_tree_to_web_user` last left it: root-owned,
/// unreadable to www-data. A blanket `chown -R www-data:www-data` over the
/// whole tree would undo exactly that: it hands the application `.git/config`
/// and `.git/hooks/`, which the next `git_build.rs` deploy then reads and
/// executes as root — the same defect `deploy.rs` was fixed to avoid (see its
/// own `hand_tree_to_web_user` doc comment). Mirror that split here: chown
/// everything except `.git` to www-data, then explicitly re-secure `.git` to
/// root, rather than trust whatever ownership the restore happened to leave it at.
async fn chown_restored_tree(target: &str) {
    if let Ok(entries) = std::fs::read_dir(target) {
        for entry in entries.flatten() {
            if entry.file_name() == std::ffi::OsStr::new(".git") {
                continue;
            }
            let _ = safe_command("chown")
                .args(["-R", "www-data:www-data", &entry.path().to_string_lossy()])
                .output()
                .await;
        }
    }

    let git_dir = format!("{target}/.git");
    if std::path::Path::new(&git_dir).exists() {
        let _ = safe_command("chown").args(["-R", "root:root", &git_dir]).output().await;
        let _ = safe_command("chmod").args(["-R", "go-rwx", &git_dir]).output().await;
    }
}

/// POST /backups/{domain}/restic/restore/{snapshot_id} — Restore from Restic snapshot.
async fn restic_restore(
    Path((domain, snapshot_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain"));
    }
    // Validate snapshot ID format (hex string)
    if snapshot_id.len() < 6 || !snapshot_id.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid snapshot ID"));
    }

    ensure_restic().await?;

    let repo = format!("/var/backups/dockpanel/restic/{}", domain.replace('.', "_"));
    let password_file = "/etc/dockpanel/restic-password";
    let site_dir = format!("/var/www/{domain}");

    // The repository has to exist before a restore can name a snapshot in it.
    // `restic_snapshots` already checks this and answers an empty list; restore
    // checked nothing and let restic fail, which arrived as an incident id.
    if !std::path::Path::new(&repo).join("config").exists() {
        return Err(err(StatusCode::NOT_FOUND,
            "No restic repository for this site on this server — nothing to restore from."));
    }

    // `--target /` reproduces each snapshot member at its original absolute
    // path; every snapshot this agent itself produces (`restic_backup` above
    // always sources from exactly `site_dir`) is confined to that tree
    // already. `--include` makes that confinement structural rather than an
    // unchecked invariant — the same guarantee the tar-based restore lane
    // gets for free from `-C site_root_str` on relative archive members.
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(600),
        safe_command("restic")
            .args(["-r", &repo, "--password-file", password_file,
                   "restore", &snapshot_id, "--target", "/", "--include", &site_dir])
            .kill_on_drop(true)
            .output()
    ).await
        .map_err(|_| err(StatusCode::GATEWAY_TIMEOUT, "Restore timed out"))?
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Restic: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Restore failed: {}", stderr.chars().take(300).collect::<String>())));
    }

    chown_restored_tree(&site_dir).await;

    tracing::info!("Restic restore completed for {domain} from {snapshot_id}");
    Ok(Json(serde_json::json!({ "ok": true, "snapshot_id": snapshot_id })))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/backups/{domain}/create", post(create))
        .route("/backups/{domain}/list", get(list))
        .route("/backups/{domain}/browse/{filename}", get(browse))
        .route("/backups/{domain}/restore/{filename}", post(restore))
        .route("/backups/{domain}/restore-file/{filename}", post(restore_file))
        .route("/backups/{domain}/{filename}", delete(remove))
        .route("/backups/{domain}/restic/backup", post(restic_backup))
        .route("/backups/{domain}/restic/snapshots", get(restic_snapshots))
        .route("/backups/{domain}/restic/restore/{snapshot_id}", post(restic_restore))
}
