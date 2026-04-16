use sqlx::PgPool;
use std::time::Duration;
use uuid::Uuid;

use crate::services::agent::AgentClient;
use crate::services::notifications;

/// Background task: checks all alert conditions every 60 seconds.
pub async fn run(pool: PgPool, agent: AgentClient, mut shutdown_rx: tokio::sync::broadcast::Receiver<()>) {
    tracing::info!("Alert engine started");

    // Initial delay (respects shutdown)
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(30)) => {}
        _ = shutdown_rx.recv() => {
            tracing::info!("Alert engine shutting down gracefully (during initial delay)");
            return;
        }
    }

    let mut interval = tokio::time::interval(Duration::from_secs(60));
    let mut tick_count: u64 = 0;
    loop {
        tokio::select! {
            _ = interval.tick() => {
                tick_count += 1;

                check_resource_thresholds(&pool).await;
                check_gpu_thresholds(&pool, &agent).await;
                check_server_offline(&pool).await;
                check_ssl_expiry(&pool).await;

                // Service health every 2 minutes (every other tick)
                if tick_count % 2 == 0 {
                    check_service_health(&pool, &agent).await;
                }

                // GAP 8: Docker container health every 2 minutes (offset from service health)
                if tick_count % 2 == 1 {
                    check_container_health(&pool, &agent).await;
                }

                // GAP 9: Escalate unacknowledged firing alerts older than 15 minutes
                check_escalations(&pool).await;

                // Purge old resolved alerts (keep 30 days) — every hour
                if tick_count % 60 == 0 {
                    let _ = sqlx::query(
                        "DELETE FROM alerts WHERE status = 'resolved' AND resolved_at < NOW() - INTERVAL '30 days'",
                    )
                    .execute(&pool)
                    .await;
                }
            }
            _ = shutdown_rx.recv() => {
                tracing::info!("Alert engine shutting down gracefully");
                break;
            }
        }
    }
}

// ─── Alert Fire with Retry ──────────────────────────────────────────────

