use crate::safe_cmd::safe_command;
use chrono::Datelike;
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
        auto_sleep_idle_containers(&pool, &agent).await;

        // Security hardening tasks (run every 2 minutes with auto-healer)
        security_ingest_suspicious_events(&pool).await;
        security_check_lockdown_expiry(&pool).await;
        security_check_canary_files(&pool).await;
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
    // Recovery check: if an exhausted service is now running, clear the state
    let exhausted_services: Vec<(String,)> = sqlx::query_as(
        "SELECT state_key FROM alert_state WHERE alert_type = 'service_down' AND current_state = 'exhausted'"
    ).fetch_all(pool).await.unwrap_or_default();

    if !exhausted_services.is_empty() {
        if let Ok(health_result) = agent.get("/services/health").await {
            if let Some(services_arr) = health_result.as_array() {
                for (service_name,) in &exhausted_services {
                    let is_running = services_arr.iter().any(|svc| {
                        svc.get("name").and_then(|n| n.as_str()) == Some(service_name.as_str())
                            && svc.get("status").and_then(|s| s.as_str()) == Some("running")
                    });

                    if is_running {
                        // Service recovered! Clear exhausted state
                        let _ = sqlx::query(
                            "DELETE FROM alert_state WHERE alert_type = 'service_down' AND state_key = $1 AND current_state = 'exhausted'"
                        ).bind(service_name).execute(pool).await;
                        tracing::info!("Auto-healer: {service_name} recovered, cleared exhausted state");

                        // Resolve the associated incident
                        let _ = sqlx::query(
                            "UPDATE managed_incidents SET status = 'resolved', updated_at = NOW() \
                             WHERE title LIKE $1 AND status NOT IN ('resolved', 'postmortem')"
                        ).bind(format!("%{}%", service_name)).execute(pool).await;

                        notifications::notify_panel(pool, None,
                            &format!("Service recovered: {}", service_name),
                            &format!("{} is running again after auto-healer exhaustion. Monitoring resumed.", service_name),
                            "info", "auto_heal", Some("/incidents")).await;

                        crate::services::system_log::log_event(
                            pool, "info", "auto_healer",
                            &format!("{service_name} recovered from exhausted state, monitoring resumed"),
                            None,
                        ).await;
                    }
                }
            }
        }
    }

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

    // GAP 35: Also clean /tmp and prune Docker when disk is critical
    if success {
        tracing::info!("Auto-heal: cleaning /tmp files older than 7 days");
        let _ = agent.post(
            "/diagnostics/fix",
            Some(serde_json::json!({ "fix_id": "clean-tmp:all" })),
        ).await;

        tracing::info!("Auto-heal: running Docker system prune");
        let _ = agent.post(
            "/diagnostics/fix",
            Some(serde_json::json!({ "fix_id": "docker-prune:all" })),
        ).await;
    }

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
        notifications::notify_panel(pool, None, "Disk cleanup completed", "Automatic disk cleanup was performed to free space (logs + /tmp + Docker prune)", "info", "auto_heal", None).await;
    }
}

