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

#[derive(Deserialize)]
struct CopyBody {
    from: String,
    to: String,
}

#[derive(Deserialize)]
struct DuplicateBody {
    path: String,
}

#[derive(Deserialize)]
struct BulkPathsBody {
    paths: Vec<String>,
}

#[derive(Deserialize)]
struct BulkTransferBody {
    sources: Vec<String>,
    target_dir: String,
}

#[derive(Deserialize)]
struct ChmodBody {
    path: String,
    mode: u32,
    #[serde(default)]
    recursive: bool,
}

#[derive(Deserialize)]
struct ChownBody {
    path: String,
    owner: String,
    group: String,
    #[serde(default)]
    recursive: bool,
}

#[derive(Deserialize)]
struct StatQuery {
    path: Option<String>,
    #[serde(default)]
    checksum: Option<bool>,
}

#[derive(Deserialize)]
struct CompressBody {
    sources: Vec<String>,
    archive_name: String,
    #[serde(default)]
    target_dir: Option<String>,
}

#[derive(Deserialize)]
struct ExtractBody {
    archive: String,
    #[serde(default)]
    target_dir: Option<String>,
    #[serde(default)]
    overwrite: Option<bool>,
}

#[derive(Deserialize)]
struct SearchQuery {
    #[serde(default)]
    path: Option<String>,
    query: Option<String>,
    #[serde(default)]
    in_content: Option<bool>,
    #[serde(default)]
    limit: Option<usize>,
}

type ApiErr = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: &str) -> ApiErr {
    (status, Json(serde_json::json!({ "error": msg })))
}

fn file_status(e: String) -> ApiErr {
    let status = if e.starts_with("File not found")
        || e == "Source does not exist"
        || e == "Path does not exist"
        || e == "Archive does not exist"
        || e == "Archive file does not exist"
    {
        StatusCode::NOT_FOUND
    } else if e == "Path already exists" || e == "Destination already exists" || e == "Destination archive already exists" {
        StatusCode::CONFLICT
    } else if e == "File too large (max 5MB)" || e == "File too large (max 2MB)" {
        StatusCode::PAYLOAD_TOO_LARGE
    } else if e == "File is binary or not readable as text" {
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    err(status, &e)
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

/// POST /files/{domain}/copy
async fn copy_entry(
    Path(domain): Path<String>,
    Json(body): Json<CopyBody>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    let from = files::resolve_safe_path(&domain, &body.from)
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;
    let to = files::resolve_safe_path(&domain, &body.to)
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;

    files::copy_entry(&from, &to)
        .await
        .map_err(file_status)?;

    Ok(Json(serde_json::json!({ "success": true })))
}

/// POST /files/{domain}/duplicate
async fn duplicate_entry(
    Path(domain): Path<String>,
    Json(body): Json<DuplicateBody>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    let safe = files::resolve_safe_child(&domain, &body.path)
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;

    let new_name = files::duplicate_entry(&safe)
        .await
        .map_err(file_status)?;

    Ok(Json(serde_json::json!({ "success": true, "name": new_name })))
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

/// POST /files/{domain}/bulk/delete
async fn bulk_delete(
    Path(domain): Path<String>,
    Json(body): Json<BulkPathsBody>,
) -> Result<Json<files::BulkResult>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }

    let mut succeeded = Vec::new();
    let mut failed = Vec::new();

    for rel in &body.paths {
        match files::resolve_safe_child(&domain, rel) {
            Ok(safe) => match files::delete_entry(&safe).await {
                Ok(_) => succeeded.push(rel.clone()),
                Err(e) => failed.push(files::BulkFailure {
                    path: rel.clone(),
                    error: e,
                }),
            },
            Err(e) => failed.push(files::BulkFailure {
                path: rel.clone(),
                error: e,
            }),
        }
    }

    Ok(Json(files::BulkResult { succeeded, failed }))
}

/// POST /files/{domain}/bulk/copy
async fn bulk_copy(
    Path(domain): Path<String>,
    Json(body): Json<BulkTransferBody>,
) -> Result<Json<files::BulkResult>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }

    let target_dir = match files::resolve_safe_path(&domain, &body.target_dir) {
        Ok(t) => t,
        Err(e) => return Err(err(StatusCode::BAD_REQUEST, &e)),
    };

    if !target_dir.is_dir() {
        return Err(err(StatusCode::BAD_REQUEST, "Target is not a directory"));
    }

    let mut succeeded = Vec::new();
    let mut failed = Vec::new();

    for src_rel in &body.sources {
        match files::resolve_safe_path(&domain, src_rel) {
            Ok(src_path) => {
                let file_name = src_path.file_name().unwrap_or_default();
                let dst_path = target_dir.join(file_name);
                match files::copy_entry(&src_path, &dst_path).await {
                    Ok(_) => succeeded.push(src_rel.clone()),
                    Err(e) => failed.push(files::BulkFailure {
                        path: src_rel.clone(),
                        error: e,
                    }),
                }
            }
            Err(e) => failed.push(files::BulkFailure {
                path: src_rel.clone(),
                error: e,
            }),
        }
    }

    Ok(Json(files::BulkResult { succeeded, failed }))
}

