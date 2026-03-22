use sqlx::PgPool;
use std::time::Duration;

use crate::services::activity;
use crate::services::agent::AgentClient;
use crate::services::notifications;

/// Background task: auto-heals common issues when detected.
/// Runs every 120 seconds (offset from alert engine to spread load).
pub async fn run(pool: PgPool, agent: AgentClient, mut shutdown_rx: tokio::sync::broadcast::Receiver<()>) {
    tracing::info!("Auto-healer started");

    // Initial delay (90s offset from alert engine's 30s, respects shutdown)
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(90)) => {}
        _ = shutdown_rx.recv() => {
            tracing::info!("Auto-healer shutting down gracefully (during initial delay)");
            return;
        }
    }

    // Track when we last ran retention cleanup (once per day)
    let mut last_retention = std::time::Instant::now() - Duration::from_secs(86400);

    let mut interval = tokio::time::interval(Duration::from_secs(120));

    loop {
        tokio::select! {
            _ = interval.tick() => {}
            _ = shutdown_rx.recv() => {
                tracing::info!("Auto-healer shutting down gracefully");
                return;
            }
        }

        // Data retention cleanup runs daily regardless of auto-heal setting
        if last_retention.elapsed() >= Duration::from_secs(86400) {
            run_retention_cleanup(&pool).await;
            last_retention = std::time::Instant::now();
        }

        // Only run auto-healing if enabled globally
        let enabled = is_enabled(&pool).await;
        if !enabled {
            continue;
        }

        auto_restart_services(&pool, &agent).await;
        auto_clean_disk(&pool, &agent).await;
        auto_renew_ssl(&pool, &agent).await;
    }
}

/// Check if auto-healing is enabled in settings.
async fn is_enabled(pool: &PgPool) -> bool {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT value FROM settings WHERE key = 'auto_heal_enabled'",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    row.map(|r| r.0 == "true").unwrap_or(false)
}

