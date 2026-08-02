use sqlx::PgPool;
use std::time::Duration;
use crate::services::agent::{AgentRegistry, FleetMember};

/// Background task that collects system metrics every 30 seconds for historical charts.
///
/// # Why this iterates the fleet
///
/// DockPanel kept its per-host telemetry in two stores that were written by
/// mutually exclusive halves of the fleet, and every consumer was therefore
/// blind to one half:
///
/// * `servers.{cpu_usage, mem_used_mb, ram_mb, disk_usage_pct, …}` is written
///   ONLY by `routes::agent_checkin`, which is reached only by an agent's
///   phone-home loop — and phone-home starts only when `agent.env` supplies
///   `DOCKPANEL_CENTRAL_URL`/`_SERVER_TOKEN`/`_SERVER_ID`, a file only
///   `install-agent.sh` writes. **The panel's own box has no such file**, so its
///   row carried NULL in every one of those columns, for ever, on every install.
///   `alert_engine::check_resource_thresholds` is their only reader and guards
///   each on an `if let Some(..)`, so the CPU, memory and disk alerts silently
///   never evaluated for the host the API runs on — which on a single-server
///   install is the only host there is. `auto_clean_disk` is triggered by a
///   firing `disk` alert, so it had consequently never executed there either.
///
/// * `metrics_history` was written ONLY here, bound to the `is_local` id, so
///   every member had zero rows: no memory-leak trend
///   (`alert_engine`), 144 empty buckets in the uptime sparkline
///   (`routes::servers::uptime`), no series in the Prometheus scrape
///   (`prometheus_exporter`) though `FEATURES.md` advertises one per server,
///   and permanently blank CPU/memory/disk columns on the fleet overview.
///
/// So this collector now reads every online server through its OWN agent and is
/// the single writer of `metrics_history` for all of them — and, for the local
/// row only, also writes the scalar columns a member reports by phoning home.
/// Members keep getting theirs from the check-in handler; writing them here too
/// would race that handler for no gain.
///
/// ⚠ It deliberately does NOT write `last_seen_at`. That column is what
/// `server_monitor` sweeps to flip a server `offline` after two minutes, and
/// `status = 'online'` is the predicate of both `AgentRegistry::online_fleet`
/// and the alert engine's own resource query. Giving the local row a
/// `last_seen_at` would make a two-minute stall in the panel's own agent drop
/// the panel host out of the security scanner, the image scanner, the alert
/// engine and the auto-healer simultaneously. `server_monitor` now states that
/// exemption explicitly rather than relying on the NULL that produced it by
/// accident.
pub async fn run(pool: PgPool, agents: AgentRegistry, mut shutdown_rx: tokio::sync::broadcast::Receiver<()>) {
    tracing::info!("Metrics collector started (30s interval, fleet-wide)");

    let mut interval = tokio::time::interval(Duration::from_secs(30));
    let mut local_failures: u32 = 0;

    loop {
        tokio::select! {
            _ = interval.tick() => {}
            _ = shutdown_rx.recv() => {
                tracing::info!("Metrics collector shutting down gracefully");
                break;
            }
        }

        // Concurrently, not serially: a fleet of N hosts must not take N round
        // trips inside a 30-second tick, or the interval falls behind and the
        // default `Burst` behaviour then fires the backlog back to back.
        let fleet = agents.online_fleet().await;
        let results = futures::future::join_all(
            fleet.iter().map(|member| collect_one(&pool, member)),
        )
        .await;

        // Preserve the pre-existing "agent unreachable" signal, which was about
        // the LOCAL agent. A member being unreachable is `online_fleet`'s
        // business (it skips it) and is not this task's alarm to raise.
        let local_ok = fleet
            .iter()
            .zip(results.iter())
            .find(|(m, _)| m.is_local)
            .map(|(_, ok)| *ok);
        match local_ok {
            Some(true) => local_failures = 0,
            Some(false) => {
                local_failures += 1;
                if local_failures == 3 {
                    crate::services::system_log::log_event(
                        &pool,
                        "warning",
                        "metrics_collector",
                        "Agent unreachable for 3+ consecutive checks",
                        None,
                    ).await;
                }
            }
            None => {}
        }

        // Cleanup: delete records older than 7 days. Unscoped by server on
        // purpose — retention is a property of the table, not of a host.
        let _ = sqlx::query("DELETE FROM metrics_history WHERE created_at < NOW() - INTERVAL '7 days'")
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM gpu_metrics_history WHERE created_at < NOW() - INTERVAL '7 days'")
            .execute(&pool)
            .await;
    }
}

