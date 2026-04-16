use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::{AuthUser, ServerScope};
use crate::error::{internal_error, err, ApiError};
use crate::AppState;

#[derive(serde::Deserialize)]
pub struct AlertQuery {
    pub status: Option<String>,
    pub alert_type: Option<String>,
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
    ServerScope(server_id, _agent): ServerScope,
    Query(q): Query<AlertQuery>,
) -> Result<Json<Vec<AlertRow>>, ApiError> {
    let limit = q.limit.unwrap_or(100).min(500);

    // Build dynamic query — server_id always filtered via ServerScope
    let mut sql = String::from(
        "SELECT id, server_id, site_id, alert_type, severity, title, message, \
         status, notified_at, resolved_at, acknowledged_at, created_at \
         FROM alerts WHERE user_id = $1 AND server_id = $2",
    );
    let mut param_idx = 3;

    if q.status.is_some() {
        sql.push_str(&format!(" AND status = ${param_idx}"));
        param_idx += 1;
    }
    if q.alert_type.is_some() {
        sql.push_str(&format!(" AND alert_type = ${param_idx}"));
        #[allow(unused_assignments)]
        { param_idx += 1; }
    }

    sql.push_str(&format!(" ORDER BY created_at DESC LIMIT {limit}"));

    let mut query = sqlx::query_as::<_, AlertRow>(&sql)
        .bind(claims.sub)
        .bind(server_id);

    if let Some(ref status) = q.status {
        query = query.bind(status);
    }
    if let Some(ref alert_type) = q.alert_type {
        query = query.bind(alert_type);
    }

    let alerts = query
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("list alerts", e))?;

    Ok(Json(alerts))
}