/// Fire an alert with retry (2 attempts, 3s delay between).
async fn fire_alert_with_retry(
    pool: &PgPool,
    user_id: Uuid,
    server_id: Option<Uuid>,
    site_id: Option<Uuid>,
    alert_type: &str,
    severity: &str,
    title: &str,
    message: &str,
) {
    for attempt in 0..2 {
        match notifications::try_fire_alert(
            pool, user_id, server_id, site_id, alert_type, severity, title, message,
        )
        .await
        {
            Ok(_) => {
                // Auto-create managed incident for critical alerts
                // GAP 11: Check for existing active incident before creating a new one
                if severity == "critical" || alert_type == "offline" || alert_type == "service_down" {
                    let incident_severity = if severity == "critical" { "critical" } else { "major" };

                    // Check if there's already an active incident for this user within the last 5 minutes
                    let existing: Option<(Uuid,)> = sqlx::query_as(
                        "SELECT id FROM managed_incidents \
                         WHERE user_id = $1 \
                         AND status NOT IN ('resolved', 'postmortem') \
                         AND created_at > NOW() - INTERVAL '5 minutes' \
                         LIMIT 1"
                    )
                    .bind(user_id)
                    .fetch_optional(pool)
                    .await
                    .ok()
                    .flatten();

                    if let Some((incident_id,)) = existing {
                        // Append as incident update instead of creating a duplicate incident
                        let _ = sqlx::query(
                            "INSERT INTO incident_updates (incident_id, status, message, author_email) \
                             VALUES ($1, 'investigating', $2, 'system')"
                        )
                        .bind(incident_id)
                        .bind(format!("Related {alert_type} alert: {message}"))
                        .execute(pool).await;

                        tracing::info!("Correlated alert to existing incident {incident_id}: {title}");
                    } else {
                        // No recent active incident — create a new one
                        let _ = sqlx::query(
                            "INSERT INTO managed_incidents (user_id, title, status, severity, description, visible_on_status_page) \
                             VALUES ($1, $2, 'investigating', $3, $4, TRUE)"
                        )
                        .bind(user_id).bind(title).bind(incident_severity).bind(message)
                        .execute(pool).await;

                        let _ = sqlx::query(
                            "INSERT INTO incident_updates (incident_id, status, message, author_email) \
                             SELECT id, 'investigating', $2, 'system' FROM managed_incidents \
                             WHERE title = $1 AND status = 'investigating' ORDER BY created_at DESC LIMIT 1"
                        )
                        .bind(title).bind(format!("Auto-created from {alert_type} alert: {message}"))
                        .execute(pool).await;
                    }
                }
                return;
            },
            Err(e) => {
                tracing::warn!("Alert fire attempt {} failed: {}", attempt + 1, e);
                if attempt == 1 {
                    // Both attempts failed — log to system_logs
                    crate::services::system_log::log_event(
                        pool,
                        "error",
                        "alert_engine",
                        &format!("Failed to fire alert: {title}"),
                        Some(&e.to_string()),
                    ).await;
                }
                if attempt < 1 {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
            }
        }
    }
}

// ─── Resource Thresholds (CPU / Memory / Disk) ─────────────────────────

#[derive(sqlx::FromRow)]
struct ServerMetrics {
    id: Uuid,
    user_id: Uuid,
    name: String,
    cpu_usage: Option<f32>,
    mem_used_mb: Option<i64>,
    ram_mb: Option<i32>,
    disk_usage_pct: Option<f32>,
}

async fn check_resource_thresholds(pool: &PgPool) {
    let servers: Vec<ServerMetrics> = match sqlx::query_as(
        "SELECT id, user_id, name, cpu_usage, mem_used_mb, ram_mb, disk_usage_pct \
         FROM servers WHERE status = 'online'",
    )
    .fetch_all(pool)
    .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("Alert engine: server query error: {e}");
            return;
        }
    };

    for server in &servers {
        let (cpu_thresh, cpu_dur, mem_thresh, mem_dur, disk_thresh, cooldown, _) =
            notifications::get_thresholds(pool, server.user_id, Some(server.id)).await;

        // CPU
        if let Some(cpu) = server.cpu_usage {
            check_threshold(
                pool,
                server,
                "cpu",
                cpu as f64,
                cpu_thresh as f64,
                cpu_dur,
                cooldown,
                &format!("CPU at {:.0}% on {}", cpu, server.name),
                &format!(
                    "CPU usage has been above {}% for {} minutes on server {}",
                    cpu_thresh, cpu_dur, server.name
                ),
            )
            .await;
        }

        // Memory
        if let (Some(used), Some(total)) = (server.mem_used_mb, server.ram_mb) {
            if total > 0 {
                let pct = (used as f64 / total as f64) * 100.0;
                check_threshold(
                    pool,
                    server,
                    "memory",
                    pct,
                    mem_thresh as f64,
                    mem_dur,
                    cooldown,
                    &format!("Memory at {:.0}% on {}", pct, server.name),
                    &format!(
                        "Memory usage has been above {}% for {} minutes on server {}",
                        mem_thresh, mem_dur, server.name
                    ),
                )
                .await;
            }
        }

        // Disk (no duration — disk doesn't fluctuate rapidly)
        if let Some(disk) = server.disk_usage_pct {
            check_threshold(
                pool,
                server,
                "disk",
                disk as f64,
                disk_thresh as f64,
                1, // fire immediately
                cooldown,
                &format!("Disk at {:.0}% on {}", disk, server.name),
                &format!(
                    "Disk usage is above {}% on server {}",
                    disk_thresh, server.name
                ),
            )
            .await;
        }

        // GAP 6: Disk-full forecast — check if disk will be full within 48 hours based on trend
        let disk_trend: Vec<(f32, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
            "SELECT disk_pct, created_at FROM metrics_history \
             WHERE server_id = $1 ORDER BY created_at DESC LIMIT 60",
        )
        .bind(server.id)
        .fetch_all(pool)
        .await
        .unwrap_or_default();

        if disk_trend.len() >= 10 {
            let newest = disk_trend.first().unwrap();
            let oldest = disk_trend.last().unwrap();
            let hours_diff = (newest.1 - oldest.1).num_seconds() as f64 / 3600.0;
            if hours_diff > 0.5 {
                let rate_per_hour = (newest.0 as f64 - oldest.0 as f64) / hours_diff;
                if rate_per_hour > 0.0 {
                    let remaining_pct = 100.0 - newest.0 as f64;
                    let hours_to_full = remaining_pct / rate_per_hour;
                    if hours_to_full < 48.0 && hours_to_full > 0.0 {
                        let severity = if hours_to_full < 12.0 { "critical" } else { "warning" };
                        fire_alert_with_retry(
                            pool,
                            server.user_id,
                            Some(server.id),
                            None,
                            "disk_forecast",
                            severity,
                            &format!("Disk will be full in {:.0} hours on {}", hours_to_full, server.name),
                            &format!(
                                "At the current growth rate of {:.1}%/hour, disk will be full in approximately {:.0} hours. Current usage: {:.1}%",
                                rate_per_hour, hours_to_full, newest.0
                            ),
                        )
                        .await;
                    }
                }
            }
        }

        // GAP 7: Memory leak detection — check for sustained upward trend in memory usage
        let mem_trend: Vec<(f32,)> = sqlx::query_as(
            "SELECT mem_pct FROM metrics_history \
             WHERE server_id = $1 ORDER BY created_at DESC LIMIT 60",
        )
        .bind(server.id)
        .fetch_all(pool)
        .await
        .unwrap_or_default();

        if mem_trend.len() >= 30 {
            let recent_avg: f64 = mem_trend[..10].iter().map(|m| m.0 as f64).sum::<f64>() / 10.0;
            let older_avg: f64 = mem_trend[20..30].iter().map(|m| m.0 as f64).sum::<f64>() / 10.0;
            let increase = recent_avg - older_avg;
            // If memory has risen >10% in the window and is above 60%, warn about leak
            if increase > 10.0 && recent_avg > 60.0 {
                fire_alert_with_retry(
                    pool,
                    server.user_id,
                    Some(server.id),
                    None,
                    "memory_leak",
                    "warning",
                    &format!("Possible memory leak detected on {}", server.name),
                    &format!(
                        "Memory usage has risen {:.1}% in the last hour (from {:.1}% to {:.1}%). \
                         This sustained increase suggests a memory leak.",
                        increase, older_avg, recent_avg
                    ),
                )
                .await;
            }
        }
    }
}

