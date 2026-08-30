use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::{AdminUser, AuthUser, ServerScope};
use crate::error::{internal_error, err, ApiError};
use crate::services::alert_runbook_defaults::DEFAULTS;
use crate::services::alert_runbooks::{get_runbook, list_runbooks, RunbookView};
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
    /// Phase 4 W3: actor who ack'd (NULL on legacy rows + un-ack'd alerts).
    acknowledged_by: Option<Uuid>,
    /// Resolved via LEFT JOIN users — surfaces email for the UI's "Ack by"
    /// column without forcing the frontend into N+1 lookups.
    acknowledged_by_email: Option<String>,
    /// Phase 4 W3: optional free-text note from the actor (500-char cap).
    acknowledged_comment: Option<String>,
    /// Phase 4 W3: position in the policy chain — 0 means initial fire.
    escalation_step_index: i32,
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

    // Build dynamic query — scoped to the caller, then to the ServerScope
    // server. NULL server_id is admitted: backup_failure, cron_failure,
    // security-scan and ssl_expiry alerts are fired with server_id = None (they
    // are site- or account-scoped), and a bare `server_id = $2` predicate hid
    // that entire class from the list and the summary permanently — including
    // criticals — while the bell, external channels and the escalation engine
    // all still paged for them. Deleting a server also NULLs the column
    // (ON DELETE SET NULL), which used to make its alert history vanish.
    // LEFT JOIN users resolves acknowledged_by → email for the UI without
    // forcing N+1 fetches from the frontend.
    let mut sql = String::from(
        "SELECT a.id, a.server_id, a.site_id, a.alert_type, a.severity, a.title, \
                a.message, a.status, a.notified_at, a.resolved_at, a.acknowledged_at, \
                a.acknowledged_by, u.email AS acknowledged_by_email, \
                a.acknowledged_comment, a.escalation_step_index, a.created_at \
         FROM alerts a \
         LEFT JOIN users u ON u.id = a.acknowledged_by \
         WHERE a.user_id = $1 AND (a.server_id = $2 OR a.server_id IS NULL)",
    );
    let mut param_idx = 3;

    if q.status.is_some() {
        sql.push_str(&format!(" AND a.status = ${param_idx}"));
        param_idx += 1;
    }
    if q.alert_type.is_some() {
        sql.push_str(&format!(" AND a.alert_type = ${param_idx}"));
        #[allow(unused_assignments)]
        { param_idx += 1; }
    }

    sql.push_str(&format!(" ORDER BY a.created_at DESC LIMIT {limit}"));

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
    // Same NULL-server_id admission as list() — the two must agree or the badge
    // count contradicts the rows underneath it.
    let counts: Vec<(String, i64)> = sqlx::query_as(
        "SELECT status, COUNT(*) FROM alerts \
         WHERE user_id = $1 AND (server_id = $2 OR server_id IS NULL) \
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

/// Phase 4 W3: optional payload for `acknowledge`. Empty body still works
/// — `Option<Json<…>>` returns `None` when no body / no content-type, so
/// older clients that PUT with no body keep functioning.
#[derive(serde::Deserialize, Default)]
pub struct AcknowledgePayload {
    #[serde(default)]
    pub comment: Option<String>,
}

/// PUT /api/alerts/{id}/acknowledge — Acknowledge an alert.
///
/// Phase 4 W3: stores the acting user (`claims.sub`) on the row plus an
/// optional 500-char comment. Older clients that PUT with no body still
/// work — the only state difference is `acknowledged_by` instead of NULL.
pub async fn acknowledge(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    body: Option<Json<AcknowledgePayload>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let payload = body.map(|j| j.0).unwrap_or_default();
    let comment = payload
        .comment
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if let Some(ref c) = comment {
        // Char count, not byte length — runs cap is "what the operator typed."
        if c.chars().count() > 500 {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "Comment cannot exceed 500 characters",
            ));
        }
    }

    let result = sqlx::query(
        "UPDATE alerts SET status = 'acknowledged', acknowledged_at = NOW(), \
                          acknowledged_by = $3, acknowledged_comment = $4 \
         WHERE id = $1 AND user_id = $2 AND status = 'firing'",
    )
    .bind(id)
    .bind(claims.sub)
    .bind(claims.sub)
    .bind(comment.as_deref())
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
    /// Phase 4 W3: escalation policy attached to this rule. NULL = pre-W3
    /// hardcoded cadence (15 min unack → 30 min re-page).
    escalation_policy_id: Option<Uuid>,
}

