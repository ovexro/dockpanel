use axum::{
    extract::{rejection::JsonRejection, Path, Query, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{err, err_coded, agent_error, ApiError, CODE_PAYLOAD_TOO_LARGE};
use crate::routes::is_safe_relative_path;
use crate::AppState;

pub const UPLOAD_MAX_FILE_BYTES: usize = 1_500_000;

fn upload_too_large() -> ApiError {
    err_coded(
        StatusCode::PAYLOAD_TOO_LARGE,
        &format!(
            "File is too large for the file manager (limit {:.1} MB). Copy it \
             with rsync or scp over the server's own SSH, or use the Migration \
             wizard to move a whole site.",
            UPLOAD_MAX_FILE_BYTES as f64 / 1_000_000.0
        ),
        CODE_PAYLOAD_TOO_LARGE,
    )
}

pub fn exceeds_upload_limit(content: &str) -> bool {
    content.len() / 4 * 3 > UPLOAD_MAX_FILE_BYTES
}

fn upload_rejection(e: JsonRejection) -> ApiError {
    if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
        upload_too_large()
    } else {
        err(StatusCode::BAD_REQUEST, &format!("Invalid upload request: {e}"))
    }
}

#[derive(serde::Deserialize)]
pub struct UploadBody {
    pub path: String,
    pub content: String,
    pub filename: String,
}

#[derive(serde::Deserialize)]
pub struct PathQuery {
    pub path: Option<String>,
    #[serde(rename = "type")]
    pub entry_type: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct WriteBody {
    pub path: String,
    pub content: String,
}

#[derive(serde::Deserialize)]
pub struct RenameBody {
    pub from: String,
    pub to: String,
}

#[derive(serde::Deserialize)]
pub struct CopyBody {
    pub from: String,
    pub to: String,
}

#[derive(serde::Deserialize)]
pub struct DuplicateBody {
    pub path: String,
}

#[derive(serde::Deserialize)]
pub struct BulkPathsBody {
    pub paths: Vec<String>,
}

#[derive(serde::Deserialize)]
pub struct BulkTransferBody {
    pub sources: Vec<String>,
    pub target_dir: String,
}

#[derive(serde::Deserialize)]
pub struct ChmodBody {
    pub path: String,
    pub mode: u32,
    #[serde(default)]
    pub recursive: bool,
}

#[derive(serde::Deserialize)]
pub struct ChownBody {
    pub path: String,
    pub owner: String,
    pub group: String,
    #[serde(default)]
    pub recursive: bool,
}

#[derive(serde::Deserialize)]
pub struct StatQuery {
    pub path: Option<String>,
    #[serde(default)]
    pub checksum: Option<bool>,
}

#[derive(serde::Deserialize)]
pub struct CompressBody {
    pub sources: Vec<String>,
    pub archive_name: String,
    #[serde(default)]
    pub target_dir: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct ExtractBody {
    pub archive: String,
    #[serde(default)]
    pub target_dir: Option<String>,
    #[serde(default)]
    pub overwrite: Option<bool>,
}

#[derive(serde::Deserialize)]
pub struct SearchQuery {
    pub path: Option<String>,
    pub query: Option<String>,
    pub in_content: Option<bool>,
    pub limit: Option<usize>,
}

/// GET /api/sites/{id}/files?path=
pub async fn list_dir(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Query(q): Query<PathQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;
    let rel_path = q.path.as_deref().unwrap_or(".");

    if rel_path != "." && !is_safe_relative_path(rel_path) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid path"));
    }

    let agent_path = format!(
        "/files/{}/list?path={}",
        domain,
        urlencoding::encode(rel_path)
    );
    let result = agent
        .get(&agent_path)
        .await
        .map_err(|e| agent_error("File manager", e))?;

    Ok(Json(result))
}

/// GET /api/sites/{id}/files/read?path=
pub async fn read_file(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Query(q): Query<PathQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;
    let rel_path = q.path.as_deref().unwrap_or("");

    if rel_path.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "path is required"));
    }
    if !is_safe_relative_path(rel_path) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid path"));
    }

    let agent_path = format!(
        "/files/{}/read?path={}",
        domain,
        urlencoding::encode(rel_path)
    );
    let result = agent
        .get(&agent_path)
        .await
        .map_err(|e| agent_error("File manager", e))?;

    Ok(Json(result))
}

