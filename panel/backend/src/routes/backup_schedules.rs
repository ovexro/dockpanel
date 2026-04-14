use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{internal_error, err, ApiError};
use crate::services::activity;
use crate::AppState;


#[derive(serde::Serialize, sqlx::FromRow)]
pub struct BackupSchedule {
    pub id: Uuid,
    pub site_id: Uuid,
    pub destination_id: Option<Uuid>,
    pub schedule: String,
    pub retention_count: i32,
    pub enabled: bool,
    pub last_run: Option<chrono::DateTime<chrono::Utc>>,
    pub last_status: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Deserialize)]
pub struct SetScheduleRequest {
    pub destination_id: Uuid,
    pub schedule: String,
    pub retention_count: Option<i32>,
    pub enabled: Option<bool>,
}

/// Verify site ownership, return domain.
async fn get_site_domain(state: &AppState, site_id: Uuid, user_id: Uuid) -> Result<String, ApiError> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT domain FROM sites WHERE id = $1 AND user_id = $2")
            .bind(site_id)
            .bind(user_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("unknown", e))?;
    row.map(|(d,)| d)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))
}

/// GET /api/sites/{id}/backup-schedule — Get the backup schedule for a site.
pub async fn get_schedule(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Option<BackupSchedule>>, ApiError> {
    get_site_domain(&state, id, claims.sub).await?;

    let schedule: Option<BackupSchedule> = sqlx::query_as(
        "SELECT * FROM backup_schedules WHERE site_id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("get schedule", e))?;

    Ok(Json(schedule))
}

/// PUT /api/sites/{id}/backup-schedule — Create or update backup schedule.
pub async fn set_schedule(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<SetScheduleRequest>,
) -> Result<Json<BackupSchedule>, ApiError> {
    let domain = get_site_domain(&state, id, claims.sub).await?;

    // Validate schedule format
    let parts: Vec<&str> = body.schedule.split_whitespace().collect();
    if parts.len() != 5 {
        return Err(err(StatusCode::BAD_REQUEST, "Schedule must have 5 fields (minute hour day month weekday)"));
    }

    // Verify destination exists and belongs to the user's server
    let dest_check: Option<(Uuid,)> = sqlx::query_as(
        "SELECT bd.id FROM backup_destinations bd JOIN servers s ON bd.server_id = s.id WHERE bd.id = $1 AND s.user_id = $2",
    )
    .bind(&body.destination_id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();
    if dest_check.is_none() {
        return Err(err(StatusCode::FORBIDDEN, "Destination not found or not owned by you"));
    }

    let retention = body.retention_count.unwrap_or(7).max(1).min(365);
    let enabled = body.enabled.unwrap_or(true);

    // Upsert (unique on site_id)
    let schedule: BackupSchedule = sqlx::query_as(
        "INSERT INTO backup_schedules (site_id, destination_id, schedule, retention_count, enabled) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (site_id) DO UPDATE SET \
         destination_id = $2, schedule = $3, retention_count = $4, enabled = $5, updated_at = NOW() \
         RETURNING *",
    )
    .bind(id)
    .bind(body.destination_id)
    .bind(body.schedule.trim())
    .bind(retention)
    .bind(enabled)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("set schedule", e))?;

    tracing::info!("Backup schedule set for {domain}: {}", schedule.schedule);
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "backup.schedule",
        Some("backup"), Some(&domain), Some(&schedule.schedule), None,
    ).await;

    Ok(Json(schedule))
}

/// DELETE /api/sites/{id}/backup-schedule — Remove backup schedule.
pub async fn remove_schedule(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = get_site_domain(&state, id, claims.sub).await?;

    let deleted = sqlx::query("DELETE FROM backup_schedules WHERE site_id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("remove schedule", e))?;

    if deleted.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "No schedule found"));
    }

    tracing::info!("Backup schedule removed for {domain}");

    Ok(Json(serde_json::json!({ "ok": true })))
}
