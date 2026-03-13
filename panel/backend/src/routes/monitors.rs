use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{err, ApiError};
use crate::AppState;

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct Monitor {
    pub id: Uuid,
    pub user_id: Uuid,
    pub site_id: Option<Uuid>,
    pub url: String,
    pub name: String,
    pub check_interval: i32,
    pub status: String,
    pub last_checked_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_response_time: Option<i32>,
    pub last_status_code: Option<i32>,
    pub enabled: bool,
    pub alert_email: bool,
    pub alert_slack_url: Option<String>,
    pub alert_discord_url: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Deserialize)]
pub struct CreateMonitor {
    pub url: String,
    pub name: String,
    pub site_id: Option<Uuid>,
    pub check_interval: Option<i32>,
    pub alert_email: Option<bool>,
    pub alert_slack_url: Option<String>,
    pub alert_discord_url: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct UpdateMonitor {
    pub name: Option<String>,
    pub url: Option<String>,
    pub check_interval: Option<i32>,
    pub enabled: Option<bool>,
    pub alert_email: Option<bool>,
    pub alert_slack_url: Option<String>,
    pub alert_discord_url: Option<String>,
}

/// GET /api/monitors — List user's monitors.
pub async fn list(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<Vec<Monitor>>, ApiError> {
    let monitors: Vec<Monitor> = sqlx::query_as(
        "SELECT * FROM monitors WHERE user_id = $1 ORDER BY created_at DESC",
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(monitors))
}

/// POST /api/monitors — Create a new monitor.
pub async fn create(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<CreateMonitor>,
) -> Result<(StatusCode, Json<Monitor>), ApiError> {
    let url = body.url.trim();
    if url.is_empty() || (!url.starts_with("http://") && !url.starts_with("https://")) {
        return Err(err(StatusCode::BAD_REQUEST, "URL must start with http:// or https://"));
    }

    let name = body.name.trim();
    if name.is_empty() || name.len() > 100 {
        return Err(err(StatusCode::BAD_REQUEST, "Name must be 1-100 characters"));
    }

    let interval = body.check_interval.unwrap_or(60).max(30).min(3600);

    // Limit monitors per user (50)
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM monitors WHERE user_id = $1")
        .bind(claims.sub)
        .fetch_one(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if count.0 >= 50 {
        return Err(err(StatusCode::BAD_REQUEST, "Monitor limit reached (50)"));
    }

    let monitor: Monitor = sqlx::query_as(
        "INSERT INTO monitors (user_id, site_id, url, name, check_interval, alert_email, alert_slack_url, alert_discord_url) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING *",
    )
    .bind(claims.sub)
    .bind(body.site_id)
    .bind(url)
    .bind(name)
    .bind(interval)
    .bind(body.alert_email.unwrap_or(true))
    .bind(&body.alert_slack_url)
    .bind(&body.alert_discord_url)
    .fetch_one(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok((StatusCode::CREATED, Json(monitor)))
}

/// PUT /api/monitors/{id} — Update a monitor.
pub async fn update(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateMonitor>,
) -> Result<Json<Monitor>, ApiError> {
    // Verify ownership
    let existing: Option<Monitor> = sqlx::query_as(
        "SELECT * FROM monitors WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if existing.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Monitor not found"));
    }

    let monitor: Monitor = sqlx::query_as(
        "UPDATE monitors SET \
         name = COALESCE($2, name), \
         url = COALESCE($3, url), \
         check_interval = COALESCE($4, check_interval), \
         enabled = COALESCE($5, enabled), \
         alert_email = COALESCE($6, alert_email), \
         alert_slack_url = COALESCE($7, alert_slack_url), \
         alert_discord_url = COALESCE($8, alert_discord_url) \
         WHERE id = $1 RETURNING *",
    )
    .bind(id)
    .bind(&body.name)
    .bind(&body.url)
    .bind(body.check_interval.map(|i| i.max(30).min(3600)))
    .bind(body.enabled)
    .bind(body.alert_email)
    .bind(&body.alert_slack_url)
    .bind(&body.alert_discord_url)
    .fetch_one(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(monitor))
}

/// DELETE /api/monitors/{id} — Delete a monitor.
pub async fn remove(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = sqlx::query("DELETE FROM monitors WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(claims.sub)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Monitor not found"));
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct CheckRecord {
    pub id: Uuid,
    pub status_code: Option<i32>,
    pub response_time: Option<i32>,
    pub error: Option<String>,
    pub checked_at: chrono::DateTime<chrono::Utc>,
}

/// GET /api/monitors/{id}/checks — Get recent check history.
pub async fn checks(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<CheckRecord>>, ApiError> {
    // Verify ownership
    let exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM monitors WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if exists.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Monitor not found"));
    }

    let records: Vec<CheckRecord> = sqlx::query_as(
        "SELECT id, status_code, response_time, error, checked_at \
         FROM monitor_checks WHERE monitor_id = $1 ORDER BY checked_at DESC LIMIT 100",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(records))
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct Incident {
    pub id: Uuid,
    pub monitor_id: Uuid,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub resolved_at: Option<chrono::DateTime<chrono::Utc>>,
    pub cause: Option<String>,
}

/// GET /api/monitors/{id}/incidents — Get incident history.
pub async fn incidents(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<Incident>>, ApiError> {
    let exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM monitors WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if exists.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Monitor not found"));
    }

    let records: Vec<Incident> = sqlx::query_as(
        "SELECT id, monitor_id, started_at, resolved_at, cause \
         FROM incidents WHERE monitor_id = $1 ORDER BY started_at DESC LIMIT 50",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(records))
}
