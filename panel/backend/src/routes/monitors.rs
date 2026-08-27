use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::{AdminUser, AuthUser, ServerScope};
use crate::error::{internal_error, err, paginate, ApiError};
use crate::AppState;

#[derive(serde::Deserialize)]
pub struct PaginationQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

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
    pub custom_headers: Option<serde_json::Value>,
    pub heartbeat_token: Option<String>,
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
    pub custom_headers: Option<serde_json::Value>,
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
    pub custom_headers: Option<serde_json::Value>,
}

/// GET /api/monitors — List user's monitors.
pub async fn list(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Query(params): Query<PaginationQuery>,
) -> Result<Json<Vec<Monitor>>, ApiError> {
    let (limit, offset) = paginate(params.limit, params.offset);

    let monitors: Vec<Monitor> = sqlx::query_as(
        "SELECT * FROM monitors WHERE user_id = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3",
    )
    .bind(claims.sub)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("list monitors", e))?;

    Ok(Json(monitors))
}

/// POST /api/monitors — Create a new monitor.
pub async fn create(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<CreateMonitor>,
) -> Result<(StatusCode, Json<Monitor>), ApiError> {
    let monitor_type = body.monitor_type.as_deref().unwrap_or("http");
    if !matches!(monitor_type, "http" | "tcp" | "ping" | "heartbeat") {
        return Err(err(StatusCode::BAD_REQUEST, "monitor_type must be 'http', 'tcp', 'ping', or 'heartbeat'"));
    }

    let url = body.url.trim();
    match monitor_type {
        "http" => {
            if url.is_empty() || (!url.starts_with("http://") && !url.starts_with("https://")) {
                return Err(err(StatusCode::BAD_REQUEST, "URL must start with http:// or https://"));
            }
            // SSRF protection: block internal URLs
            if let Err(e) = crate::helpers::validate_url_not_internal(url).await {
                return Err(err(StatusCode::BAD_REQUEST, &format!("Invalid monitor URL: {}", e)));
            }
        }
        "tcp" | "ping" => {
            // Same SSRF boundary as the HTTP lane, on the bare host these store: a tcp
            // monitor to 127.0.0.1:22 is an internal port probe, not a public check.
            if url.is_empty() {
                return Err(err(StatusCode::BAD_REQUEST, "Host/URL is required"));
            }
            let host = url.trim_start_matches("tcp://").trim_start_matches("ping://");
            let probe_port = body.port.unwrap_or(80).clamp(0, 65535) as u16;
            if let Err(e) = crate::helpers::validate_host_not_internal(host, probe_port).await {
                return Err(err(StatusCode::BAD_REQUEST, &format!("Invalid monitor host: {e}")));
            }
        }
        _ => {
            // heartbeat: passive, but still needs an identifier.
            if url.is_empty() {
                return Err(err(StatusCode::BAD_REQUEST, "Host/URL is required"));
            }
        }
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

    // SSRF protection: per-monitor alert webhook URLs must not target internal
    // addresses (parity with alerts.rs::upsert_rules — body values are otherwise
    // unvalidated; inherited-from-global values were already vetted but are cheap to re-check).
    if let Some(ref u) = slack_url {
        if !u.is_empty() {
            if let Err(e) = crate::helpers::validate_url_not_internal(u).await {
                return Err(err(StatusCode::BAD_REQUEST, &format!("Invalid Slack alert URL: {e}")));
            }
        }
    }
    if let Some(ref u) = discord_url {
        if !u.is_empty() {
            if let Err(e) = crate::helpers::validate_url_not_internal(u).await {
                return Err(err(StatusCode::BAD_REQUEST, &format!("Invalid Discord alert URL: {e}")));
            }
        }
    }

    // Limit monitors per user (50)
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM monitors WHERE user_id = $1")
        .bind(claims.sub)
        .fetch_one(&state.db)
        .await
        .map_err(|e| internal_error("create monitors", e))?;

    if count.0 >= 50 {
        return Err(err(StatusCode::BAD_REQUEST, "Monitor limit reached (50)"));
    }

    let monitor: Monitor = sqlx::query_as(
        "INSERT INTO monitors (user_id, site_id, url, name, check_interval, alert_email, alert_slack_url, alert_discord_url, monitor_type, port, keyword, keyword_must_contain, custom_headers) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) RETURNING *",
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
    .bind(&body.custom_headers)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("create monitors", e))?;

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
    .map_err(|e| internal_error("update monitors", e))?;

    let existing = match existing {
        Some(m) => m,
        None => return Err(err(StatusCode::NOT_FOUND, "Monitor not found")),
    };

    // A type change must be to a known type — create checks this, update did not, so a
    // monitor could be moved to an arbitrary string and fall through the check dispatcher.
    if let Some(ref mt) = body.monitor_type {
        if !matches!(mt.as_str(), "http" | "tcp" | "ping" | "heartbeat") {
            return Err(err(StatusCode::BAD_REQUEST, "monitor_type must be 'http', 'tcp', 'ping', or 'heartbeat'"));
        }
    }

    // SSRF protection: validate the target if the URL/host is being updated, against the
    // EFFECTIVE type after this update (the new type if one is supplied, else the stored
    // one) — otherwise switching http→tcp, or setting a host on a tcp monitor, skips the
    // guard the create path enforces.
    let effective_type = body
        .monitor_type
        .as_deref()
        .unwrap_or(existing.monitor_type.as_str());
    if let Some(ref new_url) = body.url {
        let trimmed = new_url.trim();
        match effective_type {
            "http" => {
                if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
                    if let Err(e) = crate::helpers::validate_url_not_internal(trimmed).await {
                        return Err(err(StatusCode::BAD_REQUEST, &format!("Invalid monitor URL: {}", e)));
                    }
                }
            }
            "tcp" | "ping" => {
                if !trimmed.is_empty() {
                    let host = trimmed.trim_start_matches("tcp://").trim_start_matches("ping://");
                    let probe_port = body
                        .port
                        .or(existing.port)
                        .unwrap_or(80)
                        .clamp(0, 65535) as u16;
                    if let Err(e) = crate::helpers::validate_host_not_internal(host, probe_port).await {
                        return Err(err(StatusCode::BAD_REQUEST, &format!("Invalid monitor host: {e}")));
                    }
                }
            }
            _ => {}
        }
    }

    // SSRF protection: validate per-monitor alert URLs if being updated.
    if let Some(ref u) = body.alert_slack_url {
        if !u.is_empty() {
            if let Err(e) = crate::helpers::validate_url_not_internal(u).await {
                return Err(err(StatusCode::BAD_REQUEST, &format!("Invalid Slack alert URL: {e}")));
            }
        }
    }
    if let Some(ref u) = body.alert_discord_url {
        if !u.is_empty() {
            if let Err(e) = crate::helpers::validate_url_not_internal(u).await {
                return Err(err(StatusCode::BAD_REQUEST, &format!("Invalid Discord alert URL: {e}")));
            }
        }
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
         keyword_must_contain = COALESCE($12, keyword_must_contain), \
         custom_headers = COALESCE($13, custom_headers) \
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
    .bind(&body.custom_headers)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("update monitors", e))?;

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
        .map_err(|e| internal_error("remove monitors", e))?;

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
    .map_err(|e| internal_error("checks", e))?;

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
    .map_err(|e| internal_error("checks", e))?;

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
    .map_err(|e| internal_error("incidents", e))?;

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
    .map_err(|e| internal_error("incidents", e))?;

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
    .map_err(|e| internal_error("uptime stats", e))?;

    if exists.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Monitor not found"));
    }

    // Successful check: HTTP 200-499 or TCP status_code=0 (no error means success)
    // 24h uptime
    let day: Option<(i64, i64)> = sqlx::query_as(
        "SELECT COUNT(*) FILTER (WHERE status_code IS NOT NULL AND error IS NULL), COUNT(*) \
         FROM monitor_checks WHERE monitor_id = $1 AND checked_at > NOW() - INTERVAL '24 hours'"
    ).bind(id).fetch_optional(&state.db).await
        .map_err(|e| internal_error("uptime stats 24h", e))?;

    // 7d uptime
    let week: Option<(i64, i64)> = sqlx::query_as(
        "SELECT COUNT(*) FILTER (WHERE status_code IS NOT NULL AND error IS NULL), COUNT(*) \
         FROM monitor_checks WHERE monitor_id = $1 AND checked_at > NOW() - INTERVAL '7 days'"
    ).bind(id).fetch_optional(&state.db).await
        .map_err(|e| internal_error("uptime stats 7d", e))?;

    // 30d uptime
    let month: Option<(i64, i64)> = sqlx::query_as(
        "SELECT COUNT(*) FILTER (WHERE status_code IS NOT NULL AND error IS NULL), COUNT(*) \
         FROM monitor_checks WHERE monitor_id = $1 AND checked_at > NOW() - INTERVAL '30 days'"
    ).bind(id).fetch_optional(&state.db).await
        .map_err(|e| internal_error("uptime stats 30d", e))?;

    let calc = |data: Option<(i64, i64)>| -> f64 {
        match data {
            Some((up, total)) if total > 0 => (up as f64 / total as f64 * 10000.0).round() / 100.0,
            _ => 100.0,
        }
    };

    // Average response time (24h)
    let avg_rt: Option<(Option<f64>,)> = sqlx::query_as(
        "SELECT AVG(response_time)::float8 FROM monitor_checks WHERE monitor_id = $1 AND checked_at > NOW() - INTERVAL '24 hours' AND status_code IS NOT NULL"
    ).bind(id).fetch_optional(&state.db).await
        .map_err(|e| internal_error("uptime stats avg response", e))?;

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
    .map_err(|e| internal_error("response chart", e))?;

    if exists.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Monitor not found"));
    }

    let points: Vec<(i32, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT response_time, checked_at FROM monitor_checks \
         WHERE monitor_id = $1 AND checked_at > NOW() - INTERVAL '24 hours' AND status_code IS NOT NULL \
         ORDER BY checked_at ASC"
    ).bind(id).fetch_all(&state.db).await.map_err(|e| internal_error("response chart points", e))?;

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
    .map_err(|e| internal_error("force check", e))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Monitor not found"));
    }

    Ok(Json(serde_json::json!({ "ok": true, "message": "Check will run within 60 seconds" })))
}

