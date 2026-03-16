use axum::{extract::State, http::StatusCode, Json};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::auth::AuthUser;
use crate::error::{err, ApiError};
use crate::AppState;

static INTELLIGENCE_CACHE: std::sync::LazyLock<Mutex<Option<(serde_json::Value, Instant)>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));
const CACHE_TTL: Duration = Duration::from_secs(30);

/// GET /api/dashboard/intelligence — Server health score + top issues + SSL countdowns.
pub async fn intelligence(
    AuthUser(claims): AuthUser,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Check cache first
    if let Ok(guard) = INTELLIGENCE_CACHE.lock() {
        if let Some((ref cached, ref stored_at)) = *guard {
            if stored_at.elapsed() < CACHE_TTL {
                return Ok(Json(cached.clone()));
            }
        }
    }

    // 1. Get firing alerts count
    let (firing_count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM alerts WHERE status = 'firing'",
    )
    .fetch_one(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    // 2. Get acknowledged alerts count
    let (ack_count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM alerts WHERE status = 'acknowledged'",
    )
    .fetch_one(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    // 3. Get SSL expiry data
    let ssl_sites: Vec<(String, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT domain, ssl_expiry FROM sites WHERE ssl_enabled = TRUE AND ssl_expiry IS NOT NULL ORDER BY ssl_expiry ASC LIMIT 5",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let now = chrono::Utc::now();
    let ssl_countdowns: Vec<serde_json::Value> = ssl_sites
        .iter()
        .map(|(domain, expiry)| {
            let days_left = expiry
                .map(|e| (e - now).num_days())
                .unwrap_or(0);
            let severity = if days_left <= 3 {
                "critical"
            } else if days_left <= 7 {
                "warning"
            } else if days_left <= 30 {
                "info"
            } else {
                "ok"
            };
            serde_json::json!({
                "domain": domain,
                "days_left": days_left,
                "severity": severity,
                "expiry": expiry,
            })
        })
        .collect();

    // 4. Get recent alert titles (top issues)
    let top_issues: Vec<(String, String, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT title, severity, alert_type, created_at FROM alerts WHERE status IN ('firing', 'acknowledged') ORDER BY CASE severity WHEN 'critical' THEN 0 WHEN 'warning' THEN 1 ELSE 2 END, created_at DESC LIMIT 5",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let issues: Vec<serde_json::Value> = top_issues
        .iter()
        .map(|(title, severity, alert_type, created_at)| {
            serde_json::json!({
                "title": title,
                "severity": severity,
                "type": alert_type,
                "since": created_at,
            })
        })
        .collect();

    // 5. Get diagnostics from agent
    let mut diagnostics_summary = serde_json::json!(null);
    if let Ok(diag) = state.agent.get("/diagnostics").await {
        diagnostics_summary = diag;
    }

    // 6. Compute health score (0-100)
    let diag_critical = diagnostics_summary
        .get("summary")
        .and_then(|s| s.get("critical"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let diag_warning = diagnostics_summary
        .get("summary")
        .and_then(|s| s.get("warning"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    let mut score: i64 = 100;
    score -= firing_count * 15;          // Each firing alert costs 15 points
    score -= ack_count * 5;              // Each acknowledged alert costs 5 points
    score -= diag_critical * 20;         // Each critical diagnostic costs 20 points
    score -= diag_warning * 5;           // Each warning diagnostic costs 5 points
    // SSL expiry penalties
    for ssl in &ssl_countdowns {
        match ssl.get("severity").and_then(|s| s.as_str()) {
            Some("critical") => score -= 15,
            Some("warning") => score -= 5,
            _ => {}
        }
    }
    let score = score.max(0).min(100);

    let grade = match score {
        90..=100 => "A",
        75..=89 => "B",
        60..=74 => "C",
        40..=59 => "D",
        _ => "F",
    };

    let result = serde_json::json!({
        "health_score": score,
        "grade": grade,
        "firing_alerts": firing_count,
        "acknowledged_alerts": ack_count,
        "ssl_countdowns": ssl_countdowns,
        "top_issues": issues,
        "diagnostics": diagnostics_summary,
    });

    // Store in cache
    if let Ok(mut guard) = INTELLIGENCE_CACHE.lock() {
        *guard = Some((result.clone(), Instant::now()));
    }

    Ok(Json(result))
}

/// GET /api/dashboard/metrics-history — Historical CPU/memory/disk data for charts.
/// Downsampled to ~96 points (one per 15-minute bucket) for efficient chart rendering.
pub async fn metrics_history(
    AuthUser(_claims): AuthUser,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let rows: Vec<(f64, f64, f64, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT \
            AVG(cpu_pct)::float8 AS cpu_pct, \
            AVG(mem_pct)::float8 AS mem_pct, \
            AVG(disk_pct)::float8 AS disk_pct, \
            date_trunc('hour', created_at) + \
                (EXTRACT(minute FROM created_at)::int / 15) * INTERVAL '15 minutes' AS bucket \
         FROM metrics_history \
         WHERE created_at > NOW() - INTERVAL '24 hours' \
         GROUP BY bucket \
         ORDER BY bucket ASC",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let points: Vec<serde_json::Value> = rows
        .iter()
        .map(|(cpu, mem, disk, ts)| {
            serde_json::json!({
                "cpu": (*cpu * 10.0).round() / 10.0,
                "mem": (*mem * 10.0).round() / 10.0,
                "disk": (*disk * 10.0).round() / 10.0,
                "time": ts.format("%H:%M").to_string(),
            })
        })
        .collect();

    Ok(Json(serde_json::json!({ "points": points })))
}