/// POST /files/{domain}/bulk/move
async fn bulk_move(
    Path(domain): Path<String>,
    Json(body): Json<BulkTransferBody>,
) -> Result<Json<files::BulkResult>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }

    let target_dir = match files::resolve_safe_path(&domain, &body.target_dir) {
        Ok(t) => t,
        Err(e) => return Err(err(StatusCode::BAD_REQUEST, &e)),
    };

    if !target_dir.is_dir() {
        return Err(err(StatusCode::BAD_REQUEST, "Target is not a directory"));
    }

    let mut succeeded = Vec::new();
    let mut failed = Vec::new();

    for src_rel in &body.sources {
        match files::resolve_safe_child(&domain, src_rel) {
            Ok(src_path) => {
                let file_name = src_path.file_name().unwrap_or_default();
                let dst_path = target_dir.join(file_name);
                match files::rename_entry(&src_path, &dst_path).await {
                    Ok(_) => succeeded.push(src_rel.clone()),
                    Err(e) => failed.push(files::BulkFailure {
                        path: src_rel.clone(),
                        error: e,
                    }),
                }
            }
            Err(e) => failed.push(files::BulkFailure {
                path: src_rel.clone(),
                error: e,
            }),
        }
    }

    Ok(Json(files::BulkResult { succeeded, failed }))
}

/// POST /files/{domain}/chmod
async fn chmod_entry(
    Path(domain): Path<String>,
    Json(body): Json<ChmodBody>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    let safe = files::resolve_safe_path(&domain, &body.path)
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;

    files::chmod_entry(&safe, body.mode, body.recursive)
        .await
        .map_err(file_status)?;

    Ok(Json(serde_json::json!({ "success": true })))
}

/// POST /files/{domain}/chown
async fn chown_entry(
    Path(domain): Path<String>,
    Json(body): Json<ChownBody>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    let safe = files::resolve_safe_path(&domain, &body.path)
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;

    files::chown_entry(&safe, &body.owner, &body.group, body.recursive)
        .await
        .map_err(file_status)?;

    Ok(Json(serde_json::json!({ "success": true })))
}

/// GET /files/{domain}/stat?path=&checksum=true
async fn stat_entry(
    Path(domain): Path<String>,
    Query(q): Query<StatQuery>,
) -> Result<Json<files::FileStat>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    let rel = q.path.as_deref().unwrap_or(".");
    let safe = files::resolve_safe_path(&domain, rel)
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;

    let stat = files::stat_entry(&safe, rel, q.checksum.unwrap_or(false))
        .await
        .map_err(file_status)?;

    Ok(Json(stat))
}

/// POST /files/{domain}/compress
async fn compress_entries(
    Path(domain): Path<String>,
    Json(body): Json<CompressBody>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    let target_dir = body.target_dir.as_deref().unwrap_or(".");
    let name = files::compress_entries(&domain, &body.sources, &body.archive_name, target_dir)
        .await
        .map_err(file_status)?;

    Ok(Json(serde_json::json!({ "success": true, "archive": name })))
}

/// POST /files/{domain}/extract
async fn extract_archive(
    Path(domain): Path<String>,
    Json(body): Json<ExtractBody>,
) -> Result<Json<files::ExtractResult>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    let target_dir = body.target_dir.as_deref().unwrap_or(".");
    let overwrite = body.overwrite.unwrap_or(true);

    let res = files::extract_archive(&domain, &body.archive, target_dir, overwrite)
        .await
        .map_err(file_status)?;

    Ok(Json(res))
}

/// GET /files/{domain}/inspect?path=
async fn inspect_archive(
    Path(domain): Path<String>,
    Query(q): Query<PathQuery>,
) -> Result<Json<Vec<files::ArchiveEntry>>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    let rel = q.path.as_deref().ok_or_else(|| err(StatusCode::BAD_REQUEST, "path required"))?;

    let entries = files::inspect_archive(&domain, rel)
        .await
        .map_err(file_status)?;

    Ok(Json(entries))
}

/// GET /files/{domain}/search?path=&query=&in_content=false&limit=100
async fn search_files(
    Path(domain): Path<String>,
    Query(q): Query<SearchQuery>,
) -> Result<Json<Vec<files::SearchResult>>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    let base_rel = q.path.as_deref().unwrap_or(".");
    let query_str = q.query.as_deref().unwrap_or("");
    let in_content = q.in_content.unwrap_or(false);
    let limit = q.limit.unwrap_or(100);

    let results = files::search_files(&domain, base_rel, query_str, in_content, limit)
        .await
        .map_err(file_status)?;

    Ok(Json(results))
}

/// GET /files/{domain}/storage?path=
async fn directory_storage(
    Path(domain): Path<String>,
    Query(q): Query<PathQuery>,
) -> Result<Json<files::StorageStats>, ApiErr> {
    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    let rel = q.path.as_deref().unwrap_or(".");

    let stats = files::directory_storage(&domain, rel)
        .await
        .map_err(file_status)?;

    Ok(Json(stats))
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
        return Err(err(StatusCode::BAD_REQUEST, "Cannot download a directory directly"));
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
                files::mime_type_for_path(&safe).to_string(),
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
        .route("/files/{domain}/copy", post(copy_entry))
        .route("/files/{domain}/duplicate", post(duplicate_entry))
        .route("/files/{domain}/delete", delete(delete_entry))
        .route("/files/{domain}/bulk/delete", post(bulk_delete))
        .route("/files/{domain}/bulk/copy", post(bulk_copy))
        .route("/files/{domain}/bulk/move", post(bulk_move))
        .route("/files/{domain}/chmod", post(chmod_entry))
        .route("/files/{domain}/chown", post(chown_entry))
        .route("/files/{domain}/stat", get(stat_entry))
        .route("/files/{domain}/compress", post(compress_entries))
        .route("/files/{domain}/extract", post(extract_archive))
        .route("/files/{domain}/inspect", get(inspect_archive))
        .route("/files/{domain}/search", get(search_files))
        .route("/files/{domain}/storage", get(directory_storage))
}