/// Auto-renew SSL certs using ACME Renewal Information (RFC 9773) when
/// available, falling back to a profile-aware static threshold.
///
/// Two phases per run:
/// 1. **ARI refresh** — for each SSL site whose suggestion is missing or
///    stale, fetch `/ssl/{domain}/renewal-info` from the agent and store
///    the suggested renewal window.
/// 2. **Renewal** — for sites whose `ssl_renewal_at` has passed (or whose
///    fallback threshold is hit), call `/ssl/{domain}/renew`.
///
/// The agent reads the prior cert PEM from disk and passes it as the ARI
/// `replaces` hint, so the CA sees a continuous issuance chain.
async fn auto_renew_ssl(pool: &PgPool, agent: &AgentClient) {
    // Widen the window to 45 days so we pick up short-lived (6-day) and
    // 45-day-profile certs with enough lead time. ARI trims this further.
    let sites: Vec<(
        uuid::Uuid, String, uuid::Uuid, String, Option<i32>, Option<String>, Option<String>,
        chrono::DateTime<chrono::Utc>,
        Option<chrono::DateTime<chrono::Utc>>,
        Option<chrono::DateTime<chrono::Utc>>,
        Option<String>,
    )> = match sqlx::query_as(
        "SELECT s.id, s.domain, s.user_id, s.runtime, s.proxy_port, s.php_version, s.root_path, \
                s.ssl_expiry, s.ssl_renewal_at, s.ssl_renewal_checked_at, s.ssl_profile \
         FROM sites s \
         WHERE s.ssl_enabled = TRUE AND s.ssl_expiry IS NOT NULL \
         AND s.ssl_expiry < NOW() + INTERVAL '45 days'",
    )
    .fetch_all(pool)
    .await
    {
        Ok(s) => s,
        Err(_) => return,
    };

    let now = chrono::Utc::now();

    for row in &sites {
        let (site_id, domain, user_id, runtime, proxy_port, php_version, root_path,
             ssl_expiry, ssl_renewal_at_initial, ssl_renewal_checked_at, ssl_profile) = row;
        let mut ssl_renewal_at = *ssl_renewal_at_initial;
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

        // Phase 1 — refresh ARI suggestion if stale or missing.
        let needs_ari = ssl_renewal_checked_at
            .map(|t| (now - t) > chrono::Duration::hours(6))
            .unwrap_or(true);
        if needs_ari {
            let ari_path = format!(
                "/ssl/{domain}/renewal-info?email={}",
                urlencoding::encode(&email)
            );
            match agent.get(&ari_path).await {
                Ok(v) => {
                    let when = v
                        .get("suggestion")
                        .and_then(|s| s.get("renewal_at"))
                        .and_then(|x| x.as_str())
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .map(|dt| dt.with_timezone(&chrono::Utc));

                    let _ = sqlx::query(
                        "UPDATE sites \
                         SET ssl_renewal_at = $1, ssl_renewal_checked_at = NOW(), updated_at = NOW() \
                         WHERE id = $2",
                    )
                    .bind(when)
                    .bind(site_id)
                    .execute(pool)
                    .await;
                    if let Some(when) = when {
                        ssl_renewal_at = Some(when);
                    }
                }
                Err(e) => {
                    tracing::debug!("ARI fetch for {domain} failed: {e}");
                }
            }
        }

        // Decide if this cert is due for renewal.
        let is_due = match ssl_renewal_at {
            Some(when) => when <= now,
            None => {
                // Fallback: profile-aware margin derived from expiry.
                let margin = fallback_renewal_margin(ssl_profile.as_deref());
                (*ssl_expiry - now) <= margin
            }
        };
        if !is_due {
            continue;
        }

        // 6-hour cooldown prevents hammering the CA if renewal keeps failing.
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

        // Build the renew request body. The agent reads the prior PEM from
        // disk and attaches it as the ARI `replaces` hint automatically.
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
        if let Some(profile) = ssl_profile.as_deref() {
            agent_body["profile"] = serde_json::json!(profile);
        }

        let agent_path = format!("/ssl/{domain}/renew");
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
                    // Clear ssl_renewal_at so the next auto-heal cycle
                    // re-fetches ARI against the fresh cert.
                    let _ = sqlx::query(
                        "UPDATE sites SET ssl_expiry = $1, ssl_renewal_at = NULL, \
                         ssl_renewal_checked_at = NULL, updated_at = NOW() WHERE id = $2",
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

/// Fallback renewal margin when the CA doesn't advertise ARI. Maps profile
/// → days-remaining threshold at which we trigger renewal.
///
/// - `shortlived` (~6d): renew at 2d remaining (≈ 2/3 consumed, matches LE's
///   "renew every 2-3 days" guidance).
/// - `tlsserver` (45d from 2026-05-13 onward): renew at 15d remaining (1/3).
/// - `classic` or unknown (90d today, 64d in 2027, 45d in 2028): renew at
///   30d remaining, which is safe across all three horizons.
fn fallback_renewal_margin(profile: Option<&str>) -> chrono::Duration {
    match profile {
        Some("shortlived") => chrono::Duration::days(2),
        Some("tlsserver") => chrono::Duration::days(15),
        _ => chrono::Duration::days(30),
    }
}

/// Weekly digest: sends a summary email to all admins on Mondays.
async fn send_weekly_digest(pool: &PgPool) {
    let today = chrono::Utc::now().weekday();
    if today != chrono::Weekday::Mon {
        return;
    }

    // Gather stats for last 7 days
    let alerts_7d: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM alerts WHERE created_at > NOW() - INTERVAL '7 days'",
    )
    .fetch_one(pool)
    .await
    .unwrap_or((0,));

    let backups_7d: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM backups WHERE created_at > NOW() - INTERVAL '7 days'",
    )
    .fetch_one(pool)
    .await
    .unwrap_or((0,));

    let incidents_7d: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM managed_incidents WHERE created_at > NOW() - INTERVAL '7 days'",
    )
    .fetch_one(pool)
    .await
    .unwrap_or((0,));

    let deploys_7d: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM deploy_logs WHERE created_at > NOW() - INTERVAL '7 days'",
    )
    .fetch_one(pool)
    .await
    .unwrap_or((0,));

    let security_scans_7d: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM security_scans WHERE created_at > NOW() - INTERVAL '7 days'",
    )
    .fetch_one(pool)
    .await
    .unwrap_or((0,));

    let body_html = format!(
        r#"<div style="font-family: sans-serif; max-width: 600px; margin: 0 auto;">
            <h2 style="color: #4f46e5;">DockPanel Weekly Summary</h2>
            <p>Here's what happened in the last 7 days:</p>
            <table style="border-collapse: collapse; width: 100%; margin: 16px 0;">
                <tr><td style="padding: 8px; border-bottom: 1px solid #e5e7eb; font-weight: 600;">Alerts</td><td style="padding: 8px; border-bottom: 1px solid #e5e7eb;">{}</td></tr>
                <tr><td style="padding: 8px; border-bottom: 1px solid #e5e7eb; font-weight: 600;">Backups</td><td style="padding: 8px; border-bottom: 1px solid #e5e7eb;">{}</td></tr>
                <tr><td style="padding: 8px; border-bottom: 1px solid #e5e7eb; font-weight: 600;">Incidents</td><td style="padding: 8px; border-bottom: 1px solid #e5e7eb;">{}</td></tr>
                <tr><td style="padding: 8px; border-bottom: 1px solid #e5e7eb; font-weight: 600;">Deploys</td><td style="padding: 8px; border-bottom: 1px solid #e5e7eb;">{}</td></tr>
                <tr><td style="padding: 8px; border-bottom: 1px solid #e5e7eb; font-weight: 600;">Security Scans</td><td style="padding: 8px; border-bottom: 1px solid #e5e7eb;">{}</td></tr>
            </table>
            <p style="color: #6b7280; font-size: 14px;">Log in to your DockPanel dashboard for full details.</p>
        </div>"#,
        alerts_7d.0, backups_7d.0, incidents_7d.0, deploys_7d.0, security_scans_7d.0,
    );

    // Send to all admin users
    let admins: Vec<(String,)> = sqlx::query_as(
        "SELECT email FROM users WHERE role = 'admin'",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    for (email,) in &admins {
        if let Err(e) = crate::services::email::send_email(
            pool,
            email,
            "DockPanel Weekly Summary",
            &body_html,
        )
        .await
        {
            tracing::warn!("Weekly digest email to {email} failed: {e}");
        }
    }

    if !admins.is_empty() {
        tracing::info!(
            "Weekly digest sent to {} admin(s): {} alerts, {} backups, {} incidents, {} deploys",
            admins.len(), alerts_7d.0, backups_7d.0, incidents_7d.0, deploys_7d.0,
        );
    }
}

