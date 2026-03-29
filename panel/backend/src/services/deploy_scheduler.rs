use sqlx::PgPool;
use std::time::Duration;

use crate::services::agent::AgentClient;

/// Background service that checks for scheduled git deploys and triggers them.
pub async fn run(pool: PgPool, agent: AgentClient, mut shutdown_rx: tokio::sync::broadcast::Receiver<()>) {
    tracing::info!("Deploy scheduler started");

    // Initial delay (15s to let other services start first)
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(15)) => {}
        _ = shutdown_rx.recv() => {
            tracing::info!("Deploy scheduler shutting down gracefully (during initial delay)");
            return;
        }
    }

    let mut interval = tokio::time::interval(Duration::from_secs(60));

    loop {
        tokio::select! {
            _ = interval.tick() => {}
            _ = shutdown_rx.recv() => {
                tracing::info!("Deploy scheduler shutting down gracefully");
                return;
            }
        }
        check_schedules(&pool, &agent).await;
    }
}

async fn check_schedules(pool: &PgPool, agent: &AgentClient) {
    // Get all git deploys with a cron schedule
    let deploys: Vec<(uuid::Uuid, String, String, uuid::Uuid)> = match sqlx::query_as(
        "SELECT id, name, deploy_cron, user_id FROM git_deploys WHERE deploy_cron IS NOT NULL AND deploy_cron != '' AND status != 'deploying'"
    ).fetch_all(pool).await {
        Ok(d) => d,
        Err(_) => return,
    };

    let now = chrono::Utc::now();

    for (id, name, cron_expr, user_id) in &deploys {
        if should_run(cron_expr, now) {
            // Check if we already deployed in the last 59 seconds (prevent double-fire)
            let recent: Option<(i64,)> = sqlx::query_as(
                "SELECT COUNT(*) FROM git_deploy_history WHERE git_deploy_id = $1 AND created_at > NOW() - INTERVAL '59 seconds'"
            ).bind(id).fetch_optional(pool).await.ok().flatten();

            if recent.map(|r| r.0).unwrap_or(0) > 0 {
                continue;
            }

            tracing::info!("Scheduled deploy triggered: {name} (cron: {cron_expr})");

            // Trigger deploy via the extracted task function
            crate::routes::git_deploys::trigger_deploy_task(
                pool.clone(), agent.clone(), *id, *user_id, "scheduled".to_string(),
            ).await;
        }
    }

    // GAP 58: Check for one-time scheduled deploys
    let one_time: Vec<(uuid::Uuid, String, uuid::Uuid)> = match sqlx::query_as(
        "SELECT id, name, user_id FROM git_deploys \
         WHERE scheduled_deploy_at IS NOT NULL \
         AND scheduled_deploy_at <= NOW() \
         AND status NOT IN ('building', 'deploying')"
    ).fetch_all(pool).await {
        Ok(d) => d,
        Err(_) => return,
    };

    for (id, name, user_id) in &one_time {
        tracing::info!("One-time scheduled deploy triggered: {name}");

        // Clear the schedule first (prevent re-trigger)
        sqlx::query("UPDATE git_deploys SET scheduled_deploy_at = NULL, updated_at = NOW() WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await
            .map_err(|e| tracing::warn!("Failed to clear scheduled_deploy_at for deploy {id}: {e}"))
            .ok();

        // Trigger the deploy
        crate::routes::git_deploys::trigger_deploy_task(
            pool.clone(), agent.clone(), *id, *user_id, "scheduled-once".to_string(),
        ).await;
    }
}

/// Simple cron check: supports "M H * * *" format (minute hour day month weekday)
/// Returns true if the current time matches the cron expression.
fn should_run(cron_expr: &str, now: chrono::DateTime<chrono::Utc>) -> bool {
    let parts: Vec<&str> = cron_expr.split_whitespace().collect();
    if parts.len() < 5 {
        return false;
    }

    let minute = now.format("%M").to_string().parse::<u32>().unwrap_or(99);
    let hour = now.format("%H").to_string().parse::<u32>().unwrap_or(99);
    let dom = now.format("%d").to_string().parse::<u32>().unwrap_or(99);
    let month = now.format("%m").to_string().parse::<u32>().unwrap_or(99);
    let dow = now.format("%u").to_string().parse::<u32>().unwrap_or(99); // 1=Mon, 7=Sun

    matches_field(parts[0], minute)
        && matches_field(parts[1], hour)
        && matches_field(parts[2], dom)
        && matches_field(parts[3], month)
        && matches_field(parts[4], dow)
}

fn matches_field(field: &str, value: u32) -> bool {
    if field == "*" {
        return true;
    }
    // Handle */N (every N)
    if let Some(step) = field.strip_prefix("*/") {
        if let Ok(n) = step.parse::<u32>() {
            return n > 0 && value % n == 0;
        }
    }
    // Handle comma-separated values
    for part in field.split(',') {
        // Handle range (e.g., 1-5)
        if let Some((start_s, end_s)) = part.split_once('-') {
            if let (Ok(start), Ok(end)) = (start_s.parse::<u32>(), end_s.parse::<u32>()) {
                if value >= start && value <= end {
                    return true;
                }
            }
        } else if let Ok(v) = part.parse::<u32>() {
            if v == value {
                return true;
            }
        }
    }
    false
}
