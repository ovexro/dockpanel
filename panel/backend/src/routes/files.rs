use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{err, agent_error, ApiError};
use crate::routes::is_safe_relative_path;
use crate::AppState;

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
        // Sanitize exactly as the agent does before interpolating into the header
        // (strip quote/backslash/CR/LF) so a filename can't break out of the
        // quoted-string. This fallback fires whenever the agent's own header is
        // undecodable (e.g. a non-ASCII filename), so the two paths must stay
        // consistent.
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
    Json(body): Json<UploadBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.path.contains("..") || body.path.starts_with('/') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid path"));
    }
    if !is_safe_relative_path(&body.filename) && body.filename != "." {
        if body.filename.contains("..") || body.filename.contains('/') {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid filename"));
        }
    }

    // NOTE: the EFFECTIVE cap is axum's ~2MB default request-body limit (no
    // DefaultBodyLimit override is set), which rejects larger bodies before this
    // handler runs — so this 100MB check is a secondary guard, not the real limit.
    // Raising the body limit to allow genuinely large uploads must land together
    // with a per-tenant disk quota + rate limit (tracked in tech_debt) or it opens a
    // shared-/var/www exhaustion vector.
    let content_size = body.content.len();
    let max_upload_bytes: usize = 100 * 1024 * 1024; // 100MB
    if content_size > max_upload_bytes {
        return Err(err(StatusCode::PAYLOAD_TOO_LARGE,
            &format!("File too large: {}MB (max 100MB)", content_size / (1024 * 1024))));
    }

    // Advisory hygiene only — NOT a security boundary. Tenant code isolation is the
    // per-site PHP-FPM pool + the site-root confinement in resolve_safe_path; a tenant
    // legitimately runs their own code (incl. .php) in their own webroot, so this
    // blocklist is deliberately not replicated on write/create and must never be
    // treated as preventing code execution.
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
