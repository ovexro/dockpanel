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
                check_server_offline(&pool).await;
                check_ssl_expiry(&pool).await;

                // Service health every 2 minutes (every other tick)
                if tick_count % 2 == 0 {
                    check_service_health(&pool, &agent).await;
                }

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
            Ok(_) => return,
            Err(e) => {
                tracing::warn!("Alert fire attempt {} failed: {}", attempt + 1, e);
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

        // Upsert state
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
        // Create firing state
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

                    // Update state with last_warned_day
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