/// GET /api/alerts/summary — Count of alerts by status.
pub async fn summary(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(server_id, _agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let counts: Vec<(String, i64)> = sqlx::query_as(
        "SELECT status, COUNT(*) FROM alerts WHERE user_id = $1 AND server_id = $2 \
         AND created_at > NOW() - INTERVAL '30 days' \
         GROUP BY status",
    )
    .bind(claims.sub)
    .bind(server_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("summary", e))?;

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
    .map_err(|e| internal_error("acknowledge", e))?;

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
    .map_err(|e| internal_error("resolve", e))?;

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
    notify_pagerduty_key: Option<String>,
    notify_webhook_url: Option<String>,
    /// Comma-separated alert types to suppress from external channels (Gap #69)
    muted_types: String,
    // GPU alert thresholds (Phase 2 #2)
    gpu_util_threshold: i32,
    gpu_util_duration: i32,
    gpu_temp_threshold: i32,
    gpu_vram_threshold: i32,
    alert_gpu: bool,
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
         ssl_warning_days, notify_email, notify_slack_url, notify_discord_url, cooldown_minutes, \
         notify_pagerduty_key, notify_webhook_url, muted_types, \
         gpu_util_threshold, gpu_util_duration, gpu_temp_threshold, gpu_vram_threshold, alert_gpu \
         FROM alert_rules WHERE user_id = $1 ORDER BY server_id NULLS FIRST LIMIT 500",
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("get rules", e))?;

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
    pub notify_pagerduty_key: Option<String>,
    pub notify_webhook_url: Option<String>,
    /// Comma-separated alert types to suppress from external channels (Gap #69)
    pub muted_types: Option<String>,
    // GPU alert thresholds (Phase 2 #2)
    pub gpu_util_threshold: Option<i32>,
    pub gpu_util_duration: Option<i32>,
    pub gpu_temp_threshold: Option<i32>,
    pub gpu_vram_threshold: Option<i32>,
    pub alert_gpu: Option<bool>,
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
    // SSRF protection: validate notification webhook URLs
    if let Some(ref url) = body.notify_webhook_url {
        if !url.is_empty() {
            if let Err(e) = crate::helpers::validate_url_not_internal(url).await {
                return Err(err(StatusCode::BAD_REQUEST, &format!("Invalid webhook URL: {}", e)));
            }
        }
    }
    if let Some(ref url) = body.notify_slack_url {
        if !url.is_empty() {
            if let Err(e) = crate::helpers::validate_url_not_internal(url).await {
                return Err(err(StatusCode::BAD_REQUEST, &format!("Invalid Slack URL: {}", e)));
            }
        }
    }
    if let Some(ref url) = body.notify_discord_url {
        if !url.is_empty() {
            if let Err(e) = crate::helpers::validate_url_not_internal(url).await {
                return Err(err(StatusCode::BAD_REQUEST, &format!("Invalid Discord URL: {}", e)));
            }
        }
    }

    // Check if rule exists (partial unique indexes don't work with ON CONFLICT)
    let existing: Option<(Uuid,)> = if server_id.is_some() {
        sqlx::query_as("SELECT id FROM alert_rules WHERE user_id = $1 AND server_id = $2")
            .bind(user_id).bind(server_id)
            .fetch_optional(&state.db).await
            .map_err(|e| internal_error("check alert rule exists", e))?
    } else {
        sqlx::query_as("SELECT id FROM alert_rules WHERE user_id = $1 AND server_id IS NULL")
            .bind(user_id)
            .fetch_optional(&state.db).await
            .map_err(|e| internal_error("check alert rule exists", e))?
    };

    let query = if existing.is_some() {
        let where_clause = if server_id.is_some() {
            "WHERE user_id = $1 AND server_id = $2"
        } else {
            "WHERE user_id = $1 AND server_id IS NULL"
        };
        format!(
            "UPDATE alert_rules SET \
             cpu_threshold = COALESCE($3, cpu_threshold), \
             cpu_duration = COALESCE($4, cpu_duration), \
             memory_threshold = COALESCE($5, memory_threshold), \
             memory_duration = COALESCE($6, memory_duration), \
             disk_threshold = COALESCE($7, disk_threshold), \
             alert_cpu = COALESCE($8, alert_cpu), \
             alert_memory = COALESCE($9, alert_memory), \
             alert_disk = COALESCE($10, alert_disk), \
             alert_offline = COALESCE($11, alert_offline), \
             alert_backup_failure = COALESCE($12, alert_backup_failure), \
             alert_ssl_expiry = COALESCE($13, alert_ssl_expiry), \
             alert_service_health = COALESCE($14, alert_service_health), \
             ssl_warning_days = COALESCE($15, ssl_warning_days), \
             notify_email = COALESCE($16, notify_email), \
             notify_slack_url = COALESCE($17, notify_slack_url), \
             notify_discord_url = COALESCE($18, notify_discord_url), \
             cooldown_minutes = COALESCE($19, cooldown_minutes), \
             notify_pagerduty_key = COALESCE($20, notify_pagerduty_key), \
             notify_webhook_url = COALESCE($21, notify_webhook_url), \
             muted_types = COALESCE($22, muted_types), \
             gpu_util_threshold = COALESCE($23, gpu_util_threshold), \
             gpu_util_duration = COALESCE($24, gpu_util_duration), \
             gpu_temp_threshold = COALESCE($25, gpu_temp_threshold), \
             gpu_vram_threshold = COALESCE($26, gpu_vram_threshold), \
             alert_gpu = COALESCE($27, alert_gpu), \
             updated_at = NOW() \
             {where_clause}"
        )
    } else {
        "INSERT INTO alert_rules (user_id, server_id, \
         cpu_threshold, cpu_duration, memory_threshold, memory_duration, disk_threshold, \
         alert_cpu, alert_memory, alert_disk, alert_offline, alert_backup_failure, \
         alert_ssl_expiry, alert_service_health, ssl_warning_days, \
         notify_email, notify_slack_url, notify_discord_url, cooldown_minutes, \
         notify_pagerduty_key, notify_webhook_url, muted_types, \
         gpu_util_threshold, gpu_util_duration, gpu_temp_threshold, gpu_vram_threshold, alert_gpu) \
         VALUES ($1, $2, \
         COALESCE($3, 90), COALESCE($4, 5), COALESCE($5, 90), COALESCE($6, 5), COALESCE($7, 85), \
         COALESCE($8, TRUE), COALESCE($9, TRUE), COALESCE($10, TRUE), COALESCE($11, TRUE), \
         COALESCE($12, TRUE), COALESCE($13, TRUE), COALESCE($14, TRUE), COALESCE($15, '30,14,7,3,1'), \
         COALESCE($16, TRUE), $17, $18, COALESCE($19, 60), $20, $21, COALESCE($22, ''), \
         COALESCE($23, 95), COALESCE($24, 5), COALESCE($25, 85), COALESCE($26, 95), COALESCE($27, TRUE))".to_string()
    };

    sqlx::query(&query)
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
    .bind(&body.notify_pagerduty_key)
    .bind(&body.notify_webhook_url)
    .bind(&body.muted_types)
    .bind(body.gpu_util_threshold)
    .bind(body.gpu_util_duration)
    .bind(body.gpu_temp_threshold)
    .bind(body.gpu_vram_threshold)
    .bind(body.alert_gpu)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("update server rules", e))?;

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
    .map_err(|e| internal_error("delete server rules", e))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}
