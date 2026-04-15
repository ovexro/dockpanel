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
    let safe_filename = filename.replace('"', "").replace(['\\', '\n', '\r'], "");

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
        ],
        bytes,
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