async fn check_threshold(
    pool: &PgPool,
    server: &ServerMetrics,
    alert_type: &str,
    current_value: f64,
    threshold: f64,
    required_duration: i32,
    cooldown_minutes: i32,
    title: &str,
    message: &str,
) {
    let exceeds = current_value > threshold;

    // Get or create alert state
    let state: Option<(String, i32, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT current_state, consecutive_count, last_notified_at \
         FROM alert_state WHERE server_id = $1 AND alert_type = $2 AND state_key = ''",
    )
    .bind(server.id)
    .bind(alert_type)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    let (current_state, consecutive, last_notified) = state
        .clone()
        .unwrap_or(("ok".to_string(), 0, None));

    if exceeds {
        let new_count = consecutive + 1;

        // Upsert state — PostgreSQL serializes concurrent ON CONFLICT upserts
        let _ = sqlx::query(
            "INSERT INTO alert_state (server_id, alert_type, state_key, current_state, consecutive_count, fired_at) \
             VALUES ($1, $2, '', CASE WHEN $3 >= $4 THEN 'firing' ELSE 'pending' END, $3, \
                     CASE WHEN $3 >= $4 THEN NOW() ELSE NULL END) \
             ON CONFLICT (server_id, alert_type, state_key) WHERE server_id IS NOT NULL \
             DO UPDATE SET consecutive_count = $3, \
                          current_state = CASE WHEN $3 >= $4 THEN 'firing' ELSE alert_state.current_state END, \
                          fired_at = CASE WHEN $3 >= $4 AND alert_state.current_state != 'firing' THEN NOW() ELSE alert_state.fired_at END",
        )
        .bind(server.id)
        .bind(alert_type)
        .bind(new_count)
        .bind(required_duration)
        .execute(pool)
        .await;

        // Fire alert if threshold duration met and not already notified within cooldown
        if new_count >= required_duration && (current_state != "firing" || past_cooldown(last_notified, cooldown_minutes)) {
            let severity = if current_value > threshold * 1.1 {
                "critical"
            } else {
                "warning"
            };

            fire_alert_with_retry(
                pool,
                server.user_id,
                Some(server.id),
                None,
                alert_type,
                severity,
                title,
                message,
            )
            .await;

            // Update last_notified
            let _ = sqlx::query(
                "UPDATE alert_state SET last_notified_at = NOW() \
                 WHERE server_id = $1 AND alert_type = $2 AND state_key = ''",
            )
            .bind(server.id)
            .bind(alert_type)
            .execute(pool)
            .await;
        }
    } else if current_state == "firing" {
        // Value dropped below threshold — resolve
        let _ = sqlx::query(
            "UPDATE alert_state SET current_state = 'ok', consecutive_count = 0, fired_at = NULL, last_notified_at = NULL \
             WHERE server_id = $1 AND alert_type = $2 AND state_key = ''",
        )
        .bind(server.id)
        .bind(alert_type)
        .execute(pool)
        .await;

        notifications::resolve_alert(
            pool,
            server.user_id,
            Some(server.id),
            None,
            alert_type,
            &format!("{} recovered on {}", alert_type.to_uppercase(), server.name),
            &format!(
                "{} usage has returned to normal ({:.0}%) on server {}",
                alert_type, current_value, server.name
            ),
        )
        .await;
    } else {
        // Below threshold and not firing — reset counter
        if consecutive > 0 {
            let _ = sqlx::query(
                "UPDATE alert_state SET consecutive_count = 0 \
                 WHERE server_id = $1 AND alert_type = $2 AND state_key = ''",
            )
            .bind(server.id)
            .bind(alert_type)
            .execute(pool)
            .await;
        }
    }
}

fn past_cooldown(
    last_notified: Option<chrono::DateTime<chrono::Utc>>,
    cooldown_minutes: i32,
) -> bool {
    match last_notified {
        None => true,
        Some(t) => {
            let elapsed = chrono::Utc::now() - t;
            elapsed.num_minutes() >= cooldown_minutes as i64
        }
    }
}

// ─── GPU Thresholds ─────────────────────────────────────────────────────