/// Auto-restart crashed services (service_down alerts that are firing).
async fn auto_restart_services(pool: &PgPool, agent: &AgentClient) {
    // Find service_down alerts that are currently firing
    let firing: Vec<(String,)> = match sqlx::query_as(
        "SELECT state_key FROM alert_state \
         WHERE alert_type = 'service_down' AND current_state = 'firing' AND state_key != ''",
    )
    .fetch_all(pool)
    .await
    {
        Ok(s) => s,
        Err(_) => return,
    };

    for (service_name,) in &firing {
        if service_name.is_empty() {
            continue;
        }

        // GAP 12: Check restart count in last 30 minutes — give up after 3 attempts
        let restart_count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM activity_logs \
             WHERE action = 'auto_heal.restart_service' \
             AND target_name = $1 \
             AND created_at > NOW() - INTERVAL '30 minutes'",
        )
        .bind(service_name)
        .fetch_one(pool)
        .await
        .unwrap_or((0,));

        if restart_count.0 >= 3 {
            // Stop healing — service is in a crash loop. Create incident and notify.
            tracing::warn!("Auto-healer gave up on {service_name} after 3 restarts in 30 minutes");

            // Get user_id from the first server for the incident
            let server: Option<(uuid::Uuid, uuid::Uuid, String)> = sqlx::query_as(
                "SELECT id, user_id, name FROM servers ORDER BY created_at ASC LIMIT 1",
            )
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();

            if let Some((_server_id, user_id, server_name)) = server {
                let incident_title = format!("Auto-healer exhausted: {} keeps crashing on {}", service_name, server_name);
                let incident_msg = format!(
                    "{} has been restarted 3 times in 30 minutes on {} without recovering. Manual intervention required.",
                    service_name, server_name
                );

                // Create managed incident
                let _ = sqlx::query(
                    "INSERT INTO managed_incidents (user_id, title, status, severity, description, visible_on_status_page) \
                     VALUES ($1, $2, 'investigating', 'critical', $3, TRUE)",
                )
                .bind(user_id)
                .bind(&incident_title)
                .bind(&incident_msg)
                .execute(pool)
                .await;

                // Send critical notification
                if let Some(channels) = notifications::get_user_channels(pool, user_id, None).await {
                    let subject = format!("[CRITICAL] Auto-healer gave up on {}", service_name);
                    let html = format!(
                        "<div style=\"font-family:sans-serif;max-width:600px;margin:0 auto\">\
                         <h2 style=\"color:#ef4444\">{subject}</h2>\
                         <p>{incident_msg}</p>\
                         <p style=\"color:#ef4444;font-weight:bold\">Automatic restarts have been exhausted. Manual intervention is required.</p>\
                         <p style=\"color:#6b7280;font-size:14px\">Time: {}</p>\
                         </div>",
                        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC"),
                    );
                    notifications::send_notification(pool, &channels, &subject, &incident_msg, &html).await;
                }

                // Panel notification
                notifications::notify_panel(pool, None, &format!("Auto-healer exhausted: {}", service_name), &format!("{} keeps crashing after 3 restart attempts. Manual intervention required.", service_name), "critical", "auto_heal", Some("/incidents")).await;

                // Log the exhaustion event
                crate::services::system_log::log_event(
                    pool,
                    "error",
                    "auto_healer",
                    &format!("Gave up on {service_name}: 3 restarts in 30 minutes without recovery"),
                    Some(&incident_msg),
                ).await;
            }

            // Clear the firing alert state so we don't keep trying
            let _ = sqlx::query(
                "UPDATE alert_state SET current_state = 'exhausted' \
                 WHERE alert_type = 'service_down' AND state_key = $1 AND current_state = 'firing'",
            )
            .bind(service_name)
            .execute(pool)
            .await;

            continue;
        }

        // Check if we already tried to heal this service recently (10-minute cooldown between attempts)
        let recent_heal: Option<(i64,)> = sqlx::query_as(
            "SELECT COUNT(*) FROM activity_logs \
             WHERE action = 'auto_heal.restart_service' \
             AND target_name = $1 \
             AND created_at > NOW() - INTERVAL '10 minutes'",
        )
        .bind(service_name)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();

        if recent_heal.map(|r| r.0).unwrap_or(0) > 0 {
            tracing::debug!("Auto-heal: skipping {service_name} (recently attempted)");
            continue;
        }

        tracing::info!("Auto-heal: restarting service {service_name} (attempt {} of 3 in 30m window)", restart_count.0 + 1);

        let result = agent
            .post(
                "/diagnostics/fix",
                Some(serde_json::json!({ "fix_id": format!("restart-service:{service_name}") })),
            )
            .await;

        let success = result.is_ok();
        let details = match &result {
            Ok(v) => v.to_string(),
            Err(e) => e.to_string(),
        };

        if !success {
            crate::services::system_log::log_event(
                pool,
                "error",
                "auto_healer",
                &format!("Failed to restart service: {service_name}"),
                Some(&details),
            ).await;
        }

        // Log the auto-healing action
        // Use a system UUID for auto-healer activity
        let system_id = uuid::Uuid::nil();
        activity::log_activity(
            pool,
            system_id,
            "auto-healer",
            "auto_heal.restart_service",
            Some("service"),
            Some(service_name),
            Some(&format!("success={success}, result={details}")),
            None,
        )
        .await;

        // If the restart succeeded, update alert_state to "ok" and resolve firing alerts
        // so the alert engine doesn't re-fire before its next health check confirms recovery
        if success {
            let _ = sqlx::query(
                "UPDATE alert_state SET current_state = 'ok', fired_at = NULL, last_notified_at = NULL \
                 WHERE alert_type = 'service_down' AND state_key = $1 AND current_state = 'firing'",
            )
            .bind(service_name)
            .execute(pool)
            .await;

            // Get the local server for the resolve notification
            let server: Option<(uuid::Uuid, uuid::Uuid, String)> = sqlx::query_as(
                "SELECT id, user_id, name FROM servers ORDER BY created_at ASC LIMIT 1",
            )
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();

            if let Some((server_id, user_id, server_name)) = server {
                notifications::resolve_alert(
                    pool,
                    user_id,
                    Some(server_id),
                    None,
                    "service_down",
                    &format!("Service {} auto-healed on {}", service_name, server_name),
                    &format!(
                        "The {} service was automatically restarted by auto-healer on server {}.",
                        service_name, server_name
                    ),
                )
                .await;
            }

            tracing::info!("Auto-heal: service {service_name} restarted successfully, alert resolved");
        }
    }

    // Auto-restart exited/dead Docker containers
    if let Ok(containers) = agent.get("/apps").await {
        if let Some(arr) = containers.as_array() {
            for c in arr {
                let name = c.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let state = c.get("state").and_then(|v| v.as_str()).unwrap_or("");
                let container_id = c.get("id").and_then(|v| v.as_str()).unwrap_or("");

                if (state == "exited" || state == "dead") && !name.is_empty() && !container_id.is_empty() {
                    // Check restart count in last 30 minutes — give up after 3 attempts
                    let restart_count: (i64,) = sqlx::query_as(
                        "SELECT COUNT(*) FROM activity_logs \
                         WHERE action = 'auto_heal.container_restart' AND target_name = $1 \
                         AND created_at > NOW() - INTERVAL '30 minutes'"
                    ).bind(name).fetch_one(pool).await.unwrap_or((0,));

                    if restart_count.0 >= 3 {
                        tracing::warn!("Auto-healer gave up on container {name} after 3 restarts in 30 minutes");
                        continue;
                    }

                    // 10-minute cooldown between attempts
                    let recent_heal: (i64,) = sqlx::query_as(
                        "SELECT COUNT(*) FROM activity_logs \
                         WHERE action = 'auto_heal.container_restart' AND target_name = $1 \
                         AND created_at > NOW() - INTERVAL '10 minutes'"
                    ).bind(name).fetch_one(pool).await.unwrap_or((0,));

                    if recent_heal.0 > 0 {
                        continue;
                    }

                    tracing::info!("Auto-heal: restarting container {name} (attempt {} of 3)", restart_count.0 + 1);

                    let result = agent.post(
                        &format!("/apps/{}/restart", container_id),
                        None::<serde_json::Value>,
                    ).await;

                    let success = result.is_ok();
                    let system_id = uuid::Uuid::nil();
                    activity::log_activity(
                        pool, system_id, "auto-healer", "auto_heal.container_restart",
                        Some("container"), Some(name),
                        Some(&format!("success={success}, state={state}")),
                        None,
                    ).await;

                    if success {
                        tracing::info!("Auto-healer: restarted container {name}");
                    } else {
                        tracing::warn!("Auto-healer: failed to restart container {name}");
                    }
                }
            }
        }
    }
}

