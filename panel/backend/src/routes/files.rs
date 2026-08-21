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

/// The largest file the file manager accepts, in bytes.
///
/// This is a DERIVED number, not a policy choice, and the derivation is the
/// reason the panel used to advertise a limit forty times larger than the one
/// it enforced. An upload is base64-encoded into a JSON body; that body is
/// capped at axum's 2 MiB default (`DEFAULT_LIMIT` in `axum-core`), on this
/// hop AND again on the panel to agent hop, which is a second axum service
/// with the same default. Base64 costs a third, so 2 MiB of body carries about
/// 1.57 MB of file — and the path and filename travel in the same body, so the
/// advertised number leaves room for them.
///
/// ⚠ RAISING THIS IS NOT A ONE-LINE CHANGE, and the original author's warning
/// here was right even though the tracker it pointed at never existed. Three
/// things have to be true first:
///
///  1. Both hops' body limits go up, and **per-route** — never with `.layer()`
///     on the root router, which would hand every unauthenticated caller,
///     `/login` and the public webhook doors included, a buffered allocation of
///     the same size.
///  2. There is a per-site disk quota. There is none today: `max_disk_mb` on a
///     reseller plan is an account-level figure, mail quotas are Dovecot's, and
///     disk accounting is server-wide, so a tenant filling shared `/var/www`
///     cannot even be MEASURED, let alone stopped.
///  3. The transport stops being base64-in-JSON. The whole payload is buffered
///     in RAM here and again in the agent, so a large envelope costs multiples
///     of the file per concurrent upload on a panel documented at ~49 MB.
///
/// Which is why the honest answer for genuinely large files is streaming
/// intake, and why `docs/guides/file-uploads.md` points at rsync instead.
///
/// Every surface that states a size MUST state this one: the guide, the two
/// upload pages, and the agent's own check. `upload-refusal-pin-e2e.sh` pins
/// that they agree.
pub const UPLOAD_MAX_FILE_BYTES: usize = 1_500_000;

/// The sentence an operator gets when a file will not fit through the panel.
///
/// One sentence, used by both refusals — the body-limit rejection that fires
/// before this handler and the decoded-size check inside it — so the operator
/// cannot receive two different accounts of the same limit.
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

/// Whether a base64 payload carries more than the file manager accepts.
///
/// Split out as a pure function so the boundary can be judged without a
/// request, a socket or a running panel — the same reason `url_authority` was
/// split out of the URL guard one release ago.
///
/// The estimate is deliberately an UPPER bound. Four base64 characters carry
/// three bytes, and padding means the true decoded length can be up to two
/// bytes less than this returns, so a payload within two bytes of the limit is
/// refused. Rounding against the uploader by two bytes is the safe direction
/// for a limit; rounding the other way is how a check ends up not enforcing the
/// number it prints.
pub fn exceeds_upload_limit(content: &str) -> bool {
    content.len() / 4 * 3 > UPLOAD_MAX_FILE_BYTES
}

/// Turn an extractor rejection into an answer the operator can act on.
///
/// Keyed on the rejection's STATUS rather than its variant shape: the composite
/// enum is a dependency detail that has changed between axum releases, while
/// "the body was too big" has been a 413 throughout.
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
    body: Result<Json<UploadBody>, JsonRejection>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Taking the REJECTION rather than letting the extractor short-circuit is
    // the whole point of this signature. The request-body limit refuses an
    // oversize upload before this handler would ever run, and it refuses in
    // plain text no client of ours can render, so the operator was told
    // nothing at all (#121). Everything below is unchanged; it simply now runs
    // for requests that fit, and the ones that do not get a sentence.
    let Json(body) = body.map_err(upload_rejection)?;

    if body.path.contains("..") || body.path.starts_with('/') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid path"));
    }
    if !is_safe_relative_path(&body.filename) && body.filename != "." {
        if body.filename.contains("..") || body.filename.contains('/') {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid filename"));
        }
    }

    // The payload is bounded twice: by the request-body limit that rejects
    // before this handler runs, and here, on the DECODED size — which is the
    // number the operator actually thinks in. Both refusals share one sentence.
    //
    // This check used to read 100 MB and could never fire, because the body
    // limit is ~2 MiB; the panel advertised a limit forty times the one it
    // enforced and refused in silence. It now states the truth, and
    // `UPLOAD_MAX_FILE_BYTES` carries the derivation.
    if exceeds_upload_limit(&body.content) {
        return Err(upload_too_large());
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Base64 length for `n` bytes: four characters per three bytes, padded up.
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

    /// The guard that matters most, and the one the old code failed.
    ///
    /// The advertised limit has to fit INSIDE the request-body envelope that
    /// rejects before this module runs. When it does not — as when this file
    /// advertised 100 MB against a 2 MiB envelope — the check becomes
    /// unreachable and the product refuses in silence. `DEFAULT_LIMIT` in
    /// `axum-core` is 2 MiB; base64 costs a third of that.
    #[test]
    fn the_advertised_limit_fits_inside_the_body_envelope() {
        const AXUM_DEFAULT_BODY_LIMIT: usize = 2 * 1024 * 1024;
        let carryable = AXUM_DEFAULT_BODY_LIMIT / 4 * 3;
        assert!(
            UPLOAD_MAX_FILE_BYTES <= carryable,
            "advertised {UPLOAD_MAX_FILE_BYTES} exceeds the {carryable} bytes a \
             {AXUM_DEFAULT_BODY_LIMIT}-byte body can carry — the check would be unreachable"
        );
    }
}