/// PUT /api/sites/{id}/files/write — { path, content }
pub async fn write_file(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<WriteBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !is_safe_relative_path(&body.path) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid path"));
    }
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    let agent_path = format!("/files/{}/write", domain);
    let result = agent
        .put(
            &agent_path,
            serde_json::json!({ "path": body.path, "content": body.content }),
        )
        .await
        .map_err(|e| agent_error("File manager", e))?;

    Ok(Json(result))
}

/// POST /api/sites/{id}/files/create?path=&type=file|dir
pub async fn create_entry(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Query(q): Query<PathQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;
    let rel_path = q.path.as_deref().unwrap_or("");
    let entry_type = q.entry_type.as_deref().unwrap_or("file");

    if rel_path.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "path is required"));
    }
    if !is_safe_relative_path(rel_path) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid path"));
    }
    if !["file", "dir"].contains(&entry_type) {
        return Err(err(StatusCode::BAD_REQUEST, "type must be file or dir"));
    }

    let agent_path = format!(
        "/files/{}/create?path={}&type={}",
        domain,
        urlencoding::encode(rel_path),
        entry_type
    );
    let result = agent
        .post(&agent_path, None)
        .await
        .map_err(|e| agent_error("File manager", e))?;

    Ok(Json(result))
}

/// POST /api/sites/{id}/files/rename — { from, to }
pub async fn rename_entry(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<RenameBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !is_safe_relative_path(&body.from) || !is_safe_relative_path(&body.to) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid path"));
    }
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    let agent_path = format!("/files/{}/rename", domain);
    let result = agent
        .post(
            &agent_path,
            Some(serde_json::json!({ "from": body.from, "to": body.to })),
        )
        .await
        .map_err(|e| agent_error("File manager", e))?;

    Ok(Json(result))
}

/// POST /api/sites/{id}/files/copy — { from, to }
pub async fn copy_entry(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<CopyBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !is_safe_relative_path(&body.from) || !is_safe_relative_path(&body.to) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid path"));
    }
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    let agent_path = format!("/files/{}/copy", domain);
    let result = agent
        .post(
            &agent_path,
            Some(serde_json::json!({ "from": body.from, "to": body.to })),
        )
        .await
        .map_err(|e| agent_error("File manager", e))?;

    Ok(Json(result))
}

/// POST /api/sites/{id}/files/duplicate — { path }
pub async fn duplicate_entry(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<DuplicateBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !is_safe_relative_path(&body.path) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid path"));
    }
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    let agent_path = format!("/files/{}/duplicate", domain);
    let result = agent
        .post(
            &agent_path,
            Some(serde_json::json!({ "path": body.path })),
        )
        .await
        .map_err(|e| agent_error("File manager", e))?;

    Ok(Json(result))
}

/// DELETE /api/sites/{id}/files?path=
pub async fn delete_entry(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Query(q): Query<PathQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;
    let rel_path = q.path.as_deref().unwrap_or("");

    if rel_path.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "path is required"));
    }
    if !is_safe_relative_path(rel_path) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid path"));
    }

    let agent_path = format!(
        "/files/{}/delete?path={}",
        domain,
        urlencoding::encode(rel_path)
    );
    let result = agent
        .delete(&agent_path)
        .await
        .map_err(|e| agent_error("File manager", e))?;

    Ok(Json(result))
}

/// POST /api/sites/{id}/files/bulk/delete
pub async fn bulk_delete(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<BulkPathsBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    for p in &body.paths {
        if !is_safe_relative_path(p) {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid path in bulk delete list"));
        }
    }
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    let agent_path = format!("/files/{}/bulk/delete", domain);
    let result = agent
        .post(&agent_path, Some(serde_json::json!({ "paths": body.paths })))
        .await
        .map_err(|e| agent_error("File manager", e))?;

    Ok(Json(result))
}

/// POST /api/sites/{id}/files/bulk/copy
pub async fn bulk_copy(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<BulkTransferBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.target_dir != "." && !is_safe_relative_path(&body.target_dir) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid target directory"));
    }
    for p in &body.sources {
        if !is_safe_relative_path(p) {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid source path in list"));
        }
    }
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    let agent_path = format!("/files/{}/bulk/copy", domain);
    let result = agent
        .post(
            &agent_path,
            Some(serde_json::json!({
                "sources": body.sources,
                "target_dir": body.target_dir,
            })),
        )
        .await
        .map_err(|e| agent_error("File manager", e))?;

    Ok(Json(result))
}