async fn check_gpu_thresholds(pool: &PgPool, agent: &AgentClient) {
    let gpu_info = match agent.get("/apps/gpu-info").await {
        Ok(v) => v,
        Err(_) => return,
    };

    if !gpu_info.get("available").and_then(|v| v.as_bool()).unwrap_or(false) {
        return;
    }
    let gpus = match gpu_info.get("gpus").and_then(|v| v.as_array()) {
        Some(g) if !g.is_empty() => g,
        _ => return,
    };

    // Get the local server
    let server: Option<(Uuid, Uuid, String)> = sqlx::query_as(
        "SELECT id, user_id, name FROM servers ORDER BY created_at ASC LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    let (server_id, user_id, server_name) = match server {
        Some(s) => s,
        None => return,
    };

    let (gpu_util_thresh, gpu_util_dur, gpu_temp_thresh, gpu_vram_thresh, cooldown) =
        notifications::get_gpu_thresholds(pool, user_id, Some(server_id)).await;

    for gpu in gpus {
        let idx = gpu.get("index").and_then(|v| v.as_i64()).unwrap_or(0);
        let name = gpu.get("name").and_then(|v| v.as_str()).unwrap_or("GPU");
        let util = gpu.get("utilization_gpu_pct").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let temp = gpu.get("temperature_c").and_then(|v| v.as_f64());
        let mem_used = gpu.get("memory_used_mb").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let mem_total = gpu.get("memory_total_mb").and_then(|v| v.as_f64()).unwrap_or(1.0);
        let vram_pct = if mem_total > 0.0 { (mem_used / mem_total) * 100.0 } else { 0.0 };

        let state_prefix = format!("gpu_{idx}");

        // GPU utilization threshold (with duration, like CPU)
        check_gpu_metric(
            pool, server_id, user_id, &server_name,
            &format!("{state_prefix}_util"), "gpu_utilization",
            util, gpu_util_thresh as f64, gpu_util_dur, cooldown,
            &format!("GPU {idx} ({name}) at {util:.0}% on {server_name}"),
            &format!("GPU {idx} ({name}) utilization above {gpu_util_thresh}% for {gpu_util_dur} minutes on {server_name}"),
        ).await;

        // GPU temperature threshold (fire immediately, like disk)
        if let Some(t) = temp {
            check_gpu_metric(
                pool, server_id, user_id, &server_name,
                &format!("{state_prefix}_temp"), "gpu_temperature",
                t, gpu_temp_thresh as f64, 1, cooldown,
                &format!("GPU {idx} ({name}) at {t:.0}°C on {server_name}"),
                &format!("GPU {idx} ({name}) temperature above {gpu_temp_thresh}°C on {server_name}. Current: {t:.0}°C"),
            ).await;
        }

        // VRAM threshold (fire immediately)
        check_gpu_metric(
            pool, server_id, user_id, &server_name,
            &format!("{state_prefix}_vram"), "gpu_vram",
            vram_pct, gpu_vram_thresh as f64, 1, cooldown,
            &format!("GPU {idx} ({name}) VRAM at {vram_pct:.0}% on {server_name}"),
            &format!("GPU {idx} ({name}) VRAM above {gpu_vram_thresh}% on {server_name}. Used: {mem_used:.0}/{mem_total:.0} MB"),
        ).await;
    }
}

/// Generic GPU metric threshold check using the existing alert_state machine.
async fn check_gpu_metric(
    pool: &PgPool,
    server_id: Uuid,
    user_id: Uuid,
    server_name: &str,
    state_key: &str,
    alert_type: &str,
    current_value: f64,
    threshold: f64,
    required_duration: i32,
    cooldown_minutes: i32,
    title: &str,
    message: &str,
) {
    let exceeds = current_value > threshold;

    let state: Option<(String, i32, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT current_state, consecutive_count, last_notified_at \
         FROM alert_state WHERE server_id = $1 AND alert_type = $2 AND state_key = $3",
    )
    .bind(server_id)
    .bind(alert_type)
    .bind(state_key)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    let (current_state, consecutive, last_notified) = state
        .clone()
        .unwrap_or(("ok".to_string(), 0, None));

    if exceeds {
        let new_count = consecutive + 1;

        let _ = sqlx::query(
            "INSERT INTO alert_state (server_id, alert_type, state_key, current_state, consecutive_count, fired_at) \
             VALUES ($1, $2, $3, CASE WHEN $4 >= $5 THEN 'firing' ELSE 'pending' END, $4, \
                     CASE WHEN $4 >= $5 THEN NOW() ELSE NULL END) \
             ON CONFLICT (server_id, alert_type, state_key) WHERE server_id IS NOT NULL \
             DO UPDATE SET consecutive_count = $4, \
                          current_state = CASE WHEN $4 >= $5 THEN 'firing' ELSE alert_state.current_state END, \
                          fired_at = CASE WHEN $4 >= $5 AND alert_state.current_state != 'firing' THEN NOW() ELSE alert_state.fired_at END",
        )
        .bind(server_id)
        .bind(alert_type)
        .bind(state_key)
        .bind(new_count)
        .bind(required_duration)
        .execute(pool)
        .await;

        if new_count >= required_duration && (current_state != "firing" || past_cooldown(last_notified, cooldown_minutes)) {
            let severity = if current_value > threshold * 1.1 { "critical" } else { "warning" };

            fire_alert_with_retry(pool, user_id, Some(server_id), None, alert_type, severity, title, message).await;

            let _ = sqlx::query(
                "UPDATE alert_state SET last_notified_at = NOW() \
                 WHERE server_id = $1 AND alert_type = $2 AND state_key = $3",
            )
            .bind(server_id)
            .bind(alert_type)
            .bind(state_key)
            .execute(pool)
            .await;
        }
    } else if current_state == "firing" {
        let _ = sqlx::query(
            "UPDATE alert_state SET current_state = 'ok', consecutive_count = 0, fired_at = NULL, last_notified_at = NULL \
             WHERE server_id = $1 AND alert_type = $2 AND state_key = $3",
        )
        .bind(server_id)
        .bind(alert_type)
        .bind(state_key)
        .execute(pool)
        .await;

        let type_label = match alert_type {
            "gpu_utilization" => "GPU utilization",
            "gpu_temperature" => "GPU temperature",
            "gpu_vram" => "GPU VRAM",
            _ => alert_type,
        };
        notifications::resolve_alert(
            pool, user_id, Some(server_id), None, alert_type,
            &format!("{type_label} recovered on {server_name}"),
            &format!("{type_label} has returned to normal ({current_value:.0}) on server {server_name}"),
        ).await;
    } else if consecutive > 0 {
        let _ = sqlx::query(
            "UPDATE alert_state SET consecutive_count = 0 \
             WHERE server_id = $1 AND alert_type = $2 AND state_key = $3",
        )
        .bind(server_id)
        .bind(alert_type)
        .bind(state_key)
        .execute(pool)
        .await;
    }
}

