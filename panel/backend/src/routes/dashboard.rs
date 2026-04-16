use axum::{extract::State, Json};

use crate::auth::{AdminUser, AuthUser, ServerScope};
use crate::error::{internal_error, ApiError};
use crate::AppState;

/// GET /api/dashboard/intelligence — Server health score + top issues + SSL countdowns.
pub async fn intelligence(
    AuthUser(claims): AuthUser,
    State(state): State<AppState>,
    ServerScope(server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {

    // 1. Get firing alerts count (scoped to user's sites)
    let (firing_count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM alerts WHERE status = 'firing' AND server_id = $1 AND (user_id = $2 OR user_id IS NULL)",
    )
    .bind(server_id)
    .bind(claims.sub)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("intelligence", e))?;

    // 2. Get acknowledged alerts count (scoped to user)
    let (ack_count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM alerts WHERE status = 'acknowledged' AND server_id = $1 AND (user_id = $2 OR user_id IS NULL)",
    )
    .bind(server_id)
    .bind(claims.sub)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("intelligence", e))?;

    // 3. Get SSL expiry data (scoped to user's sites)
    let ssl_sites: Vec<(String, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT domain, ssl_expiry FROM sites WHERE ssl_enabled = TRUE AND ssl_expiry IS NOT NULL AND server_id = $1 AND user_id = $2 ORDER BY ssl_expiry ASC LIMIT 5",
    )
    .bind(server_id)
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("intelligence", e))?;

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
        "SELECT title, severity, alert_type, created_at FROM alerts WHERE status IN ('firing', 'acknowledged') AND server_id = $1 AND (user_id = $2 OR user_id IS NULL) ORDER BY CASE severity WHEN 'critical' THEN 0 WHEN 'warning' THEN 1 ELSE 2 END, created_at DESC LIMIT 5",
    )
    .bind(server_id)
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("intelligence", e))?;

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
    if let Ok(diag) = agent.get("/diagnostics").await {
        diagnostics_summary = diag;
    }

    // 6. Backup freshness — sites with no backup in the last 7 days
    let (stale_backups,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM sites s WHERE s.status = 'active' AND s.server_id = $1 \
         AND NOT EXISTS (SELECT 1 FROM backups b WHERE b.site_id = s.id AND b.created_at > NOW() - INTERVAL '7 days')",
    )
    .bind(server_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or((0,));

    // 7. Security scan — latest scan critical/warning counts
    let scan_findings: Option<(i32, i32)> = sqlx::query_as(
        "SELECT critical_count, warning_count FROM security_scans \
         WHERE server_id = $1 AND status = 'completed' ORDER BY created_at DESC LIMIT 1",
    )
    .bind(server_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();
    let (scan_crits, scan_warns) = scan_findings.unwrap_or((0, 0));

    // 8. Open incidents (scoped by user, not server — incidents are user-level)
    let (open_incidents,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM managed_incidents \
         WHERE user_id = $1 AND status NOT IN ('resolved', 'postmortem')",
    )
    .bind(claims.sub)
    .fetch_one(&state.db)
    .await
    .unwrap_or((0,));

    // 9. Compute health score (0-100)
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
    // Backup freshness penalty
    score -= stale_backups * 5;          // -5 per site with stale backups
    // Security scan penalty
    score -= scan_crits as i64 * 10;    // -10 per critical finding
    score -= scan_warns as i64 * 3;     // -3 per warning finding
    // Open incident penalty
    score -= open_incidents * 10;        // -10 per active incident
    let score = score.max(0).min(100);

    let grade = match score {
        90..=100 => "A",
        75..=89 => "B",
        60..=74 => "C",
        40..=59 => "D",
        _ => "F",
    };

    // 10. Build smart recommendations
    let mut recommendations: Vec<serde_json::Value> = Vec::new();

    if stale_backups > 0 {
        recommendations.push(serde_json::json!({
            "severity": "warning",
            "message": format!("{} site(s) have no backup in the last 7 days", stale_backups),
            "action": "backup",
        }));
    }
    if scan_crits > 0 {
        recommendations.push(serde_json::json!({
            "severity": "critical",
            "message": format!("Security scan found {} critical vulnerabilit{}", scan_crits, if scan_crits == 1 { "y" } else { "ies" }),
            "action": "security",
        }));
    }
    if scan_warns > 0 {
        recommendations.push(serde_json::json!({
            "severity": "warning",
            "message": format!("Security scan found {} warning{}", scan_warns, if scan_warns == 1 { "" } else { "s" }),
            "action": "security",
        }));
    }
    if open_incidents > 0 {
        recommendations.push(serde_json::json!({
            "severity": if open_incidents >= 3 { "critical" } else { "warning" },
            "message": format!("{} active incident{} require attention", open_incidents, if open_incidents == 1 { "" } else { "s" }),
            "action": "incidents",
        }));
    }
    for ssl in &ssl_countdowns {
        let days = ssl.get("days_left").and_then(|d| d.as_i64()).unwrap_or(999);
        let domain = ssl.get("domain").and_then(|d| d.as_str()).unwrap_or("unknown");
        if days <= 14 {
            recommendations.push(serde_json::json!({
                "severity": if days <= 3 { "critical" } else { "warning" },
                "message": format!("SSL certificate for {} expires in {} day{}", domain, days, if days == 1 { "" } else { "s" }),
                "action": "ssl",
            }));
        }
    }
    if firing_count > 0 {
        recommendations.push(serde_json::json!({
            "severity": "critical",
            "message": format!("{} alert{} currently firing", firing_count, if firing_count == 1 { "" } else { "s" }),
            "action": "alerts",
        }));
    }
    if diag_critical > 0 {
        recommendations.push(serde_json::json!({
            "severity": "critical",
            "message": format!("{} critical diagnostic issue{} detected", diag_critical, if diag_critical == 1 { "" } else { "s" }),
            "action": "diagnostics",
        }));
    }

    // Sort recommendations: critical first, then warning
    recommendations.sort_by(|a, b| {
        let sev_a = a.get("severity").and_then(|s| s.as_str()).unwrap_or("info");
        let sev_b = b.get("severity").and_then(|s| s.as_str()).unwrap_or("info");
        let ord = |s: &str| -> u8 { match s { "critical" => 0, "warning" => 1, _ => 2 } };
        ord(sev_a).cmp(&ord(sev_b))
    });

    let result = serde_json::json!({
        "health_score": score,
        "grade": grade,
        "firing_alerts": firing_count,
        "acknowledged_alerts": ack_count,
        "open_incidents": open_incidents,
        "stale_backups": stale_backups,
        "scan_critical": scan_crits,
        "scan_warnings": scan_warns,
        "ssl_countdowns": ssl_countdowns,
        "top_issues": issues,
        "recommendations": recommendations,
        "diagnostics": diagnostics_summary,
    });

    Ok(Json(result))
}

/// GET /api/dashboard/docker — Docker container summary.
pub async fn docker_summary(
    AuthUser(_claims): AuthUser,
    State(_state): State<AppState>,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent.get("/apps").await.ok();

    let apps = result.and_then(|r| r.as_array().cloned()).unwrap_or_default();
    let total = apps.len();
    let running = apps
        .iter()
        .filter(|a| a.get("status").and_then(|s| s.as_str()) == Some("running"))
        .count();
    let stopped = total - running;

    Ok(Json(serde_json::json!({
        "total": total,
        "running": running,
        "stopped": stopped,
    })))
}

/// GET /api/dashboard/metrics-history — Historical CPU/memory/disk data for charts.
/// Downsampled to ~96 points (one per 15-minute bucket) for efficient chart rendering.
pub async fn metrics_history(
    AuthUser(claims): AuthUser,
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
           AND server_id IN (SELECT id FROM servers WHERE user_id = $1) \
         GROUP BY bucket \
         ORDER BY bucket ASC",
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("metrics history", e))?;

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

/// GET /api/dashboard/gpu-metrics-history — Historical GPU metrics for charts.
/// Downsampled to ~96 points (one per 15-minute bucket) per GPU for efficient chart rendering.
pub async fn gpu_metrics_history(
    AuthUser(claims): AuthUser,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let rows: Vec<(i16, f64, f64, f64, Option<f64>, Option<f64>, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT \
            gpu_index, \
            AVG(utilization_pct)::float8 AS utilization, \
            AVG(memory_used_mb)::float8 AS mem_used, \
            AVG(memory_total_mb)::float8 AS mem_total, \
            AVG(temperature_c)::float8 AS temp, \
            AVG(power_draw_w)::float8 AS power, \
            date_trunc('hour', created_at) + \
                (EXTRACT(minute FROM created_at)::int / 15) * INTERVAL '15 minutes' AS bucket \
         FROM gpu_metrics_history \
         WHERE created_at > NOW() - INTERVAL '24 hours' \
           AND server_id IN (SELECT id FROM servers WHERE user_id = $1) \
         GROUP BY gpu_index, bucket \
         ORDER BY bucket ASC, gpu_index ASC",
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("gpu metrics history", e))?;

    let points: Vec<serde_json::Value> = rows
        .iter()
        .map(|(idx, util, mem_used, mem_total, temp, power, ts)| {
            let vram_pct = if *mem_total > 0.0 { (*mem_used / *mem_total) * 100.0 } else { 0.0 };
            serde_json::json!({
                "gpu_index": idx,
                "utilization": (*util * 10.0).round() / 10.0,
                "vram_pct": (vram_pct * 10.0).round() / 10.0,
                "vram_used_mb": (*mem_used).round(),
                "vram_total_mb": (*mem_total).round(),
                "temperature": temp.map(|t| (t * 10.0).round() / 10.0),
                "power": power.map(|p| (p * 10.0).round() / 10.0),
                "time": ts.format("%H:%M").to_string(),
            })
        })
        .collect();

    Ok(Json(serde_json::json!({ "points": points })))
}

/// GET /api/dashboard/timeline — Unified chronological event feed.
/// Merges recent events from deploys, backups, incidents, alerts, and security scans.
pub async fn timeline(
    AuthUser(claims): AuthUser,
    State(state): State<AppState>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let mut events: Vec<serde_json::Value> = Vec::new();

    // Recent deploys (join sites for domain, filtered by user ownership)
    let deploys: Vec<(String, Option<String>, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT dl.status, s.domain, dl.created_at \
         FROM deploy_logs dl \
         JOIN sites s ON s.id = dl.site_id \
         WHERE s.user_id = $1 \
         ORDER BY dl.created_at DESC LIMIT 10",
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    for (status, domain, created_at) in &deploys {
        events.push(serde_json::json!({
            "type": "deploy",
            "detail": status,
            "target": domain,
            "created_at": created_at.to_rfc3339(),
        }));
    }

    // Recent backups (join sites for domain, filtered by user ownership)
    let backups: Vec<(String, Option<String>, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT b.filename, s.domain, b.created_at \
         FROM backups b \
         JOIN sites s ON s.id = b.site_id \
         WHERE s.user_id = $1 \
         ORDER BY b.created_at DESC LIMIT 10",
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    for (filename, domain, created_at) in &backups {
        events.push(serde_json::json!({
            "type": "backup",
            "detail": filename,
            "target": domain,
            "created_at": created_at.to_rfc3339(),
        }));
    }

    // Recent incidents (filtered by user)
    let incidents: Vec<(String, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT severity, title, created_at FROM managed_incidents WHERE user_id = $1 ORDER BY created_at DESC LIMIT 10",
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    for (severity, title, created_at) in &incidents {
        events.push(serde_json::json!({
            "type": "incident",
            "detail": severity,
            "target": title,
            "created_at": created_at.to_rfc3339(),
        }));
    }

    // Recent alerts (filtered by user's servers)
    let alerts: Vec<(String, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT severity, title, created_at FROM alerts \
         WHERE server_id IN (SELECT id FROM servers WHERE user_id = $1) \
         ORDER BY created_at DESC LIMIT 10",
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    for (severity, title, created_at) in &alerts {
        events.push(serde_json::json!({
            "type": "alert",
            "detail": severity,
            "target": title,
            "created_at": created_at.to_rfc3339(),
        }));
    }

    // Recent security scans (filtered by user's servers)
    let scans: Vec<(i32, i32, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT critical_count, warning_count, created_at FROM security_scans \
         WHERE server_id IN (SELECT id FROM servers WHERE user_id = $1) \
         ORDER BY created_at DESC LIMIT 5",
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    for (critical, warning, created_at) in &scans {
        events.push(serde_json::json!({
            "type": "security",
            "detail": format!("{} critical, {} warnings", critical, warning),
            "target": "Security Scan",
            "created_at": created_at.to_rfc3339(),
        }));
    }

    // Sort by created_at descending and take top 30
    events.sort_by(|a, b| {
        let ts_a = a.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
        let ts_b = b.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
        ts_b.cmp(ts_a)
    });
    events.truncate(30);

    Ok(Json(events))
}

/// GET /api/dashboard/fleet — Unified health across all servers (admin only).
/// Aggregates firing alerts, active incidents, sites, databases, and latest
/// metrics for every server the admin owns.
pub async fn fleet_overview(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Single query replaces N+1 pattern (was: 5 queries per server in a loop).
    // Uses LEFT JOIN with subqueries to aggregate all data in one round trip.
    let rows: Vec<(uuid::Uuid, String, Option<String>, i64, i64, i64, Option<f32>, Option<f32>, Option<f32>)> = sqlx::query_as(
        "SELECT s.id, s.name, s.ip_address, \
         COALESCE(a.firing, 0) AS firing_alerts, \
         COALESCE(si.cnt, 0) AS site_count, \
         COALESCE(d.cnt, 0) AS db_count, \
         m.cpu_pct, m.mem_pct, m.disk_pct \
         FROM servers s \
         LEFT JOIN LATERAL (SELECT COUNT(*) AS firing FROM alerts WHERE server_id = s.id AND status = 'firing') a ON true \
         LEFT JOIN LATERAL (SELECT COUNT(*) AS cnt FROM sites WHERE server_id = s.id AND status = 'active') si ON true \
         LEFT JOIN LATERAL (SELECT COUNT(*) AS cnt FROM databases WHERE site_id IN (SELECT id FROM sites WHERE server_id = s.id)) d ON true \
         LEFT JOIN LATERAL (SELECT cpu_pct, mem_pct, disk_pct FROM metrics_history WHERE server_id = s.id ORDER BY created_at DESC LIMIT 1) m ON true \
         WHERE s.user_id = $1 \
         ORDER BY s.name",
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("fleet overview", e))?;

    // Active incidents (global for user, not per-server)
    let active_incidents: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM managed_incidents WHERE user_id = $1 AND status NOT IN ('resolved', 'postmortem')",
    )
    .bind(claims.sub)
    .fetch_one(&state.db)
    .await
    .unwrap_or((0,));

    let mut total_firing: i64 = 0;
    let mut total_sites: i64 = 0;
    let fleet: Vec<serde_json::Value> = rows.iter().map(|r| {
        total_firing += r.3;
        total_sites += r.4;
        serde_json::json!({
            "id": r.0,
            "name": r.1,
            "ip_address": r.2,
            "firing_alerts": r.3,
            "active_incidents": active_incidents.0,
            "sites": r.4,
            "databases": r.5,
            "cpu_pct": r.6,
            "mem_pct": r.7,
            "disk_pct": r.8,
            "status": if r.3 > 0 { "warning" } else { "healthy" },
        })
    }).collect();

    Ok(Json(serde_json::json!({
        "servers": fleet,
        "total_servers": rows.len(),
        "total_firing": total_firing,
        "total_incidents": active_incidents.0,
        "total_sites": total_sites,
    })))
}