/// POST /api/sites/{id}/files/bulk/move
pub async fn bulk_move(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<BulkTransferBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.target_dir != "." && !is_safe_relative_path(&body.target_dir) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid target directory"));
    }
    for p in &body.sources {
        if !is_safe_relative_path(p) {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid source path in list"));
        }
    }
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    let agent_path = format!("/files/{}/bulk/move", domain);
    let result = agent
        .post(
            &agent_path,
            Some(serde_json::json!({
                "sources": body.sources,
                "target_dir": body.target_dir,
            })),
        )
        .await
        .map_err(|e| agent_error("File manager", e))?;

    Ok(Json(result))
}

/// POST /api/sites/{id}/files/chmod
pub async fn chmod_entry(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<ChmodBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.path != "." && !is_safe_relative_path(&body.path) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid path"));
    }
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    let agent_path = format!("/files/{}/chmod", domain);
    let result = agent
        .post(
            &agent_path,
            Some(serde_json::json!({
                "path": body.path,
                "mode": body.mode,
                "recursive": body.recursive,
            })),
        )
        .await
        .map_err(|e| agent_error("File manager", e))?;

    Ok(Json(result))
}

/// POST /api/sites/{id}/files/chown
pub async fn chown_entry(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<ChownBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.path != "." && !is_safe_relative_path(&body.path) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid path"));
    }
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    let agent_path = format!("/files/{}/chown", domain);
    let result = agent
        .post(
            &agent_path,
            Some(serde_json::json!({
                "path": body.path,
                "owner": body.owner,
                "group": body.group,
                "recursive": body.recursive,
            })),
        )
        .await
        .map_err(|e| agent_error("File manager", e))?;

    Ok(Json(result))
}

/// GET /api/sites/{id}/files/stat?path=&checksum=
pub async fn stat_entry(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Query(q): Query<StatQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let rel_path = q.path.as_deref().unwrap_or(".");
    if rel_path != "." && !is_safe_relative_path(rel_path) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid path"));
    }
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    let agent_path = format!(
        "/files/{}/stat?path={}&checksum={}",
        domain,
        urlencoding::encode(rel_path),
        q.checksum.unwrap_or(false)
    );
    let result = agent
        .get(&agent_path)
        .await
        .map_err(|e| agent_error("File manager", e))?;

    Ok(Json(result))
}

/// POST /api/sites/{id}/files/compress
pub async fn compress_entries(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<CompressBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    for p in &body.sources {
        if p != "." && !is_safe_relative_path(p) {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid source path"));
        }
    }
    if let Some(ref td) = body.target_dir {
        if td != "." && !is_safe_relative_path(td) {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid target directory"));
        }
    }
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    let agent_path = format!("/files/{}/compress", domain);
    let result = agent
        .post(
            &agent_path,
            Some(serde_json::json!({
                "sources": body.sources,
                "archive_name": body.archive_name,
                "target_dir": body.target_dir,
            })),
        )
        .await
        .map_err(|e| agent_error("File manager", e))?;

    Ok(Json(result))
}

/// POST /api/sites/{id}/files/extract
pub async fn extract_archive(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<ExtractBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !is_safe_relative_path(&body.archive) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid archive path"));
    }
    if let Some(ref td) = body.target_dir {
        if td != "." && !is_safe_relative_path(td) {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid target directory"));
        }
    }
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    let agent_path = format!("/files/{}/extract", domain);
    let result = agent
        .post(
            &agent_path,
            Some(serde_json::json!({
                "archive": body.archive,
                "target_dir": body.target_dir,
                "overwrite": body.overwrite,
            })),
        )
        .await
        .map_err(|e| agent_error("File manager", e))?;

    Ok(Json(result))
}

/// GET /api/sites/{id}/files/inspect?path=
pub async fn inspect_archive(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Query(q): Query<PathQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let rel_path = q.path.as_deref().unwrap_or("");
    if rel_path.is_empty() || !is_safe_relative_path(rel_path) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid archive path"));
    }
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    let agent_path = format!(
        "/files/{}/inspect?path={}",
        domain,
        urlencoding::encode(rel_path)
    );
    let result = agent
        .get(&agent_path)
        .await
        .map_err(|e| agent_error("File manager", e))?;

    Ok(Json(result))
}

/// GET /api/sites/{id}/files/search?path=&query=&in_content=false&limit=100
pub async fn search_files(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Query(q): Query<SearchQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let rel_path = q.path.as_deref().unwrap_or(".");
    if rel_path != "." && !is_safe_relative_path(rel_path) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid path"));
    }
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    let agent_path = format!(
        "/files/{}/search?path={}&query={}&in_content={}&limit={}",
        domain,
        urlencoding::encode(rel_path),
        urlencoding::encode(q.query.as_deref().unwrap_or("")),
        q.in_content.unwrap_or(false),
        q.limit.unwrap_or(100)
    );
    let result = agent
        .get(&agent_path)
        .await
        .map_err(|e| agent_error("File manager", e))?;

    Ok(Json(result))
}

