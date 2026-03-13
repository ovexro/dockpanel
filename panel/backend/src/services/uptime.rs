use sqlx::PgPool;
use std::time::{Duration, Instant};

#[derive(sqlx::FromRow)]
struct MonitorRow {
    id: uuid::Uuid,
    user_id: uuid::Uuid,
    url: String,
    name: String,
    status: String,
    alert_email: bool,
    alert_slack_url: Option<String>,
    alert_discord_url: Option<String>,
}

/// Background task: checks all enabled monitors periodically.
pub async fn run(pool: PgPool) {
    tracing::info!("Uptime monitor started");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(5))
        .danger_accept_invalid_certs(false)
        .build()
        .unwrap();

    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;

        // Get monitors due for checking
        let monitors: Vec<MonitorRow> = match sqlx::query_as(
            "SELECT id, user_id, url, name, status, alert_email, alert_slack_url, alert_discord_url \
             FROM monitors WHERE enabled = TRUE AND \
             (last_checked_at IS NULL OR last_checked_at < NOW() - (check_interval || ' seconds')::interval)",
        )
        .fetch_all(&pool)
        .await
        {
            Ok(m) => m,
            Err(e) => {
                tracing::error!("Uptime monitor query error: {e}");
                continue;
            }
        };

        for monitor in &monitors {
            let start = Instant::now();
            let result = client.get(&monitor.url).send().await;
            let response_time = start.elapsed().as_millis() as i32;

            let (status_code, error, new_status) = match result {
                Ok(resp) => {
                    let code = resp.status().as_u16() as i32;
                    if resp.status().is_success() {
                        (Some(code), None, "up")
                    } else {
                        (Some(code), Some(format!("HTTP {code}")), "down")
                    }
                }
                Err(e) => (None, Some(e.to_string()), "down"),
            };

            // Insert check record
            if let Err(e) = sqlx::query(
                "INSERT INTO monitor_checks (monitor_id, status_code, response_time, error) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(monitor.id)
            .bind(status_code)
            .bind(response_time)
            .bind(&error)
            .execute(&pool)
            .await {
                tracing::error!("Failed to insert monitor check for {}: {e}", monitor.name);
            }

            // Update monitor status
            if let Err(e) = sqlx::query(
                "UPDATE monitors SET status = $1, last_checked_at = NOW(), \
                 last_response_time = $2, last_status_code = $3 WHERE id = $4",
            )
            .bind(new_status)
            .bind(response_time)
            .bind(status_code)
            .bind(monitor.id)
            .execute(&pool)
            .await {
                tracing::error!("Failed to update monitor status for {}: {e}", monitor.name);
            }

            // Handle status transitions
            if new_status == "down" && monitor.status != "down" {
                // Just went down — create incident and send alerts
                let cause = error.as_deref().unwrap_or("Unknown error");
                if let Err(e) = sqlx::query(
                    "INSERT INTO incidents (monitor_id, cause, alerted) VALUES ($1, $2, TRUE)",
                )
                .bind(monitor.id)
                .bind(cause)
                .execute(&pool)
                .await {
                    tracing::error!("Failed to create incident for {}: {e}", monitor.name);
                }

                tracing::warn!("Monitor {} ({}) is DOWN: {}", monitor.name, monitor.url, cause);
                send_alerts(&pool, monitor, &format!("{} is down: {cause}", monitor.name)).await;
            } else if new_status == "up" && monitor.status == "down" {
                // Just recovered — resolve incident
                if let Err(e) = sqlx::query(
                    "UPDATE incidents SET resolved_at = NOW() \
                     WHERE monitor_id = $1 AND resolved_at IS NULL",
                )
                .bind(monitor.id)
                .execute(&pool)
                .await {
                    tracing::error!("Failed to resolve incident for {}: {e}", monitor.name);
                }

                tracing::info!("Monitor {} ({}) is back UP", monitor.name, monitor.url);
                send_alerts(&pool, monitor, &format!("{} is back up ({}ms)", monitor.name, response_time)).await;
            }
        }

        // Purge old check records (keep last 24h)
        if let Err(e) = sqlx::query(
            "DELETE FROM monitor_checks WHERE checked_at < NOW() - INTERVAL '24 hours'",
        )
        .execute(&pool)
        .await {
            tracing::error!("Failed to purge old monitor checks: {e}");
        }

        // Purge old performance metrics (keep last 7 days)
        if let Err(e) = sqlx::query(
            "DELETE FROM metrics WHERE recorded_at < NOW() - INTERVAL '7 days'",
        )
        .execute(&pool)
        .await {
            tracing::error!("Failed to purge old metrics: {e}");
        }
    }
}

async fn send_alerts(pool: &PgPool, monitor: &MonitorRow, message: &str) {
    // Build notification channels from monitor's per-monitor settings
    let email = if monitor.alert_email {
        sqlx::query_scalar::<_, String>("SELECT email FROM users WHERE id = $1")
            .bind(monitor.user_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
    } else {
        None
    };

    let channels = crate::services::notifications::NotifyChannels {
        email,
        slack_url: monitor.alert_slack_url.clone(),
        discord_url: monitor.alert_discord_url.clone(),
    };

    let subject = format!("DockPanel Alert: {}", monitor.name);
    let html = format!(
        "<h2>Monitor Alert</h2>\
         <p><strong>{}</strong></p>\
         <p>URL: {}</p>\
         <p>Time: {}</p>",
        message,
        monitor.url,
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC"),
    );

    crate::services::notifications::send_notification(pool, &channels, &subject, message, &html)
        .await;
}
