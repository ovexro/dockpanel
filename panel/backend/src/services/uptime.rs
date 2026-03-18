use sqlx::PgPool;
use std::time::{Duration, Instant};

#[derive(sqlx::FromRow, Clone)]
struct MonitorRow {
    id: uuid::Uuid,
    user_id: uuid::Uuid,
    url: String,
    name: String,
    status: String,
    alert_email: bool,
    alert_slack_url: Option<String>,
    alert_discord_url: Option<String>,
    monitor_type: String,
    port: Option<i32>,
    keyword: Option<String>,
    keyword_must_contain: bool,
}

/// Background task: checks all enabled monitors periodically.
pub async fn run(pool: PgPool, mut shutdown_rx: tokio::sync::broadcast::Receiver<()>) {
    tracing::info!("Uptime monitor started");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(5))
        .danger_accept_invalid_certs(false)
        .build()
        .unwrap();

    let mut interval = tokio::time::interval(Duration::from_secs(60));
    let mut tick_count: u64 = 0;

    loop {
        tokio::select! {
            _ = interval.tick() => {}
            _ = shutdown_rx.recv() => {
                tracing::info!("Uptime monitor shutting down gracefully");
                return;
            }
        }

        tick_count += 1;

        // Get monitors due for checking
        let monitors: Vec<MonitorRow> = match sqlx::query_as(
            "SELECT id, user_id, url, name, status, alert_email, alert_slack_url, alert_discord_url, \
             monitor_type, port, keyword, keyword_must_contain \
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

        // Process monitors concurrently (max 10 at a time)
        let mut set = tokio::task::JoinSet::new();
        for monitor in monitors {
            let c = client.clone();
            let p = pool.clone();
            set.spawn(async move {
                check_monitor(&monitor, &c, &p).await;
            });
            // Cap concurrency at 10 — wait for one to finish before spawning more
            if set.len() >= 10 {
                let _ = set.join_next().await;
            }
        }
        // Drain remaining tasks
        while let Some(_) = set.join_next().await {}

        // Purge old data only every hour (every 60th tick at 60s interval)
        if tick_count % 60 == 0 {
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
}

/// Check a single monitor: HTTP/TCP request, record result, handle status transitions.
async fn check_monitor(monitor: &MonitorRow, client: &reqwest::Client, pool: &PgPool) {
    let (status_code, error, new_status, response_time) = if monitor.monitor_type == "tcp" {
        check_tcp(monitor).await
    } else {
        check_http(monitor, client).await
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
    .execute(pool)
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
    .execute(pool)
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
        .execute(pool)
        .await {
            tracing::error!("Failed to create incident for {}: {e}", monitor.name);
        }

        tracing::warn!("Monitor {} ({}) is DOWN: {}", monitor.name, monitor.url, cause);
        crate::services::system_log::log_event(
            pool,
            "warning",
            "uptime",
            &format!("Monitor down: {} ({})", monitor.name, monitor.url),
            Some(cause),
        ).await;
        send_alerts(pool, monitor, &format!("{} is down: {cause}", monitor.name)).await;
    } else if new_status == "up" && monitor.status == "down" {
        // Just recovered — resolve incident
        if let Err(e) = sqlx::query(
            "UPDATE incidents SET resolved_at = NOW() \
             WHERE monitor_id = $1 AND resolved_at IS NULL",
        )
        .bind(monitor.id)
        .execute(pool)
        .await {
            tracing::error!("Failed to resolve incident for {}: {e}", monitor.name);
        }

        tracing::info!("Monitor {} ({}) is back UP", monitor.name, monitor.url);
        send_alerts(pool, monitor, &format!("{} is back up ({}ms)", monitor.name, response_time)).await;
    }
}

/// TCP port check — connect to host:port with timeout.
async fn check_tcp(monitor: &MonitorRow) -> (Option<i32>, Option<String>, &'static str, i32) {
    let host = monitor.url.trim_start_matches("tcp://");
    let port = monitor.port.unwrap_or(80) as u16;
    let addr = format!("{}:{}", host, port);

    let start = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::net::TcpStream::connect(&addr),
    ).await;
    let response_time = start.elapsed().as_millis() as i32;

    match result {
        Ok(Ok(_)) => (Some(0), None, "up", response_time),
        Ok(Err(e)) => (None, Some(format!("TCP connection failed: {e}")), "down", response_time),
        Err(_) => (None, Some("TCP connection timed out".to_string()), "down", response_time),
    }
}

/// HTTP check with optional keyword verification.
async fn check_http(monitor: &MonitorRow, client: &reqwest::Client) -> (Option<i32>, Option<String>, &'static str, i32) {
    let start = Instant::now();
    let result = client.get(&monitor.url).send().await;
    let response_time = start.elapsed().as_millis() as i32;

    match result {
        Ok(resp) => {
            let code = resp.status().as_u16() as i32;
            if !resp.status().is_success() {
                return (Some(code), Some(format!("HTTP {code}")), "down", response_time);
            }

            // Keyword check if configured
            if let Some(ref keyword) = monitor.keyword {
                if !keyword.is_empty() {
                    let body = resp.text().await.unwrap_or_default();
                    let contains = body.contains(keyword.as_str());
                    let must_contain = monitor.keyword_must_contain;

                    if (must_contain && !contains) || (!must_contain && contains) {
                        let error = if must_contain {
                            format!("Keyword '{}' not found in response", keyword)
                        } else {
                            format!("Keyword '{}' found in response (should not be present)", keyword)
                        };
                        return (Some(code), Some(error), "down", response_time);
                    }
                }
            }

            (Some(code), None, "up", response_time)
        }
        Err(e) => (None, Some(e.to_string()), "down", response_time),
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