/// Periodic data retention cleanup: removes old records to keep the database lean.
async fn run_retention_cleanup(pool: &PgPool) {
    tracing::info!("Running data retention cleanup...");

    // GAP 33: Weekly digest — send summary email on Mondays during cleanup cycle
    send_weekly_digest(pool).await;

    // GAP 67: Read configurable retention periods from settings (fall back to defaults)
    let settings: Vec<(String, String)> = sqlx::query_as(
        "SELECT key, value FROM settings WHERE key LIKE 'retention_%'"
    ).fetch_all(pool).await.unwrap_or_default();

    let get = |key: &str, default: i64| -> i64 {
        settings.iter().find(|(k, _)| k == key).and_then(|(_, v)| v.parse().ok()).unwrap_or(default)
    };

    let activity_days = get("retention_activity_days", 365);
    let system_log_days = get("retention_system_log_days", 30);
    let alert_days = get("retention_alert_days", 90);
    let scan_days = get("retention_scan_days", 90);
    let webhook_days = get("retention_webhook_days", 7);
    let notification_days = get("retention_notification_days", 30);
    let monitor_days = get("retention_monitor_days", 7);

    // Delete monitor_checks older than configured days (default 7)
    match sqlx::query(&format!("DELETE FROM monitor_checks WHERE checked_at < NOW() - INTERVAL '{monitor_days} days'"))
        .execute(pool)
        .await
    {
        Ok(r) => {
            if r.rows_affected() > 0 {
                tracing::info!(
                    "Retention: deleted {} old monitor_checks (>{monitor_days} days)",
                    r.rows_affected()
                );
            }
        }
        Err(e) => tracing::warn!("Retention cleanup (monitor_checks) failed: {e}"),
    }

    // Delete resolved alerts older than configured days (default 90)
    match sqlx::query(&format!(
        "DELETE FROM alerts WHERE status = 'resolved' AND created_at < NOW() - INTERVAL '{alert_days} days'",
    ))
    .execute(pool)
    .await
    {
        Ok(r) => {
            if r.rows_affected() > 0 {
                tracing::info!("Retention: deleted {} old resolved alerts (>{alert_days} days)", r.rows_affected());
            }
        }
        Err(e) => tracing::warn!("Retention cleanup (alerts) failed: {e}"),
    }

    // Delete activity_logs older than configured days (default 365)
    match sqlx::query(&format!("DELETE FROM activity_logs WHERE created_at < NOW() - INTERVAL '{activity_days} days'"))
        .execute(pool)
        .await
    {
        Ok(r) => {
            if r.rows_affected() > 0 {
                tracing::info!(
                    "Retention: deleted {} old activity_logs (>{activity_days} days)",
                    r.rows_affected()
                );
            }
        }
        Err(e) => tracing::warn!("Retention cleanup (activity_logs) failed: {e}"),
    }

    // Delete system_logs older than configured days (default 30)
    match sqlx::query(&format!("DELETE FROM system_logs WHERE created_at < NOW() - INTERVAL '{system_log_days} days'"))
        .execute(pool)
        .await
    {
        Ok(r) => {
            if r.rows_affected() > 0 {
                tracing::info!(
                    "Retention: deleted {} old system_logs (>{system_log_days} days)",
                    r.rows_affected()
                );
            }
        }
        Err(e) => tracing::warn!("Retention cleanup (system_logs) failed: {e}"),
    }

    // Extension events: configured days (default 90)
    let ext_events_deleted = sqlx::query(&format!("DELETE FROM extension_events WHERE delivered_at < NOW() - INTERVAL '{scan_days} days'"))
        .execute(pool).await.ok().map(|r| r.rows_affected()).unwrap_or(0);
    if ext_events_deleted > 0 {
        tracing::info!("Retention: deleted {ext_events_deleted} extension events (>{scan_days} days)");
    }

    // GAP 18: Webhook gateway deliveries: configured days (default 7)
    let wh_deleted = sqlx::query(&format!("DELETE FROM webhook_deliveries WHERE received_at < NOW() - INTERVAL '{webhook_days} days'"))
        .execute(pool).await.ok().map(|r| r.rows_affected()).unwrap_or(0);
    if wh_deleted > 0 {
        tracing::info!("Retention: deleted {wh_deleted} webhook deliveries (>{webhook_days} days)");
    }

    // Backup verifications: configured days (default 90)
    let bv_deleted = sqlx::query(&format!("DELETE FROM backup_verifications WHERE created_at < NOW() - INTERVAL '{scan_days} days'"))
        .execute(pool).await.ok().map(|r| r.rows_affected()).unwrap_or(0);
    if bv_deleted > 0 {
        tracing::info!("Retention: deleted {bv_deleted} backup verifications (>{scan_days} days)");
    }

    // User sessions: 24 hours since last seen (JWT expires after 2h, but clean stale records)
    let sess_deleted = sqlx::query("DELETE FROM user_sessions WHERE last_seen_at < NOW() - INTERVAL '24 hours'")
        .execute(pool).await.ok().map(|r| r.rows_affected()).unwrap_or(0);
    if sess_deleted > 0 {
        tracing::info!("Retention: deleted {sess_deleted} expired user sessions (>24h)");
    }

    // Panel notifications: configured days (default 30)
    let notif_deleted = sqlx::query(&format!("DELETE FROM panel_notifications WHERE created_at < NOW() - INTERVAL '{notification_days} days'"))
        .execute(pool).await.ok().map(|r| r.rows_affected()).unwrap_or(0);
    if notif_deleted > 0 {
        tracing::info!("Retention: deleted {notif_deleted} panel notifications (>{notification_days} days)");
    }

    // GAP 66: Clean expired token blacklist entries
    let bl_deleted = sqlx::query("DELETE FROM token_blacklist WHERE expires_at < NOW()")
        .execute(pool).await.ok().map(|r| r.rows_affected()).unwrap_or(0);
    if bl_deleted > 0 {
        tracing::info!("Retention: deleted {bl_deleted} expired token blacklist entries");
    }

    // Clean expired terminal shares (older than 1 hour, timestamp stored as prefix in value)
    let ts_deleted = sqlx::query(
        "DELETE FROM settings WHERE key LIKE 'terminal_share_%' AND \
         CAST(SPLIT_PART(value, '|', 1) AS BIGINT) < EXTRACT(EPOCH FROM NOW()) - 3600"
    ).execute(pool).await.ok().map(|r| r.rows_affected()).unwrap_or(0);
    if ts_deleted > 0 {
        tracing::info!("Retention: deleted {ts_deleted} expired terminal shares (>1 hour)");
    }

    // ── Backup Retention Enforcement ────────────────────────────────────
    // For each backup schedule, enforce retention_count by deleting oldest backups
    // that exceed the limit (both DB records and local files via filesystem).

    let schedules: Vec<(uuid::Uuid, uuid::Uuid, i32, String)> = sqlx::query_as(
        "SELECT bs.id, bs.site_id, bs.retention_count, s.domain \
         FROM backup_schedules bs JOIN sites s ON s.id = bs.site_id \
         WHERE bs.retention_count > 0"
    ).fetch_all(pool).await.unwrap_or_default();

    let mut total_pruned = 0u64;
    for (_schedule_id, site_id, retention_count, domain) in &schedules {
        // Find backups exceeding retention_count (ordered newest first, skip retention_count)
        let excess: Vec<(uuid::Uuid, String)> = sqlx::query_as(
            "SELECT id, filename FROM backups WHERE site_id = $1 \
             ORDER BY created_at DESC OFFSET $2"
        )
        .bind(site_id)
        .bind(*retention_count)
        .fetch_all(pool).await.unwrap_or_default();

        for (backup_id, filename) in &excess {
            // Delete the local backup file if it exists
            let filepath = format!("/var/backups/dockpanel/{domain}/{filename}");
            let _ = std::fs::remove_file(&filepath);

            // Delete the DB record
            let _ = sqlx::query("DELETE FROM backups WHERE id = $1")
                .bind(backup_id)
                .execute(pool).await;

            total_pruned += 1;
        }
    }
    if total_pruned > 0 {
        tracing::info!("Retention: pruned {total_pruned} backups exceeding retention_count limits");
    }

    // Enforce retention for backup policies (database_backups + volume_backups)
    let policies: Vec<(uuid::Uuid, i32)> = sqlx::query_as(
        "SELECT id, retention_count FROM backup_policies WHERE retention_count > 0"
    ).fetch_all(pool).await.unwrap_or_default();

    for (policy_id, retention_count) in &policies {
        // Prune excess database backups for this policy
        let excess_db: Vec<(uuid::Uuid, String)> = sqlx::query_as(
            "SELECT id, filename FROM database_backups WHERE policy_id = $1 \
             ORDER BY created_at DESC OFFSET $2"
        )
        .bind(policy_id).bind(*retention_count)
        .fetch_all(pool).await.unwrap_or_default();

        for (id, filename) in &excess_db {
            let filepath = format!("/var/backups/dockpanel/databases/{filename}");
            let _ = std::fs::remove_file(&filepath);
            let _ = sqlx::query("DELETE FROM database_backups WHERE id = $1")
                .bind(id).execute(pool).await;
            total_pruned += 1;
        }

        // Prune excess volume backups for this policy
        let excess_vol: Vec<(uuid::Uuid, String)> = sqlx::query_as(
            "SELECT id, filename FROM volume_backups WHERE policy_id = $1 \
             ORDER BY created_at DESC OFFSET $2"
        )
        .bind(policy_id).bind(*retention_count)
        .fetch_all(pool).await.unwrap_or_default();

        for (id, filename) in &excess_vol {
            let filepath = format!("/var/backups/dockpanel/volumes/{filename}");
            let _ = std::fs::remove_file(&filepath);
            let _ = sqlx::query("DELETE FROM volume_backups WHERE id = $1")
                .bind(id).execute(pool).await;
            total_pruned += 1;
        }
    }
    if total_pruned > 0 {
        tracing::info!("Retention: total {total_pruned} excess backups pruned (schedules + policies)");
    }

    // ── Security Enhancement Retention ─────────────────────────────────

    // Clean suspicious_events older than 90 days
    let sus_deleted = sqlx::query(
        "DELETE FROM suspicious_events WHERE created_at < NOW() - INTERVAL '90 days'"
    ).execute(pool).await.ok().map(|r| r.rows_affected()).unwrap_or(0);
    if sus_deleted > 0 {
        tracing::info!("Retention: deleted {sus_deleted} old suspicious_events (>90 days)");
    }

    // Clean old session recordings (>30 days)
    let rec_dir = "/var/lib/dockpanel/recordings";
    if let Ok(entries) = std::fs::read_dir(rec_dir) {
        let mut rec_deleted = 0u64;
        let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86400);
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if let Ok(created) = meta.created() {
                    if created < cutoff {
                        let _ = std::fs::remove_file(entry.path());
                        rec_deleted += 1;
                    }
                }
            }
        }
        if rec_deleted > 0 {
            tracing::info!("Retention: deleted {rec_deleted} old session recordings (>30 days)");
        }
    }

    // Clean old audit log files (>365 days)
    let audit_dir = "/var/lib/dockpanel/audit";
    if let Ok(entries) = std::fs::read_dir(audit_dir) {
        let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(365 * 86400);
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if let Ok(created) = meta.created() {
                    if created < cutoff {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
        }
    }

    // DB auto-backup: done via direct pg_dump (doesn't need agent client)
    if super::security_hardening::get_setting_bool(pool, "security_db_backup_enabled", true).await {
        tracing::info!("Triggering DockPanel DB auto-backup...");
        match safe_command("sh")
            .args(["-c", &format!(
                "docker exec dockpanel-postgres pg_dump -U dockpanel dockpanel | gzip > /var/backups/dockpanel/dockpanel-db-{}.sql.gz",
                chrono::Utc::now().format("%Y%m%d_%H%M%S")
            )])
            .output().await
        {
            Ok(o) if o.status.success() => {
                tracing::info!("DockPanel DB auto-backup completed");
                // Cleanup old backups (keep 7)
                if let Ok(entries) = std::fs::read_dir("/var/backups/dockpanel") {
                    let mut files: Vec<_> = entries
                        .filter_map(|e| e.ok())
                        .filter(|e| e.file_name().to_string_lossy().starts_with("dockpanel-db-"))
                        .collect();
                    files.sort_by_key(|e| std::cmp::Reverse(e.file_name().to_string_lossy().to_string()));
                    for old in files.iter().skip(7) {
                        let _ = std::fs::remove_file(old.path());
                    }
                }
            }
            Ok(o) => tracing::warn!("DB auto-backup failed: {}", String::from_utf8_lossy(&o.stderr)),
            Err(e) => tracing::warn!("DB auto-backup failed: {e}"),
        }
    }
}

