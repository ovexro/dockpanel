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
    pub monitor_type: String,
    pub port: Option<i32>,
    pub keyword: Option<String>,
    pub keyword_must_contain: bool,
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
    pub monitor_type: Option<String>,
    pub port: Option<i32>,
    pub keyword: Option<String>,
    pub keyword_must_contain: Option<bool>,
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
    pub monitor_type: Option<String>,
    pub port: Option<i32>,
    pub keyword: Option<String>,
    pub keyword_must_contain: Option<bool>,
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
    let monitor_type = body.monitor_type.as_deref().unwrap_or("http");
    if monitor_type != "http" && monitor_type != "tcp" {
        return Err(err(StatusCode::BAD_REQUEST, "monitor_type must be 'http' or 'tcp'"));
    }

    let url = body.url.trim();
    if monitor_type == "http" {
        if url.is_empty() || (!url.starts_with("http://") && !url.starts_with("https://")) {
            return Err(err(StatusCode::BAD_REQUEST, "URL must start with http:// or https://"));
        }
    } else if url.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Host is required for TCP monitors"));
    }

    let name = body.name.trim();
    if name.is_empty() || name.len() > 100 {
        return Err(err(StatusCode::BAD_REQUEST, "Name must be 1-100 characters"));
    }

    let interval = body.check_interval.unwrap_or(60).max(30).min(3600);

    // Inherit alert URLs from global alert rules if not provided
    let mut slack_url = body.alert_slack_url.clone();
    let mut discord_url = body.alert_discord_url.clone();

    if slack_url.as_ref().map_or(true, |s| s.is_empty())
        || discord_url.as_ref().map_or(true, |s| s.is_empty())
    {
        let global: Option<(Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT notify_slack_url, notify_discord_url FROM alert_rules WHERE user_id = $1 AND server_id IS NULL LIMIT 1",
        )
        .bind(claims.sub)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();

        if let Some((global_slack, global_discord)) = global {
            if slack_url.as_ref().map_or(true, |s| s.is_empty()) {
                slack_url = global_slack;
            }
            if discord_url.as_ref().map_or(true, |s| s.is_empty()) {
                discord_url = global_discord;
            }
        }
    }

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
        "INSERT INTO monitors (user_id, site_id, url, name, check_interval, alert_email, alert_slack_url, alert_discord_url, monitor_type, port, keyword, keyword_must_contain) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) RETURNING *",
    )
    .bind(claims.sub)
    .bind(body.site_id)
    .bind(url)
    .bind(name)
    .bind(interval)
    .bind(body.alert_email.unwrap_or(true))
    .bind(&slack_url)
    .bind(&discord_url)
    .bind(monitor_type)
    .bind(body.port)
    .bind(&body.keyword)
    .bind(body.keyword_must_contain.unwrap_or(true))
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
         alert_discord_url = COALESCE($8, alert_discord_url), \
         monitor_type = COALESCE($9, monitor_type), \
         port = COALESCE($10, port), \
         keyword = COALESCE($11, keyword), \
         keyword_must_contain = COALESCE($12, keyword_must_contain) \
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
    .bind(&body.monitor_type)
    .bind(body.port)
    .bind(&body.keyword)
    .bind(body.keyword_must_contain)
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

