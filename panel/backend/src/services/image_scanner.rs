// Background sweeper for per-image vulnerability scans.
// Iterates the deduped set of images currently used by DockPanel-managed
// containers and rescans any whose newest finding is older than the
// configured interval. Distinct from services::security_scanner which
// runs the full-server scan.

use sqlx::PgPool;
use std::collections::HashSet;
use std::time::Duration;

use crate::routes::image_scans;
use crate::services::agent::{AgentRegistry, FleetMember};

const CHECK_INTERVAL: Duration = Duration::from_secs(30 * 60); // 30 minutes
const STARTUP_DELAY: Duration = Duration::from_secs(10 * 60);  // 10 minutes

/// Sweep every online server's images, not just the panel's.
///
/// Like the security scanner this asks a MACHINE what it is running, so there
/// is no row whose `server_id` could be threaded — it needs the fleet loop. It
/// hard-coded the local handle, so a member's containers were never swept
/// and its images never scanned, while the Apps page badged them with whatever
/// the panel happened to have found for an image of the same name.
pub async fn run(
    pool: PgPool,
    agents: AgentRegistry,
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

    loop {
        for member in agents.online_fleet().await {
            if let Err(e) = sweep_once(&pool, &member).await {
                tracing::warn!("Image scanner sweep failed on {}: {e}", member.name);
            }
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

async fn sweep_once(pool: &PgPool, member: &FleetMember) -> Result<(), String> {
    let agent = &member.agent;
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
        // Skip if a fresh enough scan exists FOR THIS SERVER. Unscoped, one
        // host's fresh scan of `postgres:16` suppressed the rescan on every
        // other host running it — indefinitely, since the suppressor keeps
        // refreshing itself.
        let last: Option<(chrono::DateTime<chrono::Utc>,)> = sqlx::query_as(
            "SELECT scanned_at FROM image_scan_findings WHERE server_id = $1 AND image = $2 \
             ORDER BY scanned_at DESC LIMIT 1",
        )
        .bind(member.id)
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

        tracing::info!("Image scanner: scanning {image} on {}", member.name);
        if let Err(e) = image_scans::scan_and_store(pool, member.id, agent, &image).await {
            tracing::warn!("Image scan failed for {image} on {}: {e:?}", member.name);
        }

        // Yield between scans so the agent isn't slammed.
        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    Ok(())
}
