use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{err, ApiError};
use crate::AppState;

#[derive(serde::Deserialize)]
pub struct AlertQuery {
    pub status: Option<String>,
    pub alert_type: Option<String>,
    pub server_id: Option<String>,
    pub limit: Option<i64>,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct AlertRow {
    id: Uuid,
    server_id: Option<Uuid>,
    site_id: Option<Uuid>,
    alert_type: String,
    severity: String,
    title: String,
    message: String,
    status: String,
    notified_at: chrono::DateTime<chrono::Utc>,
    resolved_at: Option<chrono::DateTime<chrono::Utc>>,
    acknowledged_at: Option<chrono::DateTime<chrono::Utc>>,
    created_at: chrono::DateTime<chrono::Utc>,
}

/// GET /api/alerts — List alerts with optional filters.
pub async fn list(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Query(q): Query<AlertQuery>,
) -> Result<Json<Vec<AlertRow>>, ApiError> {
    let limit = q.limit.unwrap_or(100).min(500);

    // Build dynamic query
    let mut sql = String::from(
        "SELECT id, server_id, site_id, alert_type, severity, title, message, \
         status, notified_at, resolved_at, acknowledged_at, created_at \
         FROM alerts WHERE user_id = $1",
    );
    let mut param_idx = 2;

    if q.status.is_some() {
        sql.push_str(&format!(" AND status = ${param_idx}"));
        param_idx += 1;
    }
    if q.alert_type.is_some() {
        sql.push_str(&format!(" AND alert_type = ${param_idx}"));
        param_idx += 1;
    }
    if q.server_id.is_some() {
        sql.push_str(&format!(" AND server_id = ${param_idx}"));
        // param_idx += 1;
    }

    sql.push_str(&format!(" ORDER BY created_at DESC LIMIT {limit}"));

    let mut query = sqlx::query_as::<_, AlertRow>(&sql).bind(claims.sub);

    if let Some(ref status) = q.status {
        query = query.bind(status);
    }
    if let Some(ref alert_type) = q.alert_type {
        query = query.bind(alert_type);
    }
    if let Some(ref server_id) = q.server_id {
        let sid: Uuid = server_id
            .parse()
            .map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid server_id"))?;
        query = query.bind(sid);
    }

    let alerts = query
        .fetch_all(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(alerts))
}

/// GET /api/alerts/summary — Count of alerts by status.
pub async fn summary(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let counts: Vec<(String, i64)> = sqlx::query_as(
        "SELECT status, COUNT(*) FROM alerts WHERE user_id = $1 \
         AND created_at > NOW() - INTERVAL '30 days' \
         GROUP BY status",
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let firing = counts
        .iter()
        .find(|(s, _)| s == "firing")
        .map(|(_, c)| *c)
        .unwrap_or(0);
    let acknowledged = counts
        .iter()
        .find(|(s, _)| s == "acknowledged")
        .map(|(_, c)| *c)
        .unwrap_or(0);
    let resolved = counts
        .iter()
        .find(|(s, _)| s == "resolved")
        .map(|(_, c)| *c)
        .unwrap_or(0);

    Ok(Json(serde_json::json!({
        "firing": firing,
        "acknowledged": acknowledged,
        "resolved": resolved,
    })))
}

/// PUT /api/alerts/{id}/acknowledge — Acknowledge an alert.
pub async fn acknowledge(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = sqlx::query(
        "UPDATE alerts SET status = 'acknowledged', acknowledged_at = NOW() \
         WHERE id = $1 AND user_id = $2 AND status = 'firing'",
    )
    .bind(id)
    .bind(claims.sub)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Alert not found or already handled"));
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// PUT /api/alerts/{id}/resolve — Manually resolve an alert.
pub async fn resolve(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = sqlx::query(
        "UPDATE alerts SET status = 'resolved', resolved_at = NOW() \
         WHERE id = $1 AND user_id = $2 AND status IN ('firing', 'acknowledged')",
    )
    .bind(id)
    .bind(claims.sub)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Alert not found or already resolved"));
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct AlertRuleRow {
    id: Uuid,
    server_id: Option<Uuid>,
    cpu_threshold: i32,
    cpu_duration: i32,
    memory_threshold: i32,
    memory_duration: i32,
    disk_threshold: i32,
    alert_cpu: bool,
    alert_memory: bool,
    alert_disk: bool,
    alert_offline: bool,
    alert_backup_failure: bool,
    alert_ssl_expiry: bool,
    alert_service_health: bool,
    ssl_warning_days: String,
    notify_email: bool,
    notify_slack_url: Option<String>,
    notify_discord_url: Option<String>,
    cooldown_minutes: i32,
}

/// GET /api/alert-rules — Get user's alert rules.
pub async fn get_rules(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<Vec<AlertRuleRow>>, ApiError> {
    let rules: Vec<AlertRuleRow> = sqlx::query_as(
        "SELECT id, server_id, cpu_threshold, cpu_duration, memory_threshold, memory_duration, \
         disk_threshold, alert_cpu, alert_memory, alert_disk, alert_offline, \
         alert_backup_failure, alert_ssl_expiry, alert_service_health, \
         ssl_warning_days, notify_email, notify_slack_url, notify_discord_url, cooldown_minutes \
         FROM alert_rules WHERE user_id = $1 ORDER BY server_id NULLS FIRST",
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(rules))
}

#[derive(serde::Deserialize)]
pub struct UpdateRules {
    pub cpu_threshold: Option<i32>,
    pub cpu_duration: Option<i32>,
    pub memory_threshold: Option<i32>,
    pub memory_duration: Option<i32>,
    pub disk_threshold: Option<i32>,
    pub alert_cpu: Option<bool>,
    pub alert_memory: Option<bool>,
    pub alert_disk: Option<bool>,
    pub alert_offline: Option<bool>,
    pub alert_backup_failure: Option<bool>,
    pub alert_ssl_expiry: Option<bool>,
    pub alert_service_health: Option<bool>,
    pub ssl_warning_days: Option<String>,
    pub notify_email: Option<bool>,
    pub notify_slack_url: Option<String>,
    pub notify_discord_url: Option<String>,
    pub cooldown_minutes: Option<i32>,
}

/// PUT /api/alert-rules — Create or update global alert rules.
pub async fn update_rules(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<UpdateRules>,
) -> Result<Json<serde_json::Value>, ApiError> {
    upsert_rules(&state, claims.sub, None, &body).await
}

/// PUT /api/alert-rules/{server_id} — Create or update per-server alert rules.
pub async fn update_server_rules(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(server_id): Path<Uuid>,
    Json(body): Json<UpdateRules>,
) -> Result<Json<serde_json::Value>, ApiError> {
    upsert_rules(&state, claims.sub, Some(server_id), &body).await
}

async fn upsert_rules(
    state: &AppState,
    user_id: Uuid,
    server_id: Option<Uuid>,
    body: &UpdateRules,
) -> Result<Json<serde_json::Value>, ApiError> {
    sqlx::query(
        "INSERT INTO alert_rules (user_id, server_id, \
         cpu_threshold, cpu_duration, memory_threshold, memory_duration, disk_threshold, \
         alert_cpu, alert_memory, alert_disk, alert_offline, alert_backup_failure, \
         alert_ssl_expiry, alert_service_health, ssl_warning_days, \
         notify_email, notify_slack_url, notify_discord_url, cooldown_minutes) \
         VALUES ($1, $2, \
         COALESCE($3, 90), COALESCE($4, 5), COALESCE($5, 90), COALESCE($6, 5), COALESCE($7, 85), \
         COALESCE($8, TRUE), COALESCE($9, TRUE), COALESCE($10, TRUE), COALESCE($11, TRUE), \
         COALESCE($12, TRUE), COALESCE($13, TRUE), COALESCE($14, TRUE), COALESCE($15, '30,14,7,3,1'), \
         COALESCE($16, TRUE), $17, $18, COALESCE($19, 60)) \
         ON CONFLICT (user_id, server_id) DO UPDATE SET \
         cpu_threshold = COALESCE($3, alert_rules.cpu_threshold), \
         cpu_duration = COALESCE($4, alert_rules.cpu_duration), \
         memory_threshold = COALESCE($5, alert_rules.memory_threshold), \
         memory_duration = COALESCE($6, alert_rules.memory_duration), \
         disk_threshold = COALESCE($7, alert_rules.disk_threshold), \
         alert_cpu = COALESCE($8, alert_rules.alert_cpu), \
         alert_memory = COALESCE($9, alert_rules.alert_memory), \
         alert_disk = COALESCE($10, alert_rules.alert_disk), \
         alert_offline = COALESCE($11, alert_rules.alert_offline), \
         alert_backup_failure = COALESCE($12, alert_rules.alert_backup_failure), \
         alert_ssl_expiry = COALESCE($13, alert_rules.alert_ssl_expiry), \
         alert_service_health = COALESCE($14, alert_rules.alert_service_health), \
         ssl_warning_days = COALESCE($15, alert_rules.ssl_warning_days), \
         notify_email = COALESCE($16, alert_rules.notify_email), \
         notify_slack_url = COALESCE($17, alert_rules.notify_slack_url), \
         notify_discord_url = COALESCE($18, alert_rules.notify_discord_url), \
         cooldown_minutes = COALESCE($19, alert_rules.cooldown_minutes), \
         updated_at = NOW()",
    )
    .bind(user_id)
    .bind(server_id)
    .bind(body.cpu_threshold)
    .bind(body.cpu_duration)
    .bind(body.memory_threshold)
    .bind(body.memory_duration)
    .bind(body.disk_threshold)
    .bind(body.alert_cpu)
    .bind(body.alert_memory)
    .bind(body.alert_disk)
    .bind(body.alert_offline)
    .bind(body.alert_backup_failure)
    .bind(body.alert_ssl_expiry)
    .bind(body.alert_service_health)
    .bind(&body.ssl_warning_days)
    .bind(body.notify_email)
    .bind(&body.notify_slack_url)
    .bind(&body.notify_discord_url)
    .bind(body.cooldown_minutes)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// DELETE /api/alert-rules/{server_id} — Remove server-specific override.
pub async fn delete_server_rules(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(server_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    sqlx::query(
        "DELETE FROM alert_rules WHERE user_id = $1 AND server_id = $2",
    )
    .bind(claims.sub)
    .bind(server_id)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}