/// GET /api/status-page — Public status page data (no auth required).
pub async fn status_page(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // The one gate every unauthenticated status-page route passes through.
    crate::services::public_status::require_enabled(&state.db).await?;

    // Get all enabled monitors (no user filter — this is public)
    //
    // ⚠ The error is propagated, and on THIS route that is the whole point. An
    // empty list renders as a status page with nothing wrong on it, which is the
    // most reassuring page the product can serve and the one a reader consults
    // precisely when they suspect something is. `require_enabled` above already
    // returns on a dead pool, so what reached this line was a failure of this
    // query alone — a statement timeout, a type mismatch — and the honest answer
    // to "I could not read the monitors" is never "there are no monitors".
    let monitors: Vec<(String, String, String, Option<i32>, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT name, url, status, last_response_time, last_checked_at FROM monitors WHERE enabled = true ORDER BY name"
    ).fetch_all(&state.db).await.map_err(|e| internal_error("status page monitors", e))?;

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

/// The one severity ladder both certificate lists use.
///
/// `None` is `unknown`, and that rung exists because its absence was a defect: a
/// missing expiry became 999 days and therefore the green OK badge — the most
/// reassuring answer on the page, produced by an absence of information. Every
/// certificate the panel did not issue arrives that way.
///
/// `renewal_failing` is the same argument applied to the other axis. Every rung
/// but that one is a function of the clock alone, so a certificate whose renewal
/// is failing right now still read `ok` for its first three hundred days and
/// `warning` for the next twenty-three — the page described the certificate and
/// never the machinery that is supposed to replace it. v2.157.0 made
/// `ssl_renewal_failure` resolve itself on a successful renewal, which sharpened
/// the gap rather than closing it: a failure that gets fixed now disappears from
/// the Alerts list, and one that does not had nowhere on this page to appear.
///
/// ⚠ It sits BELOW `expired` and ABOVE everything else. `expired` outranks it
/// because the outage has already happened and no longer depends on the renewal;
/// it outranks `unknown` because "the renewal is failing" is a fact and `unknown`
/// is the absence of one; and it outranks `critical`/`warning`/`ok` because those
/// three say when the certificate dies while this says nothing is coming to save
/// it. The days column still carries the clock, so no information is displaced.
pub(crate) fn expiry_status(days_left: Option<i64>, renewal_failing: bool) -> &'static str {
    match days_left {
        Some(d) if d < 0 => "expired",
        _ if renewal_failing => "renewal_failed",
        None => "unknown",
        Some(d) if d <= 7 => "critical",
        Some(d) if d <= 30 => "warning",
        _ => "ok",
    }
}

