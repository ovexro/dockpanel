use sqlx::PgPool;
use std::time::Duration;
use crate::services::agent::AgentClient;

/// Background task that collects system metrics every 30 seconds for historical charts.
pub async fn run(pool: PgPool, agent: AgentClient, mut shutdown_rx: tokio::sync::broadcast::Receiver<()>) {
    tracing::info!("Metrics collector started (30s interval)");

    let mut interval = tokio::time::interval(Duration::from_secs(30));

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

        // Fetch current system info from agent
        match agent.get("/system/info").await {
            Ok(info) => {
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
                tracing::warn!("Metrics collector: agent unreachable: {e}");
            }
        }

        // Cleanup: delete records older than 7 days
        let _ = sqlx::query("DELETE FROM metrics_history WHERE created_at < NOW() - INTERVAL '7 days'")
            .execute(&pool)
            .await;
    }
}
