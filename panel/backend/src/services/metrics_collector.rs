use sqlx::PgPool;
use std::time::Duration;
use crate::services::agent::AgentClient;

/// Background task that collects system metrics every 60 seconds for historical charts.
pub async fn run(pool: PgPool, agent: AgentClient) {
    tracing::info!("Metrics collector started (60s interval)");

    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;

        // Fetch current system info from agent
        match agent.get("/system/info").await {
            Ok(info) => {
                let cpu = info.get("cpu_usage").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                let mem = info.get("mem_usage_pct").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                let disk = info.get("disk_usage_pct").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;

                if let Err(e) = sqlx::query(
                    "INSERT INTO metrics_history (cpu_pct, mem_pct, disk_pct) VALUES ($1, $2, $3)",
                )
                .bind(cpu)
                .bind(mem)
                .bind(disk)
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