// ── Security Hardening Background Tasks ─────────────────────────────

/// Ingest suspicious events written by the agent (from JSONL file).
/// Reads /var/lib/dockpanel/suspicious-events.jsonl, records each event,
/// then truncates the file. Runs every 2 minutes with auto-healer.
async fn security_ingest_suspicious_events(pool: &PgPool) {
    let path = "/var/lib/dockpanel/suspicious-events.jsonl";

    let content = match std::fs::read_to_string(path) {
        Ok(c) if !c.is_empty() => c,
        _ => return,
    };

    // Truncate the file immediately to avoid re-processing
    let _ = std::fs::write(path, "");

    let mut count = 0u32;
    for line in content.lines() {
        if line.trim().is_empty() { continue; }
        if let Ok(event) = serde_json::from_str::<serde_json::Value>(line) {
            let event_type = event["event_type"].as_str().unwrap_or("unknown");
            let actor_email = event["actor_email"].as_str();
            let command = event["command"].as_str();
            let domain = event["domain"].as_str().unwrap_or("");

            let details = format!("domain={}, command={}", domain, command.unwrap_or("-"));

            // Record suspicious event (may trigger auto-lockdown)
            let locked = super::security_hardening::record_suspicious_event(
                pool, event_type, actor_email, None, Some(&details),
            ).await;

            // Audit log
            super::security_hardening::audit_log(
                pool, event_type, actor_email, None,
                Some("terminal"), Some(domain),
                Some(&details), None, "warning",
            ).await;

            // If lockdown was triggered, send alert
            if locked {
                super::security_hardening::alert_lockdown(
                    pool,
                    &format!("Suspicious terminal command by {} on {}: {}", actor_email.unwrap_or("?"), domain, command.unwrap_or("?")),
                    "auto",
                ).await;
            }

            count += 1;
        }
    }

    if count > 0 {
        tracing::warn!("Ingested {count} suspicious terminal events from agent");
    }
}