/// GET /api/monitors/{id}/uptime — Calculate uptime percentage.
pub async fn uptime_stats(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
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

    // Successful check: HTTP 200-499 or TCP status_code=0 (no error means success)
    // 24h uptime
    let day: Option<(i64, i64)> = sqlx::query_as(
        "SELECT COUNT(*) FILTER (WHERE status_code IS NOT NULL AND error IS NULL), COUNT(*) \
         FROM monitor_checks WHERE monitor_id = $1 AND checked_at > NOW() - INTERVAL '24 hours'"
    ).bind(id).fetch_optional(&state.db).await.ok().flatten();

    // 7d uptime
    let week: Option<(i64, i64)> = sqlx::query_as(
        "SELECT COUNT(*) FILTER (WHERE status_code IS NOT NULL AND error IS NULL), COUNT(*) \
         FROM monitor_checks WHERE monitor_id = $1 AND checked_at > NOW() - INTERVAL '7 days'"
    ).bind(id).fetch_optional(&state.db).await.ok().flatten();

    // 30d uptime
    let month: Option<(i64, i64)> = sqlx::query_as(
        "SELECT COUNT(*) FILTER (WHERE status_code IS NOT NULL AND error IS NULL), COUNT(*) \
         FROM monitor_checks WHERE monitor_id = $1 AND checked_at > NOW() - INTERVAL '30 days'"
    ).bind(id).fetch_optional(&state.db).await.ok().flatten();

    let calc = |data: Option<(i64, i64)>| -> f64 {
        match data {
            Some((up, total)) if total > 0 => (up as f64 / total as f64 * 10000.0).round() / 100.0,
            _ => 100.0,
        }
    };

    // Average response time (24h)
    let avg_rt: Option<(Option<f64>,)> = sqlx::query_as(
        "SELECT AVG(response_time)::float8 FROM monitor_checks WHERE monitor_id = $1 AND checked_at > NOW() - INTERVAL '24 hours' AND status_code IS NOT NULL"
    ).bind(id).fetch_optional(&state.db).await.ok().flatten();

    Ok(Json(serde_json::json!({
        "uptime_24h": calc(day),
        "uptime_7d": calc(week),
        "uptime_30d": calc(month),
        "avg_response_ms": avg_rt.and_then(|r| r.0).unwrap_or(0.0).round() as i32,
    })))
}

/// GET /api/monitors/{id}/chart — Get response time history for charting.
pub async fn response_chart(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
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

    let points: Vec<(i32, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT response_time, checked_at FROM monitor_checks \
         WHERE monitor_id = $1 AND checked_at > NOW() - INTERVAL '24 hours' AND status_code IS NOT NULL \
         ORDER BY checked_at ASC"
    ).bind(id).fetch_all(&state.db).await.unwrap_or_default();

    let data: Vec<serde_json::Value> = points.iter().map(|(rt, time)| {
        serde_json::json!({ "time": time.timestamp(), "ms": rt })
    }).collect();

    Ok(Json(serde_json::json!({ "points": data })))
}

/// POST /api/monitors/{id}/check — Force an immediate check.
pub async fn force_check(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Verify ownership
    let result = sqlx::query(
        "UPDATE monitors SET last_checked_at = NOW() - INTERVAL '1 hour' WHERE id = $1 AND user_id = $2"
    )
    .bind(id)
    .bind(claims.sub)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Monitor not found"));
    }

    Ok(Json(serde_json::json!({ "ok": true, "message": "Check will run within 60 seconds" })))
}

/// GET /api/status-page — Public status page data (no auth required).
pub async fn status_page(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Check if status page is enabled
    let enabled: Option<(String,)> = sqlx::query_as(
        "SELECT value FROM settings WHERE key = 'status_page_enabled'"
    ).fetch_optional(&state.db).await.ok().flatten();

    if enabled.map(|(v,)| v).unwrap_or_else(|| "false".to_string()) != "true" {
        return Err(err(StatusCode::NOT_FOUND, "Status page not enabled"));
    }

    // Get all enabled monitors (no user filter — this is public)
    let monitors: Vec<(String, String, String, Option<i32>, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT name, url, status, last_response_time, last_checked_at FROM monitors WHERE enabled = true ORDER BY name"
    ).fetch_all(&state.db).await.unwrap_or_default();

    let items: Vec<serde_json::Value> = monitors.iter().map(|(name, _url, status, rt, checked)| {
        serde_json::json!({
            "name": name,
            "status": status,
            "response_time": rt,
            "last_checked": checked,
        })
    }).collect();

    let all_up = items.iter().all(|i| i["status"] == "up");

    Ok(Json(serde_json::json!({
        "status": if all_up { "operational" } else { "degraded" },
        "monitors": items,
        "updated_at": chrono::Utc::now(),
    })))
}