/// The sites among `site_ids` whose certificate renewal is currently failing.
///
/// ⚠ Keyed on the THREE `state_key`s that assert a renewal did not happen, taken
/// from `notifications::renewal_success_clears` rather than re-listed here. That
/// function answers "does a successful renewal disprove this alert?", and its own
/// doc gives the reason the two questions have one answer: those three are exactly
/// the keys that say a renewal DID NOT HAPPEN, which is what makes a later success
/// contradict them. Re-listing the keys here would be a second classification of
/// the same six keys, free to drift from the first — and its fall-through is
/// `false`, so a key added later is excluded from both until somebody decides.
///
/// The other three describe the certificate installed now, not a failure to
/// renew: `DECLINED` is somebody else's certificate, `MAIL_HOST_CONFLICT` is a
/// renewal deliberately refused to protect a wildcard, and `DNS01_DOWNGRADED` is
/// a renewal that succeeded and covered fewer names. Rendering any of them as
/// "renewal failed" would report a failure the panel did not have.
///
/// ⚠ `status = 'firing'` ONLY, and that is a constraint rather than a
/// simplification. `resolve_alert` matches `status = 'firing'` in every arm, so
/// an ACKNOWLEDGED renewal alert is never resolved by a later success — including
/// it here would pin the rung on permanently for any operator who acknowledged
/// the alert before fixing the cause, and the page would keep asserting a failure
/// that a successful renewal had already disproved.
///
/// Scoped by `site_id` alone, for both callers. The per-caller list is already
/// filtered to its owner's sites and the admin list to one server, so the ids ARE
/// the scope — and routing this through a `user_id`/`server_id` arm would
/// reproduce the shape that made `resolve_alert` miss every row the security
/// scanner raises with no server id.
async fn renewal_failing_sites(
    pool: &sqlx::PgPool,
    site_ids: &[Uuid],
) -> Result<std::collections::HashSet<Uuid>, ApiError> {
    if site_ids.is_empty() {
        return Ok(std::collections::HashSet::new());
    }

    let keys: Vec<&str> = crate::services::notifications::ssl_renewal_key::ALL
        .into_iter()
        .filter(|k| crate::services::notifications::renewal_success_clears(k))
        .collect();

    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT DISTINCT site_id FROM alerts \
         WHERE alert_type = 'ssl_renewal_failure' AND status = 'firing' \
           AND site_id = ANY($1) AND state_key = ANY($2)",
    )
    .bind(site_ids)
    .bind(&keys)
    .fetch_all(pool)
    .await
    .map_err(|e| internal_error("renewal failure lookup", e))?;

    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// GET /api/monitors/certificates — List all SSL certificates with expiry status.
