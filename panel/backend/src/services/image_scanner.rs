// Background sweeper for per-image vulnerability scans.
// Iterates the deduped set of images currently used by DockPanel-managed
// containers and rescans any whose newest finding is older than the
// configured interval. Distinct from services::security_scanner which
// runs the full-server scan.

use sqlx::PgPool;
use std::collections::HashSet;
use std::time::Duration;

use crate::routes::image_scans;
use crate::services::agent::AgentClient;

const CHECK_INTERVAL: Duration = Duration::from_secs(30 * 60); // 30 minutes
const STARTUP_DELAY: Duration = Duration::from_secs(10 * 60);  // 10 minutes

pub async fn run(
    pool: PgPool,
    agent: AgentClient,
    mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
) {
    tracing::info!("Image scanner background task started");

    tokio::select! {
        _ = tokio::time::sleep(STARTUP_DELAY) => {}
        _ = shutdown_rx.recv() => {
            tracing::info!("Image scanner shutting down (initial delay)");
            return;
        }
    }

    let agent_handle = crate::services::agent::AgentHandle::Local(agent);

    loop {
        if let Err(e) = sweep_once(&pool, &agent_handle).await {
            tracing::warn!("Image scanner sweep failed: {e}");
        }

        tokio::select! {
            _ = tokio::time::sleep(CHECK_INTERVAL) => {}
            _ = shutdown_rx.recv() => {
                tracing::info!("Image scanner shutting down");
                return;
            }
        }
    }
}

async fn sweep_once(
    pool: &PgPool,
    agent: &crate::services::agent::AgentHandle,
) -> Result<(), String> {
    let (enabled, _on_deploy, _gate, interval_hours) = image_scans::read_settings(pool)
        .await
        .map_err(|e| format!("read settings: {e}"))?;
    if !enabled {
        return Ok(());
    }

    // Gather distinct images from running DockPanel-managed apps.
    let apps = agent
        .get("/apps")
        .await
        .map_err(|e| format!("list apps: {e}"))?;
    let arr = match apps.as_array() {
        Some(a) => a,
        None => return Ok(()),
    };

    let mut images: HashSet<String> = HashSet::new();
    for a in arr {
        if let Some(img) = a.get("image").and_then(|v| v.as_str()) {
            if !img.is_empty() {
                images.insert(img.to_string());
            }
        }
    }

    if images.is_empty() {
        return Ok(());
    }

    let interval_secs = (interval_hours.max(1) as i64) * 3600;

    for image in images {
        // Skip if a fresh enough scan exists.
        let last: Option<(chrono::DateTime<chrono::Utc>,)> = sqlx::query_as(
            "SELECT scanned_at FROM image_scan_findings WHERE image = $1 \
             ORDER BY scanned_at DESC LIMIT 1",
        )
        .bind(&image)
        .fetch_optional(pool)
        .await
        .map_err(|e| format!("read last scan: {e}"))?;

        if let Some((ts,)) = last {
            let age = (chrono::Utc::now() - ts).num_seconds();
            if age < interval_secs {
                continue;
            }
        }

        tracing::info!("Image scanner: scanning {image}");
        if let Err(e) = image_scans::scan_and_store(pool, agent, &image).await {
            tracing::warn!("Image scan failed for {image}: {e:?}");
        }

        // Yield between scans so the agent isn't slammed.
        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    Ok(())
}
