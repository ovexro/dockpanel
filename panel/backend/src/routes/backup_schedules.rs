use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::auth::Claims;
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

/// Helper: resolve a site this caller may act on, and return its domain.
///
/// One line over [`crate::helpers::site_domain_for_caller`], which is shared with
/// the five other modules that each carried their own copy of this query. The
/// rules — including why only the admin arm is scoped by server — live there.
async fn get_site_domain(state: &AppState, site_id: Uuid, claims: &Claims) -> Result<String, ApiError> {
    crate::helpers::site_domain_for_caller(state, site_id, claims).await
}

/// GET /api/sites/{id}/backup-schedule — Get the backup schedule for a site.
pub async fn get_schedule(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Option<BackupSchedule>>, ApiError> {
    get_site_domain(&state, id, &claims).await?;

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
    let domain = get_site_domain(&state, id, &claims).await?;

    // Validate schedule format
    let parts: Vec<&str> = body.schedule.split_whitespace().collect();
    if parts.len() != 5 {
        return Err(err(StatusCode::BAD_REQUEST, "Schedule must have 5 fields (minute hour day month weekday)"));
    }

    // Verify the destination exists and is one this user may send backups to.
    //
    // The inner JOIN this replaces could never match. `backup_destinations.server_id`
    // is nullable by design — the migration that added it calls destinations
    // "shared" — and `create` inserts only (name, dtype, config), so the column is
    // NULL on every destination the panel has ever made. An inner join on a column
    // that is always NULL selects nothing, so this check answered 403 "Destination
    // not found or not owned by you" for every destination that existed, and the
    // per-site schedule form's Destination dropdown could not be used at all.
    //
    // A NULL server_id means unscoped: any user may target it. That is the model
    // destinations already have — they are created and deleted through admin-only
    // routes, so the operator choosing them is the same person who owns the bucket,
    // and a site owner can upload to it but never read it. A destination that IS
    // pinned to a server still has to belong to this user.
    let dest_check: Option<(Uuid,)> = sqlx::query_as(
        "SELECT bd.id FROM backup_destinations bd \
         LEFT JOIN servers s ON bd.server_id = s.id \
         WHERE bd.id = $1 AND (bd.server_id IS NULL OR s.user_id = $2)",
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
    let domain = get_site_domain(&state, id, &claims).await?;

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

#[derive(serde::Serialize)]
pub struct BackupSetupStatus {
    pub has_schedule: bool,
    pub has_backup: bool,
}

/// GET /api/backup-setup-status — Whether the current user has any backup schedule
/// or any stored backup (site, database, or volume). Used by the Dashboard onboarding
/// step "Set up backups" so a one-off manual backup or scheduled backup counts as done.
pub async fn setup_status(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<BackupSetupStatus>, ApiError> {
    let (schedule_count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM backup_schedules bs \
         JOIN sites s ON s.id = bs.site_id \
         WHERE s.user_id = $1",
    )
    .bind(claims.sub)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("setup-status schedules", e))?;

    let (site_bk,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM backups b \
         JOIN sites s ON s.id = b.site_id \
         WHERE s.user_id = $1",
    )
    .bind(claims.sub)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("setup-status backups", e))?;

    let (db_bk,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM database_backups")
        .fetch_one(&state.db)
        .await
        .map_err(|e| internal_error("setup-status db_backups", e))?;

    let (vol_bk,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM volume_backups")
        .fetch_one(&state.db)
        .await
        .map_err(|e| internal_error("setup-status volume_backups", e))?;

    Ok(Json(BackupSetupStatus {
        has_schedule: schedule_count > 0,
        has_backup: site_bk + db_bk + vol_bk > 0,
    }))
}
