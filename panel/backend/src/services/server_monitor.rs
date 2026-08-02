use sqlx::PgPool;
use std::time::Duration;

/// Background task that marks servers as offline if they haven't checked in recently.
pub async fn run(pool: PgPool, mut shutdown_rx: tokio::sync::broadcast::Receiver<()>) {
    tracing::info!("Server health monitor started");

    let mut interval = tokio::time::interval(Duration::from_secs(120));

    loop {
        tokio::select! {
            _ = interval.tick() => {
                // Mark servers as offline if last_seen_at > 2 minutes ago.
                //
                // `is_local` is exempt, and the exemption is deliberate rather
                // than incidental. `last_seen_at` is written only by the
                // check-in handler, which only a phoning-home MEMBER reaches, so
                // the local row's value is NULL — and `NULL < …` yields NULL, not
                // true, so this sweep has never matched it. That accident is now
                // a stated rule, because `status = 'online'` is the predicate of
                // `AgentRegistry::online_fleet` AND of the alert engine's
                // resource query: were the local row ever given a `last_seen_at`,
                // a two-minute stall in the panel's own agent would drop the
                // panel host out of the security scanner, the image scanner, the
                // alert engine and the auto-healer at the same moment. The
                // panel's liveness is not a check-in, and must not be inferred
                // from one. (Detecting a dead agent BESIDE a live API is a real
                // gap and is registered as deferred scope; it needs its own
                // signal, not this one.)
                match sqlx::query(
                    "UPDATE servers SET status = 'offline' \
                     WHERE status = 'online' AND is_local = false \
                       AND last_seen_at < NOW() - INTERVAL '2 minutes'",
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