/// Auto-clean logs when disk usage > 90%.
async fn auto_clean_disk(pool: &PgPool, agent: &AgentClient) {
    // Check if disk alert is firing
    let firing: Option<(String,)> = sqlx::query_as(
        "SELECT current_state FROM alert_state \
         WHERE alert_type = 'disk' AND current_state = 'firing' LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    if firing.is_none() {
        return;
    }

    // Check if we already cleaned recently
    let recent: Option<(i64,)> = sqlx::query_as(
        "SELECT COUNT(*) FROM activity_logs \
         WHERE action = 'auto_heal.clean_logs' \
         AND created_at > NOW() - INTERVAL '1 hour'",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    if recent.map(|r| r.0).unwrap_or(0) > 0 {
        return;
    }

    tracing::info!("Auto-heal: cleaning logs to free disk space");

    let result = agent
        .post(
            "/diagnostics/fix",
            Some(serde_json::json!({ "fix_id": "clean-logs:all" })),
        )
        .await;

    let success = result.is_ok();
    let system_id = uuid::Uuid::nil();
    activity::log_activity(
        pool,
        system_id,
        "auto-healer",
        "auto_heal.clean_logs",
        Some("system"),
        Some("logs"),
        Some(&format!("success={success}")),
        None,
    )
    .await;

    // If cleanup succeeded, reset the disk alert_state so the alert engine doesn't
    // re-fire immediately (let it re-evaluate on the next cycle with fresh metrics)
    if success {
        let _ = sqlx::query(
            "UPDATE alert_state SET current_state = 'ok', consecutive_count = 0, \
             fired_at = NULL, last_notified_at = NULL \
             WHERE alert_type = 'disk' AND current_state = 'firing'",
        )
        .execute(pool)
        .await;

        tracing::info!("Auto-heal: disk cleanup succeeded, disk alert state reset");

        // Panel notification
        notifications::notify_panel(pool, None, "Disk cleanup completed", "Automatic disk cleanup was performed to free space", "info", "auto_heal", None).await;
    }
}

/// Auto-renew SSL certs expiring within 30 days.
/// Let's Encrypt certs last 90 days; renewing at 30 days gives a comfortable margin
/// and aligns with the standard certbot renewal window.
async fn auto_renew_ssl(pool: &PgPool, agent: &AgentClient) {
    // Fetch sites with SSL expiring within 30 days, along with owner email and site details
    // needed to call the agent's /ssl/provision/{domain} endpoint.
    let sites: Vec<(uuid::Uuid, String, uuid::Uuid, String, Option<i32>, Option<String>, Option<String>)> = match sqlx::query_as(
        "SELECT s.id, s.domain, s.user_id, s.runtime, s.proxy_port, s.php_version, s.root_path \
         FROM sites s \
         WHERE s.ssl_enabled = TRUE AND s.ssl_expiry IS NOT NULL \
         AND s.ssl_expiry < NOW() + INTERVAL '30 days'",
    )
    .fetch_all(pool)
    .await
    {
        Ok(s) => s,
        Err(_) => return,
    };

    for (site_id, domain, user_id, runtime, proxy_port, php_version, root_path) in &sites {
        // 6-hour cooldown prevents hammering Let's Encrypt if renewal keeps failing.
        // With a 30-day threshold, we get ~120 retry windows before expiry.
        let recent: Option<(i64,)> = sqlx::query_as(
            "SELECT COUNT(*) FROM activity_logs \
             WHERE action = 'auto_heal.renew_ssl' \
             AND target_name = $1 \
             AND created_at > NOW() - INTERVAL '6 hours'",
        )
        .bind(domain)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();

        if recent.map(|r| r.0).unwrap_or(0) > 0 {
            continue;
        }

        tracing::info!("Auto-heal: renewing SSL for {domain}");

        // Look up the site owner's email for ACME registration
        let email: String = match sqlx::query_scalar(
            "SELECT email FROM users WHERE id = $1",
        )
        .bind(user_id)
        .fetch_optional(pool)
        .await
        {
            Ok(Some(e)) => e,
            _ => {
                tracing::warn!("Auto-heal: cannot renew SSL for {domain} — owner email not found");
                continue;
            }
        };

        // Build the provision request body (same format as /api/sites/{id}/ssl)
        let mut agent_body = serde_json::json!({
            "email": email,
            "runtime": runtime,
        });
        if let Some(port) = proxy_port {
            agent_body["proxy_port"] = serde_json::json!(port);
        }
        if let Some(php) = php_version {
            agent_body["php_socket"] = serde_json::json!(format!("/run/php/php{php}-fpm.sock"));
        }
        if let Some(root) = root_path {
            agent_body["root"] = serde_json::json!(root);
        }

        // Call the correct agent endpoint: /ssl/provision/{domain}
        let agent_path = format!("/ssl/provision/{domain}");
        let result = agent.post(&agent_path, Some(agent_body)).await;

        let success = result.is_ok();
        let details = match &result {
            Ok(v) => v.to_string(),
            Err(e) => e.to_string(),
        };

        let system_id = uuid::Uuid::nil();
        activity::log_activity(
            pool,
            system_id,
            "auto-healer",
            "auto_heal.renew_ssl",
            Some("site"),
            Some(domain),
            Some(&format!("site_id={site_id}, success={success}, result={details}")),
            None,
        )
        .await;

        if success {
            // Update ssl_expiry from the agent response if available
            if let Ok(ref resp) = result {
                let new_expiry = resp
                    .get("expiry")
                    .and_then(|v| v.as_str())
                    .and_then(|s| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f UTC").ok())
                    .map(|dt| dt.and_utc());

                if let Some(expiry) = new_expiry {
                    let _ = sqlx::query(
                        "UPDATE sites SET ssl_expiry = $1, updated_at = NOW() WHERE id = $2",
                    )
                    .bind(expiry)
                    .bind(site_id)
                    .execute(pool)
                    .await;
                }
            }
            tracing::info!("Auto-heal: SSL renewed for {domain}");

            // Panel notification
            notifications::notify_panel(pool, None, &format!("SSL renewed: {}", domain), &format!("SSL certificate for {} was automatically renewed", domain), "info", "ssl", None).await;
        } else {
            // Fire an alert so the user is notified about the SSL renewal failure
            let server: Option<(uuid::Uuid,)> = sqlx::query_as(
                "SELECT id FROM servers ORDER BY created_at ASC LIMIT 1",
            )
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();

            notifications::fire_alert(
                pool,
                *user_id,
                server.map(|s| s.0),
                Some(*site_id),
                "ssl_renewal_failure",
                "critical",
                &format!("SSL renewal failed: {domain}"),
                &format!(
                    "Auto-healer failed to renew the SSL certificate for {domain}: {details}. \
                     The certificate may expire soon — check the domain configuration and DNS."
                ),
            )
            .await;

            crate::services::system_log::log_event(
                pool,
                "error",
                "auto_healer",
                &format!("SSL renewal failed for {domain}"),
                Some(&details),
            ).await;

            tracing::warn!("Auto-heal: SSL renewal failed for {domain}: {details}");
        }
    }
}

/// Periodic data retention cleanup: removes old records to keep the database lean.
async fn run_retention_cleanup(pool: &PgPool) {
    tracing::info!("Running data retention cleanup...");

    // Delete monitor_checks older than 7 days
    match sqlx::query("DELETE FROM monitor_checks WHERE checked_at < NOW() - INTERVAL '7 days'")
        .execute(pool)
        .await
    {
        Ok(r) => {
            if r.rows_affected() > 0 {
                tracing::info!(
                    "Retention: deleted {} old monitor_checks",
                    r.rows_affected()
                );
            }
        }
        Err(e) => tracing::warn!("Retention cleanup (monitor_checks) failed: {e}"),
    }

    // Delete resolved alerts older than 90 days
    match sqlx::query(
        "DELETE FROM alerts WHERE status = 'resolved' AND created_at < NOW() - INTERVAL '90 days'",
    )
    .execute(pool)
    .await
    {
        Ok(r) => {
            if r.rows_affected() > 0 {
                tracing::info!("Retention: deleted {} old resolved alerts", r.rows_affected());
            }
        }
        Err(e) => tracing::warn!("Retention cleanup (alerts) failed: {e}"),
    }

    // Delete activity_logs older than 1 year
    match sqlx::query("DELETE FROM activity_logs WHERE created_at < NOW() - INTERVAL '1 year'")
        .execute(pool)
        .await
    {
        Ok(r) => {
            if r.rows_affected() > 0 {
                tracing::info!(
                    "Retention: deleted {} old activity_logs",
                    r.rows_affected()
                );
            }
        }
        Err(e) => tracing::warn!("Retention cleanup (activity_logs) failed: {e}"),
    }

    // Delete system_logs older than 30 days
    match sqlx::query("DELETE FROM system_logs WHERE created_at < NOW() - INTERVAL '30 days'")
        .execute(pool)
        .await
    {
        Ok(r) => {
            if r.rows_affected() > 0 {
                tracing::info!(
                    "Retention: deleted {} old system_logs",
                    r.rows_affected()
                );
            }
        }
        Err(e) => tracing::warn!("Retention cleanup (system_logs) failed: {e}"),
    }

    // Extension events: 90 days
    let ext_events_deleted = sqlx::query("DELETE FROM extension_events WHERE delivered_at < NOW() - INTERVAL '90 days'")
        .execute(pool).await.ok().map(|r| r.rows_affected()).unwrap_or(0);
    if ext_events_deleted > 0 {
        tracing::info!("Retention: deleted {ext_events_deleted} extension events (>90 days)");
    }

    // GAP 18: Webhook gateway deliveries: 7 days
    let wh_deleted = sqlx::query("DELETE FROM webhook_deliveries WHERE received_at < NOW() - INTERVAL '7 days'")
        .execute(pool).await.ok().map(|r| r.rows_affected()).unwrap_or(0);
    if wh_deleted > 0 {
        tracing::info!("Retention: deleted {wh_deleted} webhook deliveries (>7 days)");
    }

    // Backup verifications: 90 days
    let bv_deleted = sqlx::query("DELETE FROM backup_verifications WHERE created_at < NOW() - INTERVAL '90 days'")
        .execute(pool).await.ok().map(|r| r.rows_affected()).unwrap_or(0);
    if bv_deleted > 0 {
        tracing::info!("Retention: deleted {bv_deleted} backup verifications (>90 days)");
    }

    // User sessions: 24 hours since last seen (JWT expires after 2h, but clean stale records)
    let sess_deleted = sqlx::query("DELETE FROM user_sessions WHERE last_seen_at < NOW() - INTERVAL '24 hours'")
        .execute(pool).await.ok().map(|r| r.rows_affected()).unwrap_or(0);
    if sess_deleted > 0 {
        tracing::info!("Retention: deleted {sess_deleted} expired user sessions (>24h)");
    }

    // Panel notifications: 30 days
    let notif_deleted = sqlx::query("DELETE FROM panel_notifications WHERE created_at < NOW() - INTERVAL '30 days'")
        .execute(pool).await.ok().map(|r| r.rows_affected()).unwrap_or(0);
    if notif_deleted > 0 {
        tracing::info!("Retention: deleted {notif_deleted} panel notifications (>30 days)");
    }
}
