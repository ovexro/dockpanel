use axum::{extract::State, http::StatusCode, Json};

use crate::auth::AuthUser;
use crate::error::{err, ApiError};
use crate::AppState;

/// GET /api/dashboard/intelligence — Server health score + top issues + SSL countdowns.
pub async fn intelligence(
    AuthUser(claims): AuthUser,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
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

    Ok(Json(serde_json::json!({
        "health_score": score,
        "grade": grade,
        "firing_alerts": firing_count,
        "acknowledged_alerts": ack_count,
        "ssl_countdowns": ssl_countdowns,
        "top_issues": issues,
        "diagnostics": diagnostics_summary,
    })))
}

/// GET /api/dashboard/metrics-history — Historical CPU/memory/disk data for charts.
pub async fn metrics_history(
    AuthUser(_claims): AuthUser,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let rows: Vec<(f32, f32, f32, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT cpu_pct, mem_pct, disk_pct, created_at FROM metrics_history \
         WHERE created_at > NOW() - INTERVAL '24 hours' \
         ORDER BY created_at ASC",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let points: Vec<serde_json::Value> = rows
        .iter()
        .map(|(cpu, mem, disk, ts)| {
            serde_json::json!({
                "cpu": cpu,
                "mem": mem,
                "disk": disk,
                "time": ts.format("%H:%M").to_string(),
            })
        })
        .collect();

    Ok(Json(serde_json::json!({ "points": points })))
}