pub async fn certificate_dashboard(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    // No `require_admin`. The query below is already `WHERE user_id = $1`, so the
    // gate never decided WHAT this returns — only WHO is refused their own rows.
    // The Dashboard tile that links here reads the same certificates out of
    // `dashboard::intelligence`, which is `AuthUser` and user-scoped, so a client
    // was told "SSL — 2 certs, expires in 9 days" and then met "Admin access
    // required" over "No SSL certificates found" on the page the tile points at.
    // One of those two screens was lying about the same rows; it was this one.
    let certs: Vec<(uuid::Uuid, String, bool, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT id, domain, ssl_enabled, ssl_expiry FROM sites WHERE user_id = $1 AND ssl_enabled = true ORDER BY ssl_expiry ASC NULLS LAST"
    ).bind(claims.sub).fetch_all(&state.db).await.map_err(|e| internal_error("certificate list", e))?;

    // A certificate whose expiry the panel does not know is `unknown`, not `ok`.
    //
    // The missing date used to become `999` days and therefore the green OK
    // badge — the most reassuring answer available, produced by an absence of
    // information. Every certificate the panel did not issue itself arrives
    // that way: until v2.139.0 `upload_ssl` stored `ssl_enabled` alone, so an
    // operator's own certificate had a NULL expiry by construction, and it is
    // exactly the certificate nobody renews for you. `999` also flowed into the
    // page's own countdown column, printing a confident "999d".
    let now = chrono::Utc::now();
    let ids: Vec<Uuid> = certs.iter().map(|(id, _, _, _)| *id).collect();
    let failing = renewal_failing_sites(&state.db, &ids).await?;
    let items: Vec<serde_json::Value> = certs.iter().map(|(id, domain, _, expiry)| {
        let days_left = expiry.map(|e| (e - now).num_days());
        // `stack_id` is null on every row here and still on the wire: this list is
        // site-scoped by construction, and the page that consumes both lists must
        // not see the field appear and disappear between them.
        serde_json::json!({ "site_id": id, "stack_id": serde_json::Value::Null, "domain": domain, "expiry": expiry, "days_left": days_left, "status": expiry_status(days_left, failing.contains(id)) })
    }).collect();

    Ok(Json(serde_json::json!({ "certificates": items })))
}