/// Take one server's readings from that server's own agent. Returns whether the
/// system-info read succeeded.
async fn collect_one(pool: &PgPool, member: &FleetMember) -> bool {
    let (sys_res, gpu_res) = tokio::join!(
        member.agent.get("/system/info"),
        member.agent.get("/apps/gpu-info"),
    );

    let info = match sys_res {
        Ok(info) => info,
        Err(e) => {
            tracing::warn!("Metrics collector: agent unreachable for {}: {e}", member.name);
            return false;
        }
    };

    let cpu = info.get("cpu_usage").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
    let mem = info.get("mem_usage_pct").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
    let disk = info.get("disk_usage_pct").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;

    if let Err(e) = sqlx::query(
        "INSERT INTO metrics_history (cpu_pct, mem_pct, disk_pct, server_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(cpu)
    .bind(mem)
    .bind(disk)
    .bind(member.id)
    .execute(pool)
    .await
    {
        tracing::error!("Failed to store metrics for {}: {e}", member.name);
    }

    // The local row has no check-in to populate its scalar columns, so this is
    // the only place they can come from. Same field names the check-in payload
    // uses, read from the same agent endpoint, so the two paths agree.
    // COALESCE-free: unlike a check-in body these values are always present.
    if member.is_local {
        let mem_used_mb = info.get("mem_used_mb").and_then(|v| v.as_i64());
        let ram_mb = info.get("mem_total_mb").and_then(|v| v.as_i64()).map(|v| v as i32);
        let cpu_cores = info.get("cpu_count").and_then(|v| v.as_i64()).map(|v| v as i32);
        let uptime_secs = info.get("uptime_secs").and_then(|v| v.as_i64());
        let disk_gb = info.get("disk_total_gb").and_then(|v| v.as_f64()).map(|v| v as i32);
        let disk_used_gb = info.get("disk_used_gb").and_then(|v| v.as_f64()).map(|v| v as i32);

        if let Err(e) = sqlx::query(
            "UPDATE servers SET \
             cpu_usage = $2, \
             mem_used_mb = $3, \
             ram_mb = COALESCE($4, ram_mb), \
             mem_usage_pct = $5, \
             disk_usage_pct = $6, \
             cpu_cores = COALESCE($7, cpu_cores), \
             uptime_secs = $8, \
             disk_gb = COALESCE($9, disk_gb), \
             disk_used_gb = $10 \
             WHERE id = $1",
        )
        .bind(member.id)
        .bind(cpu)
        .bind(mem_used_mb)
        .bind(ram_mb)
        .bind(mem)
        .bind(disk)
        .bind(cpu_cores)
        .bind(uptime_secs)
        .bind(disk_gb)
        .bind(disk_used_gb)
        .execute(pool)
        .await
        {
            tracing::error!("Failed to store local server metrics: {e}");
        }
    }

    // Store GPU metrics (if GPUs available)
    if let Ok(gpu_info) = gpu_res {
        if gpu_info.get("available").and_then(|v| v.as_bool()).unwrap_or(false) {
            if let Some(gpus) = gpu_info.get("gpus").and_then(|v| v.as_array()) {
                for gpu in gpus {
                    let idx = gpu.get("index").and_then(|v| v.as_i64()).unwrap_or(0) as i16;
                    let util = gpu.get("utilization_gpu_pct").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                    let mem_used = gpu.get("memory_used_mb").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                    let mem_total = gpu.get("memory_total_mb").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                    let temp = gpu.get("temperature_c").and_then(|v| v.as_f64()).map(|v| v as f32);
                    let power = gpu.get("power_draw_w").and_then(|v| v.as_f64()).map(|v| v as f32);

                    if let Err(e) = sqlx::query(
                        "INSERT INTO gpu_metrics_history \
                         (gpu_index, utilization_pct, memory_used_mb, memory_total_mb, temperature_c, power_draw_w, server_id) \
                         VALUES ($1, $2, $3, $4, $5, $6, $7)",
                    )
                    .bind(idx)
                    .bind(util)
                    .bind(mem_used)
                    .bind(mem_total)
                    .bind(temp)
                    .bind(power)
                    .bind(member.id)
                    .execute(pool)
                    .await
                    {
                        tracing::error!("Failed to store GPU metrics for {}: {e}", member.name);
                    }
                }
            }
        }
    }

    true
}
