use axum::{
    extract::{Path, Query},
    http::StatusCode,
    routing::{delete, get, post, put},
    Json, Router,
};
use serde::Deserialize;

use super::{is_valid_domain, AppState};
use crate::services::files;

#[derive(Deserialize)]
struct UploadRequest {
    path: String,
    content: String, // base64-encoded
    filename: String,
}

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
    files::ensure_site_root(&domain).map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
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
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
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
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

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
    let from = files::resolve_safe_path(&domain, &body.from)
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;
    let to = files::resolve_safe_path(&domain, &body.to)
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;

    files::rename_entry(&from, &to)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

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
    let safe = files::resolve_safe_path(&domain, rel)
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;

    files::delete_entry(&safe)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    Ok(Json(serde_json::json!({ "success": true })))
}

/// GET /files/{domain}/download?path= — Download a file as raw bytes.
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

    let bytes = tokio::fs::read(&safe)
        .await
        .map_err(|_| err(StatusCode::NOT_FOUND, "File not found"))?;

    let filename = safe
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("download");

    Ok((
        [
            (
                axum::http::header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
            (
                axum::http::header::CONTENT_TYPE,
                "application/octet-stream".to_string(),
            ),
        ],
        bytes,
    ))
}

/// POST /files/{domain}/upload — Upload a file (base64-encoded content).
async fn upload_file(
    Path(domain): Path<String>,
    Json(body): Json<UploadRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    if body.filename.contains("..") || body.filename.contains('/') || body.filename.contains('\\') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid filename"));
    }
    if body.path.contains("..") {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid path"));
    }

    let site_dir = format!("/var/www/{domain}");
    if !std::path::Path::new(&site_dir).exists() {
        return Err(err(StatusCode::NOT_FOUND, "Site not found"));
    }

    let target_dir = if body.path.is_empty() || body.path == "/" || body.path == "." {
        site_dir.clone()
    } else {
        format!("{site_dir}/{}", body.path.trim_start_matches('/'))
    };

    let full_path = format!("{target_dir}/{}", body.filename);

    // Ensure target directory exists
    tokio::fs::create_dir_all(&target_dir).await.ok();

    // Validate path stays within site dir
    let canon_site = std::path::Path::new(&site_dir)
        .canonicalize()
        .map_err(|_| err(StatusCode::NOT_FOUND, "Site not found"))?;
    let canon_target = std::path::Path::new(&target_dir)
        .canonicalize()
        .map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid target directory"))?;
    if !canon_target.starts_with(&canon_site) {
        return Err(err(StatusCode::FORBIDDEN, "Access denied"));
    }

    // Decode base64
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&body.content)
        .map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid base64 content"))?;

    // Limit: 50MB
    if bytes.len() > 50 * 1024 * 1024 {
        return Err(err(
            StatusCode::PAYLOAD_TOO_LARGE,
            "File too large (max 50MB)",
        ));
    }

    tokio::fs::write(&full_path, &bytes)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Write failed: {e}")))?;

    // Fix ownership
    let _ = tokio::process::Command::new("chown")
        .args(["www-data:www-data", &full_path])
        .output()
        .await;

    tracing::info!("File uploaded: {full_path} ({} bytes)", bytes.len());
    Ok(Json(serde_json::json!({ "ok": true, "size": bytes.len() })))
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