/// GET /api/admin/certificates — every certificate on this server, admin only.
///
/// A SEPARATE route rather than a role branch inside `certificate_dashboard`,
/// because that is the shape this codebase already chose for the identical
/// problem: `sites::list` stayed scoped to the caller and admins got
/// `/api/admin/sites` with its own projection, so that a per-caller list can
/// never quietly start returning other people's rows.
///
/// **Why it had to exist.** The agent's diagnostics walks `/etc/dockpanel/ssl`
/// on the HOST and raises "SSL certificate expiring soon: {domain}" for anything
/// it finds, so an administrator on a multi-tenant box was shown a finding — with
/// a Fix button — about a certificate that appeared on no list they could open.
/// The API would already let them renew it (`SITE_CALLER_PREDICATE` carries an
/// admin arm); only the list refused. The finding and the list disagreed about
/// what existed.
///
/// So this returns the union of two populations, and says which is which:
///   * every SSL-enabled site row on this server, with its owner resolved, and
///   * every certificate the host actually holds that no such row explains —
///     a DNS-01 wildcard apex, a Docker app's certificate (Docker apps have no
///     table at all), one installed by hand.
///
/// The second half needs an agent that can enumerate certificates. An older
/// agent has no such route, and the honest answer to that is to say so rather
/// than to imply the list is complete: `host_scan` reports whether the disk was
/// actually read. A limitation the product will not speak at the moment it bites
/// is, from the operator's chair, an undocumented one.
pub async fn certificate_dashboard_for_admin(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let certs: Vec<(uuid::Uuid, String, Option<chrono::DateTime<chrono::Utc>>, Option<String>)> =
        sqlx::query_as(
            "SELECT s.id, s.domain, s.ssl_expiry, u.email AS owner_email \
             FROM sites s LEFT JOIN users u ON u.id = s.user_id \
             WHERE s.server_id = $1 AND s.ssl_enabled = true \
             ORDER BY s.ssl_expiry ASC NULLS LAST",
        )
        .bind(server_id)
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("admin certificate list", e))?;

    let now = chrono::Utc::now();
    let ids: Vec<Uuid> = certs.iter().map(|(id, _, _, _)| *id).collect();
    let failing = renewal_failing_sites(&state.db, &ids).await?;
    let mut items: Vec<serde_json::Value> = certs
        .iter()
        .map(|(id, domain, expiry, owner)| {
            let days_left = expiry.map(|e| (e - now).num_days());
            serde_json::json!({
                "site_id": id,
                // Always on the wire, even as null. A field that is present on
                // some rows and absent on others reaches TypeScript as
                // `undefined`, and `undefined !== null` is true — which would
                // hand every ordinary site row the control meant for stacks.
                "stack_id": serde_json::Value::Null,
                "domain": domain,
                "expiry": expiry,
                "days_left": days_left,
                "status": expiry_status(days_left, failing.contains(id)),
                "owner_email": owner,
                "managed": true,
            })
        })
        .collect();

    // Every stack on this host that has a domain, read ONCE. The walk below needs
    // to ask "is this certificate a stack's?" per directory on disk, and a query
    // per directory would be a round trip per certificate.
    //
    // `ssl_expiry` comes along for the offline arm at the bottom, which is the
    // only reader that has nothing else to fall back on.
    let stacks: Vec<(Uuid, String, Option<String>, Option<String>, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as(
            "SELECT id, domain, tls_mode, ssl_email, ssl_expiry FROM docker_stacks \
             WHERE server_id = $1 AND domain IS NOT NULL",
        )
        .bind(server_id)
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("admin certificate list stacks", e))?;

    // Stack-level `ssl_renewal_failure` alerts carry `site_id = NULL` (there is
    // no sites row to name) and are keyed instead by
    // `stack_renewal_state_key(domain)` under this server — see that
    // function's own doc. `renewal_failing_sites` above filters
    // `site_id = ANY($1)`, which a NULL site_id can never satisfy, so it is the
    // wrong lookup for these rows; this is `renewal_failing_sites`'s sibling,
    // scoped by server + state_key instead of by site id, because a stack
    // alert has no site id to scope by.
    let stack_keys: Vec<String> = stacks
        .iter()
        .map(|(_, d, _, _, _)| crate::services::security_scanner::stack_renewal_state_key(d))
        .collect();
    let failing_stacks: std::collections::HashSet<String> = if stack_keys.is_empty() {
        std::collections::HashSet::new()
    } else {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT DISTINCT state_key FROM alerts \
             WHERE alert_type = 'ssl_renewal_failure' AND status = 'firing' \
               AND server_id = $1 AND state_key = ANY($2)",
        )
        .bind(server_id)
        .bind(&stack_keys)
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("stack renewal failure lookup", e))?;
        rows.into_iter().map(|(k,)| k).collect()
    };

    // ⛔ Only an ACME stack is ours. A `provided` stack serves its certificate from
    //    the registry, and the file this walk finds under /etc/dockpanel/ssl is a
    //    LEFTOVER from before the switch — offering to renew it, or stamping the
    //    row with its expiry, would be the panel describing a certificate the
    //    stack does not serve. Read through the one spelling of the rule.
    let acme_stack = |domain: &str| -> Option<(Uuid, Option<chrono::DateTime<chrono::Utc>>)> {
        stacks
            .iter()
            .find(|(_, d, mode, email, _)| {
                d.eq_ignore_ascii_case(domain)
                    && crate::routes::stacks::effective_tls_mode(mode.as_deref(), email.as_deref())
                        == "acme"
            })
            .map(|(id, _, _, _, recorded)| (*id, *recorded))
    };

    // Best-effort, and its failure is REPORTED rather than swallowed: without it
    // the list silently reverts to the DB-only view that caused the problem.
    let host_scan = match agent.get("/ssl/certificates").await {
        Ok(v) => {
            let known: std::collections::HashSet<&str> =
                certs.iter().map(|(_, d, _, _)| d.as_str()).collect();
            if let Some(arr) = v.as_array() {
                for c in arr {
                    let Some(domain) = c.get("domain").and_then(|d| d.as_str()) else { continue };
                    if known.contains(domain) {
                        continue;
                    }
                    let days_left = c.get("days_remaining").and_then(|d| d.as_i64());
                    let expiry = c.get("not_after").and_then(|d| d.as_str())
                        .and_then(crate::helpers::parse_agent_cert_expiry);
                    let stack = acme_stack(domain);
                    let stack_id = stack.map(|(id, _)| id);

                    // BOOTSTRAP. The agent's walk is the only thing that knows when
                    // a stack's certificate expires, and until now the panel read
                    // that answer, rendered it, and threw it away — so nothing in
                    // Postgres could ever answer the question, and the arm below
                    // had nothing to fall back on.
                    //
                    // ⛔ Best-effort by construction: `let _`, never `?`. This is a
                    //    GET, and a failed bookkeeping write must not turn a page
                    //    the operator asked for into an error. The next read
                    //    retries it.
                    // ⚠ Only when it actually MOVED. This runs per certificate on
                    //    disk, on every load of this page, and a row that already
                    //    holds the right date does not need writing — an admin
                    //    refreshing would otherwise issue one UPDATE per stack
                    //    certificate every time, for ever, to change nothing.
                    if let (Some((id, recorded)), Some(exp)) = (stack, expiry) {
                        if recorded != Some(exp) {
                            let _ = sqlx::query(
                                "UPDATE docker_stacks SET ssl_expiry = $1 WHERE id = $2",
                            )
                            .bind(exp)
                            .bind(id)
                            .execute(&state.db)
                            .await;
                        }
                    }

                    items.push(serde_json::json!({
                        "site_id": serde_json::Value::Null,
                        "stack_id": stack_id,
                        "domain": domain,
                        "expiry": expiry,
                        "days_left": days_left,
                        "status": expiry_status(
                            days_left,
                            failing_stacks.contains(
                                &crate::services::security_scanner::stack_renewal_state_key(domain),
                            ),
                        ),
                        "issuer": c.get("issuer"),
                        "owner_email": serde_json::Value::Null,
                        // A certificate on disk that belongs to no site and no
                        // ACME stack: nothing here can renew or remove it, and the
                        // page says so rather than offering a control that fails.
                        "managed": false,
                    }));
                }
            }
            true
        }
        Err(e) => {
            tracing::warn!("Admin certificate list: this server's agent could not enumerate certificates ({e}) — falling back to what the panel recorded");

            // ⭐ THE OFFLINE ANSWER, and the reason `docker_stacks.ssl_expiry`
            //    exists. A site keeps its row in the list when the agent is
            //    unreachable, because the panel stores `sites.ssl_expiry`. A stack
            //    had no such column, so every stack certificate simply VANISHED
            //    from this page for as long as the agent was down — at exactly the
            //    moment an operator is most likely to be looking at it.
            //
            //    These rows are the panel's own record, not a live read. They
            //    carry no issuer (nothing re-read the file) and no control: this
            //    half of the page is reached only when the agent cannot be asked,
            //    and a Renew button here would post to a host that is not
            //    answering.
            for (id, domain, mode, email, expiry) in &stacks {
                if crate::routes::stacks::effective_tls_mode(mode.as_deref(), email.as_deref())
                    != "acme"
                {
                    continue;
                }
                let Some(expiry) = expiry else { continue };
                let days_left = Some((*expiry - now).num_days());
                items.push(serde_json::json!({
                    "site_id": serde_json::Value::Null,
                    "stack_id": id,
                    "domain": domain,
                    "expiry": expiry,
                    "days_left": days_left,
                    "status": expiry_status(
                        days_left,
                        failing_stacks.contains(
                            &crate::services::security_scanner::stack_renewal_state_key(domain),
                        ),
                    ),
                    "issuer": serde_json::Value::Null,
                    "owner_email": serde_json::Value::Null,
                    "managed": true,
                    // The one thing this row must not do is look like a live read.
                    "stale": true,
                }));
            }
            false
        }
    };

    Ok(Json(serde_json::json!({ "certificates": items, "host_scan": host_scan })))
}

