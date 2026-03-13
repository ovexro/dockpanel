use sqlx::PgPool;
use std::time::Duration;

use crate::services::activity;
use crate::services::agent::AgentClient;

/// Background task: auto-heals common issues when detected.
/// Runs every 120 seconds (offset from alert engine to spread load).
pub async fn run(pool: PgPool, agent: AgentClient) {
    tracing::info!("Auto-healer started");

    // Initial delay (90s offset from alert engine's 30s)
    tokio::time::sleep(Duration::from_secs(90)).await;

    // Track when we last ran retention cleanup (once per day)
    let mut last_retention = std::time::Instant::now() - Duration::from_secs(86400);

    loop {
        tokio::time::sleep(Duration::from_secs(120)).await;

        // Data retention cleanup runs daily regardless of auto-heal setting
        if last_retention.elapsed() >= Duration::from_secs(86400) {
            run_retention_cleanup(&pool).await;
            last_retention = std::time::Instant::now();
        }

        // Only run auto-healing if enabled globally
        let enabled = is_enabled(&pool).await;
        if !enabled {
            continue;
        }

        auto_restart_services(&pool, &agent).await;
        auto_clean_disk(&pool, &agent).await;
        auto_renew_ssl(&pool, &agent).await;
    }
}

/// Check if auto-healing is enabled in settings.
async fn is_enabled(pool: &PgPool) -> bool {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT value FROM settings WHERE key = 'auto_heal_enabled'",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    row.map(|r| r.0 == "true").unwrap_or(false)
}

/// Auto-restart crashed services (service_down alerts that are firing).
async fn auto_restart_services(pool: &PgPool, agent: &AgentClient) {
    // Find service_down alerts that are currently firing
    let firing: Vec<(String,)> = match sqlx::query_as(
        "SELECT state_key FROM alert_state \
         WHERE alert_type = 'service_down' AND current_state = 'firing' AND state_key != ''",
    )
    .fetch_all(pool)
    .await
    {
        Ok(s) => s,
        Err(_) => return,
    };

    for (service_name,) in &firing {
        if service_name.is_empty() {
            continue;
        }

        // Check if we already tried to heal this service recently (avoid restart loops)
        let recent_heal: Option<(i64,)> = sqlx::query_as(
            "SELECT COUNT(*) FROM activity_logs \
             WHERE action = 'auto_heal.restart_service' \
             AND target_name = $1 \
             AND created_at > NOW() - INTERVAL '10 minutes'",
        )
        .bind(service_name)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();

        if recent_heal.map(|r| r.0).unwrap_or(0) > 0 {
            tracing::debug!("Auto-heal: skipping {service_name} (recently attempted)");
            continue;
        }

        tracing::info!("Auto-heal: restarting service {service_name}");

        let result = agent
            .post(
                "/diagnostics/fix",
                Some(serde_json::json!({ "fix_id": format!("restart-service:{service_name}") })),
            )
            .await;

        let success = result.is_ok();
        let details = match &result {
            Ok(v) => v.to_string(),
            Err(e) => e.to_string(),
        };

        // Log the auto-healing action
        // Use a system UUID for auto-healer activity
        let system_id = uuid::Uuid::nil();
        activity::log_activity(
            pool,
            system_id,
            "auto-healer",
            "auto_heal.restart_service",
            Some("service"),
            Some(service_name),
            Some(&format!("success={success}, result={details}")),
            None,
        )
        .await;
    }
}