// ─── Server Offline ─────────────────────────────────────────────────────

async fn check_server_offline(pool: &PgPool) {
    // Find servers that just went offline (status = offline, no firing alert state yet)
    let offline: Vec<(Uuid, Uuid, String)> = match sqlx::query_as(
        "SELECT s.id, s.user_id, s.name FROM servers s \
         WHERE s.status = 'offline' \
         AND NOT EXISTS ( \
             SELECT 1 FROM alert_state \
             WHERE server_id = s.id AND alert_type = 'offline' AND current_state = 'firing' \
         )",
    )
    .fetch_all(pool)
    .await
    {
        Ok(s) => s,
        Err(_) => return,
    };

    for (server_id, user_id, name) in &offline {
        // Create firing state — PostgreSQL serializes concurrent ON CONFLICT upserts
        let _ = sqlx::query(
            "INSERT INTO alert_state (server_id, alert_type, state_key, current_state, fired_at, last_notified_at) \
             VALUES ($1, 'offline', '', 'firing', NOW(), NOW()) \
             ON CONFLICT (server_id, alert_type, state_key) WHERE server_id IS NOT NULL \
             DO UPDATE SET current_state = 'firing', fired_at = NOW(), last_notified_at = NOW()",
        )
        .bind(server_id)
        .execute(pool)
        .await;

        fire_alert_with_retry(
            pool,
            *user_id,
            Some(*server_id),
            None,
            "offline",
            "critical",
            &format!("Server {} is offline", name),
            &format!(
                "Server {} has stopped responding and is now marked offline. Last seen more than 2 minutes ago.",
                name
            ),
        )
        .await;
    }

    // Check for servers that came back online (state firing but server now online)
    let recovered: Vec<(Uuid, Uuid, String)> = match sqlx::query_as(
        "SELECT s.id, s.user_id, s.name FROM servers s \
         JOIN alert_state ast ON ast.server_id = s.id \
         WHERE s.status = 'online' AND ast.alert_type = 'offline' AND ast.current_state = 'firing'",
    )
    .fetch_all(pool)
    .await
    {
        Ok(s) => s,
        Err(_) => return,
    };

    for (server_id, user_id, name) in &recovered {
        let _ = sqlx::query(
            "UPDATE alert_state SET current_state = 'ok', fired_at = NULL, last_notified_at = NULL \
             WHERE server_id = $1 AND alert_type = 'offline'",
        )
        .bind(server_id)
        .execute(pool)
        .await;

        notifications::resolve_alert(
            pool,
            *user_id,
            Some(*server_id),
            None,
            "offline",
            &format!("Server {} is back online", name),
            &format!("Server {} has reconnected and is responding normally.", name),
        )
        .await;
    }
}

// ─── SSL Expiry ─────────────────────────────────────────────────────────