/// GET /api/sites/{id}/files/storage?path=
pub async fn directory_storage(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Query(q): Query<PathQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let rel_path = q.path.as_deref().unwrap_or(".");
    if rel_path != "." && !is_safe_relative_path(rel_path) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid path"));
    }
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    let agent_path = format!(
        "/files/{}/storage?path={}",
        domain,
        urlencoding::encode(rel_path)
    );
    let result = agent
        .get(&agent_path)
        .await
        .map_err(|e| agent_error("File manager", e))?;

    Ok(Json(result))
}

/// GET /api/sites/{id}/files/download?path= — Download a file.
pub async fn download_file(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Query(q): Query<PathQuery>,
) -> Result<impl axum::response::IntoResponse, ApiError> {
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;
    let rel_path = q.path.as_deref().unwrap_or("");

    if rel_path.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "path is required"));
    }
    if !is_safe_relative_path(rel_path) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid path"));
    }

    let agent_path = format!(
        "/files/{}/download?path={}",
        domain,
        urlencoding::encode(rel_path)
    );
    let (bytes, content_disposition) = agent
        .get_bytes(&agent_path)
        .await
        .map_err(|e| agent_error("File download", e))?;

    let disposition = content_disposition.unwrap_or_else(|| {
        let filename: String = rel_path
            .split('/')
            .last()
            .unwrap_or("download")
            .replace(['"', '\\', '\n', '\r'], "");
        format!("attachment; filename=\"{filename}\"")
    });

    Ok((
        [
            (
                axum::http::header::CONTENT_DISPOSITION,
                disposition,
            ),
            (
                axum::http::header::CONTENT_TYPE,
                "application/octet-stream".to_string(),
            ),
        ],
        bytes,
    ))
}

/// POST /api/sites/{id}/files/upload — Upload a file.
pub async fn upload_file(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    body: Result<Json<UploadBody>, JsonRejection>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let Json(body) = body.map_err(upload_rejection)?;

    if body.path.contains("..") || body.path.starts_with('/') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid path"));
    }
    if !is_safe_relative_path(&body.filename) && body.filename != "." {
        if body.filename.contains("..") || body.filename.contains('/') {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid filename"));
        }
    }

    if exceeds_upload_limit(&body.content) {
        return Err(upload_too_large());
    }

    let lower_name = body.filename.to_lowercase();
    let dangerous_exts = [".phar", ".pht", ".phtml", ".shtml", ".htaccess"];
    if dangerous_exts.iter().any(|ext| lower_name.ends_with(ext)) {
        return Err(err(StatusCode::BAD_REQUEST,
            "File type not allowed (dangerous extension)"));
    }

    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    let agent_path = format!("/files/{}/upload", domain);
    let result = agent
        .post(
            &agent_path,
            Some(serde_json::json!({
                "path": body.path,
                "content": body.content,
                "filename": body.filename,
            })),
        )
        .await
        .map_err(|e| agent_error("File upload", e))?;

    Ok(Json(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64_len(n: usize) -> usize {
        n.div_ceil(3) * 4
    }

    #[test]
    fn a_payload_at_the_limit_is_accepted() {
        let at = "A".repeat(b64_len(UPLOAD_MAX_FILE_BYTES));
        assert!(!exceeds_upload_limit(&at), "a file of exactly the advertised limit must upload");
    }

    #[test]
    fn a_payload_over_the_limit_is_refused() {
        let over = "A".repeat(b64_len(UPLOAD_MAX_FILE_BYTES + 1));
        assert!(exceeds_upload_limit(&over), "one byte over the advertised limit must be refused");
    }

    #[test]
    fn an_empty_payload_is_not_too_large() {
        assert!(!exceeds_upload_limit(""));
    }

    #[test]
    fn the_advertised_limit_fits_inside_the_body_envelope() {
        const AXUM_DEFAULT_BODY_LIMIT: usize = 2 * 1024 * 1024;
        let carryable = AXUM_DEFAULT_BODY_LIMIT / 4 * 3;
        assert!(
            UPLOAD_MAX_FILE_BYTES <= carryable,
            "advertised {UPLOAD_MAX_FILE_BYTES} exceeds the {carryable} bytes a \
             {AXUM_DEFAULT_BODY_LIMIT}-byte body can carry"
        );
    }
}