/// GET /api/alert-rules — Get user's alert rules.
pub async fn get_rules(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<Vec<AlertRuleRow>>, ApiError> {
    let mut rules: Vec<AlertRuleRow> = sqlx::query_as(
        "SELECT id, server_id, cpu_threshold, cpu_duration, memory_threshold, memory_duration, \
         disk_threshold, alert_cpu, alert_memory, alert_disk, alert_offline, \
         alert_backup_failure, alert_ssl_expiry, alert_service_health, \
         ssl_warning_days, notify_email, notify_slack_url, notify_discord_url, cooldown_minutes, \
         notify_pagerduty_key, notify_webhook_url, muted_types, \
         gpu_util_threshold, gpu_util_duration, gpu_temp_threshold, gpu_vram_threshold, alert_gpu, \
         escalation_policy_id \
         FROM alert_rules WHERE user_id = $1 ORDER BY server_id NULLS FIRST LIMIT 500",
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("get rules", e))?;

    // These four columns are encrypted at rest (encrypt_credential in
    // upsert_rules below); the caller reading their OWN rule row back to
    // pre-fill the settings form is the expected, same-account case
    // `decrypt_credential_or_legacy` exists for.
    for r in rules.iter_mut() {
        r.notify_slack_url = r.notify_slack_url.as_deref().map(|v| {
            crate::services::secrets_crypto::decrypt_credential_or_legacy(v, &state.config.jwt_secret)
        });
        r.notify_discord_url = r.notify_discord_url.as_deref().map(|v| {
            crate::services::secrets_crypto::decrypt_credential_or_legacy(v, &state.config.jwt_secret)
        });
        r.notify_pagerduty_key = r.notify_pagerduty_key.as_deref().map(|v| {
            crate::services::secrets_crypto::decrypt_credential_or_legacy(v, &state.config.jwt_secret)
        });
        r.notify_webhook_url = r.notify_webhook_url.as_deref().map(|v| {
            crate::services::secrets_crypto::decrypt_credential_or_legacy(v, &state.config.jwt_secret)
        });
    }

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

    // Encrypt at rest — validated above on PLAINTEXT (SSRF check needs a real
    // URL), encrypted here before it ever reaches SQL. `None` means "leave the
    // stored value alone" (the COALESCE below depends on that), `Some("")`
    // means "clear it", so both are passed through unencrypted; only a
    // genuine non-empty value is encrypted. Read back via
    // `decrypt_credential_or_legacy` in `get_rules` above.
    let jwt_secret = &state.config.jwt_secret;
    let encrypt_opt = |v: &Option<String>| -> Result<Option<String>, ApiError> {
        match v {
            None => Ok(None),
            Some(s) if s.is_empty() => Ok(Some(String::new())),
            Some(s) => crate::services::secrets_crypto::encrypt_credential(s, jwt_secret)
                .map(Some)
                .map_err(|e| internal_error("encrypt notification credential", e)),
        }
    };
    let enc_slack_url = encrypt_opt(&body.notify_slack_url)?;
    let enc_discord_url = encrypt_opt(&body.notify_discord_url)?;
    let enc_pagerduty_key = encrypt_opt(&body.notify_pagerduty_key)?;
    let enc_webhook_url = encrypt_opt(&body.notify_webhook_url)?;

    // A suppression list is matched token-for-token against the alert type on
    // the fan-out, so a token that names nothing suppresses nothing. Stored
    // unchecked it then echoed back on every read as though it had worked, and
    // the operator kept being paged by a type their own settings showed as off.
    // Reject it here, naming the token, rather than accept a setting that
    // cannot do what it says.
    if let Some(ref raw) = body.muted_types {
        let unknown = crate::services::notifications::unknown_suppressible_types(raw);
        if !unknown.is_empty() {
            return Err(err(
                StatusCode::BAD_REQUEST,
                &format!(
                    "Unknown alert type(s) in muted_types: {}. Suppressible types are: {}",
                    unknown.join(", "),
                    crate::services::notifications::SUPPRESSIBLE_ALERT_TYPES.join(", ")
                ),
            ));
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
    .bind(&enc_slack_url)
    .bind(&enc_discord_url)
    .bind(body.cooldown_minutes)
    .bind(&enc_pagerduty_key)
    .bind(&enc_webhook_url)
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

/// Phase 4 W3: payload for attaching/detaching an escalation policy.
/// `policy_id = null` clears the attachment (rule reverts to pre-W3
/// hardcoded cadence).
#[derive(serde::Deserialize)]
pub struct AttachPolicy {
    pub policy_id: Option<Uuid>,
}

/// PUT /api/alert-rules/{rule_id}/escalation-policy — Admin: attach or
/// detach an escalation policy on a single rule. Admin-only because
/// mis-attaching a policy can route 3 AM pages to the wrong on-call user.
pub async fn attach_escalation_policy(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Path(rule_id): Path<Uuid>,
    Json(body): Json<AttachPolicy>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // If setting (not clearing), verify the policy exists so the FK violation
    // surfaces as a 400 instead of a 500.
    if let Some(pid) = body.policy_id {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM escalation_policies WHERE id = $1)",
        )
        .bind(pid)
        .fetch_one(&state.db)
        .await
        .map_err(|e| internal_error("check policy exists", e))?;
        if !exists {
            return Err(err(StatusCode::BAD_REQUEST, "Policy not found"));
        }
    }
    let result = sqlx::query(
        "UPDATE alert_rules SET escalation_policy_id = $2, updated_at = NOW() WHERE id = $1",
    )
    .bind(rule_id)
    .bind(body.policy_id)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("attach policy", e))?;
    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Alert rule not found"));
    }
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

// ---------------------------------------------------------------------------
// Runbooks (Phase 4 W2)
//
// Markdown-per-alert-type, attached to fired alerts and rendered into
// notification payloads. Admin-only. Resolution is DB-row-then-default —
// fresh installs still produce useful payloads from the const slice without
// the operator having to click "Seed missing default runbooks" first.
// ---------------------------------------------------------------------------

const RUNBOOK_MD_MAX_BYTES: usize = 50 * 1024;

#[derive(serde::Deserialize)]
pub struct RunbookUpsert {
    pub runbook_md: String,
    pub severity_default: String,
}

#[derive(serde::Serialize)]
pub struct ApplyDefaultsResponse {
    pub inserted: Vec<String>,
    pub skipped: Vec<String>,
}

/// GET /api/alerts/runbooks — List every known runbook.
/// Returns the union of DB rows and compile-time defaults; defaults that have
/// no DB row yet are flagged `is_default = true`.
pub async fn list_runbooks_route(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<Vec<RunbookView>>, ApiError> {
    Ok(Json(list_runbooks(&state.db).await))
}

/// GET /api/alerts/runbooks/{alert_type} — Single runbook with `is_default` flag.
pub async fn get_runbook_route(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Path(alert_type): Path<String>,
) -> Result<Json<RunbookView>, ApiError> {
    match get_runbook(&state.db, &alert_type).await {
        Some(r) => Ok(Json(r)),
        None => Err(err(StatusCode::NOT_FOUND, "no runbook for alert_type")),
    }
}

/// PUT /api/alerts/runbooks/{alert_type} — Upsert. Sets updated_by from auth.
pub async fn put_runbook(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(alert_type): Path<String>,
    Json(body): Json<RunbookUpsert>,
) -> Result<Json<RunbookView>, ApiError> {
    if body.runbook_md.len() > RUNBOOK_MD_MAX_BYTES {
        return Err(err(
            StatusCode::PAYLOAD_TOO_LARGE,
            "runbook_md exceeds 50KB",
        ));
    }
    if !matches!(
        body.severity_default.as_str(),
        "info" | "warning" | "critical"
    ) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "severity_default must be info, warning, or critical",
        ));
    }
    if alert_type.is_empty() || alert_type.len() > 64 {
        return Err(err(StatusCode::BAD_REQUEST, "invalid alert_type"));
    }

    sqlx::query(
        "INSERT INTO alert_runbooks (alert_type, runbook_md, severity_default, updated_by, updated_at) \
         VALUES ($1, $2, $3, $4, NOW()) \
         ON CONFLICT (alert_type) DO UPDATE SET \
           runbook_md = EXCLUDED.runbook_md, \
           severity_default = EXCLUDED.severity_default, \
           updated_by = EXCLUDED.updated_by, \
           updated_at = NOW()",
    )
    .bind(&alert_type)
    .bind(&body.runbook_md)
    .bind(&body.severity_default)
    .bind(claims.sub)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("upsert runbook", e))?;

    match get_runbook(&state.db, &alert_type).await {
        Some(r) => Ok(Json(r)),
        None => Err(internal_error(
            "read-back runbook after upsert",
            "missing row",
        )),
    }
}