/// Check canary files for access (Feature 12).
/// Compares atime (last access) against a stored baseline.
/// If atime changed, someone accessed the file — trigger alert.
async fn security_check_canary_files(pool: &PgPool) {
    use std::os::unix::fs::MetadataExt;

    let canary_paths = [
        "/etc/.dockpanel-canary",
        "/root/.dockpanel-canary",
        "/home/.dockpanel-canary",
        "/var/www/.dockpanel-canary",
    ];

    for path in &canary_paths {
        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(_) => continue, // File doesn't exist (not set up yet)
        };

        let atime = meta.atime();
        let _mtime = meta.mtime();

        // If accessed more recently than modified, someone read it
        // (mtime is set when we create it; atime changes on read)
        // Use a settings key to store the last known atime
        let key = format!("canary_atime_{}", path.replace('/', "_"));
        let stored: Option<(String,)> = sqlx::query_as(
            "SELECT value FROM settings WHERE key = $1"
        ).bind(&key).fetch_optional(pool).await.ok().flatten();

        let stored_atime: i64 = stored
            .and_then(|(v,)| v.parse().ok())
            .unwrap_or(0);

        if stored_atime == 0 {
            // First run: store current atime as baseline
            let _ = sqlx::query(
                "INSERT INTO settings (key, value) VALUES ($1, $2) ON CONFLICT (key) DO UPDATE SET value = $2"
            ).bind(&key).bind(atime.to_string()).execute(pool).await;
            continue;
        }

        if atime > stored_atime {
            // Canary was accessed! Alert immediately
            tracing::error!("CANARY TRIGGERED: {path} was accessed (atime changed from {stored_atime} to {atime})");

            super::security_hardening::audit_log(
                pool, "canary.triggered", None, None,
                Some("canary"), Some(path),
                Some(&format!("Canary file accessed at {}", chrono::DateTime::from_timestamp(atime, 0).map(|d| d.to_rfc3339()).unwrap_or_default())),
                None, "critical",
            ).await;

            // Record as suspicious event (may trigger auto-lockdown)
            super::security_hardening::record_suspicious_event(
                pool, "canary.triggered", None, None,
                Some(&format!("Canary file {path} was accessed")),
            ).await;

            // Send alert to all admins
            let admins: Vec<(uuid::Uuid,)> = sqlx::query_as(
                "SELECT id FROM users WHERE role = 'admin'"
            ).fetch_all(pool).await.unwrap_or_default();

            let subject = format!("🚨 CANARY TRIGGERED: {path}");
            let message = format!(
                "A canary file was accessed on the server!\n\
                 File: {path}\n\
                 This indicates unauthorized filesystem exploration.\n\
                 Check forensic snapshot and audit log immediately."
            );
            let html = format!(
                "<h2 style='color:red'>Canary File Triggered</h2>\
                 <p><strong>File:</strong> {path}</p>\
                 <p>This indicates unauthorized filesystem exploration.</p>\
                 <p>Check forensic snapshot and audit log immediately.</p>"
            );

            for (admin_id,) in &admins {
                if let Some(channels) = super::notifications::get_user_channels(pool, *admin_id, None).await {
                    super::notifications::send_notification(pool, &channels, &subject, &message, &html).await;
                }
            }

            // Update stored atime
            let _ = sqlx::query(
                "UPDATE settings SET value = $1 WHERE key = $2"
            ).bind(atime.to_string()).bind(&key).execute(pool).await;
        }
    }
}

