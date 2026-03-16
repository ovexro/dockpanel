use sqlx::PgPool;
use std::time::Duration;

/// Background task that marks servers as offline if they haven't checked in recently.
pub async fn run(pool: PgPool, mut shutdown_rx: tokio::sync::broadcast::Receiver<()>) {
    tracing::info!("Server health monitor started");

    let mut interval = tokio::time::interval(Duration::from_secs(120));

    loop {
        tokio::select! {
            _ = interval.tick() => {
                // Mark servers as offline if last_seen_at > 2 minutes ago
                match sqlx::query(
                    "UPDATE servers SET status = 'offline' \
                     WHERE status = 'online' AND last_seen_at < NOW() - INTERVAL '2 minutes'",
                )
                .execute(&pool)
                .await
                {
                    Ok(result) => {
                        if result.rows_affected() > 0 {
                            tracing::info!(
                                "Marked {} server(s) as offline",
                                result.rows_affected()
                            );
                        }
                    }
                    Err(e) => {
                        tracing::error!("Server monitor error: {e}");
                    }
                }
            }
            _ = shutdown_rx.recv() => {
                tracing::info!("Server monitor shutting down gracefully");
                break;
            }
        }
    }
}
