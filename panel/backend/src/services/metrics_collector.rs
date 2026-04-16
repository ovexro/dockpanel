use sqlx::PgPool;
use std::time::Duration;
use crate::services::agent::AgentClient;

/// Background task that collects system metrics every 30 seconds for historical charts.
pub async fn run(pool: PgPool, agent: AgentClient, mut shutdown_rx: tokio::sync::broadcast::Receiver<()>) {
    tracing::info!("Metrics collector started (30s interval)");

    let mut interval = tokio::time::interval(Duration::from_secs(30));
    let mut consecutive_failures: u32 = 0;

    loop {
        tokio::select! {
            _ = interval.tick() => {}
            _ = shutdown_rx.recv() => {
                tracing::info!("Metrics collector shutting down gracefully");
                break;
            }
        }

        // Get the local server's ID for multi-server charting
        let local_server_id: Option<uuid::Uuid> = sqlx::query_scalar(
            "SELECT id FROM servers ORDER BY created_at ASC LIMIT 1",
        )
        .fetch_optional(&pool)
        .await
        .ok()
        .flatten();

        // Fetch system info and GPU info concurrently
        let (sys_res, gpu_res) = tokio::join!(
            agent.get("/system/info"),
            agent.get("/apps/gpu-info"),
        );

        match sys_res {
            Ok(info) => {
                consecutive_failures = 0;
                let cpu = info.get("cpu_usage").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                let mem = info.get("mem_usage_pct").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                let disk = info.get("disk_usage_pct").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;

                if let Err(e) = sqlx::query(
                    "INSERT INTO metrics_history (cpu_pct, mem_pct, disk_pct, server_id) VALUES ($1, $2, $3, $4)",
                )
                .bind(cpu)
                .bind(mem)
                .bind(disk)
                .bind(local_server_id)
                .execute(&pool)
                .await
                {
                    tracing::error!("Failed to store metrics: {e}");
                }
            }
            Err(e) => {
                consecutive_failures += 1;
                tracing::warn!("Metrics collector: agent unreachable: {e}");
                if consecutive_failures == 3 {
                    crate::services::system_log::log_event(
                        &pool,
                        "warning",
                        "metrics_collector",
                        "Agent unreachable for 3+ consecutive checks",
                        Some(&e.to_string()),
                    ).await;
                }
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
                        .bind(local_server_id)
                        .execute(&pool)
                        .await
                        {
                            tracing::error!("Failed to store GPU metrics: {e}");
                        }
                    }
                }
            }
        }

        // Cleanup: delete records older than 7 days
        let _ = sqlx::query("DELETE FROM metrics_history WHERE created_at < NOW() - INTERVAL '7 days'")
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM gpu_metrics_history WHERE created_at < NOW() - INTERVAL '7 days'")
            .execute(&pool)
            .await;
    }
}