/// Check if lockdown should auto-expire (24h max by default).
async fn security_check_lockdown_expiry(pool: &PgPool) {
    let row: Option<(bool, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT active, triggered_at FROM lockdown_state WHERE id = 1"
    ).fetch_optional(pool).await.ok().flatten();

    if let Some((true, Some(triggered_at))) = row {
        let hours_locked = (chrono::Utc::now() - triggered_at).num_hours();
        if hours_locked >= 24 {
            super::security_hardening::deactivate_lockdown(pool, "auto-expire (24h)").await;
            super::security_hardening::audit_log(
                pool, "lockdown.auto_expire", None, None,
                Some("system"), None,
                Some(&format!("Lockdown auto-expired after {}h", hours_locked)),
                None, "info",
            ).await;
            tracing::info!("Lockdown auto-expired after {hours_locked}h");
        }
    }
}

/// Auto-sleep: stop containers that have been idle beyond their configured threshold.
async fn auto_sleep_idle_containers(pool: &PgPool, agent: &AgentClient) {
    // Fetch all containers with auto-sleep enabled and not already sleeping
    let configs: Vec<(String, String, Option<String>, i32)> = sqlx::query_as(
        "SELECT container_id, container_name, domain, sleep_after_minutes \
         FROM container_sleep_config \
         WHERE auto_sleep_enabled = true AND is_sleeping = false"
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    if configs.is_empty() {
        return;
    }

    let now = chrono::Utc::now();

    for (container_id, container_name, _domain, threshold_minutes) in &configs {
        // Check last activity: use the stored last_activity_at
        let last_activity: Option<(Option<chrono::DateTime<chrono::Utc>>,)> = sqlx::query_as(
            "SELECT last_activity_at FROM container_sleep_config WHERE container_id = $1"
        )
        .bind(container_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();

        let idle = match last_activity.and_then(|r| r.0) {
            Some(last) => (now - last).num_minutes() >= *threshold_minutes as i64,
            None => {
                // No activity recorded yet — check if container is running via agent
                if let Ok(result) = agent.get("/apps").await {
                    if let Some(apps) = result.as_array() {
                        let is_running = apps.iter().any(|a|
                            a.get("container_id").and_then(|v| v.as_str()) == Some(container_id) &&
                            a.get("status").and_then(|v| v.as_str()) == Some("running")
                        );
                        if is_running {
                            // First run: record activity and skip
                            let _ = sqlx::query(
                                "UPDATE container_sleep_config SET last_activity_at = NOW() WHERE container_id = $1"
                            ).bind(container_id).execute(pool).await;
                            false
                        } else {
                            false // Not running, nothing to sleep
                        }
                    } else { false }
                } else { false }
            }
        };

        if idle {
            tracing::info!("Auto-sleeping idle container: {container_name} ({container_id})");

            // Stop the container via agent
            let stop_result = agent.post(
                &format!("/apps/{container_id}/stop"),
                None::<serde_json::Value>,
            ).await;

            match stop_result {
                Ok(_) => {
                    // Update sleep state
                    let _ = sqlx::query(
                        "UPDATE container_sleep_config SET is_sleeping = true, last_slept_at = NOW(), \
                         total_sleeps = total_sleeps + 1, updated_at = NOW() \
                         WHERE container_id = $1"
                    )
                    .bind(container_id)
                    .execute(pool)
                    .await;

                    activity::log_activity(
                        pool, uuid::Uuid::nil(), "auto-sleeper", "container.auto_sleep",
                        Some("container"), Some(container_name),
                        Some(&format!("Idle {}+ minutes", threshold_minutes)),
                        None,
                    ).await;

                    // Notify
                    notifications::notify_panel(
                        pool,
                        None,
                        "Auto-Sleep",
                        &format!("Container {} auto-slept (idle {}+ min)", container_name, threshold_minutes),
                        "info",
                        "system",
                        None,
                    ).await;
                }
                Err(e) => {
                    tracing::warn!("Failed to auto-sleep container {container_name}: {e}");
                }
            }
        }
    }
}
