use crate::safe_cmd::safe_command;
use chrono::Datelike;
use sqlx::PgPool;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::services::activity;
use crate::services::agent::AgentRegistry;
use crate::services::notifications;

/// Cumulative count of auto-healer SSL renewal attempts that succeeded.
/// Read by the Prometheus exporter and published as
/// `dockpanel_cert_renewals_total{result="success"}`. In-process atomic
/// (resets across restart — Prometheus `increase()` handles the reset).
pub static SSL_RENEWALS_SUCCESS: AtomicU64 = AtomicU64::new(0);
/// Cumulative count of auto-healer SSL renewal attempts that failed.
/// Published as `dockpanel_cert_renewals_total{result="failure"}`.
pub static SSL_RENEWALS_FAILURE: AtomicU64 = AtomicU64::new(0);

/// Background task: auto-heals common issues when detected.
/// Runs every 120 seconds (offset from alert engine to spread load).
/// Takes the fleet registry and NOT the panel's local `AgentClient`, deliberately.
/// Every leg of this loop reads rows that name a host and then writes through an
/// agent, so there is no legitimate reason for it to hold a handle to this box in
/// particular. Until v2.80.0 it held both and split them four lines apart — the
/// two legs given the local client (`auto_renew_ssl`, `auto_sleep_idle_containers`)
/// were the two that acted on the wrong machine. Removing the parameter is what
/// keeps that from being re-introduced by an author who sees one in scope.
pub async fn run(
    pool: PgPool,
    agents: AgentRegistry,
    mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
) {
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
        if is_enabled(&pool).await {
            auto_restart_services(&pool, &agents).await;
            auto_clean_disk(&pool, &agents).await;
            auto_renew_ssl(&pool, &agents).await;
            auto_sleep_idle_containers(&pool, &agents).await;
        }

        // Security hardening tasks share the healer's 2-minute tick but NOT its
        // switch: `auto_heal_enabled` names auto-healing, and until v2.46.0
        // turning it off also silently stopped suspicious-event ingestion,
        // auto-lockdown expiry and canary monitoring. Same reasoning as the
        // retention cleanup above, which was already carved out.
        security_ingest_suspicious_events(&pool).await;
        security_check_lockdown_expiry(&pool).await;
        if super::security_hardening::get_setting_bool(&pool, "security_canary_enabled", true).await
        {
            security_check_canary_files(&pool).await;
        }
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

/// Restart crashed services and containers — on the host they crashed on.
///
/// Every read here used to be fleet-blind and the restart always went to the
/// panel's own agent. That combination did not misbehave, and the reason is
/// worth stating: the alert engine also read the panel's agent while labelling
/// the rows with the oldest `servers` row, so the label was wrong, this function
/// ignored the label, and the restart landed on the machine the reading actually
/// came from. **Two bugs that cancel are two bugs.** The alert engine now writes
/// correct server ids, which ends the accident — nginx dying on a member would
/// restart nginx on the panel — so this must be scoped in the same ship, and is.
///
/// Three properties this now holds, mirroring `auto_clean_disk`:
/// * **Each firing service is healed on its own agent**, resolved through
///   `AgentRegistry::for_server`, driven by a JOIN that carries the server the
///   row already names.
/// * **The cooldown is real, and it is per server.** It counted `activity_logs`
///   rows written with the nil uuid, which violates `fk_activity_logs_user`, so
///   the insert always failed and the count was always 0: neither the 10-minute
///   gap nor the give-up-after-3 rule had ever engaged, and a crash-looping
///   service was restarted every 120 seconds for ever. The row is now written
///   against the server's owner AND stamped with `server_id`, so two hosts
///   running a service of the same name no longer share one budget.
///   ⚠ This ARMS the exhaustion path for the first time: a service that fails 3
///   restarts in 30 minutes now opens a managed incident and stops being healed
///   until it comes back up. That is the designed behaviour and it has never
///   once run.
/// * **Recovery is scoped too.** The state clears and the incident resolves for
///   the server the service actually recovered on, not for whichever row the
///   `servers` table happened to return first.
async fn auto_restart_services(pool: &PgPool, agents: &AgentRegistry) {
    // Recovery check: if an exhausted service is now running, clear the state.
    // The JOIN is what carries the host: `alert_state` is keyed per server, and
    // the owner and name we resolve here are the ones the alert engine stamped
    // onto the incident title we are about to match.
    let exhausted_rows: Vec<(uuid::Uuid, String, uuid::Uuid, String)> = sqlx::query_as(
        "SELECT a.server_id, a.state_key, s.user_id, s.name \
         FROM alert_state a JOIN servers s ON s.id = a.server_id \
         WHERE a.alert_type = 'service_down' AND a.current_state = 'exhausted' \
           AND a.server_id IS NOT NULL",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    // Group by host so `/services/health` is asked once per machine rather than
    // once per exhausted service.
    let mut exhausted_by_server: std::collections::HashMap<
        uuid::Uuid,
        (uuid::Uuid, String, Vec<String>),
    > = std::collections::HashMap::new();
    for (server_id, state_key, user_id, server_name) in exhausted_rows {
        exhausted_by_server
            .entry(server_id)
            .or_insert_with(|| (user_id, server_name, Vec::new()))
            .2
            .push(state_key);
    }

    for (server_id, (owner_id, server_name, service_names)) in exhausted_by_server {
        let agent = match agents.for_server(server_id).await {
            Ok(a) => a,
            Err(e) => {
                tracing::debug!("Auto-healer: no agent for {server_name} ({server_id}): {e}");
                continue;
            }
        };

        if let Ok(health_result) = agent.get("/services/health").await {
            if let Some(services_arr) = health_result.as_array() {
                for service_name in &service_names {
                    let is_running = services_arr.iter().any(|svc| {
                        svc.get("name").and_then(|n| n.as_str()) == Some(service_name.as_str())
                            && svc.get("status").and_then(|s| s.as_str()) == Some("running")
                    });

                    if is_running {
                        // Service recovered! Clear exhausted state
                        let _ = sqlx::query(
                            "DELETE FROM alert_state WHERE server_id = $1 AND alert_type = 'service_down' AND state_key = $2 AND current_state = 'exhausted'"
                        ).bind(server_id).bind(service_name).execute(pool).await;
                        tracing::info!("Auto-healer: {service_name} recovered on {server_name}, cleared exhausted state");

                        // Resolve the associated incident — the one the alert
                        // engine opened for THIS service on THIS box, matched by
                        // its exact generated title and scoped to the owner.
                        //
                        // This used to be `title LIKE '%<service>%'` with no
                        // user_id: a tenant incident whose title merely contained
                        // a system-service substring ("nginx-edge is down",
                        // "postgres migration") was silently flipped to resolved
                        // when the panel's own service recovered — turning their
                        // status page green mid-outage with no incident update to
                        // show for it. Same unscoped-title-match class as the
                        // incidents.rs and uptime.rs resolves.
                        let _ = sqlx::query(
                            "UPDATE managed_incidents SET status = 'resolved', updated_at = NOW() \
                             WHERE user_id = $1 AND title IN ($2, $3) \
                             AND status NOT IN ('resolved', 'postmortem')"
                        )
                        .bind(owner_id)
                        .bind(format!("Service {service_name} is stopped on {server_name}"))
                        .bind(format!("Service {service_name} is failed on {server_name}"))
                        .execute(pool).await;

                        notifications::notify_panel(pool, None,
                            &format!("Service recovered: {}", service_name),
                            &format!("{} is running again on {} after auto-healer exhaustion. Monitoring resumed.", service_name, server_name),
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

    // Find service_down alerts that are currently firing, each carrying the
    // server it fired on. The row is the authority on the host — the same rule
    // the webhook routes learned at v2.57.0.
    let firing: Vec<(uuid::Uuid, String, uuid::Uuid, String)> = match sqlx::query_as(
        "SELECT a.server_id, a.state_key, s.user_id, s.name \
         FROM alert_state a JOIN servers s ON s.id = a.server_id \
         WHERE a.alert_type = 'service_down' AND a.current_state = 'firing' \
           AND a.state_key != '' AND a.server_id IS NOT NULL",
    )
    .fetch_all(pool)
    .await
    {
        Ok(s) => s,
        Err(_) => return,
    };

    for (server_id, service_name, user_id, server_name) in &firing {
        let (server_id, user_id) = (*server_id, *user_id);
        if service_name.is_empty() {
            continue;
        }

        let agent = match agents.for_server(server_id).await {
            Ok(a) => a,
            Err(e) => {
                tracing::debug!("Auto-healer: no agent for {server_name} ({server_id}): {e}");
                continue;
            }
        };

        // GAP 12: Check restart count in last 30 minutes — give up after 3 attempts.
        // Scoped to this server: a service of the same name on two hosts had
        // shared one budget, so a crash loop on one box would have exhausted
        // healing for a healthy namesake on the other.
        let restart_count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM activity_logs \
             WHERE action = 'auto_heal.restart_service' \
             AND target_name = $1 AND server_id = $2 \
             AND created_at > NOW() - INTERVAL '30 minutes'",
        )
        .bind(service_name)
        .bind(server_id)
        .fetch_one(pool)
        .await
        .unwrap_or((0,));

        if restart_count.0 >= 3 {
            // Stop healing — service is in a crash loop. Create incident and notify.
            tracing::warn!("Auto-healer gave up on {service_name} after 3 restarts in 30 minutes");

            // The owner and name come from the row's own server. This used to
            // be the oldest `servers` row, so a member's crash loop opened an
            // incident naming the panel — on the panel owner's status page.
            {
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

            // Clear the firing alert state so we don't keep trying — on THIS
            // server only. Unscoped, giving up on a member's nginx also stopped
            // the panel's own nginx from ever being healed again.
            let _ = sqlx::query(
                "UPDATE alert_state SET current_state = 'exhausted' \
                 WHERE server_id = $1 AND alert_type = 'service_down' AND state_key = $2 AND current_state = 'firing'",
            )
            .bind(server_id)
            .bind(service_name)
            .execute(pool)
            .await;

            continue;
        }

        // Check if we already tried to heal this service recently (10-minute cooldown between attempts)
        let recent_heal: Option<(i64,)> = sqlx::query_as(
            "SELECT COUNT(*) FROM activity_logs \
             WHERE action = 'auto_heal.restart_service' \
             AND target_name = $1 AND server_id = $2 \
             AND created_at > NOW() - INTERVAL '10 minutes'",
        )
        .bind(service_name)
        .bind(server_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();

        if recent_heal.map(|r| r.0).unwrap_or(0) > 0 {
            tracing::debug!("Auto-heal: skipping {service_name} (recently attempted)");
            continue;
        }

        tracing::info!("Auto-heal: restarting service {service_name} on {server_name} (attempt {} of 3 in 30m window)", restart_count.0 + 1);

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

        // Written against the server's OWNER, not the nil uuid, and stamped
        // with the server. This row IS the cooldown gate read above — written
        // against the nil uuid it violated `fk_activity_logs_user` and failed
        // every time, so the count was always 0 and a crash-looping service was
        // restarted every 120 seconds for ever, with no audit trail of any of it.
        activity::log_activity_on_server(
            pool,
            user_id,
            "auto-healer",
            "auto_heal.restart_service",
            Some("service"),
            Some(service_name),
            Some(&format!("server={server_name} success={success}, result={details}")),
            None,
            Some(server_id),
        )
        .await;

        // If the restart succeeded, update alert_state to "ok" and resolve firing alerts
        // so the alert engine doesn't re-fire before its next health check confirms recovery
        if success {
            let _ = sqlx::query(
                "UPDATE alert_state SET current_state = 'ok', fired_at = NULL, last_notified_at = NULL \
                 WHERE server_id = $1 AND alert_type = 'service_down' AND state_key = $2 AND current_state = 'firing'",
            )
            .bind(server_id)
            .bind(service_name)
            .execute(pool)
            .await;

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

            tracing::info!("Auto-heal: service {service_name} restarted successfully on {server_name}, alert resolved");
        }
    }

    // Auto-restart exited/dead Docker containers, on every online server.
    //
    // ⚠ This block had never run once, for two independent reasons, and both
    // were invisible because a missing JSON field reads as an empty string. The
    // agent's `/apps` entries carry `status` and `container_id`
    // (`DeployedApp`, agent `services/docker_apps.rs`); this asked for `state`
    // and `id`, so the state test could never match "exited" and the emptiness
    // guard could never pass. The alert engine reads the correct field name for
    // the same payload — in the same subsystem, for the same containers.
    for member in agents.online_fleet().await {
        let containers = match member.agent.get("/apps").await {
            Ok(c) => c,
            Err(_) => continue,
        };
        let arr = match containers.as_array() {
            Some(a) => a.clone(),
            None => continue,
        };

        for c in &arr {
            let name = c.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let state = c.get("status").and_then(|v| v.as_str()).unwrap_or("");
            let container_id = c.get("container_id").and_then(|v| v.as_str()).unwrap_or("");

            if (state != "exited" && state != "dead") || name.is_empty() || container_id.is_empty() {
                continue;
            }

            // Check restart count in last 30 minutes — give up after 3 attempts
            let restart_count: (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM activity_logs \
                 WHERE action = 'auto_heal.container_restart' AND target_name = $1 AND server_id = $2 \
                 AND created_at > NOW() - INTERVAL '30 minutes'"
            ).bind(name).bind(member.id).fetch_one(pool).await.unwrap_or((0,));

            if restart_count.0 >= 3 {
                tracing::warn!("Auto-healer gave up on container {name} on {} after 3 restarts in 30 minutes", member.name);
                continue;
            }

            // 10-minute cooldown between attempts
            let recent_heal: (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM activity_logs \
                 WHERE action = 'auto_heal.container_restart' AND target_name = $1 AND server_id = $2 \
                 AND created_at > NOW() - INTERVAL '10 minutes'"
            ).bind(name).bind(member.id).fetch_one(pool).await.unwrap_or((0,));

            if recent_heal.0 > 0 {
                continue;
            }

            tracing::info!("Auto-heal: restarting container {name} on {} (attempt {} of 3)", member.name, restart_count.0 + 1);

            let result = member.agent.post(
                &format!("/apps/{}/restart", container_id),
                None::<serde_json::Value>,
            ).await;

            let success = result.is_ok();
            activity::log_activity_on_server(
                pool, member.user_id, "auto-healer", "auto_heal.container_restart",
                Some("container"), Some(name),
                Some(&format!("server={} success={success}, state={state}", member.name)),
                None,
                Some(member.id),
            ).await;

            if success {
                tracing::info!("Auto-healer: restarted container {name} on {}", member.name);
            } else {
                tracing::warn!("Auto-healer: failed to restart container {name} on {}", member.name);
            }
        }
    }
}

/// Free disk space on servers whose disk alert is firing — on THOSE servers.
///
/// This used to read one firing `disk` row with no `server_id` predicate and send
/// the fixes to the local agent. `alert_state` is keyed per server
/// (`idx_alert_state_server` on `(server_id, alert_type, state_key)`), so in a
/// fleet ANY member crossing its threshold made the panel host clean and prune
/// ITSELF, forever, while the full machine was never touched. Proven on a
/// two-box fleet: member at 93%, panel host at 18%, and the panel host lost a
/// tenant's container and image.
///
/// Three properties this now holds:
/// * **Each firing server is healed on its own agent**, resolved through
///   `AgentRegistry::for_server` — the same primitive every HTTP route already
///   uses via `ServerScope`.
/// * **The cooldown is real.** It used to count `activity_logs` rows written with
///   `Uuid::nil()`, which violates `fk_activity_logs_user`, so the insert always
///   failed, the count was always 0, and the "hourly" escalation was armed on
///   every 120s tick. The record is now written against the server's owner, so it
///   both gates the next run and gives the operator the audit trail that
///   destruction of this size owes them.
/// * **Recovery goes through the alert engine's own path.** A raw UPDATE to `ok`
///   consumed the firing->ok transition without ever calling `resolve_alert`, so
///   the `alerts` row stayed `firing` for ever — and retention only purges
///   `status = 'resolved'`, making those rows unpurgeable.
async fn auto_clean_disk(pool: &PgPool, agents: &AgentRegistry) {
    let firing: Vec<(uuid::Uuid, uuid::Uuid, String)> = sqlx::query_as(
        "SELECT a.server_id, s.user_id, s.name \
         FROM alert_state a JOIN servers s ON s.id = a.server_id \
         WHERE a.alert_type = 'disk' AND a.current_state = 'firing' \
           AND a.server_id IS NOT NULL",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    for (server_id, user_id, server_name) in firing {
        // Cooldown is per server: a fleet where one box is chronically full must
        // not suppress healing on another that has just filled up.
        // Keyed on `server_id`, like the four other cooldown gates in this file.
        // It used to key on `target_name` holding the server's UUID as text —
        // which worked, but spent the operator-facing column on a machine
        // identifier, so the audit feed rendered this row as a bare UUID. The
        // column that means "which host" now exists, so the gate uses it and
        // `target_name` is free to hold the server's name.
        //
        // One-time effect on upgrade: rows written under the old convention
        // carry no `server_id`, so they no longer suppress. A host whose disk
        // alert is firing may therefore clean its logs once more than the hour
        // would otherwise allow. The action is idempotent and this does not
        // repeat.
        let recent: Option<(i64,)> = sqlx::query_as(
            "SELECT COUNT(*) FROM activity_logs \
             WHERE action = 'auto_heal.clean_logs' AND server_id = $1 \
             AND created_at > NOW() - INTERVAL '1 hour'",
        )
        .bind(server_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();

        if recent.map(|r| r.0).unwrap_or(0) > 0 {
            continue;
        }

        let agent = match agents.for_server(server_id).await {
            Ok(a) => a,
            Err(e) => {
                // Refuse rather than fall back to the local agent. Falling back is
                // precisely the defect: it destroys a healthy host to "fix" one we
                // cannot reach.
                tracing::warn!(
                    "Auto-heal: disk alert firing on {server_name} ({server_id}) but its \
                     agent is unreachable ({e}) — NOT cleaning. Refusing to act on a \
                     different host."
                );
                continue;
            }
        };

        tracing::info!("Auto-heal: cleaning logs on {server_name} to free disk space");
        let success = agent
            .post(
                "/diagnostics/fix",
                Some(serde_json::json!({ "fix_id": "clean-logs:all" })),
            )
            .await
            .is_ok();

        // Written against the server's OWNER, not the nil uuid — this row is both
        // the cooldown gate and the operator's only record that this ran. The
        // host now travels in `server_id`, which is what the gate above reads,
        // so `target_name` can say which machine in words.
        activity::log_activity_on_server(
            pool,
            user_id,
            "auto-healer",
            "auto_heal.clean_logs",
            Some("server"),
            Some(&server_name),
            Some(&format!("server={server_name} success={success}")),
            None,
            Some(server_id),
        )
        .await;

        if !success {
            continue;
        }

        tracing::info!("Auto-heal: cleaning /tmp files older than 7 days on {server_name}");
        let _ = agent
            .post(
                "/diagnostics/fix",
                Some(serde_json::json!({ "fix_id": "clean-tmp:all" })),
            )
            .await;

        // The reclaim is opt-in and scoped; see `docker-reclaim` in the agent's
        // diagnostics. It never removes anything DockPanel manages, and never
        // anything a tenant is only sleeping.
        if reclaim_enabled(pool).await {
            tracing::info!("Auto-heal: reclaiming unmanaged Docker resources on {server_name}");
            let _ = agent
                .post(
                    "/diagnostics/fix",
                    Some(serde_json::json!({ "fix_id": "docker-reclaim:all" })),
                )
                .await;
        }

        // Hand recovery back to the alert engine, scoped to THIS server, so the
        // `alerts` row is resolved and the operator is told it recovered.
        let _ = sqlx::query(
            "UPDATE alert_state SET current_state = 'ok', consecutive_count = 0, \
             fired_at = NULL, last_notified_at = NULL \
             WHERE server_id = $1 AND alert_type = 'disk' AND current_state = 'firing'",
        )
        .bind(server_id)
        .execute(pool)
        .await;

        notifications::resolve_alert(
            pool,
            user_id,
            Some(server_id),
            None,
            "disk",
            &format!("DISK recovered on {server_name}"),
            &format!("Automatic disk cleanup freed space on server {server_name}"),
        )
        .await;

        tracing::info!("Auto-heal: disk cleanup succeeded on {server_name}, alert resolved");

        notifications::notify_panel(
            pool,
            None,
            "Disk cleanup completed",
            &format!(
                "Automatic disk cleanup ran on {server_name} (logs + /tmp{})",
                if reclaim_enabled(pool).await { " + unmanaged Docker resources" } else { "" }
            ),
            "info",
            "auto_heal",
            None,
        )
        .await;
    }
}

/// Whether the operator has opted into reclaiming unused Docker resources during
/// a disk heal. Defaults to **false**: the previous behaviour ran
/// `docker system prune -af --volumes` — every stopped container, every image not
/// held by a running one, every unused volume — on a consent screen that said
/// "cleans logs".
async fn reclaim_enabled(pool: &PgPool) -> bool {
    sqlx::query_scalar::<_, String>(
        "SELECT value FROM settings WHERE key = 'auto_heal_docker_reclaim'",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .map(|v| v == "true")
    .unwrap_or(false)
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
/// Raise an alert when a certificate cannot even be *attempted*.
///
/// This is the failure that hides best. Issuance has a fallback contact address,
/// so a box whose owner email cannot be an ACME contact gets its certificate and
/// looks fine — and then, sixty days later, the renewal loop reaches this branch
/// every two minutes and says nothing anybody reads. The certificate expires on
/// a working, unattended server. Deduped to one alert per site per twelve hours:
/// the cause is a configuration problem, not a transient.
async fn ssl_renewal_blocked(
    pool: &PgPool,
    user_id: uuid::Uuid,
    site_id: uuid::Uuid,
    domain: &str,
    reason: &str,
) {
    notifications::fire_alert_deduped(
        pool,
        user_id,
        None,
        Some(site_id),
        "ssl_renewal_failure",
        "critical",
        &format!("SSL renewal blocked: {domain}"),
        &format!(
            "DockPanel cannot renew the certificate for {domain}: {reason}. \
             Until this is fixed the certificate will expire and the site will \
             stop loading. Set a contact address under Settings → SSL, or on the \
             site owner's account."
        ),
        12,
    )
    .await;
}

async fn auto_renew_ssl(pool: &PgPool, agents: &AgentRegistry) {
    // Widen the window to 45 days so we pick up short-lived (6-day) and
    // 45-day-profile certs with enough lead time. ARI trims this further.
    //
    // `s.server_id` names the host that actually serves this site, and the
    // renewal now runs against THAT host's agent. It did not until v2.80.0:
    // this loop took the panel's own `AgentClient` while the query above it was
    // fleet-wide, so for every site on a fleet member the panel asked its own
    // box to renew a certificate for a domain that box does not serve. HTTP-01
    // cannot validate from there, so the certificate simply expired on a live
    // customer site — unattended, every two minutes, needing no attacker. The
    // inverse was worse: a local success (DNS-01, or a same-named vhost on the
    // panel) wrote the LOCAL certificate's expiry onto the remote site's row,
    // pushing it out of the 45-day window so the loop stopped trying, leaving a
    // guaranteed outage behind a green panel.
    //
    // The column is NOT NULL since the multi-server migration, so this needs no
    // migration and cannot widen the result set.
    let sites: Vec<(
        uuid::Uuid, String, uuid::Uuid, String, Option<i32>, Option<String>, Option<String>,
        chrono::DateTime<chrono::Utc>,
        Option<chrono::DateTime<chrono::Utc>>,
        Option<chrono::DateTime<chrono::Utc>>,
        Option<String>,
        uuid::Uuid,
    )> = match sqlx::query_as(
        "SELECT s.id, s.domain, s.user_id, s.runtime, s.proxy_port, s.php_version, s.root_path, \
                s.ssl_expiry, s.ssl_renewal_at, s.ssl_renewal_checked_at, s.ssl_profile, \
                s.server_id \
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
             ssl_expiry, ssl_renewal_at_initial, ssl_renewal_checked_at, ssl_profile,
             server_id) = row;

        // Resolve the host from the ROW before doing anything else. Every leg
        // below writes: ARI state, the certificate itself, `ssl_expiry`, and a
        // full vhost re-render. Aiming any of them at a host the site does not
        // run on is a cross-host write, so an unreachable server is a reason to
        // retry on the next tick and never a reason to act somewhere else —
        // the same rule `preview_cleanup` and `backup_policy_executor` follow.
        let agent = match agents.for_server(*server_id).await {
            Ok(a) => a,
            Err(e) => {
                tracing::warn!(
                    "Auto-heal: site {domain} lives on server {server_id}, which is \
                     unreachable ({e}) — skipping SSL renewal this cycle. Refusing to \
                     renew a certificate through a different host."
                );
                continue;
            }
        };

        let mut ssl_renewal_at = *ssl_renewal_at_initial;
        let owner_email: String = match sqlx::query_scalar(
            "SELECT email FROM users WHERE id = $1",
        )
        .bind(user_id)
        .fetch_optional(pool)
        .await
        {
            Ok(Some(e)) => e,
            _ => {
                tracing::warn!("Auto-heal: cannot renew SSL for {domain} — owner email not found");
                ssl_renewal_blocked(
                    pool,
                    *user_id,
                    *site_id,
                    domain,
                    "the site's owner account has no email address on file",
                )
                .await;
                continue;
            }
        };
        // Resolve through the SAME path issuance uses, so the panel-wide
        // `acme_contact_email` rescue is not limited to human-triggered issuance.
        // Otherwise a cert that was only issuable thanks to that fallback stops
        // renewing the moment nobody is clicking.
        let email: String = match crate::routes::ssl::resolve_acme_contact(pool, &owner_email).await {
            Ok(addr) => addr,
            Err(reason) => {
                tracing::warn!("Auto-heal: cannot renew SSL for {domain} — {reason}");
                ssl_renewal_blocked(pool, *user_id, *site_id, domain, &reason).await;
                continue;
            }
        };

        // Phase 1 — refresh ARI suggestion if stale or missing. Cooldown is
        // profile-aware so shortlived (6-day) certs poll the CA more often
        // than long-lived ones.
        let needs_ari = ssl_renewal_checked_at
            .map(|t| (now - t) > profile_cooldown(ssl_profile.as_deref()))
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

        // Profile-aware cooldown prevents hammering the CA if renewal keeps
        // failing. Shortlived gets 1h so a failed attempt near expiry doesn't
        // burn the whole renewal window; everything else stays at 6h.
        // cooldown_hours is a typed i64 — safe to interpolate directly.
        let cooldown_hours = profile_cooldown(ssl_profile.as_deref()).num_hours();
        let recent: Option<(i64,)> = sqlx::query_as(&format!(
            "SELECT COUNT(*) FROM activity_logs \
             WHERE action = 'auto_heal.renew_ssl' \
             AND target_name = $1 \
             AND created_at > NOW() - INTERVAL '{cooldown_hours} hours'",
        ))
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
            agent_body["php_socket"] = serde_json::json!(format!("unix:/run/php/php{php}-fpm.sock"));
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

        // THIS ROW IS THE COOLDOWN. The gate above counts these, so until v2.60.0
        // it was counting rows that could not exist: this call passed
        // `uuid::Uuid::nil()`, `fk_activity_logs_user` rejected the insert, and
        // `log_activity` swallowed the error into a warn. `COUNT(*)` was therefore
        // permanently 0, the `if recent > 0 { continue }` above never fired, and a
        // certificate DockPanel cannot renew was re-ordered from the CA on every
        // 120-second tick, for ever — by the cooldown whose own comment says it
        // exists "to prevent hammering the CA if renewal keeps failing". The agent
        // does not cheaply refuse these: it validates the domain's shape and then
        // places a real ACME order, so this was real load on Let's Encrypt.
        //
        // Named against the site's OWNER (a real `users` row) and stamped with the
        // site's server, exactly as `auto_heal.restart_service` has been since
        // v2.58.0. The stamp also makes the gate per-host rather than per-domain
        // if the fleet leg lands later.
        activity::log_activity_on_server(
            pool,
            *user_id,
            "auto-healer",
            "auto_heal.renew_ssl",
            Some("site"),
            Some(domain),
            Some(&format!("site_id={site_id}, success={success}, result={details}")),
            None,
            Some(*server_id),
        )
        .await;

        if success {
            // Update ssl_expiry from the agent response if available
            if let Ok(ref resp) = result {
                let new_expiry = resp
                    .get("expiry")
                    .and_then(|v| v.as_str())
                    .and_then(crate::helpers::parse_agent_cert_expiry);

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

            // Re-render the full vhost so the auto-renewal preserves the site's
            // WAF / CSP / Permissions-Policy / rate-limit / custom_nginx /
            // bot-protection (the agent's renew only renders a subset). Best-effort.
            if let Ok(site) = sqlx::query_as::<_, crate::models::Site>("SELECT * FROM sites WHERE id = $1")
                .bind(site_id)
                .fetch_one(pool)
                .await
            {
                // Same rule as the security scanner's renewal arm: a disabled
                // site is not rebuilt by an unattended loop. This one ticks
                // every two minutes when enabled, so it was the faster route
                // back onto the internet of the two.
                if !site.enabled {
                    tracing::info!(
                        "Auto-heal: renewed SSL for {} but skipped the vhost rebuild — the site is disabled",
                        site.domain
                    );
                } else if let Err(e) = agent
                    .put(
                        &format!("/nginx/sites/{}", site.domain),
                        crate::routes::sites::build_nginx_body(&site),
                    )
                    .await
                {
                    tracing::warn!("Auto-heal: full vhost rebuild after renewal failed for {}: {e}", site.domain);
                }
            }

            SSL_RENEWALS_SUCCESS.fetch_add(1, Ordering::Relaxed);

            // Panel notification
            notifications::notify_panel(pool, None, &format!("SSL renewed: {}", domain), &format!("SSL certificate for {} was automatically renewed", domain), "info", "ssl", None).await;
        } else {
            // Fire an alert so the user is notified about the SSL renewal failure.
            //
            // The server this alert names is the one the SITE is on. It used to be
            // `SELECT id FROM servers ORDER BY created_at ASC LIMIT 1` — the oldest
            // row on the panel, with no filter of any kind, which is the laundering
            // shape this project has been removing since v2.56.0. That row is
            // deterministically the panel's own local server (it is created at
            // startup and cannot be deleted), so a member's certificate failure was
            // filed against the panel. `routes/alerts.rs` admits only rows matching
            // the caller's selected server or a NULL one, so an admin who had
            // switched the picker to the affected member could not see the critical
            // alert about that member's own certificate.
            //
            // Deduped: the attempt cooldown is one hour for short-lived profiles,
            // so an unconditional alert here would page two dozen times a day for
            // a single stuck certificate — and the security scanner alerts on the
            // same certificate from its own loop.
            notifications::fire_alert_deduped(
                pool,
                *user_id,
                Some(*server_id),
                Some(*site_id),
                "ssl_renewal_failure",
                "critical",
                &format!("SSL renewal failed: {domain}"),
                &format!(
                    "Auto-healer failed to renew the SSL certificate for {domain}: {details}. \
                     The certificate may expire soon — check the domain configuration and DNS."
                ),
                12,
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
            SSL_RENEWALS_FAILURE.fetch_add(1, Ordering::Relaxed);
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

/// Per-profile cooldown for ARI re-fetch + post-attempt retry. Shortlived
/// (6-day) certs poll tighter so a CA-issued early-renew nudge isn't missed
/// by a full quarter-day, and a failed attempt near expiry doesn't burn the
/// whole renewal window. Longer profiles stay at 6h to keep CA-side rate
/// pressure low.
fn profile_cooldown(profile: Option<&str>) -> chrono::Duration {
    match profile {
        Some("shortlived") => chrono::Duration::hours(1),
        _ => chrono::Duration::hours(6),
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

/// Retire one policy-created backup: remove the archive, then the row that names it.
///
/// The row is the ONLY record of where the archive lives, so it is deleted last and
/// only when the archive is genuinely gone. Two cases keep it:
///
/// * **The backup belongs to another server.** `std::fs` reaches this host and no
///   other, so unlinking here would leave a remote archive alive with nothing
///   pointing at it. Retention for remote backups needs the owning server's agent
///   (`AgentRegistry::for_server`); until the sweep is threaded through it, refuse
///   loudly rather than silently destroy the record.
/// * **The unlink failed for a reason other than "already gone".** A read-only
///   mount or a permission problem is exactly when the operator most needs the row
///   to still say what is on disk.
///
/// Returns true when the backup was actually retired.
async fn prune_policy_backup(
    pool: &PgPool,
    table: &str,
    id: uuid::Uuid,
    filepath: &str,
    row_server: Option<uuid::Uuid>,
    local_server: Option<uuid::Uuid>,
) -> bool {
    if let (Some(row_s), Some(local_s)) = (row_server, local_server) {
        if row_s != local_s {
            tracing::warn!(
                "Retention: {table} {id} belongs to server {row_s}, not this host — \
                 keeping the row. A local unlink cannot reach it, and deleting the row \
                 would orphan the archive with no record of where it is."
            );
            return false;
        }
    }

    match std::fs::remove_file(filepath) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Already gone — the row is the only thing left to clean up.
        }
        Err(e) => {
            tracing::warn!(
                "Retention: could not remove {filepath} ({e}) — keeping the {table} row \
                 so the archive stays accounted for."
            );
            return false;
        }
    }

    // `table` is a compile-time literal from this module, never user input.
    let deleted = sqlx::query(&format!("DELETE FROM {table} WHERE id = $1"))
        .bind(id)
        .execute(pool)
        .await;
    match deleted {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!("Retention: removed {filepath} but could not delete {table} {id}: {e}");
            false
        }
    }
}

/// Periodic data retention cleanup: removes old records to keep the database lean.
async fn run_retention_cleanup(pool: &PgPool) {
    tracing::info!("Running data retention cleanup...");

    // GAP 33: Weekly digest — send summary email on Mondays during cleanup cycle
    send_weekly_digest(pool).await;

    // Phase 4 W4: panel snapshot retention. Always-keep last 3, deletes
    // anything older than 7 days beyond that floor. File-deletes first;
    // DB row only deleted if file removal succeeded (retries next sweep
    // otherwise). See `services::panel_snapshot::retention_sweep`.
    match crate::services::panel_snapshot::retention_sweep(pool).await {
        Ok(0) => {}
        Ok(n) => tracing::info!("Retention: removed {n} aged panel snapshot(s)"),
        Err(e) => tracing::warn!("Retention cleanup (panel_snapshots) failed: {e}"),
    }

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

    // The local server's id. A retention sweep unlinks a path on THIS filesystem,
    // so it may only act on rows whose backup was written here — see
    // `prune_policy_backup` for what happens to the rest.
    let local_server_id: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT id FROM servers WHERE is_local = TRUE LIMIT 1")
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();

    for (policy_id, retention_count) in &policies {
        // Retention is per DATABASE, not per policy. `OFFSET n` over a policy's
        // whole history kept n backups in TOTAL across every database the policy
        // covers, so a policy protecting five databases kept five backups and
        // four of those databases ended up with none.
        let excess_db: Vec<(uuid::Uuid, String, String, Option<uuid::Uuid>)> = sqlx::query_as(
            "SELECT id, filename, db_name, server_id FROM ( \
                 SELECT id, filename, db_name, server_id, \
                        ROW_NUMBER() OVER (PARTITION BY database_id ORDER BY created_at DESC) AS rn \
                 FROM database_backups WHERE policy_id = $1 \
             ) ranked WHERE rn > $2"
        )
        .bind(policy_id).bind(*retention_count)
        .fetch_all(pool).await.unwrap_or_default();

        for (id, filename, db_name, server_id) in &excess_db {
            // `database_backup::backup_dir` nests per database. The old path
            // omitted `{db_name}`, so the unlink could never match a real file
            // while the DELETE below ran anyway — every dump was orphaned on
            // disk and its only record destroyed.
            let filepath = format!("/var/backups/dockpanel/databases/{db_name}/{filename}");
            if prune_policy_backup(pool, "database_backups", *id, &filepath, *server_id, local_server_id).await {
                total_pruned += 1;
            }
        }

        // Same shape for volumes: per container+volume, and nested per container.
        let excess_vol: Vec<(uuid::Uuid, String, String, Option<uuid::Uuid>)> = sqlx::query_as(
            "SELECT id, filename, container_name, server_id FROM ( \
                 SELECT id, filename, container_name, server_id, \
                        ROW_NUMBER() OVER (PARTITION BY container_id, volume_name ORDER BY created_at DESC) AS rn \
                 FROM volume_backups WHERE policy_id = $1 \
             ) ranked WHERE rn > $2"
        )
        .bind(policy_id).bind(*retention_count)
        .fetch_all(pool).await.unwrap_or_default();

        for (id, filename, container_name, server_id) in &excess_vol {
            let filepath = format!("/var/backups/dockpanel/volumes/{container_name}/{filename}");
            if prune_policy_backup(pool, "volume_backups", *id, &filepath, *server_id, local_server_id).await {
                total_pruned += 1;
            }
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
        // `bash` and `set -o pipefail` are both load-bearing (lesson #51). The exit
        // status of `pg_dump | gzip` is *gzip's*, and gzip compresses a truncated
        // stream and exits 0 — so under the previous `sh -c` (which is dash here,
        // and has no pipefail) a pg_dump that died halfway was written out, this
        // arm matched, and the healer logged "DB auto-backup completed" over a
        // backup that could never be restored. Sibling of the panel_snapshot.rs
        // dump fixed in v2.11.5; this call site was missed then.
        let backup_file = format!(
            "/var/backups/dockpanel/dockpanel-db-{}.sql.gz",
            chrono::Utc::now().format("%Y%m%d_%H%M%S")
        );
        // `umask 077` is load-bearing and is NOT tidiness: the `>` redirect
        // creates the file 0666 & ~umask, so without it this dump — the whole
        // panel database, `servers.agent_token` in cleartext included — lands
        // 0644 on a host that also runs other people's PHP as www-data.
        match safe_command("bash")
            .args(["-c", &format!(
                "set -o pipefail; umask 077; docker exec dockpanel-postgres pg_dump -U dockpanel dockpanel | gzip > {backup_file}"
            )])
            .output().await
        {
            Ok(o) if o.status.success() => {
                // A zero exit is not the success condition — a whole dump is
                // (lesson #51 rule 4). pg_dump emits this marker last, so its
                // absence means the file is short whatever the statuses claimed.
                // This gates the retention prune below on purpose: pruning to 7
                // after writing a corrupt backup would evict a GOOD one to make
                // room for a useless one.
                let complete = safe_command("bash")
                    .args(["-c", &format!("gunzip -c {backup_file} | tail -20")])
                    .output().await
                    .map(|t| String::from_utf8_lossy(&t.stdout)
                        .contains("PostgreSQL database dump complete"))
                    .unwrap_or(false);
                if !complete {
                    tracing::warn!(
                        "DB auto-backup discarded: {backup_file} is incomplete \
                         (completion marker absent) — keeping earlier backups"
                    );
                    let _ = std::fs::remove_file(&backup_file);
                    return;
                }
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

            // The agent stamps every line with the time the command was actually
            // run. Ingesting the backlog as if it happened NOW collapses a queue
            // that may have taken months to build into one instant, which trips the
            // "N events in M minutes" rule on the first tick — see
            // `record_suspicious_event_at`. An unparseable or absent timestamp
            // falls back to now, which is the old behaviour and the safe direction
            // for an event we cannot place in time.
            let occurred_at = event["timestamp"]
                .as_str()
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                .map(|t| t.with_timezone(&chrono::Utc));

            let details = format!("domain={}, command={}", domain, command.unwrap_or("-"));

            // A REFUSED command is audited but does NOT count toward auto-lockdown.
            //
            // `record_suspicious_event_at` counts EVERY row in `suspicious_events`
            // inside the window against `security_lockdown_threshold` (5 in 10
            // minutes by default), and tripping it blocks every non-admin for 24
            // hours. `terminal.blocked_command` is emitted by the agent's site
            // blocklist, whose `TERMINAL_BLOCKED_PATTERNS` contains a bare `".."`
            // SUBSTRING — its own comment admits `echo "done..."` is refused — so
            // an ordinary tenant produces refusals by accident. Counting those
            // would turn a deliberately blunt blocklist into a self-inflicted
            // outage, which is a worse defect than the silence it replaced.
            // The operator still sees every one of them: `audit_log` below writes
            // `security_audit_log`, which is what `SecurityHardening.tsx` renders.
            let counts_toward_lockdown = event_type != "terminal.blocked_command";
            let locked = if counts_toward_lockdown {
                super::security_hardening::record_suspicious_event_at(
                    pool, event_type, actor_email, None, Some(&details), occurred_at,
                ).await
            } else {
                false
            };

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

    // How many canaries this tick could actually be examined. A tripwire with nothing
    // on the wire reports "all clear" forever, which is the one answer an intrusion
    // detector must never give by default — and the Settings toggle renders ON unless
    // the operator has explicitly turned it off, so silence read as protection.
    //
    // Nothing in the tree plants these files: the writer is the agent's
    // `/security/canary/setup`, which has no caller anywhere. Two of the four paths
    // are additionally unreadable to this process on a stock install, because the
    // unit runs under `ProtectHome=yes`. Both cases used to take the same silent
    // `continue`, so "never armed" and "armed and quiet" were indistinguishable.
    let mut examined = 0usize;

    for path in &canary_paths {
        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) => {
                // Distinguished deliberately: NotFound means nobody ever planted it;
                // anything else (notably PermissionDenied under ProtectHome) means a
                // canary may exist and we cannot see it — which is a blind spot, not
                // an all-clear, and is worth saying out loud.
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(
                        "Canary {path} exists or may exist but is unreadable ({e}); \
                         it is NOT being monitored"
                    );
                }
                continue;
            }
        };
        examined += 1;

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

    // Not armed at all: say so once per process rather than every two minutes, and say
    // it at WARN so it is visible in the same journal an operator checks after a
    // scare. Without this the feature's only observable behaviour on a stock install
    // is silence, which is identical to the behaviour of a working tripwire.
    if examined == 0 {
        static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            tracing::warn!(
                "Canary file monitoring is enabled but NOT ARMED — none of the {} canary \
                 paths could be examined, so nothing is being watched and no alert can \
                 ever fire. Disable the setting or create the canary files.",
                canary_paths.len()
            );
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
async fn auto_sleep_idle_containers(pool: &PgPool, agents: &AgentRegistry) {
    // Fetch all containers with auto-sleep enabled and not already sleeping.
    //
    // `server_id` was added to this table in v2.80.0 and is what makes the loop
    // fleet-correct. Before it existed there was no row to resolve a host from,
    // so every leg below ran against the panel's own agent: the last-activity
    // question went to the LOCAL nginx (which has never heard of a member's
    // domain, so it answered "unknown" and real traffic never counted), the
    // running-check listed the LOCAL host's containers, and the stop was posted
    // to the LOCAL agent. Auto-sleep was quietly inoperative for every fleet
    // member — the failure looked exactly like "nothing was idle".
    let configs: Vec<(String, String, Option<String>, i32, Option<uuid::Uuid>)> = sqlx::query_as(
        "SELECT container_id, container_name, domain, sleep_after_minutes, server_id \
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

    for (container_id, container_name, domain, threshold_minutes, server_id) in &configs {
        // Resolve the host from the row. The backfill gave every existing row a
        // server, so a NULL here means a row written by something that does not
        // set the column yet — refuse rather than falling back to this box,
        // which is the behaviour that made the bug invisible in the first place.
        let agent = match server_id {
            Some(sid) => match agents.for_server(*sid).await {
                Ok(a) => a,
                Err(e) => {
                    tracing::warn!(
                        "Auto-sleep: {container_name} lives on server {sid}, which is \
                         unreachable ({e}) — skipping this cycle rather than acting on \
                         a different host."
                    );
                    continue;
                }
            },
            None => {
                tracing::warn!(
                    "Auto-sleep: {container_name} has no server recorded — skipping. \
                     Guessing a host here would stop a container on the wrong machine."
                );
                continue;
            }
        };
        // Real traffic counts as activity, and it is the only thing that should.
        //
        // Nothing else writes `last_activity_at` from visitors: the column moves
        // only when an admin wakes the container by hand, on the first-run
        // bootstrap below, or through an admin-only ping endpoint the frontend
        // does not call. So without this lookup, "idle" meant "nobody used the
        // *panel*", and a container serving requests every few seconds was
        // stopped out from under its users on the timer — verified on a real box
        // against a continuous stream of HTTP 200s, which the sleeper then turned
        // into 502s. Ask nginx when the domain last answered, and let that count.
        if let Some(domain) = domain.as_deref().filter(|d| !d.is_empty()) {
            if let Ok(result) = agent.get(&format!("/nginx/last-activity/{domain}")).await {
                // A domain with no access log yet reports `null` — unknown, not
                // idle. Leave the stored value alone and let the checks below
                // decide on what they do know.
                if let Some(secs) = result.get("seconds_ago").and_then(|v| v.as_u64()) {
                    let seen = chrono::Utc::now() - chrono::Duration::seconds(secs as i64);
                    let _ = sqlx::query(
                        "UPDATE container_sleep_config SET last_activity_at = $2 \
                         WHERE container_id = $1 \
                           AND (last_activity_at IS NULL OR last_activity_at < $2)"
                    )
                    .bind(container_id)
                    .bind(seen)
                    .execute(pool)
                    .await;
                }
            }
        }

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

                    // No user initiates an auto-sleep and `container_sleep_config`
                    // names no owner, so this genuinely has no user to name — which
                    // is what NULL is for. It passed the nil uuid instead, so the
                    // insert was rejected and stopping a customer's container left
                    // no audit record at all.
                    activity::log_activity_system(
                        pool, "auto-sleeper", "container.auto_sleep",
                        Some("container"), Some(container_name),
                        Some(&format!("Idle {}+ minutes", threshold_minutes)),
                        None, None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cooldown_shortlived_is_one_hour() {
        assert_eq!(profile_cooldown(Some("shortlived")), chrono::Duration::hours(1));
    }

    #[test]
    fn cooldown_tlsserver_is_six_hours() {
        assert_eq!(profile_cooldown(Some("tlsserver")), chrono::Duration::hours(6));
    }

    #[test]
    fn cooldown_classic_is_six_hours() {
        assert_eq!(profile_cooldown(Some("classic")), chrono::Duration::hours(6));
    }

    #[test]
    fn cooldown_none_is_six_hours() {
        assert_eq!(profile_cooldown(None), chrono::Duration::hours(6));
    }
}
