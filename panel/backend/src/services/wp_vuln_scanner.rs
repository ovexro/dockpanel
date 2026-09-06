// Background sweeper for WordPress vulnerability scans.
//
// Direct peer of `image_scanner`: iterates every site on every online fleet
// member, detects which are WordPress, and rescans any whose newest finding
// is older than the configured interval. Before this, `wordpress::vuln_scan`
// only ran when an operator opened a site's toolkit and clicked Scan — a
// critically-vulnerable plugin stayed invisible indefinitely otherwise,
// unlike Docker image scanning (its `spawn_supervised`'d sibling), which has
// carried a schedule and a `fire_alert`/`resolve_alert` lifecycle since
// v2.something. See FEATURES.md / the s469 loose-ends audit for the gap this
// closes.

use sqlx::PgPool;
use std::time::Duration;

use crate::routes::wordpress;
use crate::services::agent::{AgentRegistry, FleetMember};

const CHECK_INTERVAL: Duration = Duration::from_secs(30 * 60); // 30 minutes
const STARTUP_DELAY: Duration = Duration::from_secs(10 * 60); // 10 minutes
const WP_DETECT_TIMEOUT: Duration = Duration::from_secs(5);

pub async fn run(
    pool: PgPool,
    agents: AgentRegistry,
    mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
) {
    tracing::info!("WordPress vuln scanner background task started");

    tokio::select! {
        _ = tokio::time::sleep(STARTUP_DELAY) => {}
        _ = shutdown_rx.recv() => {
            tracing::info!("WordPress vuln scanner shutting down (initial delay)");
            return;
        }
    }

    loop {
        for member in agents.online_fleet().await {
            if let Err(e) = sweep_once(&pool, &member).await {
                tracing::warn!("WordPress vuln scanner sweep failed on {}: {e}", member.name);
            }
        }

        tokio::select! {
            _ = tokio::time::sleep(CHECK_INTERVAL) => {}
            _ = shutdown_rx.recv() => {
                tracing::info!("WordPress vuln scanner shutting down");
                return;
            }
        }
    }
}

async fn sweep_once(pool: &PgPool, member: &FleetMember) -> Result<(), String> {
    let agent = &member.agent;
    let (enabled, interval_hours) = wordpress::read_wp_scan_settings(pool)
        .await
        .map_err(|e| format!("read settings: {e}"))?;
    if !enabled {
        return Ok(());
    }
    let interval_secs = (interval_hours.max(1) as i64) * 3600;

    let sites: Vec<(uuid::Uuid, String, uuid::Uuid)> = sqlx::query_as(
        "SELECT id, domain, user_id FROM sites WHERE server_id = $1 ORDER BY domain",
    )
    .bind(member.id)
    .fetch_all(pool)
    .await
    .map_err(|e| format!("list sites: {e}"))?;

    for (site_id, domain, user_id) in sites {
        // Detect WordPress the same way `wordpress::all_wp_sites` does — a
        // quick, timeout-bounded probe, not an assumption from the site's
        // runtime field (sites are provisioned as generic PHP, WordPress is
        // installed on top).
        let is_wp = tokio::time::timeout(WP_DETECT_TIMEOUT, agent.get(&format!("/wordpress/{domain}/info")))
            .await
            .map(|r| r.is_ok())
            .unwrap_or(false);
        if !is_wp {
            continue;
        }

        let last: Option<(chrono::DateTime<chrono::Utc>,)> = sqlx::query_as(
            "SELECT scanned_at FROM wp_vuln_scans WHERE site_id = $1 ORDER BY scanned_at DESC LIMIT 1",
        )
        .bind(site_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| format!("read last scan: {e}"))?;

        if let Some((ts,)) = last {
            let age = (chrono::Utc::now() - ts).num_seconds();
            if age < interval_secs {
                continue;
            }
        }

        tracing::info!("WordPress vuln scanner: scanning {domain} on {}", member.name);
        if let Err(e) = wordpress::scan_and_store(pool, site_id, user_id, &domain, agent).await {
            tracing::warn!("WordPress vuln scan failed for {domain} on {}: {e:?}", member.name);
        }

        // Yield between scans so the agent isn't slammed — mirrors
        // `image_scanner`'s own pacing.
        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    Ok(())
}