/// DELETE /api/alerts/runbooks/{alert_type} — Remove DB row → next read falls back to default.
pub async fn delete_runbook(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Path(alert_type): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let res = sqlx::query("DELETE FROM alert_runbooks WHERE alert_type = $1")
        .bind(&alert_type)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("delete runbook", e))?;

    Ok(Json(serde_json::json!({
        "deleted": res.rows_affected() > 0,
        "alert_type": alert_type,
    })))
}

/// POST /api/alerts/runbooks/apply-defaults — Insert-or-skip from const slice.
/// **Never overwrites operator edits** (ON CONFLICT DO NOTHING).
pub async fn apply_defaults(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<Json<ApplyDefaultsResponse>, ApiError> {
    let mut inserted = Vec::new();
    let mut skipped = Vec::new();

    for d in DEFAULTS {
        let res = sqlx::query(
            "INSERT INTO alert_runbooks (alert_type, runbook_md, severity_default, updated_by, updated_at) \
             VALUES ($1, $2, $3, $4, NOW()) \
             ON CONFLICT (alert_type) DO NOTHING",
        )
        .bind(d.alert_type)
        .bind(d.runbook_md)
        .bind(d.severity)
        .bind(claims.sub)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("apply runbook defaults", e))?;

        if res.rows_affected() == 1 {
            inserted.push(d.alert_type.to_string());
        } else {
            skipped.push(d.alert_type.to_string());
        }
    }

    Ok(Json(ApplyDefaultsResponse { inserted, skipped }))
}
