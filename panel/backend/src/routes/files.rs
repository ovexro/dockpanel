use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{err, ApiError};
use crate::routes::is_safe_relative_path;
use crate::AppState;

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

/// Verify site ownership, return domain.
async fn get_site_domain(state: &AppState, site_id: Uuid, user_id: Uuid) -> Result<String, ApiError> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT domain FROM sites WHERE id = $1 AND user_id = $2")
            .bind(site_id)
            .bind(user_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    row.map(|(d,)| d)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))
}

/// GET /api/sites/{id}/files?path=
pub async fn list_dir(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Query(q): Query<PathQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = get_site_domain(&state, id, claims.sub).await?;
    let rel_path = q.path.as_deref().unwrap_or(".");

    if rel_path != "." && !is_safe_relative_path(rel_path) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid path"));
    }

    let agent_path = format!(
        "/files/{}/list?path={}",
        domain,
        urlencoding::encode(rel_path)
    );
    let result = state
        .agent
        .get(&agent_path)
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Agent error: {e}")))?;

    Ok(Json(result))
}

/// GET /api/sites/{id}/files/read?path=
pub async fn read_file(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Query(q): Query<PathQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = get_site_domain(&state, id, claims.sub).await?;
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
    let result = state
        .agent
        .get(&agent_path)
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Agent error: {e}")))?;

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
    let domain = get_site_domain(&state, id, claims.sub).await?;

    let agent_path = format!("/files/{}/write", domain);
    let result = state
        .agent
        .put(
            &agent_path,
            serde_json::json!({ "path": body.path, "content": body.content }),
        )
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Agent error: {e}")))?;

    Ok(Json(result))
}

/// POST /api/sites/{id}/files/create?path=&type=file|dir
pub async fn create_entry(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Query(q): Query<PathQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = get_site_domain(&state, id, claims.sub).await?;
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
    let result = state
        .agent
        .post(&agent_path, None)
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Agent error: {e}")))?;

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
    let domain = get_site_domain(&state, id, claims.sub).await?;

    let agent_path = format!("/files/{}/rename", domain);
    let result = state
        .agent
        .post(
            &agent_path,
            Some(serde_json::json!({ "from": body.from, "to": body.to })),
        )
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Agent error: {e}")))?;

    Ok(Json(result))
}

/// DELETE /api/sites/{id}/files?path=
pub async fn delete_entry(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Query(q): Query<PathQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = get_site_domain(&state, id, claims.sub).await?;
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
    let result = state
        .agent
        .delete(&agent_path)
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Agent error: {e}")))?;

    Ok(Json(result))
}