/// POST /api/monitors/maintenance — Create a maintenance window.
pub async fn create_maintenance(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Maintenance windows are per-caller in all three statements — this INSERT
    // stamps `user_id`, the list filters on it, the delete requires it — and what
    // a window does is silence THAT caller's alerts. A site owner who has alerts
    // (`alerts` is user-scoped for every role) is exactly who needs to silence
    // them while they work on their own site.
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("Maintenance");
    let starts_at = body.get("starts_at").and_then(|v| v.as_str()).unwrap_or("");
    let ends_at = body.get("ends_at").and_then(|v| v.as_str()).unwrap_or("");

    if starts_at.is_empty() || ends_at.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "starts_at and ends_at required"));
    }

    let id: (uuid::Uuid,) = sqlx::query_as(
        "INSERT INTO maintenance_windows (user_id, name, starts_at, ends_at) VALUES ($1, $2, $3::timestamptz, $4::timestamptz) RETURNING id"
    ).bind(claims.sub).bind(name).bind(starts_at).bind(ends_at)
    .fetch_one(&state.db).await
    .map_err(|e| internal_error("create maintenance", e))?;

    Ok(Json(serde_json::json!({ "ok": true, "id": id.0 })))
}

/// GET /api/monitors/maintenance — List maintenance windows.
pub async fn list_maintenance(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Per-caller by `WHERE user_id = $1` — see `create_maintenance` above.
    //
    // ⚠ The error is propagated because an empty list is not a neutral answer
    // here. `uptime.rs` skips every monitor belonging to a user with a currently
    // active window, so a failed read used to show an operator no windows while
    // one of them was still suppressing their alerting — the screen said nothing
    // was muting anything, and the delete control that is the way out was not
    // offered, because the row it belongs to was never drawn.
    let windows: Vec<(uuid::Uuid, String, chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT id, name, starts_at, ends_at FROM maintenance_windows WHERE user_id = $1 ORDER BY starts_at DESC LIMIT 20"
    ).bind(claims.sub).fetch_all(&state.db).await.map_err(|e| internal_error("maintenance windows", e))?;

    let now = chrono::Utc::now();
    let items: Vec<serde_json::Value> = windows.iter().map(|(id, name, start, end)| {
        let active = now >= *start && now <= *end;
        serde_json::json!({ "id": id, "name": name, "starts_at": start, "ends_at": end, "active": active })
    }).collect();

    Ok(Json(serde_json::json!({ "windows": items })))
}

