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

/// POST /backups/{domain}/create — Create a backup.
async fn create(
    Path(domain): Path<String>,
) -> Result<Json<backups::BackupInfo>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }

    let info = backups::create_backup(&domain)
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
async fn restore(
    Path((domain, filename)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }

    backups::restore_backup(&domain, &filename)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(serde_json::json!({ "success": true })))
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

    // Ensure restic is installed
    let has_restic = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        safe_command("which").arg("restic").output()
    ).await.ok().and_then(|r| r.ok()).map(|o| o.status.success()).unwrap_or(false);

    if !has_restic {
        return Err(err(StatusCode::PRECONDITION_FAILED,
            "Restic not installed. Run: apt-get install restic"));
    }

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
            .output()
    ).await
        .map_err(|_| err(StatusCode::GATEWAY_TIMEOUT, "Timeout"))?
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Restic: {e}")))?;

    let snapshots: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap_or_default();
    let total = snapshots.len();

    Ok(Json(serde_json::json!({
        "snapshots": snapshots,
        "total": total,
    })))
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

    let repo = format!("/var/backups/dockpanel/restic/{}", domain.replace('.', "_"));
    let password_file = "/etc/dockpanel/restic-password";

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(600),
        safe_command("restic")
            .args(["-r", &repo, "--password-file", password_file,
                   "restore", &snapshot_id, "--target", "/"])
            .output()
    ).await
        .map_err(|_| err(StatusCode::GATEWAY_TIMEOUT, "Restore timed out"))?
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Restic: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Restore failed: {}", stderr.chars().take(300).collect::<String>())));
    }

    // Fix ownership
    let _ = safe_command("chown")
        .args(["-R", "www-data:www-data", &format!("/var/www/{domain}")])
        .output()
        .await;

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
