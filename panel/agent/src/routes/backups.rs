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

    // Held through the extraction+chown below — see site_lock's module doc
    // for why a restore and e.g. a concurrent hardening apply must not
    // interleave on the same site's files.
    let _guard = crate::site_lock::lock_site(&domain).await;

    let databases = body.map(|Json(b)| b.databases).unwrap_or_default();

    let report = backups::restore_backup(&domain, &filename, &databases)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    // `restore_backup`'s tar extraction runs `--no-same-owner`, which leaves the
    // whole site root:root — unwritable by the www-data-running app until this
    // runs.
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

use crate::safe_cmd::safe_command;

/// Hand a restored site tree back to the web server without exposing `.git`.
///
/// `restore_backup`'s tar extraction runs `--no-same-owner`, so the whole
/// site comes back root-owned — unwritable to the web-server-running app
/// until this runs. A blanket `chown -R www-data:www-data` over the whole
/// tree would also hand the application `.git/config` and `.git/hooks/` if
/// this site uses Git Deploy, which the next `git_build.rs` deploy then reads
/// and executes as root — the same defect `deploy.rs` was fixed to avoid (see
/// its own `hand_tree_to_web_user` doc comment). Mirror that split here:
/// chown everything except `.git` to www-data, then explicitly re-secure
/// `.git` to root, rather than trust whatever ownership the extraction
/// happened to leave it at.
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

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/backups/{domain}/create", post(create))
        .route("/backups/{domain}/list", get(list))
        .route("/backups/{domain}/browse/{filename}", get(browse))
        .route("/backups/{domain}/restore/{filename}", post(restore))
        .route("/backups/{domain}/restore-file/{filename}", post(restore_file))
        .route("/backups/{domain}/{filename}", delete(remove))
}