async fn check_ssl_expiry(pool: &PgPool) {
    let sites: Vec<(Uuid, Uuid, String, chrono::DateTime<chrono::Utc>)> = match sqlx::query_as(
        "SELECT s.id, s.user_id, s.domain, s.ssl_expiry \
         FROM sites s WHERE s.ssl_enabled = TRUE AND s.ssl_expiry IS NOT NULL",
    )
    .fetch_all(pool)
    .await
    {
        Ok(s) => s,
        Err(_) => return,
    };

    let now = chrono::Utc::now();

    for (site_id, user_id, domain, ssl_expiry) in &sites {
        let days_left = (*ssl_expiry - now).num_days();
        if days_left < 0 {
            // Already expired
            fire_ssl_alert(pool, *user_id, *site_id, domain, 0, "critical").await;
            continue;
        }

        let (_, _, _, _, _, _, ssl_days_str) =
            notifications::get_thresholds(pool, *user_id, None).await;
        let warning_days: Vec<i64> = ssl_days_str
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();

        // Find the highest warning day that we've crossed
        for &warn_day in &warning_days {
            if days_left <= warn_day {
                // Check if we already warned at this level
                let state: Option<(serde_json::Value,)> = sqlx::query_as(
                    "SELECT COALESCE(metadata, '{}') FROM alert_state \
                     WHERE site_id = $1 AND alert_type = 'ssl_expiry' AND state_key = ''",
                )
                .bind(site_id)
                .fetch_optional(pool)
                .await
                .ok()
                .flatten();

                let last_warned_day = state
                    .as_ref()
                    .and_then(|s| s.0.get("last_warned_day"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(999);

                if warn_day < last_warned_day {
                    let severity = if days_left <= 3 {
                        "critical"
                    } else if days_left <= 7 {
                        "warning"
                    } else {
                        "info"
                    };

                    fire_ssl_alert(pool, *user_id, *site_id, domain, days_left, severity).await;

                    // Update state with last_warned_day — PostgreSQL serializes concurrent ON CONFLICT upserts
                    let _ = sqlx::query(
                        "INSERT INTO alert_state (site_id, alert_type, state_key, current_state, last_notified_at, metadata) \
                         VALUES ($1, 'ssl_expiry', '', 'firing', NOW(), $2) \
                         ON CONFLICT (site_id, alert_type, state_key) WHERE site_id IS NOT NULL \
                         DO UPDATE SET last_notified_at = NOW(), metadata = $2",
                    )
                    .bind(site_id)
                    .bind(serde_json::json!({ "last_warned_day": warn_day }))
                    .execute(pool)
                    .await;
                }

                break; // Only fire once per check for the highest threshold crossed
            }
        }
    }
}

async fn fire_ssl_alert(
    pool: &PgPool,
    user_id: Uuid,
    site_id: Uuid,
    domain: &str,
    days_left: i64,
    severity: &str,
) {
    let title = if days_left <= 0 {
        format!("SSL certificate EXPIRED for {domain}")
    } else {
        format!("SSL certificate expires in {days_left} days for {domain}")
    };

    let message = if days_left <= 0 {
        format!(
            "The SSL certificate for {domain} has expired. Visitors will see security warnings. Renew immediately."
        )
    } else {
        format!(
            "The SSL certificate for {domain} will expire in {days_left} days. Please renew it before it expires."
        )
    };

    fire_alert_with_retry(
        pool, user_id, None, Some(site_id), "ssl_expiry", severity, &title, &message,
    )
    .await;
}

// ─── Service Health ─────────────────────────────────────────────────────

async fn check_service_health(pool: &PgPool, agent: &AgentClient) {
    let services: Vec<serde_json::Value> = match agent.get("/services/health").await {
        Ok(val) => {
            if let Some(arr) = val.as_array() {
                arr.clone()
            } else {
                return;
            }
        }
        Err(e) => {
            tracing::debug!("Service health check skipped: {e}");
            return;
        }
    };

    // Get the local server — find the server with NULL team_id or the first one
    let server: Option<(Uuid, Uuid, String)> = sqlx::query_as(
        "SELECT id, user_id, name FROM servers ORDER BY created_at ASC LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    let (server_id, user_id, server_name) = match server {
        Some(s) => s,
        None => return,
    };

    for svc in &services {
        let name = svc["name"].as_str().unwrap_or("");
        let status = svc["status"].as_str().unwrap_or("unknown");

        if name.is_empty() || status == "not_installed" || status == "disabled" {
            continue;
        }

        if status == "stopped" || status == "failed" {
            // Skip alerting if auto-healer recently handled this service (within 5 minutes)
            let recently_healed: Option<(i64,)> = sqlx::query_as(
                "SELECT COUNT(*) FROM activity_logs \
                 WHERE action = 'auto_heal.restart_service' \
                 AND target_name = $1 \
                 AND created_at > NOW() - INTERVAL '5 minutes'",
            )
            .bind(name)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();

            if recently_healed.map(|r| r.0).unwrap_or(0) > 0 {
                tracing::debug!("Alert engine: skipping {name} alert (auto-healer recently handled it)");
                continue;
            }

            // Check if already firing
            let state: Option<(String,)> = sqlx::query_as(
                "SELECT current_state FROM alert_state \
                 WHERE server_id = $1 AND alert_type = 'service_down' AND state_key = $2",
            )
            .bind(server_id)
            .bind(name)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();

            if state.as_ref().map(|s| s.0.as_str()) != Some("firing") {
                // PostgreSQL serializes concurrent ON CONFLICT upserts
                let _ = sqlx::query(
                    "INSERT INTO alert_state (server_id, alert_type, state_key, current_state, fired_at, last_notified_at) \
                     VALUES ($1, 'service_down', $2, 'firing', NOW(), NOW()) \
                     ON CONFLICT (server_id, alert_type, state_key) WHERE server_id IS NOT NULL \
                     DO UPDATE SET current_state = 'firing', fired_at = NOW(), last_notified_at = NOW()",
                )
                .bind(server_id)
                .bind(name)
                .execute(pool)
                .await;

                fire_alert_with_retry(
                    pool,
                    user_id,
                    Some(server_id),
                    None,
                    "service_down",
                    "critical",
                    &format!("Service {} is {} on {}", name, status, server_name),
                    &format!(
                        "The {} service is {} on server {}. This may cause site downtime.",
                        name, status, server_name
                    ),
                )
                .await;
            }
        } else if status == "running" {
            // Check if was previously firing — resolve
            let state: Option<(String,)> = sqlx::query_as(
                "SELECT current_state FROM alert_state \
                 WHERE server_id = $1 AND alert_type = 'service_down' AND state_key = $2",
            )
            .bind(server_id)
            .bind(name)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();

            if state.as_ref().map(|s| s.0.as_str()) == Some("firing") {
                let _ = sqlx::query(
                    "UPDATE alert_state SET current_state = 'ok', fired_at = NULL, last_notified_at = NULL \
                     WHERE server_id = $1 AND alert_type = 'service_down' AND state_key = $2",
                )
                .bind(server_id)
                .bind(name)
                .execute(pool)
                .await;

                notifications::resolve_alert(
                    pool,
                    user_id,
                    Some(server_id),
                    None,
                    "service_down",
                    &format!("Service {} recovered on {}", name, server_name),
                    &format!("The {} service is running again on server {}.", name, server_name),
                )
                .await;
            }
        }
    }
}

// ─── GAP 8: Docker Container Health ──────────────────────────────────────

async fn check_container_health(pool: &PgPool, agent: &AgentClient) {
    let containers: Vec<serde_json::Value> = match agent.get("/apps").await {
        Ok(val) => {
            if let Some(arr) = val.as_array() {
                arr.clone()
            } else {
                return;
            }
        }
        Err(e) => {
            tracing::debug!("Container health check skipped: {e}");
            return;
        }
    };

    // Get the local server for alert association
    let server: Option<(Uuid, Uuid, String)> = sqlx::query_as(
        "SELECT id, user_id, name FROM servers ORDER BY created_at ASC LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    let (server_id, user_id, _server_name) = match server {
        Some(s) => s,
        None => return,
    };

    for c in &containers {
        let name = c.get("name").and_then(|v| v.as_str()).unwrap_or("unknown");
        let state = c.get("status").and_then(|v| v.as_str()).unwrap_or("");
        let health = c.get("health").and_then(|v| v.as_str());

        if state == "exited" || state == "dead" {
            // Check if already firing for this container
            let existing: Option<(String,)> = sqlx::query_as(
                "SELECT current_state FROM alert_state \
                 WHERE server_id = $1 AND alert_type = 'container_down' AND state_key = $2",
            )
            .bind(server_id)
            .bind(name)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();

            if existing.as_ref().map(|s| s.0.as_str()) != Some("firing") {
                let _ = sqlx::query(
                    "INSERT INTO alert_state (server_id, alert_type, state_key, current_state, fired_at, last_notified_at) \
                     VALUES ($1, 'container_down', $2, 'firing', NOW(), NOW()) \
                     ON CONFLICT (server_id, alert_type, state_key) WHERE server_id IS NOT NULL \
                     DO UPDATE SET current_state = 'firing', fired_at = NOW(), last_notified_at = NOW()",
                )
                .bind(server_id)
                .bind(name)
                .execute(pool)
                .await;

                fire_alert_with_retry(
                    pool,
                    user_id,
                    Some(server_id),
                    None,
                    "container_down",
                    "critical",
                    &format!("Container '{}' is {}", name, state),
                    &format!(
                        "Docker container '{}' has stopped (state: {}). It may need to be restarted.",
                        name, state
                    ),
                )
                .await;
            }
        } else if state == "restarting" {
            // Container in restart loop
            let existing: Option<(String,)> = sqlx::query_as(
                "SELECT current_state FROM alert_state \
                 WHERE server_id = $1 AND alert_type = 'container_crashloop' AND state_key = $2",
            )
            .bind(server_id)
            .bind(name)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();

            if existing.as_ref().map(|s| s.0.as_str()) != Some("firing") {
                let _ = sqlx::query(
                    "INSERT INTO alert_state (server_id, alert_type, state_key, current_state, fired_at, last_notified_at) \
                     VALUES ($1, 'container_crashloop', $2, 'firing', NOW(), NOW()) \
                     ON CONFLICT (server_id, alert_type, state_key) WHERE server_id IS NOT NULL \
                     DO UPDATE SET current_state = 'firing', fired_at = NOW(), last_notified_at = NOW()",
                )
                .bind(server_id)
                .bind(name)
                .execute(pool)
                .await;

                fire_alert_with_retry(
                    pool,
                    user_id,
                    Some(server_id),
                    None,
                    "container_crashloop",
                    "critical",
                    &format!("Container '{}' is crash-looping", name),
                    &format!(
                        "Docker container '{}' is in a restart loop (state: restarting), indicating repeated crashes.",
                        name
                    ),
                )
                .await;
            }
        } else if health == Some("unhealthy") {
            // Container running but health check failing
            let existing: Option<(String,)> = sqlx::query_as(
                "SELECT current_state FROM alert_state \
                 WHERE server_id = $1 AND alert_type = 'container_unhealthy' AND state_key = $2",
            )
            .bind(server_id)
            .bind(name)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();

            if existing.as_ref().map(|s| s.0.as_str()) != Some("firing") {
                let _ = sqlx::query(
                    "INSERT INTO alert_state (server_id, alert_type, state_key, current_state, fired_at, last_notified_at) \
                     VALUES ($1, 'container_unhealthy', $2, 'firing', NOW(), NOW()) \
                     ON CONFLICT (server_id, alert_type, state_key) WHERE server_id IS NOT NULL \
                     DO UPDATE SET current_state = 'firing', fired_at = NOW(), last_notified_at = NOW()",
                )
                .bind(server_id)
                .bind(name)
                .execute(pool)
                .await;

                fire_alert_with_retry(
                    pool,
                    user_id,
                    Some(server_id),
                    None,
                    "container_unhealthy",
                    "warning",
                    &format!("Container '{}' is unhealthy", name),
                    &format!("Docker container '{}' health check is failing.", name),
                )
                .await;
            }
        } else if state == "running" && health != Some("unhealthy") {
            // Container is healthy — resolve any previous container alerts
            for alert_type in &["container_down", "container_unhealthy", "container_crashloop"] {
                let was_firing: Option<(String,)> = sqlx::query_as(
                    "SELECT current_state FROM alert_state \
                     WHERE server_id = $1 AND alert_type = $2 AND state_key = $3",
                )
                .bind(server_id)
                .bind(*alert_type)
                .bind(name)
                .fetch_optional(pool)
                .await
                .ok()
                .flatten();

                if was_firing.as_ref().map(|s| s.0.as_str()) == Some("firing") {
                    let _ = sqlx::query(
                        "UPDATE alert_state SET current_state = 'ok', fired_at = NULL, last_notified_at = NULL \
                         WHERE server_id = $1 AND alert_type = $2 AND state_key = $3",
                    )
                    .bind(server_id)
                    .bind(*alert_type)
                    .bind(name)
                    .execute(pool)
                    .await;

                    notifications::resolve_alert(
                        pool,
                        user_id,
                        Some(server_id),
                        None,
                        alert_type,
                        &format!("Container '{}' recovered", name),
                        &format!("Docker container '{}' is running and healthy again.", name),
                    )
                    .await;
                }
            }
        }
    }
}

// ─── GAP 9: Alert Escalation ────────────────────────────────────────────

/// Re-notify for unacknowledged firing alerts older than 15 minutes.
/// Escalation repeats every 30 minutes until the alert is acknowledged or resolved.
async fn check_escalations(pool: &PgPool) {
    let escalated: Vec<(Uuid, Uuid, String, String)> = match sqlx::query_as(
        "SELECT id, user_id, title, message FROM alerts \
         WHERE status = 'firing' AND acknowledged_at IS NULL \
         AND created_at < NOW() - INTERVAL '15 minutes' \
         AND (escalated_at IS NULL OR escalated_at < NOW() - INTERVAL '30 minutes')",
    )
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::debug!("Escalation query skipped: {e}");
            return;
        }
    };

    for (alert_id, user_id, title, message) in &escalated {
        let esc_subject = format!("[ESCALATED] {}", title);

        // Send escalated notification via user's configured channels
        if let Some(channels) = notifications::get_user_channels(pool, *user_id, None).await {
            let html = format!(
                "<div style=\"font-family:sans-serif;max-width:600px;margin:0 auto\">\
                 <h2 style=\"color:#ef4444\">[ESCALATED] {}</h2>\
                 <p>{}</p>\
                 <p style=\"color:#ef4444;font-weight:bold\">This alert has not been acknowledged. Please investigate immediately.</p>\
                 <p style=\"color:#6b7280;font-size:14px\">Time: {}</p>\
                 </div>",
                title,
                message,
                chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC"),
            );
            notifications::send_notification(pool, &channels, &esc_subject, message, &html).await;
        }

        // Mark escalation timestamp so we don't re-escalate for another 30 minutes
        let _ = sqlx::query("UPDATE alerts SET escalated_at = NOW() WHERE id = $1")
            .bind(alert_id)
            .execute(pool)
            .await;

        tracing::info!("Escalated unacknowledged alert: {}", title);
    }
}
