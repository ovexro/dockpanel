use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{err, paginate, ApiError};
use crate::services::activity;
use crate::AppState;

#[derive(serde::Deserialize)]
pub struct BackupListQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct Backup {
    pub id: Uuid,
    pub site_id: Uuid,
    pub filename: String,
    pub size_bytes: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
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

/// POST /api/sites/{id}/backups — Create a backup.
pub async fn create(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<Backup>), ApiError> {
    let domain = get_site_domain(&state, id, claims.sub).await?;

    let agent_path = format!("/backups/{}/create", domain);
    let result = state
        .agent
        .post(&agent_path, None)
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Backup failed: {e}")))?;

    let filename = result
        .get("filename")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let size_bytes = result
        .get("size_bytes")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as i64;

    // Record in DB
    let backup: Backup = sqlx::query_as(
        "INSERT INTO backups (site_id, filename, size_bytes) VALUES ($1, $2, $3) RETURNING *",
    )
    .bind(id)
    .bind(&filename)
    .bind(size_bytes)
    .fetch_one(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    tracing::info!("Backup created: {filename} for {domain}");
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "backup.create",
        Some("backup"), Some(&domain), Some(&filename), None,
    ).await;

    Ok((StatusCode::CREATED, Json(backup)))
}

/// GET /api/sites/{id}/backups — List backups.
pub async fn list(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Query(params): Query<BackupListQuery>,
) -> Result<Json<Vec<Backup>>, ApiError> {
    // Verify ownership
    get_site_domain(&state, id, claims.sub).await?;

    let (limit, offset) = paginate(params.limit, params.offset);

    let backups: Vec<Backup> = sqlx::query_as(
        "SELECT * FROM backups WHERE site_id = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3",
    )
    .bind(id)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(backups))
}

/// POST /api/sites/{id}/backups/{backup_id}/restore — Restore a backup.
pub async fn restore(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path((id, backup_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = get_site_domain(&state, id, claims.sub).await?;

    let backup: Backup = sqlx::query_as(
        "SELECT * FROM backups WHERE id = $1 AND site_id = $2",
    )
    .bind(backup_id)
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Backup not found"))?;

    let agent_path = format!("/backups/{}/restore/{}", domain, backup.filename);
    state
        .agent
        .post(&agent_path, None)
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Restore failed: {e}")))?;

    tracing::info!("Backup restored: {} for {domain}", backup.filename);
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "backup.restore",
        Some("backup"), Some(&domain), Some(&backup.filename), None,
    ).await;

    Ok(Json(serde_json::json!({ "success": true })))
}

/// DELETE /api/sites/{id}/backups/{backup_id} — Delete a backup.
pub async fn remove(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path((id, backup_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = get_site_domain(&state, id, claims.sub).await?;

    let backup: Backup = sqlx::query_as(
        "SELECT * FROM backups WHERE id = $1 AND site_id = $2",
    )
    .bind(backup_id)
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Backup not found"))?;

    // Delete from agent (must succeed before DB deletion)
    let agent_path = format!("/backups/{}/{}", domain, backup.filename);
    state.agent.delete(&agent_path).await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Failed to delete backup file: {e}")))?;

    // Delete from DB
    sqlx::query("DELETE FROM backups WHERE id = $1")
        .bind(backup_id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    tracing::info!("Backup deleted: {} for {domain}", backup.filename);

    Ok(Json(serde_json::json!({ "ok": true })))
}
