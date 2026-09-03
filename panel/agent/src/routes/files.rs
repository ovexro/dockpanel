use axum::{
    body::Body,
    extract::{Path, Query},
    http::StatusCode,
    routing::{delete, get, post, put},
    Json, Router,
};
use serde::Deserialize;

use super::{is_valid_domain, AppState};
use crate::services::files;

#[derive(Deserialize)]
struct PathQuery {
    path: Option<String>,
}

#[derive(Deserialize)]
struct CreateQuery {
    path: Option<String>,
    r#type: Option<String>, // "file" or "dir"
}

#[derive(Deserialize)]
struct RenameBody {
    from: String,
    to: String,
}

#[derive(Deserialize)]
struct WriteBody {
    path: String,
    content: String,
}

type ApiErr = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: &str) -> ApiErr {
    (status, Json(serde_json::json!({ "error": msg })))
}

/// GET /files/{domain}/list?path=
async fn list_dir(
    Path(domain): Path<String>,
    Query(q): Query<PathQuery>,
) -> Result<Json<Vec<files::FileEntry>>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    let rel = q.path.as_deref().unwrap_or("/");
    let safe = files::resolve_safe_path(&domain, rel)
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;

    if !safe.is_dir() {
        return Err(err(StatusCode::BAD_REQUEST, "Not a directory"));
    }

    let site_root = std::path::PathBuf::from(format!("/var/www/{domain}"));
    let entries = files::list_directory(&safe, Some(&site_root))
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(entries))
}

/// Status for a file-service error: a refusal the operator can act on, or a
/// fault only an incident id should describe.
///
/// Every sentence below is a decision the service made deliberately — a size
/// cap, a text-vs-binary test, a name that is already taken, a path that is
/// already gone. The agent stat'd the file successfully and refused; nothing
/// broke. As 5xx the panel replaced all of them with "Operation failed.
/// Reference: {uuid}", so "New Folder" onto an existing name and a genuine agent
/// outage were the same screen, and opening a .png in the editor looked like
/// infrastructure failure. The path refusals four lines up were already 400 and
/// already survived — these are the ones that did not.
fn file_status(e: String) -> ApiErr {
    let status = if e.starts_with("File not found")
        || e == "Source does not exist"
        || e == "Path does not exist"
    {
        StatusCode::NOT_FOUND
    } else if e == "Path already exists" || e == "Destination already exists" {
        StatusCode::CONFLICT
    } else if e == "File too large (max 2MB)" {
        StatusCode::PAYLOAD_TOO_LARGE
    } else if e == "File is binary or not readable as text" {
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    err(status, &e)
}

/// GET /files/{domain}/read?path=
async fn read_file(
    Path(domain): Path<String>,
    Query(q): Query<PathQuery>,
) -> Result<Json<files::FileContent>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    let rel = q.path.as_deref().ok_or_else(|| err(StatusCode::BAD_REQUEST, "path required"))?;
    let safe = files::resolve_safe_path(&domain, rel)
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;

    let content = files::read_file(&safe)
        .await
        .map_err(file_status)?;
    Ok(Json(content))
}

/// PUT /files/{domain}/write
async fn write_file(
    Path(domain): Path<String>,
    Json(body): Json<WriteBody>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    let safe = files::resolve_safe_path(&domain, &body.path)
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;

    files::write_file(&safe, &body.content)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    Ok(Json(serde_json::json!({ "success": true })))
}

/// POST /files/{domain}/create?path=&type=
async fn create_entry(
    Path(domain): Path<String>,
    Query(q): Query<CreateQuery>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    let rel = q.path.as_deref().ok_or_else(|| err(StatusCode::BAD_REQUEST, "path required"))?;
    let is_dir = q.r#type.as_deref() == Some("dir");
    let safe = files::resolve_safe_path(&domain, rel)
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;

    files::create_entry(&safe, is_dir)
        .await
        .map_err(file_status)?;

    Ok(Json(serde_json::json!({ "success": true })))
}

/// POST /files/{domain}/rename
async fn rename_entry(
    Path(domain): Path<String>,
    Json(body): Json<RenameBody>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    let from = files::resolve_safe_child(&domain, &body.from)
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;
    let to = files::resolve_safe_path(&domain, &body.to)
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;

    files::rename_entry(&from, &to)
        .await
        .map_err(file_status)?;

    Ok(Json(serde_json::json!({ "success": true })))
}

/// DELETE /files/{domain}/delete?path=
async fn delete_entry(
    Path(domain): Path<String>,
    Query(q): Query<PathQuery>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    let rel = q.path.as_deref().ok_or_else(|| err(StatusCode::BAD_REQUEST, "path required"))?;
    let safe = files::resolve_safe_child(&domain, rel)
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;

    files::delete_entry(&safe)
        .await
        .map_err(file_status)?;

    Ok(Json(serde_json::json!({ "success": true })))
}

/// GET /files/{domain}/download?path= — Download a file as raw bytes.
///
/// Streamed, not read into memory whole: a site's own webroot routinely holds
/// files well past the file manager's 2MB text-editor cap (DB dumps, logs,
/// media) with no per-site disk quota (see `resolve_safe_path`'s own callers),
/// and this handler is reachable by the site's own non-admin owner. The
/// agent — the one process per box mediating deploys/backups/terminal/Docker
/// for every tenant — runs under a 512MB `MemoryMax` cgroup
/// (`dockpanel-agent.service`); `tokio::fs::read`-ing an arbitrarily large
/// file into a `Vec<u8>` before sending a single response byte let an
/// ordinary tenant OOM-kill it with one request.
async fn download_file(
    Path(domain): Path<String>,
    Query(q): Query<PathQuery>,
) -> Result<impl axum::response::IntoResponse, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    let rel = q.path.as_deref().ok_or_else(|| err(StatusCode::BAD_REQUEST, "path required"))?;
    let safe = files::resolve_safe_path(&domain, rel)
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;

    if safe.is_dir() {
        return Err(err(StatusCode::BAD_REQUEST, "Cannot download a directory"));
    }

    let file = tokio::fs::File::open(&safe)
        .await
        .map_err(|_| err(StatusCode::NOT_FOUND, "File not found"))?;
    let len = file
        .metadata()
        .await
        .map(|m| m.len())
        .map_err(|_| err(StatusCode::NOT_FOUND, "File not found"))?;

    let filename = safe
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("download");
    let safe_filename = filename.replace('"', "").replace(['\\', '\n', '\r'], "");

    let stream = tokio_util::io::ReaderStream::new(file);

    Ok((
        [
            (
                axum::http::header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{safe_filename}\""),
            ),
            (
                axum::http::header::CONTENT_TYPE,
                "application/octet-stream".to_string(),
            ),
            (axum::http::header::CONTENT_LENGTH, len.to_string()),
        ],
        Body::from_stream(stream),
    ))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/files/{domain}/list", get(list_dir))
        .route("/files/{domain}/read", get(read_file))
        .route("/files/{domain}/download", get(download_file))
        .route("/files/{domain}/write", put(write_file))
        .route("/files/{domain}/create", post(create_entry))
        .route("/files/{domain}/rename", post(rename_entry))
        .route("/files/{domain}/delete", delete(delete_entry))
}
