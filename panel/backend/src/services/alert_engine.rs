use sqlx::PgPool;
use std::collections::HashSet;
use std::time::Duration;
use uuid::Uuid;

use crate::services::agent::{AgentRegistry, FleetMember};
use crate::services::expected_stops;
use crate::services::notifications;

/// Background task: checks all alert conditions every 60 seconds.
pub async fn run(pool: PgPool, agents: AgentRegistry, mut shutdown_rx: tokio::sync::broadcast::Receiver<()>) {
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

                // Whose alerts are silenced right now. Loaded ONCE per tick and
                // handed to every check, the way `uptime.rs` does it — the
                // alternative is one query per server per alert type.
                let maint = maintenance_users(&pool).await;

                check_resource_thresholds(&pool, &maint).await;
                check_server_offline(&pool, &maint).await;
                check_ssl_expiry(&pool, &maint).await;
                check_stack_ssl_expiry(&pool, &maint).await;
                check_registered_cert_expiry(&pool, &maint).await;

                // The agent-driven checks run once PER ONLINE SERVER. Until
                // v2.58.0 all three asked the panel's own agent and labelled the
                // labelled the answers with the OLDEST `servers` row, selected
                // by ascending creation date and commented "the local server",
                // which it is only on an
                // install that never added one. On a fleet that is the panel's
                // GPUs, the panel's services and the panel's containers filed
                // under whichever server was registered first.
                let fleet = agents.online_fleet().await;

                for member in &fleet {
                    // One skip covers all three: every alert these raise is
                    // addressed to `member.user_id` and nobody else.
                    if maint.contains(&member.user_id) {
                        continue;
                    }

                    check_gpu_thresholds(&pool, member).await;

                    // Service health every 2 minutes (every other tick)
                    if tick_count % 2 == 0 {
                        check_service_health(&pool, member).await;
                    }

                    // GAP 8: Docker container health every 2 minutes (offset from service health)
                    if tick_count % 2 == 1 {
                        check_container_health(&pool, member).await;
                    }
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

// ─── Maintenance Windows ────────────────────────────────────────────────

/// Users with an open maintenance window at this instant.
///
/// The panel offers a button labelled *Silence Alerts*. Until this existed the
/// only service that honoured it was `uptime.rs`, which owns 5 of the 19 alert
/// types; the engine in this file owns the other 14 and had never read the
/// table. So the alerts a planned maintenance is certain to cause — `offline`,
/// `service_down`, `container_down` — were precisely the ones that still paged.
///
/// ⚠ The set is applied at each check's SOURCE, never at the fire. That is not
/// a style preference. `check_server_offline` selects the servers that have no
/// firing row and then writes one; `check_threshold` and the SSL ladder stamp
/// `alert_state` beside their fire. Suppressing at the fire would leave those
/// stamps claiming the operator was paged while nothing was sent, and the
/// condition would never page again — the permanent silence that
/// `fire_alert_with_retry`'s own contract exists to prevent. Skipping the row
/// before any of that runs means the condition is still true on the first tick
/// after the window closes, and pages then.
///
/// The six types raised elsewhere (`backup_failure`, `backup_verification_failed`,
/// `cron_failure`, `security`, `image_scan`, `ssl_renewal_failure`) are
/// deliberately NOT suppressed: each report is the result of a specific run
/// (a backup, a cron job, a scan), not a standing condition THIS engine
/// re-evaluates on its own 60-second loop — this function's suppression exists
/// for the 14 types checked below, in this loop. A run that happens to fall
/// inside a maintenance window is not deferred to the next tick the way a
/// standing condition is; suppressing it here would lose that run's result
/// outright, not merely delay it.
async fn maintenance_users(pool: &PgPool) -> HashSet<Uuid> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT DISTINCT user_id FROM maintenance_windows \
         WHERE starts_at <= NOW() AND ends_at >= NOW()",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .collect()
}

// ─── Alert Fire with Retry ──────────────────────────────────────────────

/// Fire an alert with retry (2 attempts, 3s delay between).
///
/// Returns whether the alert was actually recorded. Callers that write a dedup
/// stamp MUST gate it on this: stamping after a failed fire silences the
/// condition permanently, because the stamp says "already paged" while nothing
/// was ever delivered and no `alerts` row exists for `check_escalations` to
/// re-page from.
async fn fire_alert_with_retry(
    pool: &PgPool,
    user_id: Uuid,
    server_id: Option<Uuid>,
    site_id: Option<Uuid>,
    alert_type: &str,
    state_key: &str,
    severity: &str,
    title: &str,
    message: &str,
) -> bool {
    for attempt in 0..2 {
        match notifications::try_fire_alert(
            pool, user_id, server_id, site_id, alert_type, state_key, severity, title, message,
        )
        .await
        {
            // `Ok(false)` is not a fire. The operator has this type switched off,
            // so `try_fire_alert` wrote no `alerts` row, rang no bell and sent
            // nothing — and the two things below both have to know that.
            //
            // Returning `true` here published the outage on the operator's PUBLIC
            // status page (the incident is created `visible_on_status_page = TRUE`)
            // for the one alert type they had asked not to hear about, and it
            // also satisfied this function's own contract, so the SSL ladder
            // stamped `last_warned_day` for a page that never happened. Because
            // `ssl_decision` only fires a rung TIGHTER than the one stamped, that
            // rung was then gone for good — re-enabling the type did not bring it
            // back.
            //
            // Retrying is pointless too: the answer is a stored preference, not a
            // transient error, so this returns rather than looping.
            Ok(false) => return false,
            Ok(true) => {
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
                        // No recent active incident — create a new one. RETURNING id
                        // binds the follow-up update to the row we just wrote; the
                        // previous re-lookup matched on `title` alone across the whole
                        // table, so two tenants whose servers share a name (the default
                        // hostname, "vps", "web") could have the update land on the
                        // OTHER tenant's investigating incident.
                        let new_incident: Option<(Uuid,)> = sqlx::query_as(
                            "INSERT INTO managed_incidents (user_id, title, status, severity, description, visible_on_status_page) \
                             VALUES ($1, $2, 'investigating', $3, $4, TRUE) RETURNING id"
                        )
                        .bind(user_id).bind(title).bind(incident_severity).bind(message)
                        .fetch_optional(pool).await.ok().flatten();

                        if let Some((incident_id,)) = new_incident {
                            let _ = sqlx::query(
                                "INSERT INTO incident_updates (incident_id, status, message, author_email) \
                                 VALUES ($1, 'investigating', $2, 'system')"
                            )
                            .bind(incident_id)
                            .bind(format!("Auto-created from {alert_type} alert: {message}"))
                            .execute(pool).await;
                        }
                    }
                }
                return true;
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
    false
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

async fn check_resource_thresholds(pool: &PgPool, maint: &HashSet<Uuid>) {
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
        if maint.contains(&server.user_id) {
            continue;
        }

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
        // Bounded by TIME, not by row count. `LIMIT 60` against a 30s collector
        // cadence (metrics_collector.rs:9) is a ~30-minute window, and
        // disk_full_forecast requires 6 hours — so the forecast this guide
        // advertises ("full within 48 hours") could never fire, for any input,
        // on any install. Proven on this box: 20,159 rows at a measured 30.0s
        // mean gap, and `alerts` holds 0 disk_forecast rows against 7 for
        // memory_leak, which runs the identical query without the time gate.
        // 12h at 30s is ~1440 rows; the LIMIT stays as a bound on a stalled or
        // backfilled collector, not as the window itself.
        let disk_trend: Vec<(f32, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
            "SELECT disk_pct, created_at FROM metrics_history \
             WHERE server_id = $1 AND created_at > NOW() - INTERVAL '12 hours' \
             ORDER BY created_at DESC LIMIT 1440",
        )
        .bind(server.id)
        .fetch_all(pool)
        .await
        .unwrap_or_default();

        // Both trend alerts below run through claim_trend_fire/clear_trend_alert
        // rather than calling fire_alert_with_retry directly: the condition holds
        // across many consecutive 60s ticks, and an ungated fire on every tick
        // meant one page per minute to every channel plus one never-purged
        // `alerts` row per minute for a single event — and, because nothing ever
        // resolved them, a check_escalations re-page every 30 minutes forever.
        //
        // That storm is/was live for memory_leak. disk_forecast was UNREACHABLE
        // and had never fired, for the reason recorded above the query: a
        // row-count LIMIT against a 30s cadence could not span the 6-hour
        // minimum. The query is now time-bounded, so this is the tick where the
        // forecast becomes reachable for the first time — which is exactly why
        // the gate below matters rather than being belt-and-braces. It is the
        // thing standing between a real forecast and the per-minute page storm
        // memory_leak demonstrated. Verified load-bearing before arming this:
        // all 7 memory_leak fires on this box carry a non-null resolved_at.
        match disk_full_forecast(&disk_trend) {
            Some((hours_to_full, rate_per_hour, current_pct)) => {
                if claim_trend_fire(pool, server.id, "disk_forecast", cooldown).await {
                    let severity = if hours_to_full < 12.0 { "critical" } else { "warning" };
                    fire_alert_with_retry(
                        pool,
                        server.user_id,
                        Some(server.id),
                        None,
                        "disk_forecast",
                        "",
                        severity,
                        &format!("Disk will be full in {:.0} hours on {}", hours_to_full, server.name),
                        &format!(
                            "At the current growth rate of {:.1}%/hour, disk will be full in approximately {:.0} hours. Current usage: {:.1}%",
                            rate_per_hour, hours_to_full, current_pct
                        ),
                    )
                    .await;
                }
            }
            None => {
                clear_trend_alert(
                    pool,
                    server.user_id,
                    server.id,
                    "disk_forecast",
                    &format!("Disk fill forecast cleared on {}", server.name),
                    &format!(
                        "Disk usage on server {} is no longer trending toward full within 48 hours.",
                        server.name
                    ),
                )
                .await;
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

        match memory_leak_trend(&mem_trend) {
            Some((increase, older_avg, recent_avg)) => {
                if claim_trend_fire(pool, server.id, "memory_leak", cooldown).await {
                    fire_alert_with_retry(
                        pool,
                        server.user_id,
                        Some(server.id),
                        None,
                        "memory_leak",
                        "",
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
            None => {
                clear_trend_alert(
                    pool,
                    server.user_id,
                    server.id,
                    "memory_leak",
                    &format!("Memory leak signal cleared on {}", server.name),
                    &format!(
                        "Memory usage on server {} is no longer trending upward.",
                        server.name
                    ),
                )
                .await;
            }
        }
    }
}

/// GAP 6 predicate: `Some((hours_to_full, rate_per_hour, current_pct))` while the
/// disk is forecast to fill within 48 hours, `None` once the trend clears.
///
/// ⚠ This note used to say the forecast was unreachable because production
/// supplied a ~30-minute window that could never satisfy the 6-hour minimum
/// below. That stopped being true when the query grew its 12-hour gate: the
/// live trend measures 1441 rows spanning 11.98 hours, so the window clears the
/// floor with room to spare and the `LIMIT 1440` is a bound on a stalled
/// collector rather than the window itself.
///
/// What still keeps it quiet on a healthy box is the second gate, which is the
/// one doing the work it was designed to do: a disk under 60% is not on a
/// runway to full, so `None` is the right answer rather than an accident. Both
/// gates are described below; neither is dead.
///
/// Skip the forecast on fresh installs / short trend windows. Linear
/// extrapolation over 30-60 minutes catches the install-time write spike
/// (binaries, frontend tarball, postgres init, container layers) and
/// predicts "disk full in 9 hours" at <5% real usage. Require:
///  - at least 6 hours of trend data (so the install spike has bled out), AND
///  - current disk usage already over 60% (so we're actually on a runway
///    to a real full disk, not extrapolating from noise on an empty box).
fn disk_full_forecast(
    trend: &[(f32, chrono::DateTime<chrono::Utc>)],
) -> Option<(f64, f64, f64)> {
    if trend.len() < 10 {
        return None;
    }
    let newest = trend.first()?;
    let oldest = trend.last()?;
    let hours_diff = (newest.1 - oldest.1).num_seconds() as f64 / 3600.0;
    if hours_diff < 6.0 || newest.0 < 60.0 {
        return None;
    }
    let rate_per_hour = (newest.0 as f64 - oldest.0 as f64) / hours_diff;
    if rate_per_hour <= 0.0 {
        return None;
    }
    let hours_to_full = (100.0 - newest.0 as f64) / rate_per_hour;
    if hours_to_full <= 0.0 || hours_to_full >= 48.0 {
        return None;
    }
    Some((hours_to_full, rate_per_hour, newest.0 as f64))
}

/// GAP 7 predicate: `Some((increase, older_avg, recent_avg))` while memory shows
/// a sustained climb (>10 points over the trend window and above 60%), `None`
/// once it flattens or falls back.
fn memory_leak_trend(trend: &[(f32,)]) -> Option<(f64, f64, f64)> {
    if trend.len() < 30 {
        return None;
    }
    let recent_avg: f64 = trend[..10].iter().map(|m| m.0 as f64).sum::<f64>() / 10.0;
    let older_avg: f64 = trend[20..30].iter().map(|m| m.0 as f64).sum::<f64>() / 10.0;
    let increase = recent_avg - older_avg;
    if increase > 10.0 && recent_avg > 60.0 {
        Some((increase, older_avg, recent_avg))
    } else {
        None
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
                // check_threshold's own alert_state rows are keyed '' — these are
                // conditions of the server as a whole, not of an entity on it.
                "",
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
            "",
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

/// Fire-gate for the trend-derived alerts (`memory_leak`, `disk_forecast`),
/// which have no per-sample state machine of their own the way
/// `check_threshold` does.
///
/// Returns `true` at most once per `cooldown_minutes` window while the
/// condition holds. The claim and the stamp are one statement, so two engines
/// racing the same window can't both page — whichever loses the `ON CONFLICT`
/// sees the fresh `last_notified_at` and returns no row.
///
/// The gate is purely time-based: it deliberately does NOT re-arm just because
/// the state row reads 'ok'. These predicates sit on a threshold (memory_leak
/// needs `recent_avg > 60.0`), so a server plateaued at the boundary flips the
/// condition on and off with ordinary jitter. Re-arming on the 'ok' transition
/// would let that flap page once per tick with the cooldown never applying —
/// reintroducing, through the gate meant to stop it, the exact storm this
/// function exists to prevent.
async fn claim_trend_fire(
    pool: &PgPool,
    server_id: Uuid,
    alert_type: &str,
    cooldown_minutes: i32,
) -> bool {
    // A rule with cooldown_minutes <= 0 re-arms on every tick, which is exactly
    // the storm this gate exists to stop — floor it at one tick.
    let cooldown = cooldown_minutes.max(1);

    let claimed: Option<(Uuid,)> = sqlx::query_as(
        "INSERT INTO alert_state (server_id, alert_type, state_key, current_state, fired_at, last_notified_at) \
         VALUES ($1, $2, '', 'firing', NOW(), NOW()) \
         ON CONFLICT (server_id, alert_type, state_key) WHERE server_id IS NOT NULL \
         DO UPDATE SET current_state = 'firing', \
                       fired_at = COALESCE(alert_state.fired_at, NOW()), \
                       last_notified_at = NOW() \
         WHERE alert_state.last_notified_at IS NULL \
            OR alert_state.last_notified_at < NOW() - make_interval(mins => $3) \
         RETURNING id",
    )
    .bind(server_id)
    .bind(alert_type)
    .bind(cooldown)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    claimed.is_some()
}

/// Resolve a trend-derived alert once its condition clears.
///
/// Without this the `alerts` row stays `status = 'firing'` forever: the hourly
/// purge only deletes resolved rows, and `check_escalations` keeps re-paging
/// on-call every 30 minutes for a leak that recovered or a disk that stopped
/// filling. Sends the recovery notification only on the actual transition, so a
/// steady-state server doesn't emit a "recovered" page every tick.
///
/// `last_notified_at` is deliberately PRESERVED: it is the cooldown clock
/// `claim_trend_fire` reads, and clearing it here would let a flapping
/// predicate re-page on the very next tick. Keeping it caps a flap at one fire
/// plus one recovery per cooldown window — once a re-fire is suppressed the
/// state row stays 'ok', so this UPDATE matches nothing and no second recovery
/// page is sent either.
async fn clear_trend_alert(
    pool: &PgPool,
    user_id: Uuid,
    server_id: Uuid,
    alert_type: &str,
    title: &str,
    message: &str,
) {
    let transitioned = sqlx::query(
        "UPDATE alert_state SET current_state = 'ok', fired_at = NULL \
         WHERE server_id = $1 AND alert_type = $2 AND state_key = '' AND current_state = 'firing'",
    )
    .bind(server_id)
    .bind(alert_type)
    .execute(pool)
    .await
    .map(|r| r.rows_affected() > 0)
    .unwrap_or(false);

    if transitioned {
        notifications::resolve_alert(
            pool,
            user_id,
            Some(server_id),
            None,
            alert_type,
            "",
            title,
            message,
        )
        .await;
    }
}

// ─── GPU Thresholds ─────────────────────────────────────────────────────

async fn check_gpu_thresholds(pool: &PgPool, member: &FleetMember) {
    let gpu_info = match member.agent.get("/apps/gpu-info").await {
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

    let (server_id, user_id, server_name) = (member.id, member.user_id, member.name.clone());

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

            fire_alert_with_retry(pool, user_id, Some(server_id), None, alert_type, state_key, severity, title, message).await;

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
            pool, user_id, Some(server_id), None, alert_type, state_key,
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

async fn check_server_offline(pool: &PgPool, maint: &HashSet<Uuid>) {
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
        // Before the firing row is written, not after: this loop's own query
        // excludes anything already firing, so a row stamped without a page
        // would never page again.
        if maint.contains(user_id) {
            continue;
        }

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
            "",
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
            "",
            &format!("Server {} is back online", name),
            &format!("Server {} has reconnected and is responding normally.", name),
        )
        .await;
    }
}

// ─── SSL Expiry ─────────────────────────────────────────────────────────

/// `last_warned_day` sentinel for "this site has never been paged". Every real
/// warning day is far below it, so the first crossing always pages.
const SSL_NEVER_WARNED: i64 = 999;

/// Dedup key for the EXPIRED page. Tighter than any configurable warning day
/// (those are >= 0), so an expired cert pages once and then stays quiet instead
/// of re-firing on every 60s tick until someone renews it.
const SSL_EXPIRED_WARN_DAY: i64 = -1;

/// What `check_ssl_expiry` should do for one site this tick.
#[derive(Debug, PartialEq, Eq)]
enum SslAction {
    /// Certificate moved back past the rung we last paged at — it was renewed.
    Resolve,
    /// Page at `warn_day` (the dedup key to stamp on success).
    Fire { warn_day: i64, severity: &'static str },
    /// Nothing to do.
    Nothing,
}

/// The SSL warning ladder, extracted so the lifecycle is testable without a DB.
///
/// `warning_days` must be descending (see `parse_ssl_warning_days`).
fn ssl_decision(
    days_left: i64,
    expired: bool,
    last_warned_day: i64,
    warning_days: &[i64],
) -> SslAction {
    // Renewed past the rung we last paged at → resolve. Nothing used to resolve
    // an ssl_expiry row, so a cert renewed weeks ago left a 'firing' row that
    // check_escalations re-paged every 30 minutes forever and the purge
    // (resolved-only) never collected.
    //
    // The test is against `last_warned_day`, NOT against the widest configured
    // rung: a cert renewed back to a value still inside the warning window (a
    // 90-day renewal under a '90' config, or a short-lived ACME profile whose
    // whole validity sits below the widest rung) is still a renewal. Gating on
    // the widest rung would leave exactly those sites ratcheting downward across
    // renewals until no rung — not even the EXPIRED sentinel — could ever fire
    // again. Ticking down normally can never satisfy this, since days_left only
    // falls.
    //
    // `!expired` is load-bearing. `days_left` truncates toward zero, so for the
    // whole first 24 hours after a lapse it reads 0 while the stamp written by
    // the EXPIRED page is -1 — and `0 > -1` would read as "renewed". That
    // alternates EXPIRED page / false-recovery page once per tick for a day,
    // which is precisely the storm this ship exists to kill. An expired
    // certificate is never a renewed one.
    if !expired && last_warned_day != SSL_NEVER_WARNED && days_left > last_warned_day {
        return SslAction::Resolve;
    }

    // The TIGHTEST rung crossed, not the first one listed. The old loop broke on
    // the first config entry satisfying `days_left <= warn_day`, so with the
    // default descending '30,14,7,3,1' it always picked 30 and the
    // `warn_day < last_warned_day` test then blocked every later rung — one
    // warning per certificate, ever, instead of the configured ladder.
    let warn_day = if expired {
        SSL_EXPIRED_WARN_DAY
    } else {
        match warning_days.iter().copied().filter(|d| days_left <= *d).min() {
            Some(d) => d,
            None => return SslAction::Nothing,
        }
    };

    if warn_day >= last_warned_day {
        return SslAction::Nothing; // already paged at this rung or a tighter one
    }

    let severity = if expired || days_left <= 3 {
        "critical"
    } else if days_left <= 7 {
        "warning"
    } else {
        "info"
    };

    SslAction::Fire { warn_day, severity }
}

async fn check_ssl_expiry(pool: &PgPool, maint: &HashSet<Uuid>) {
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
        if maint.contains(user_id) {
            continue;
        }

        let remaining = *ssl_expiry - now;
        // Expiry is decided on the SIGNED duration, not on num_days(), which
        // truncates toward zero: a certificate that lapsed anything under 24
        // hours ago still reports days_left = 0. Reading expiry off that meant
        // the EXPIRED page could not fire for a full day after the cert went
        // invalid — and by then rung 1 was already consumed, so browsers showed
        // a security warning for 24 hours with no notification on any channel.
        let expired = remaining < chrono::Duration::zero();
        let days_left = remaining.num_days();

        let (_, _, _, _, _, _, ssl_days_str) =
            notifications::get_thresholds(pool, *user_id, None).await;
        let warning_days = parse_ssl_warning_days(&ssl_days_str);

        // The tightest rung this site has already been paged at. Read once per
        // site — the expired branch needs it too, and it used to be read only
        // from inside the warning-ladder loop.
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
            .unwrap_or(SSL_NEVER_WARNED);

        let action = ssl_decision(days_left, expired, last_warned_day, &warning_days);

        if matches!(action, SslAction::Resolve) {
            let _ = sqlx::query(
                "UPDATE alert_state SET current_state = 'ok', last_notified_at = NULL, metadata = NULL \
                 WHERE site_id = $1 AND alert_type = 'ssl_expiry' AND state_key = ''",
            )
            .bind(site_id)
            .execute(pool)
            .await;

            notifications::resolve_alert(
                pool,
                *user_id,
                None,
                Some(*site_id),
                "ssl_expiry",
                "",
                &format!("SSL certificate renewed for {domain}"),
                &format!(
                    "The SSL certificate for {domain} is valid again — {days_left} days remaining."
                ),
            )
            .await;
            continue;
        }

        let SslAction::Fire { warn_day, severity } = action else {
            continue;
        };

        // Stamp the dedup key ONLY if the page actually landed. A transient DB
        // error inside fire_alert_with_retry used to still advance
        // last_warned_day, which permanently silenced the site: the stamp claims
        // the rung was paged, but no notification went out and no `alerts` row
        // exists for check_escalations to re-page from. Skipping the stamp makes
        // the next 60s tick retry, which is what the pre-rewrite code got for
        // free by re-firing every tick.
        if !fire_ssl_alert(pool, *user_id, *site_id, domain, days_left, expired, severity).await {
            tracing::warn!(
                "SSL alert for {domain} could not be recorded; leaving the warning ladder unstamped so the next tick retries"
            );
            continue;
        }

        // Update state with last_warned_day — PostgreSQL serializes concurrent ON CONFLICT upserts
        let _ = sqlx::query(
            "INSERT INTO alert_state (site_id, alert_type, state_key, current_state, last_notified_at, metadata) \
             VALUES ($1, 'ssl_expiry', '', 'firing', NOW(), $2) \
             ON CONFLICT (site_id, alert_type, state_key) WHERE site_id IS NOT NULL \
             DO UPDATE SET current_state = 'firing', last_notified_at = NOW(), metadata = $2",
        )
        .bind(site_id)
        .bind(serde_json::json!({ "last_warned_day": warn_day }))
        .execute(pool)
        .await;
    }
}

/// `check_ssl_expiry`'s sibling for Docker Compose stacks. Reuses
/// `ssl_decision` and `parse_ssl_warning_days` UNCHANGED — the ladder is a
/// pure function of days-left / expired / last-warned-day / rungs, and
/// neither cares what kind of row it is deciding for.
///
/// ACME-mode only, through `effective_tls_mode`: a `provided` stack serves its
/// certificate from the TLS registry, and a stale `ssl_expiry` on such a row
/// is not the panel's cert to alert on — the same rule
/// `certificate_dashboard_for_admin` and `renew_stack_certificate` already
/// enforce.
///
/// Keyed by `(server_id, "ssl_expiry", stack_ssl_expiry_state_key(domain))`.
/// `alert_state`/`alerts` carry no `docker_stack_id` column, only `server_id`
/// — mirroring `stack_renewal_state_key`'s own reasoning — so the domain has
/// to be IN the key or two stacks on one server would share one dedup bucket.
/// Reuses `alert_type = "ssl_expiry"` rather than inventing a second type, so
/// a stack's warning inherits the SAME `alert_ssl_expiry` toggle and
/// `ssl_warning_days` rungs a site's certificate already uses — an operator
/// configures "how many days before I'm warned" once, not per certificate
/// kind.
async fn check_stack_ssl_expiry(pool: &PgPool, maint: &HashSet<Uuid>) {
    let stacks: Vec<(
        Uuid,
        Uuid,
        Uuid,
        String,
        chrono::DateTime<chrono::Utc>,
        Option<String>,
        Option<String>,
    )> = match sqlx::query_as(
        "SELECT id, user_id, server_id, domain, ssl_expiry, tls_mode, ssl_email \
         FROM docker_stacks WHERE domain IS NOT NULL AND ssl_expiry IS NOT NULL",
    )
    .fetch_all(pool)
    .await
    {
        Ok(s) => s,
        Err(_) => return,
    };

    let now = chrono::Utc::now();

    for (_stack_id, user_id, server_id, domain, ssl_expiry, tls_mode, ssl_email) in &stacks {
        if maint.contains(user_id) {
            continue;
        }

        if crate::routes::stacks::effective_tls_mode(tls_mode.as_deref(), ssl_email.as_deref())
            != "acme"
        {
            continue;
        }

        let remaining = *ssl_expiry - now;
        let expired = remaining < chrono::Duration::zero();
        let days_left = remaining.num_days();

        let (_, _, _, _, _, _, ssl_days_str) =
            notifications::get_thresholds(pool, *user_id, None).await;
        let warning_days = parse_ssl_warning_days(&ssl_days_str);

        let state_key = crate::services::security_scanner::stack_ssl_expiry_state_key(domain);

        let state: Option<(serde_json::Value,)> = sqlx::query_as(
            "SELECT COALESCE(metadata, '{}') FROM alert_state \
             WHERE server_id = $1 AND alert_type = 'ssl_expiry' AND state_key = $2",
        )
        .bind(server_id)
        .bind(&state_key)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();

        let last_warned_day = state
            .as_ref()
            .and_then(|s| s.0.get("last_warned_day"))
            .and_then(|v| v.as_i64())
            .unwrap_or(SSL_NEVER_WARNED);

        let action = ssl_decision(days_left, expired, last_warned_day, &warning_days);

        if matches!(action, SslAction::Resolve) {
            let _ = sqlx::query(
                "UPDATE alert_state SET current_state = 'ok', last_notified_at = NULL, metadata = NULL \
                 WHERE server_id = $1 AND alert_type = 'ssl_expiry' AND state_key = $2",
            )
            .bind(server_id)
            .bind(&state_key)
            .execute(pool)
            .await;

            notifications::resolve_alert(
                pool,
                *user_id,
                Some(*server_id),
                None,
                "ssl_expiry",
                &state_key,
                &format!("SSL certificate renewed for {domain}"),
                &format!(
                    "The SSL certificate for {domain} is valid again — {days_left} days remaining."
                ),
            )
            .await;
            continue;
        }

        let SslAction::Fire { warn_day, severity } = action else {
            continue;
        };

        // Same unstamped-on-failure discipline as `check_ssl_expiry`: only stamp
        // the dedup key if the page actually landed, so a transient DB error
        // retries on the next tick instead of permanently silencing the stack.
        if !fire_stack_ssl_alert(
            pool, *user_id, *server_id, &state_key, domain, days_left, expired, severity,
        )
        .await
        {
            tracing::warn!(
                "Stack SSL alert for {domain} could not be recorded; leaving the warning ladder unstamped so the next tick retries"
            );
            continue;
        }

        let _ = sqlx::query(
            "INSERT INTO alert_state (server_id, alert_type, state_key, current_state, last_notified_at, metadata) \
             VALUES ($1, 'ssl_expiry', $2, 'firing', NOW(), $3) \
             ON CONFLICT (server_id, alert_type, state_key) WHERE server_id IS NOT NULL \
             DO UPDATE SET current_state = 'firing', last_notified_at = NOW(), metadata = $3",
        )
        .bind(server_id)
        .bind(&state_key)
        .bind(serde_json::json!({ "last_warned_day": warn_day }))
        .execute(pool)
        .await;
    }
}

/// `check_ssl_expiry`/`check_stack_ssl_expiry`'s sibling for a REGISTERED
/// (uploaded, non agent-managed) certificate in `tls_certificates` — the one
/// population neither of those two functions can see: `check_ssl_expiry` only
/// reads `sites.ssl_expiry` (ACME-issued, single-site certs) and
/// `check_stack_ssl_expiry` deliberately SKIPS a `provided`-mode stack's
/// `ssl_expiry` (it isn't the panel's cert to alert on — see that function's
/// own comment). A certificate served through the registry has its real
/// expiry in `tls_certificates.not_after`, and until now nothing ever read it
/// on a schedule; the only visibility was an operator opening the certificate
/// dashboard and reading `days_left` themselves.
///
/// Scoped to certificates a stack is ACTUALLY SERVING right now
/// (`docker_stacks.tls_certificate_id`), not every row in the registry — the
/// same "is this the panel's problem right now" framing `dashboard.rs` and
/// `check_stack_ssl_expiry` already apply. An operator who uploaded a
/// certificate ahead of assigning it to anything is not yet exposing traffic
/// through it, so there is nothing to page about.
async fn check_registered_cert_expiry(pool: &PgPool, maint: &HashSet<Uuid>) {
    let certs: Vec<(Uuid, Uuid, Uuid, String, Vec<String>, chrono::DateTime<chrono::Utc>)> =
        match sqlx::query_as(
            "SELECT DISTINCT tc.id, tc.user_id, tc.server_id, tc.alias, tc.dns_names, tc.not_after \
             FROM tls_certificates tc \
             JOIN docker_stacks ds ON ds.tls_certificate_id = tc.id \
             WHERE tc.not_after IS NOT NULL",
        )
        .fetch_all(pool)
        .await
        {
            Ok(c) => c,
            Err(_) => return,
        };

    let now = chrono::Utc::now();

    for (_cert_id, user_id, server_id, alias, dns_names, not_after) in &certs {
        if maint.contains(user_id) {
            continue;
        }

        let display_name = dns_names.first().cloned().unwrap_or_else(|| alias.clone());

        let remaining = *not_after - now;
        let expired = remaining < chrono::Duration::zero();
        let days_left = remaining.num_days();

        let (_, _, _, _, _, _, ssl_days_str) =
            notifications::get_thresholds(pool, *user_id, None).await;
        let warning_days = parse_ssl_warning_days(&ssl_days_str);

        let state_key = crate::services::security_scanner::registered_cert_expiry_state_key(alias);

        let state: Option<(serde_json::Value,)> = sqlx::query_as(
            "SELECT COALESCE(metadata, '{}') FROM alert_state \
             WHERE server_id = $1 AND alert_type = 'ssl_expiry' AND state_key = $2",
        )
        .bind(server_id)
        .bind(&state_key)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();

        let last_warned_day = state
            .as_ref()
            .and_then(|s| s.0.get("last_warned_day"))
            .and_then(|v| v.as_i64())
            .unwrap_or(SSL_NEVER_WARNED);

        let action = ssl_decision(days_left, expired, last_warned_day, &warning_days);

        if matches!(action, SslAction::Resolve) {
            let _ = sqlx::query(
                "UPDATE alert_state SET current_state = 'ok', last_notified_at = NULL, metadata = NULL \
                 WHERE server_id = $1 AND alert_type = 'ssl_expiry' AND state_key = $2",
            )
            .bind(server_id)
            .bind(&state_key)
            .execute(pool)
            .await;

            notifications::resolve_alert(
                pool,
                *user_id,
                Some(*server_id),
                None,
                "ssl_expiry",
                &state_key,
                &format!("Registered certificate renewed for {display_name}"),
                &format!(
                    "The registered certificate \"{display_name}\" is valid again — {days_left} days remaining."
                ),
            )
            .await;
            continue;
        }

        let SslAction::Fire { warn_day, severity } = action else {
            continue;
        };

        // Same unstamped-on-failure discipline as `check_stack_ssl_expiry`:
        // only stamp the dedup key if the page actually landed.
        if !fire_stack_ssl_alert(
            pool, *user_id, *server_id, &state_key, &display_name, days_left, expired, severity,
        )
        .await
        {
            tracing::warn!(
                "Registered-certificate SSL alert for {display_name} could not be recorded; leaving the warning ladder unstamped so the next tick retries"
            );
            continue;
        }

        let _ = sqlx::query(
            "INSERT INTO alert_state (server_id, alert_type, state_key, current_state, last_notified_at, metadata) \
             VALUES ($1, 'ssl_expiry', $2, 'firing', NOW(), $3) \
             ON CONFLICT (server_id, alert_type, state_key) WHERE server_id IS NOT NULL \
             DO UPDATE SET current_state = 'firing', last_notified_at = NOW(), metadata = $3",
        )
        .bind(server_id)
        .bind(&state_key)
        .bind(serde_json::json!({ "last_warned_day": warn_day }))
        .execute(pool)
        .await;
    }
}

/// Parse `alert_rules.ssl_warning_days` into a descending, de-duplicated list of
/// non-negative rungs. Falls back to the shipped default when the column is
/// empty or entirely unparseable, so a bad value can't silence SSL alerting.
fn parse_ssl_warning_days(raw: &str) -> Vec<i64> {
    let mut days: Vec<i64> = raw
        .split(',')
        .filter_map(|s| s.trim().parse::<i64>().ok())
        .filter(|d| *d >= 0)
        .collect();
    days.sort_unstable_by(|a, b| b.cmp(a));
    days.dedup();
    if days.is_empty() {
        days = vec![30, 14, 7, 3, 1];
    }
    days
}

/// Title + message text for an SSL-expiry ladder page. `expired` is passed
/// explicitly rather than inferred from `days_left <= 0`: `days_left` is
/// truncated toward zero, so a still-valid certificate with up to 23 hours
/// left also reads 0 and used to be announced to the operator as already
/// EXPIRED.
///
/// Shared by the site and stack ladders — the copy depends only on the domain
/// and how much time is left, never on what kind of row is expiring.
fn ssl_alert_copy(domain: &str, days_left: i64, expired: bool) -> (String, String) {
    let window = if days_left <= 0 {
        "less than a day".to_string()
    } else if days_left == 1 {
        "1 day".to_string()
    } else {
        format!("{days_left} days")
    };

    let title = if expired {
        format!("SSL certificate EXPIRED for {domain}")
    } else {
        format!("SSL certificate expires in {window} for {domain}")
    };

    let message = if expired {
        format!(
            "The SSL certificate for {domain} has expired. Visitors will see security warnings. Renew immediately."
        )
    } else {
        format!(
            "The SSL certificate for {domain} will expire in {window}. Please renew it before it expires."
        )
    };

    (title, message)
}

async fn fire_ssl_alert(
    pool: &PgPool,
    user_id: Uuid,
    site_id: Uuid,
    domain: &str,
    days_left: i64,
    expired: bool,
    severity: &str,
) -> bool {
    let (title, message) = ssl_alert_copy(domain, days_left, expired);
    fire_alert_with_retry(
        pool, user_id, None, Some(site_id), "ssl_expiry", "", severity, &title, &message,
    )
    .await
}

/// `fire_ssl_alert`'s sibling for a Compose stack. A stack alert has no
/// `site_id` — a `docker_stacks` row has no `sites` row to point at — so this
/// fires with `server_id: Some` and a domain-derived `state_key` instead of
/// the site ladder's `site_id: Some` + empty key.
async fn fire_stack_ssl_alert(
    pool: &PgPool,
    user_id: Uuid,
    server_id: Uuid,
    state_key: &str,
    domain: &str,
    days_left: i64,
    expired: bool,
    severity: &str,
) -> bool {
    let (title, message) = ssl_alert_copy(domain, days_left, expired);
    fire_alert_with_retry(
        pool, user_id, Some(server_id), None, "ssl_expiry", state_key, severity, &title, &message,
    )
    .await
}

// ─── Service Health ─────────────────────────────────────────────────────

async fn check_service_health(pool: &PgPool, member: &FleetMember) {
    let services: Vec<serde_json::Value> = match member.agent.get("/services/health").await {
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

    let (server_id, user_id, server_name) = (member.id, member.user_id, member.name.clone());

    for svc in &services {
        let name = svc["name"].as_str().unwrap_or("");
        let status = svc["status"].as_str().unwrap_or("unknown");

        if name.is_empty() || status == "not_installed" || status == "disabled" {
            continue;
        }

        if status == "stopped" || status == "failed" {
            // Skip alerting if auto-healer recently handled this service (within 5
            // minutes) — ON THIS HOST. The writer of these rows has stamped
            // `server_id` since v2.58.0 and its own cooldown reads it back, but
            // this reader was left matching on `target_name` alone, so a heal of
            // `nginx` on one server silenced the `service_down` alert for `nginx`
            // on every other server for five minutes. Service names are the least
            // distinctive key in the fleet — every host has an `nginx` — so this
            // is the shape that goes wrong the moment a second server exists.
            let recently_healed: Option<(i64,)> = sqlx::query_as(
                "SELECT COUNT(*) FROM activity_logs \
                 WHERE action = 'auto_heal.restart_service' \
                 AND target_name = $1 AND server_id = $2 \
                 AND created_at > NOW() - INTERVAL '5 minutes'",
            )
            .bind(name)
            .bind(server_id)
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
                    name,
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
                    name,
                    &format!("Service {} recovered on {}", name, server_name),
                    &format!("The {} service is running again on server {}.", name, server_name),
                )
                .await;
            }
        }
    }
}

// ─── GAP 8: Docker Container Health ──────────────────────────────────────

async fn check_container_health(pool: &PgPool, member: &FleetMember) {
    // Stamped BEFORE the snapshot, because the walk below awaits a database and
    // notification round-trip per container: an expectation recorded by an
    // operator mid-walk is NEWER than this evidence and must survive it.
    let observed_at = chrono::Utc::now();

    let containers: Vec<serde_json::Value> = match member.agent.get("/apps").await {
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

    let (server_id, user_id) = (member.id, member.user_id);

    // Containers this host is holding stopped ON PURPOSE. Loaded once per
    // member rather than queried per container, which would multiply this
    // sweep's round-trips by the app count.
    let expected_stopped = expected_stops::expected_on_server(pool, server_id).await;

    // Every container this host still reports, whatever its state. Reaching
    // this line means the agent answered with a well-formed array, so a name
    // absent from it is genuinely absent from the host — see the sweep at the
    // end of this function.
    let observed: std::collections::HashSet<&str> = containers
        .iter()
        .filter_map(|c| c.get("name").and_then(|v| v.as_str()))
        .collect();

    for c in &containers {
        let name = c.get("name").and_then(|v| v.as_str()).unwrap_or("unknown");
        let state = c.get("status").and_then(|v| v.as_str()).unwrap_or("");
        let health = c.get("health").and_then(|v| v.as_str());

        // ── The expectation is cleared from OBSERVATION, above the ladder ──
        //
        // Docker running this container is what falsifies "a stop is expected",
        // and that fact is independent of the healthcheck. The recovery arm
        // below is `running && health != unhealthy` and sits under the unhealthy
        // arm, so a container that comes back up with a failing healthcheck —
        // the ordinary case for something that was stopped BECAUSE it was
        // misbehaving — would never reach it, keep its expectation for ever, and
        // be suppressed when it finally died. `restarting`, `paused` and
        // `created` have no arm at all. So the clear is keyed on the one thing
        // that matters: this container is not stopped.
        if state != "exited" && state != "dead" && expected_stopped.contains(name) {
            expected_stops::clear_if_older_than(pool, server_id, name, observed_at).await;
        }

        if state == "exited" || state == "dead" {
            // ⛔ A stop the panel carried out is not an outage. Skipping the
            // WHOLE branch — not just the fire — is deliberate: the `alert_state`
            // row is written 'firing' immediately above the fire, and the branch
            // is guarded by `!= Some("firing")`, so stamping the row while
            // sending nothing would leave this container claiming it had paged
            // and silently suppress every future `container_down` for that name.
            // This file's own comment further down records a live instance of
            // exactly that tombstone, still firing four months later.
            if expected_stopped.contains(name) {
                continue;
            }

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
                    name,
                    // The severity this product DECLARES for this alert:
                    // `alert_runbook_defaults.rs` lists container_down under
                    // "Info", and the runbook attached to every one of these
                    // notifications says "a planned `docker stop` is normal
                    // operational use, so we don't page on it" and "Don't page
                    // on this alone". It was raised at "critical", which is the
                    // string the incident branch gates on — so a container going
                    // down opened a CRITICAL incident on the operator's public
                    // status page, contradicting the runbook in the same email.
                    "info",
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
                    name,
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
                    name,
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
                        name,
                        &format!("Container '{}' recovered", name),
                        &format!("Docker container '{}' is running and healthy again.", name),
                    )
                    .await;
                }
            }
        }
    }

    // A REMOVED container never appears in `/apps` again, so neither the fire
    // branches nor the recovered branch above can ever run for it and its row
    // stays `firing` for ever. Not theoretical: this panel carried a
    // `container_down` row for a container deleted on 2026-03-23, still
    // `firing` four months later, with ZERO matching `alerts` rows ever
    // written — because the fire branches skip a key that already reads
    // `firing`, that one row had silently suppressed every future alert for
    // that container name. Retention cannot clear it either; both purges only
    // delete `status = 'resolved'`.
    //
    // Threading this check makes the sweep both possible and necessary: the
    // subject is now "the containers THIS member reports", so absence from that
    // list is a fact about one host rather than about the whole fleet.
    let stale: Vec<(String, String)> = sqlx::query_as(
        "SELECT alert_type, state_key FROM alert_state \
         WHERE server_id = $1 AND current_state = 'firing' \
           AND alert_type IN ('container_down', 'container_unhealthy', 'container_crashloop')",
    )
    .bind(server_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    for (alert_type, state_key) in &stale {
        if observed.contains(state_key.as_str()) {
            continue;
        }

        let _ = sqlx::query(
            "UPDATE alert_state SET current_state = 'ok', fired_at = NULL, last_notified_at = NULL \
             WHERE server_id = $1 AND alert_type = $2 AND state_key = $3",
        )
        .bind(server_id)
        .bind(alert_type)
        .bind(state_key)
        .execute(pool)
        .await;

        notifications::resolve_alert(
            pool,
            user_id,
            Some(server_id),
            None,
            alert_type,
            state_key,
            &format!("Container '{}' is no longer present", state_key),
            &format!(
                "Docker container '{}' is no longer reported by {}. Its alert state has been \
                 cleared so a container of that name can raise an alert again.",
                state_key, member.name
            ),
        )
        .await;

        tracing::info!(
            "Alert engine: cleared stale {alert_type} state for absent container {state_key} on {}",
            member.name
        );
    }

    // The same absence, one table over — and it had no sweep at all.
    //
    // The clear above the ladder runs from OBSERVING a container alive, so a
    // REMOVED container never reaches it: its expectation row outlives it, and
    // the next container to claim that name inherits the silence. Removing an
    // app and redeploying it under the same name is the supported way to rebuild
    // one while keeping its data, so the container that inherits it is the one
    // the operator has just been trying to repair — and if it fails to start,
    // this engine skips its `container_down`, the auto-heal restart leg skips
    // it, and the Apps page files it under "stopped on purpose", naming whoever
    // stopped the container that no longer exists.
    //
    // The removal doors clear their own row, which is exact and has no window.
    // This sweep is what covers a removal the panel did not perform — `docker
    // rm` on the box — and any door added later, which is the failure the
    // hardcoded list in this table's own pin suite could not see.
    let observed_names: Vec<String> = observed.iter().map(|n| (*n).to_string()).collect();
    expected_stops::clear_absent(pool, server_id, &observed_names, observed_at).await;
}

// ─── GAP 9: Alert Escalation ────────────────────────────────────────────

/// What a policy-driven row should do on this tick.
///
/// Extracted as a pure function for the same reason `ssl_decision` above is:
/// the loop that used to hold this inline needed a live pool, a live policy and
/// a firing alert to exercise, so nothing exercised it, and the branch that
/// decided an exhausted chain should fall silent for ever was never once run in
/// a test. The arithmetic is the whole behaviour; keep it reachable.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum EscalationAction {
    /// Not eligible this tick, and its sort position must not move.
    Skip,
    /// Nothing to page, but stamp it so it rotates to the back of the window.
    Terminal,
    /// Page this step and record it as the chain's new position.
    Advance(usize),
    /// The chain is spent: page its last step again, leaving the position.
    RepeatLast,
}

/// `after` is each step's `after_minutes`, in chain order. `since_escalated` is
/// minutes since this row was last paged, `None` if it never has been.
fn escalation_decision(
    after: &[i64],
    current_index: usize,
    elapsed_minutes: i64,
    since_escalated: Option<i64>,
) -> EscalationAction {
    if after.is_empty() {
        return EscalationAction::Terminal;
    }

    // Highest step past the current position whose threshold has been crossed.
    // Walking from the end jumps straight to the furthest-eligible rung rather
    // than creeping one rung per tick.
    for i in (0..after.len()).rev() {
        if i <= current_index {
            break;
        }
        if after[i] <= elapsed_minutes {
            return EscalationAction::Advance(i);
        }
    }

    // Spent. It must not go quiet — that made a policy strictly worse than no
    // policy at all — so it drops onto the cadence an unattached rule gets.
    // Skip, not Terminal: a spent chain is no longer terminal for anything, so
    // stamping it here would only push its own clock forward and delay the
    // first fallback page past the fifteen minutes the cadence promises. It
    // cannot starve the sweep window either — a row this young sorts to the
    // back of the least-recently-paged ordering by its `created_at` alone.
    if elapsed_minutes < 15 {
        return EscalationAction::Skip;
    }
    match since_escalated {
        Some(m) if m < 30 => EscalationAction::Skip,
        _ => EscalationAction::RepeatLast,
    }
}

/// Re-notify for unacknowledged firing alerts.
///
/// Phase 4 W3: two paths.
/// - When the owning `alert_rules` row has `escalation_policy_id IS NULL`, the
///   pre-W3 behaviour is preserved exactly: re-page every 30 min for alerts
///   older than 15 min. The W3 ship also folds in the W2 runbook payload here
///   — bare `send_notification` was leaving the runbook excerpt + URL out of
///   escalation pages even though `try_fire_alert` carried them on the
///   original fire.
/// - When `escalation_policy_id IS NOT NULL`, the policy chain drives
///   `(after_minutes, route)` steps. Each minute we advance to the highest
///   step whose `after_minutes <= alert age` AND whose index is strictly
///   greater than `alerts.escalation_step_index`. Once the last step has
///   fired the chain stays exhausted, but the alert does NOT go quiet: it
///   drops onto the same 30-minute cadence an unattached rule gets, paging the
///   last rung's route until it is acknowledged or resolved.
///
///   That fallback is the whole point. Terminating for good made attaching a
///   policy strictly worse than attaching none — and the policy editor seeded
///   one step by default, so the ordinary outcome of building a policy was a
///   single page on a firing critical and then silence. The operator-facing
///   documentation has always promised a repeating cadence for every alert;
///   this is the path that makes that true rather than the sentence that
///   explained it away.
async fn check_escalations(pool: &PgPool) {
    #[derive(sqlx::FromRow)]
    struct EscalationRow {
        id: Uuid,
        user_id: Uuid,
        server_id: Option<Uuid>,
        alert_type: String,
        severity: String,
        title: String,
        message: String,
        created_at: chrono::DateTime<chrono::Utc>,
        escalation_step_index: i32,
        escalated_at: Option<chrono::DateTime<chrono::Utc>>,
    }

    // Bounded sweep. This query used to be unbounded, and every row cost 3-5 DB
    // round-trips per 60s tick (policy lookup + runbook + base URL) *before* any
    // eligibility check — so a pile-up of never-resolving firing rows turned the
    // escalation pass into self-amplifying load on the shared pool.
    //
    // Ordered least-recently-paged first, NOT newest-first. Any fixed window over
    // a sorted-by-age list can starve: newest-first drops an old unacknowledged
    // critical out of the window once enough newer rows exist, and oldest-first
    // does the reverse. COALESCE(escalated_at, created_at) makes it a fair
    // rotation instead — paging a row stamps escalated_at, which sends it to the
    // back, so every eligible row is serviced in turn and none can be crowded out
    // indefinitely. Rows never paged sort by age, and a brand-new row is not
    // eligible for 15 minutes anyway, by which point it competes on equal terms.
    const ESCALATION_SWEEP_LIMIT: i64 = 500;

    let rows: Vec<EscalationRow> = match sqlx::query_as(
        "SELECT id, user_id, server_id, alert_type, severity, title, message, \
                created_at, escalation_step_index, escalated_at \
         FROM alerts \
         WHERE status = 'firing' AND acknowledged_at IS NULL \
           AND created_at > NOW() - INTERVAL '7 days' \
         ORDER BY COALESCE(escalated_at, created_at) ASC \
         LIMIT $1",
    )
    .bind(ESCALATION_SWEEP_LIMIT)
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::debug!("Escalation query skipped: {e}");
            return;
        }
    };

    if rows.len() as i64 == ESCALATION_SWEEP_LIMIT {
        tracing::warn!(
            "Escalation sweep hit its {ESCALATION_SWEEP_LIMIT}-row cap — older unacknowledged \
             alerts were not considered this tick; acknowledge or resolve the backlog"
        );
    }

    let now = chrono::Utc::now();

    // Per-tick memoization. Firing alerts cluster hard on a few users, servers
    // and alert types, so these turn O(rows) queries into O(distinct keys).
    let mut policy_cache: std::collections::HashMap<(Uuid, Option<Uuid>), Option<Uuid>> =
        std::collections::HashMap::new();
    let mut steps_cache: std::collections::HashMap<Uuid, Vec<crate::models::EscalationStep>> =
        std::collections::HashMap::new();
    let mut runbook_cache: std::collections::HashMap<String, (Option<String>, Option<String>)> =
        std::collections::HashMap::new();
    // Same shape, for the per-type disable switch. Firing rows cluster hard on a
    // few (user, server, type) keys, so this is O(distinct keys) not O(rows).
    let mut enabled_cache: std::collections::HashMap<(Uuid, Option<Uuid>, String), bool> =
        std::collections::HashMap::new();

    // Rows the policy branch declines to page because the chain is exhausted (or
    // the policy has no steps). They can never page again, but they keep the
    // oldest sort key, so under the least-recently-paged ordering they would pin
    // the head of the window forever and crowd out live alerts. Stamped in one
    // batch below so they rotate to the back. Only the POLICY branch may do
    // this: the NULL-policy branch reads escalated_at for its 30-minute re-page
    // rule, so stamping a not-yet-eligible row there would delay its first page.
    let mut terminal_ids: Vec<Uuid> = Vec::new();

    for row in &rows {
        // The operator can switch an alert type off. Until now that switch had
        // exactly ONE caller — inside `try_fire_alert` — so it governed only
        // whether a row was ever RECORDED, and said nothing about a row already
        // firing when the switch was flipped. This sweep selects on
        // `status = 'firing'` alone and is provenance-blind, so the one control
        // meaning "do not tell me about this" left the alert re-paging every
        // thirty minutes until it aged out of the seven-day window above.
        //
        // ⚠ Skipped WITHOUT stamping `escalated_at`. Stamping would rotate the
        // row to the back of the least-recently-paged ordering — which is what
        // `terminal_ids` exists for — but `escalated_at` means *this row was
        // paged at this time*, and writing it for a page that was never sent is
        // exactly the harm `maintenance_users` documents a few hundred lines
        // above: a stamp claiming the operator was told. A re-enabled type pages
        // on the NEXT tick, not thirty minutes after one.
        //
        // ⚠ Scope: `is_alert_enabled` returns true for any type it has no column
        // for, so this narrows paging for the eight mapped types ONLY (cpu,
        // memory, disk, offline, backup_failure, ssl_expiry, service_down and
        // the three gpu_* sharing `alert_gpu`). Every other type escalates
        // exactly as before.
        let enabled_key = (row.user_id, row.server_id, row.alert_type.clone());
        let enabled = match enabled_cache.get(&enabled_key) {
            Some(cached) => *cached,
            None => {
                let e =
                    notifications::is_alert_enabled(pool, row.user_id, row.server_id, &row.alert_type)
                        .await;
                enabled_cache.insert(enabled_key, e);
                e
            }
        };
        if !enabled {
            tracing::debug!(
                "Alert type '{}' is switched off for this owner — skipping escalation for alert {}",
                row.alert_type,
                row.id
            );
            continue;
        }

        let policy_key = (row.user_id, row.server_id);
        let policy_id = match policy_cache.get(&policy_key) {
            Some(cached) => *cached,
            None => {
                let pid = notifications::get_user_policy_id(pool, row.user_id, row.server_id).await;
                policy_cache.insert(policy_key, pid);
                pid
            }
        };

        // Decide first, work second: everything below this match (runbook load,
        // HTML build, dispatch) is skipped for rows that don't page this tick.
        let (step, next_index) = match policy_id {
            None => {
                // Pre-W3 default cadence: 15 min unack threshold, 30 min re-page.
                let age = now - row.created_at;
                if age < chrono::Duration::minutes(15) {
                    continue;
                }
                if let Some(escalated_at) = row.escalated_at {
                    if now - escalated_at < chrono::Duration::minutes(30) {
                        continue;
                    }
                }

                // Fan out via the alert owner's channels — same audience as
                // try_fire_alert's NULL-policy path. dispatch_escalation_step
                // handles per-user mute checks and (W2 repair) carries the
                // runbook payload.
                (
                    crate::models::EscalationStep {
                        after_minutes: 0,
                        route: "all_channels".to_string(),
                    },
                    None,
                )
            }
            Some(pid) => {
                if !steps_cache.contains_key(&pid) {
                    let loaded = notifications::load_escalation_steps(pool, pid).await;
                    steps_cache.insert(pid, loaded);
                }
                let steps = &steps_cache[&pid];
                if steps.is_empty() {
                    tracing::warn!(
                        "Alert {} references escalation_policy {pid} with empty steps; skipping",
                        row.id
                    );
                    terminal_ids.push(row.id);
                    continue;
                }

                let elapsed_minutes = (now - row.created_at).num_minutes();
                let current_index = row.escalation_step_index as usize;
                let after: Vec<i64> = steps.iter().map(|s| s.after_minutes as i64).collect();
                let since_escalated = row.escalated_at.map(|e| (now - e).num_minutes());

                match escalation_decision(&after, current_index, elapsed_minutes, since_escalated) {
                    EscalationAction::Advance(i) => (steps[i].clone(), Some(i)),
                    EscalationAction::RepeatLast => {
                        // The chain is spent. It used to stop here for good,
                        // which made attaching a policy strictly worse than
                        // attaching none: an unattached rule re-pages an
                        // unacknowledged alert every 30 minutes for as long as
                        // the sweep can still see it, and the shipped operator
                        // documentation promises exactly that, without
                        // qualification, for every alert. A policy built to be
                        // paged HARDER cannot be the thing that stops paging
                        // you — and the editor seeds a single-step policy, so
                        // the ordinary outcome was one page and then silence.
                        //
                        // The LAST rung's route, not the default fan-out:
                        // those are the destinations the operator chose for the
                        // worst case, and this is the worst case. The index
                        // stays put, so the chain stays spent and only the
                        // clock advances.
                        (steps[steps.len() - 1].clone(), None)
                    }
                    EscalationAction::Terminal => {
                        terminal_ids.push(row.id);
                        continue;
                    }
                    EscalationAction::Skip => continue,
                }
            }
        };

        if !runbook_cache.contains_key(&row.alert_type) {
            let payload = notifications::load_runbook_payload(pool, &row.alert_type).await;
            runbook_cache.insert(row.alert_type.clone(), payload);
        }
        let (runbook_excerpt, runbook_url) = runbook_cache[&row.alert_type].clone();

        // Single-sourced with try_fire_alert's own initial-page HTML
        // (notifications::severity_color) so an escalated warning does not
        // read as an escalated critical — the row's OWN stored severity, not
        // a hardcoded red for every re-page.
        let esc_color = notifications::severity_color(&row.severity);
        let esc_subject = format!("[ESCALATED] {}", row.title);
        let html = format!(
            "<div style=\"font-family:sans-serif;max-width:600px;margin:0 auto\">\
             <h2 style=\"color:{esc_color}\">[ESCALATED] {}</h2>\
             <p>{}</p>\
             <p style=\"color:{esc_color};font-weight:bold\">This alert has not been acknowledged. Please investigate immediately.</p>\
             <p style=\"color:#6b7280;font-size:14px\">Time: {}</p>\
             </div>",
            row.title,
            row.message,
            now.format("%Y-%m-%d %H:%M:%S UTC"),
        );

        notifications::dispatch_escalation_step(
            pool,
            row.user_id,
            row.server_id,
            &row.alert_type,
            &step,
            &esc_subject,
            &row.message,
            &html,
            &row.severity,
            runbook_excerpt.as_deref(),
            runbook_url.as_deref(),
        )
        .await;

        match next_index {
            None => {
                let _ = sqlx::query("UPDATE alerts SET escalated_at = $1 WHERE id = $2")
                    .bind(now)
                    .bind(row.id)
                    .execute(pool)
                    .await;

                tracing::info!(
                    "Escalated unacknowledged alert (default cadence): {}",
                    row.title
                );
            }
            Some(i) => {
                let _ = sqlx::query(
                    "UPDATE alerts SET escalation_step_index = $1, escalated_at = $2 WHERE id = $3",
                )
                .bind(i as i32)
                .bind(now)
                .bind(row.id)
                .execute(pool)
                .await;

                tracing::info!("Escalated alert (policy step {i}): {}", row.title);
            }
        }
    }

    // One statement, not one per row — these are precisely the rows we did NOT
    // want to spend per-row work on. Marking them "considered" moves them off
    // the head of the least-recently-paged ordering so a pile of exhausted
    // policy alerts (a cron failing every 5 minutes, say — nothing resolves
    // those) cannot fill the window and starve a fresh critical out of it.
    if !terminal_ids.is_empty() {
        let _ = sqlx::query("UPDATE alerts SET escalated_at = $1 WHERE id = ANY($2)")
            .bind(now)
            .bind(&terminal_ids)
            .execute(pool)
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use EscalationAction::*;

    /// THE REGRESSION. The policy editor seeds a chain with one step, and a
    /// one-step chain is exhausted from the first sweep. This used to be
    /// terminal for ever: the operator got the page that fired the alert and
    /// then silence, on a firing critical, for having built a policy at all.
    #[test]
    fn a_spent_chain_keeps_paging_instead_of_falling_silent() {
        // One step, never paged since, an hour old and unacknowledged.
        assert_eq!(escalation_decision(&[0], 0, 60, None), RepeatLast);
        // And it keeps saying so, tick after tick, once the cadence is due.
        assert_eq!(escalation_decision(&[0], 0, 600, Some(31)), RepeatLast);
        assert_eq!(escalation_decision(&[0], 0, 10_000, Some(45)), RepeatLast);
    }

    /// The fallback is the SAME cadence an unattached rule gets, so a policy is
    /// never worse than no policy: nothing before 15 minutes, then every 30.
    #[test]
    fn the_fallback_matches_the_unattached_cadence() {
        assert_eq!(escalation_decision(&[0], 0, 14, None), Skip, "too young to page");
        assert_eq!(escalation_decision(&[0], 0, 15, None), RepeatLast, "eligible at 15");
        assert_eq!(escalation_decision(&[0], 0, 60, Some(29)), Skip, "inside the 30-min window");
        assert_eq!(escalation_decision(&[0], 0, 60, Some(30)), RepeatLast, "the window has lapsed");
    }

    /// A row that is merely waiting must NOT be stamped: `Terminal` moves it to
    /// the back of the least-recently-paged rotation, and doing that on every
    /// tick of the 30-minute wait would keep resetting its own clock.
    #[test]
    fn waiting_is_not_the_same_as_terminal() {
        assert_ne!(escalation_decision(&[0], 0, 60, Some(5)), Terminal);
        assert_eq!(escalation_decision(&[0], 0, 60, Some(5)), Skip);
    }

    /// The chain still drives the escalation while it has rungs left, and it
    /// jumps to the furthest one whose threshold has passed rather than
    /// creeping a rung per tick.
    #[test]
    fn the_chain_advances_to_the_furthest_eligible_rung() {
        let chain = [0, 15, 60];
        assert_eq!(escalation_decision(&chain, 0, 16, None), Advance(1));
        assert_eq!(escalation_decision(&chain, 0, 90, None), Advance(2), "jumps past rung 1");
        assert_eq!(escalation_decision(&chain, 1, 90, None), Advance(2));
        // Only once it is genuinely spent does the fallback take over.
        assert_eq!(escalation_decision(&chain, 2, 90, None), RepeatLast);
    }

    /// A rung already paged is never paged again by the chain — the position
    /// only ever moves forward. Regression on the `i <= current_index` break.
    #[test]
    fn the_chain_never_walks_backwards() {
        let chain = [0, 15, 60];
        for elapsed in [16, 61, 600] {
            match escalation_decision(&chain, 2, elapsed, None) {
                Advance(i) => panic!("advanced to {i} from a spent chain at {elapsed}m"),
                _ => {}
            }
        }
        assert_eq!(escalation_decision(&chain, 1, 20, Some(1)), Skip,
                   "rung 2 not yet due and rung 1 already paged");
    }

    /// A policy row with no steps has nothing to page and must rotate out of
    /// the sweep window rather than pin its head.
    #[test]
    fn an_empty_chain_is_terminal() {
        assert_eq!(escalation_decision(&[], 0, 600, None), Terminal);
    }

    fn ts(minutes_ago: i64) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc::now() - chrono::Duration::minutes(minutes_ago)
    }

    #[test]
    fn ssl_warning_days_sorts_descending_and_dedupes() {
        assert_eq!(parse_ssl_warning_days("7, 30,14,7"), vec![30, 14, 7]);
    }

    #[test]
    fn ssl_warning_days_falls_back_when_unusable() {
        // An empty, garbage, or all-negative column must not silence SSL alerting.
        assert_eq!(parse_ssl_warning_days(""), vec![30, 14, 7, 3, 1]);
        assert_eq!(parse_ssl_warning_days("soon, later"), vec![30, 14, 7, 3, 1]);
        assert_eq!(parse_ssl_warning_days("-5,-1"), vec![30, 14, 7, 3, 1]);
    }

    #[test]
    fn ssl_ladder_picks_the_tightest_rung_crossed() {
        // The regression this replaced: iterating the config in its stored
        // (descending) order and breaking on the first match always yielded 30,
        // so `warn_day < last_warned_day` blocked every later rung and only one
        // warning per certificate ever fired.
        let rungs = parse_ssl_warning_days("30,14,7,3,1");
        let tightest = |days_left: i64| {
            rungs
                .iter()
                .copied()
                .filter(|d| days_left <= *d)
                .min()
        };
        assert_eq!(tightest(29), Some(30));
        assert_eq!(tightest(14), Some(14));
        assert_eq!(tightest(10), Some(14));
        assert_eq!(tightest(1), Some(1));
        assert_eq!(tightest(31), None);
    }

    /// Drive a certificate's whole life through `ssl_decision`, threading
    /// `last_warned_day` exactly as the DB does, and assert what the operator
    /// receives at each step.
    fn walk(steps: &[(i64, bool)], rungs: &[i64]) -> Vec<SslAction> {
        let mut last = SSL_NEVER_WARNED;
        let mut out = Vec::new();
        for &(days_left, expired) in steps {
            let action = ssl_decision(days_left, expired, last, rungs);
            match action {
                SslAction::Fire { warn_day, .. } => last = warn_day,
                SslAction::Resolve => last = SSL_NEVER_WARNED,
                SslAction::Nothing => {}
            }
            out.push(action);
        }
        out
    }

    #[test]
    fn ssl_ladder_pages_once_per_rung_then_once_on_expiry() {
        let rungs = parse_ssl_warning_days("30,14,7,3,1");
        // 40 -> 25 -> 25 -> 10 -> 5 -> 2 -> 0 -> expired -> expired -> expired
        let life = [
            (40, false),
            (25, false),
            (25, false),
            (10, false),
            (5, false),
            (2, false),
            (0, false),
            (-1, true),
            (-2, true),
            (-3, true),
        ];
        let seen = walk(&life, &rungs);

        let fires: Vec<i64> = seen
            .iter()
            .filter_map(|a| match a {
                SslAction::Fire { warn_day, .. } => Some(*warn_day),
                _ => None,
            })
            .collect();
        // One page per rung crossed, then exactly ONE expired page — the storm
        // this ship exists to kill fired one page per 60s tick forever here.
        assert_eq!(fires, vec![30, 14, 7, 3, 1, SSL_EXPIRED_WARN_DAY]);
        assert_eq!(seen[0], SslAction::Nothing, "40 days is outside every rung");
        assert_eq!(seen[2], SslAction::Nothing, "same rung must not re-page");
        assert_eq!(seen[8], SslAction::Nothing, "expired must page once, not per tick");
        assert_eq!(seen[9], SslAction::Nothing);
    }

    #[test]
    fn ssl_expired_cert_pages_once_and_never_claims_renewal() {
        // The 24 hours after a lapse: `expired` is true but days_left truncates
        // to 0, so a resolve test of `days_left > last_warned_day` would see
        // `0 > -1` and announce a renewal — then re-page EXPIRED, once per tick,
        // for a full day. Feed the post-expiry ticks back in and assert silence.
        let rungs = parse_ssl_warning_days("30,14,7,3,1");
        let mut life = vec![(1, false)];
        life.extend(std::iter::repeat((0, true)).take(6)); // <24h since lapse
        life.extend(std::iter::repeat((-1, true)).take(3)); // >24h since lapse
        let seen = walk(&life, &rungs);

        assert!(matches!(seen[0], SslAction::Fire { warn_day: 1, .. }));
        assert!(matches!(seen[1], SslAction::Fire { warn_day: -1, .. }), "expired must page");
        for (i, action) in seen.iter().enumerate().skip(2) {
            assert_eq!(*action, SslAction::Nothing, "tick {i} must stay silent while expired");
        }
    }

    #[test]
    fn ssl_expiry_uses_the_expired_flag_not_truncated_days() {
        // The 24-hour window after expiry: num_days() truncates to 0, so gating
        // the EXPIRED page on `days_left < 0` left a lapsed certificate silent
        // for a full day — rung 1 was already spent, and -1 had not been reached.
        let rungs = parse_ssl_warning_days("30,14,7,3,1");
        assert_eq!(
            ssl_decision(0, true, 1, &rungs),
            SslAction::Fire { warn_day: SSL_EXPIRED_WARN_DAY, severity: "critical" },
            "a cert that lapsed under 24h ago still reports days_left = 0 and must page"
        );
        // Still valid with under a day left, rung 1 already paged: stay quiet.
        assert_eq!(ssl_decision(0, false, 1, &rungs), SslAction::Nothing);
    }

    #[test]
    fn ssl_short_lived_cert_does_not_ratchet_itself_silent() {
        // A ~6-day ACME profile never exceeds the widest rung (30), so gating the
        // resolve on "clear of the widest rung" meant last_warned_day only ever
        // fell — after two renewal cycles the site was permanently silent, even
        // for expiry. Renewal must reset the ladder.
        let rungs = parse_ssl_warning_days("30,14,7,3,1");
        let two_cycles = [
            (6, false),  // fire rung 7
            (3, false),  // fire rung 3
            (2, false),  // quiet
            (6, false),  // RENEWED -> must resolve and reset
            (6, false),  // fire rung 7 again
            (3, false),  // fire rung 3 again
        ];
        let seen = walk(&two_cycles, &rungs);
        assert!(matches!(seen[0], SslAction::Fire { warn_day: 7, .. }));
        assert!(matches!(seen[1], SslAction::Fire { warn_day: 3, .. }));
        assert_eq!(seen[2], SslAction::Nothing);
        assert_eq!(seen[3], SslAction::Resolve, "renewal inside the window is still a renewal");
        assert!(matches!(seen[4], SslAction::Fire { warn_day: 7, .. }), "ladder must restart");
        assert!(matches!(seen[5], SslAction::Fire { warn_day: 3, .. }));
    }

    #[test]
    fn ssl_recovers_from_an_expired_state_on_renewal() {
        let rungs = parse_ssl_warning_days("30,14,7,3,1");
        let seen = walk(&[(-1, true), (-2, true), (89, false), (89, false)], &rungs);
        assert!(matches!(seen[0], SslAction::Fire { warn_day: -1, .. }));
        assert_eq!(seen[1], SslAction::Nothing);
        assert_eq!(seen[2], SslAction::Resolve, "renewal after expiry must resolve");
        assert_eq!(seen[3], SslAction::Nothing, "and must not resolve twice");
    }

    #[test]
    fn ssl_severity_escalates_with_urgency() {
        let rungs = parse_ssl_warning_days("30,14,7,3,1");
        let sev = |d: i64, last: i64| match ssl_decision(d, false, last, &rungs) {
            SslAction::Fire { severity, .. } => severity,
            other => panic!("expected a fire, got {other:?}"),
        };
        assert_eq!(sev(25, SSL_NEVER_WARNED), "info");
        assert_eq!(sev(6, 14), "warning");
        assert_eq!(sev(2, 7), "critical");
    }

    #[test]
    fn expired_sentinel_is_tighter_than_every_configurable_rung() {
        // Guarantees the expired page dedupes: once last_warned_day is -1, no
        // rung (all >= 0) can satisfy `warn_day < last_warned_day` again.
        // ⚠ The `|| d == 0 && SSL_EXPIRED_WARN_DAY < d` this used to carry was the
        // SAME comparison twice: `A || (d == 0 && A)` is just `A`, so the second
        // clause could never change the verdict and the `d == 0` rung it looked
        // like it was covering was in fact covered by the first. Clippy calls it a
        // logic bug and it is right — an assertion whose extra half cannot fail
        // reads as more coverage than it has.
        for d in parse_ssl_warning_days("30,14,7,3,1,0") {
            assert!(SSL_EXPIRED_WARN_DAY < d, "rung {d} must stay above the expired sentinel");
        }
        assert!(SSL_EXPIRED_WARN_DAY < SSL_NEVER_WARNED);
    }

    /// `fire_ssl_alert` used to build this text inline; extracting
    /// `ssl_alert_copy` behind it (so the stack ladder can share it) must not
    /// change a single character of what a site's own SSL page already reads.
    #[test]
    fn ssl_alert_copy_unchanged_by_the_shared_helper_refactor() {
        let (title, message) = ssl_alert_copy("example.com", 3, false);
        assert_eq!(title, "SSL certificate expires in 3 days for example.com");
        assert_eq!(
            message,
            "The SSL certificate for example.com will expire in 3 days. Please renew it before it expires."
        );

        let (title, message) = ssl_alert_copy("example.com", 1, false);
        assert_eq!(title, "SSL certificate expires in 1 day for example.com");
        assert!(message.contains("will expire in 1 day."));

        let (title, message) = ssl_alert_copy("example.com", 0, true);
        assert_eq!(title, "SSL certificate EXPIRED for example.com");
        assert!(message.contains("has expired"));
    }

    #[test]
    fn disk_forecast_needs_a_long_enough_window_and_a_real_runway() {
        // 8h of trend, 70% -> 78%: 1%/h with 22% left = ~22h to full.
        let long_window = vec![(78.0f32, ts(0)), (70.0f32, ts(480))];
        let padded: Vec<_> = std::iter::repeat(long_window[0])
            .take(9)
            .chain(std::iter::once(long_window[1]))
            .collect();
        let (hours, rate, pct) = disk_full_forecast(&padded).expect("should forecast");
        assert!(hours > 20.0 && hours < 24.0, "hours_to_full = {hours}");
        assert!(rate > 0.9 && rate < 1.1);
        assert_eq!(pct, 78.0);

        // Same slope inside a 30-minute window returns None — correct for the
        // function, and until v2.66.0 this was ALSO the only window the caller
        // could ever supply (`LIMIT 60` at a 30s cadence), so the feature was
        // dead in production while this test stayed green. The assertions here
        // were never wrong; they are about the pure function, and nothing
        // measured the caller. That gap is now pinned at the query itself in
        // `docs-claims-pin-e2e.sh` — do not treat this arm as covering it.
        let short: Vec<_> = std::iter::repeat((78.0f32, ts(0)))
            .take(9)
            .chain(std::iter::once((70.0f32, ts(30))))
            .collect();
        assert!(disk_full_forecast(&short).is_none());

        // Long window, but nowhere near full — nothing to forecast.
        let low: Vec<_> = std::iter::repeat((12.0f32, ts(0)))
            .take(9)
            .chain(std::iter::once((4.0f32, ts(480))))
            .collect();
        assert!(disk_full_forecast(&low).is_none());
    }

    #[test]
    fn disk_forecast_ignores_flat_and_falling_disks() {
        let flat: Vec<_> = std::iter::repeat((70.0f32, ts(0)))
            .take(9)
            .chain(std::iter::once((70.0f32, ts(480))))
            .collect();
        assert!(disk_full_forecast(&flat).is_none());

        let falling: Vec<_> = std::iter::repeat((70.0f32, ts(0)))
            .take(9)
            .chain(std::iter::once((90.0f32, ts(480))))
            .collect();
        assert!(disk_full_forecast(&falling).is_none());
    }

    #[test]
    fn disk_forecast_needs_ten_samples() {
        let two = vec![(78.0f32, ts(0)), (70.0f32, ts(480))];
        assert!(disk_full_forecast(&two).is_none());
    }

    #[test]
    fn memory_leak_needs_a_sustained_climb_above_sixty_percent() {
        // newest-first: samples 0..10 are "recent", 20..30 are "older".
        let mut climbing = vec![(85.0f32,); 20];
        climbing.extend(vec![(60.0f32,); 10]);
        let (increase, older, recent) = memory_leak_trend(&climbing).expect("should detect");
        assert_eq!(recent, 85.0);
        assert_eq!(older, 60.0);
        assert_eq!(increase, 25.0);

        // Same 25-point climb, but low absolute usage — not a leak worth paging.
        let mut low = vec![(35.0f32,); 20];
        low.extend(vec![(10.0f32,); 10]);
        assert!(memory_leak_trend(&low).is_none());

        // High but steady.
        let steady = vec![(85.0f32,); 30];
        assert!(memory_leak_trend(&steady).is_none());

        // Not enough history yet.
        let short = vec![(85.0f32,); 29];
        assert!(memory_leak_trend(&short).is_none());
    }
}