/// Auto-clean logs when disk usage > 90%.
async fn auto_clean_disk(pool: &PgPool, agent: &AgentClient) {
    // Check if disk alert is firing
    let firing: Option<(String,)> = sqlx::query_as(
        "SELECT current_state FROM alert_state \
         WHERE alert_type = 'disk' AND current_state = 'firing' LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    if firing.is_none() {
        return;
    }

    // Check if we already cleaned recently
    let recent: Option<(i64,)> = sqlx::query_as(
        "SELECT COUNT(*) FROM activity_logs \
         WHERE action = 'auto_heal.clean_logs' \
         AND created_at > NOW() - INTERVAL '1 hour'",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    if recent.map(|r| r.0).unwrap_or(0) > 0 {
        return;
    }

    tracing::info!("Auto-heal: cleaning logs to free disk space");

    let result = agent
        .post(
            "/diagnostics/fix",
            Some(serde_json::json!({ "fix_id": "clean-logs:all" })),
        )
        .await;

    let success = result.is_ok();
    let system_id = uuid::Uuid::nil();
    activity::log_activity(
        pool,
        system_id,
        "auto-healer",
        "auto_heal.clean_logs",
        Some("system"),
        Some("logs"),
        Some(&format!("success={success}")),
        None,
    )
    .await;
}

/// Auto-renew SSL certs expiring within 3 days.
async fn auto_renew_ssl(pool: &PgPool, agent: &AgentClient) {
    let sites: Vec<(uuid::Uuid, String)> = match sqlx::query_as(
        "SELECT id, domain FROM sites \
         WHERE ssl_enabled = TRUE AND ssl_expiry IS NOT NULL \
         AND ssl_expiry < NOW() + INTERVAL '3 days'",
    )
    .fetch_all(pool)
    .await
    {
        Ok(s) => s,
        Err(_) => return,
    };

    for (site_id, domain) in &sites {
        // Check if we already tried recently
        let recent: Option<(i64,)> = sqlx::query_as(
            "SELECT COUNT(*) FROM activity_logs \
             WHERE action = 'auto_heal.renew_ssl' \
             AND target_name = $1 \
             AND created_at > NOW() - INTERVAL '6 hours'",
        )
        .bind(domain)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();

        if recent.map(|r| r.0).unwrap_or(0) > 0 {
            continue;
        }

        tracing::info!("Auto-heal: renewing SSL for {domain}");

        let result = agent
            .post(
                &format!("/ssl/renew"),
                Some(serde_json::json!({ "domain": domain })),
            )
            .await;

        let success = result.is_ok();
        let system_id = uuid::Uuid::nil();
        activity::log_activity(
            pool,
            system_id,
            "auto-healer",
            "auto_heal.renew_ssl",
            Some("site"),
            Some(domain),
            Some(&format!("site_id={site_id}, success={success}")),
            None,
        )
        .await;
    }
}

/// Periodic data retention cleanup: removes old records to keep the database lean.
async fn run_retention_cleanup(pool: &PgPool) {
    tracing::info!("Running data retention cleanup...");

    // Delete monitor_checks older than 7 days
    match sqlx::query("DELETE FROM monitor_checks WHERE checked_at < NOW() - INTERVAL '7 days'")
        .execute(pool)
        .await
    {
        Ok(r) => {
            if r.rows_affected() > 0 {
                tracing::info!(
                    "Retention: deleted {} old monitor_checks",
                    r.rows_affected()
                );
            }
        }
        Err(e) => tracing::warn!("Retention cleanup (monitor_checks) failed: {e}"),
    }

    // Delete resolved alerts older than 90 days
    match sqlx::query(
        "DELETE FROM alerts WHERE status = 'resolved' AND created_at < NOW() - INTERVAL '90 days'",
    )
    .execute(pool)
    .await
    {
        Ok(r) => {
            if r.rows_affected() > 0 {
                tracing::info!("Retention: deleted {} old resolved alerts", r.rows_affected());
            }
        }
        Err(e) => tracing::warn!("Retention cleanup (alerts) failed: {e}"),
    }

    // Delete activity_logs older than 1 year
    match sqlx::query("DELETE FROM activity_logs WHERE created_at < NOW() - INTERVAL '1 year'")
        .execute(pool)
        .await
    {
        Ok(r) => {
            if r.rows_affected() > 0 {
                tracing::info!(
                    "Retention: deleted {} old activity_logs",
                    r.rows_affected()
                );
            }
        }
        Err(e) => tracing::warn!("Retention cleanup (activity_logs) failed: {e}"),
    }
}