/// DELETE /api/monitors/maintenance/{id}
pub async fn delete_maintenance(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // `AND user_id = $2` is the authorization — a caller can only delete a window
    // they own, which is the same rule the gate above used to approximate.
    //
    // The result is checked rather than discarded, and the reason is not tidiness.
    // `uptime.rs` skips EVERY monitor belonging to a user with a currently-active
    // window, so a window that outlives its "Maintenance window deleted" message
    // leaves that whole account with no uptime checks, no downtime detection and no
    // alerts — and the ordinary trigger for pressing delete is finishing maintenance
    // early to resume exactly those. Reporting success for a row that is still there
    // is the one answer the panel must not give here.
    let deleted = sqlx::query("DELETE FROM maintenance_windows WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(claims.sub)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("delete maintenance window", e))?;

    if deleted.rows_affected() == 0 {
        return Err(err(
            StatusCode::NOT_FOUND,
            "That maintenance window no longer exists, or it belongs to another account. \
             Monitoring is unchanged — reload the list before retrying.",
        ));
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/heartbeat/{monitor_id}/{token} — Receive heartbeat ping (no auth).
pub async fn heartbeat(
    State(state): State<AppState>,
    Path((monitor_id, token)): Path<(uuid::Uuid, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Validate monitor exists and is a heartbeat type
    let monitor: Option<(uuid::Uuid, String, Option<String>)> = sqlx::query_as(
        "SELECT id, COALESCE(name, ''), heartbeat_token FROM monitors WHERE id = $1 AND monitor_type = 'heartbeat'"
    ).bind(monitor_id).fetch_optional(&state.db).await
        .map_err(|e| internal_error("heartbeat monitor lookup", e))?;

    let monitor = monitor.ok_or_else(|| err(StatusCode::NOT_FOUND, "Monitor not found"))?;

    // Verify heartbeat token
    if monitor.2.as_deref() != Some(&token) {
        return Err(err(StatusCode::UNAUTHORIZED, "Invalid heartbeat token"));
    }

    // Record successful check
    sqlx::query("INSERT INTO monitor_checks (monitor_id, status_code, response_time, checked_at) VALUES ($1, 200, 0, NOW())")
        .bind(monitor_id).execute(&state.db).await.ok();

    // Update monitor status to up
    sqlx::query("UPDATE monitors SET status = 'up', last_checked_at = NOW(), last_response_time = 0, last_status_code = 200 WHERE id = $1")
        .bind(monitor_id).execute(&state.db).await.ok();

    // Resolve any open incidents
    sqlx::query("UPDATE incidents SET resolved_at = NOW() WHERE monitor_id = $1 AND resolved_at IS NULL")
        .bind(monitor_id).execute(&state.db).await.ok();

    Ok(Json(serde_json::json!({ "ok": true })))
}
