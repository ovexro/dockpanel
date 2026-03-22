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
    check_interval: i32,
    last_checked_at: Option<chrono::DateTime<chrono::Utc>>,
    custom_headers: Option<serde_json::Value>,
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

        // Get monitors due for checking (HTTP/TCP/ping) + all heartbeat monitors (checked separately)
        let monitors: Vec<MonitorRow> = match sqlx::query_as(
            "SELECT id, user_id, url, name, status, alert_email, alert_slack_url, alert_discord_url, \
             monitor_type, port, keyword, keyword_must_contain, check_interval, last_checked_at, custom_headers \
             FROM monitors WHERE enabled = TRUE AND \
             (monitor_type = 'heartbeat' OR last_checked_at IS NULL OR last_checked_at < NOW() - (check_interval || ' seconds')::interval)",
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

        // Batch-load users in maintenance windows (avoid N+1 query per monitor)
        let maintenance_users: std::collections::HashSet<uuid::Uuid> = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT DISTINCT user_id FROM maintenance_windows WHERE starts_at <= NOW() AND ends_at >= NOW()"
        ).fetch_all(&pool).await.unwrap_or_default().into_iter().collect();

        // Process monitors concurrently (max 10 at a time)
        let mut set = tokio::task::JoinSet::new();
        for monitor in monitors {
            // Skip monitors for users in maintenance windows
            if maintenance_users.contains(&monitor.user_id) {
                continue;
            }

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

/// Check a single monitor: HTTP/TCP/ping request, record result, handle status transitions.
async fn check_monitor(monitor: &MonitorRow, client: &reqwest::Client, pool: &PgPool) {
    // Heartbeat monitors are passive — check if we missed a beat
    if monitor.monitor_type == "heartbeat" {
        check_heartbeat(monitor, pool).await;
        return;
    }

    let (status_code, error, new_status, response_time) = match monitor.monitor_type.as_str() {
        "tcp" => check_tcp(monitor).await,
        "ping" => check_ping(monitor).await,
        _ => check_http(monitor, client).await,
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

        // GAP 3: Auto-create managed incident for status page
        let _ = create_auto_incident(pool, monitor, cause).await;

        // GAP 19: Notify status page subscribers
        notify_status_subscribers(pool, &monitor.name, "investigating", &format!("{} is experiencing issues: {cause}", monitor.name)).await;
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

        // GAP 3: Auto-resolve managed incident
        let _ = resolve_auto_incident(pool, monitor).await;

        // GAP 19: Notify subscribers of recovery
        notify_status_subscribers(pool, &monitor.name, "resolved", &format!("{} is back online", monitor.name)).await;

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

/// Ping/ICMP check — uses system ping command.
async fn check_ping(monitor: &MonitorRow) -> (Option<i32>, Option<String>, &'static str, i32) {
    let host = monitor.url.trim_start_matches("ping://");
    let start = Instant::now();

    let output = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::process::Command::new("ping")
            .args(["-c", "1", "-W", "5", host])
            .output()
    ).await;

    let response_time = start.elapsed().as_millis() as i32;

    match output {
        Ok(Ok(o)) if o.status.success() => {
            // Parse response time from ping output: "time=X.XX ms"
            let stdout = String::from_utf8_lossy(&o.stdout);
            let ping_time = stdout.split("time=").nth(1)
                .and_then(|s| s.split_whitespace().next())
                .and_then(|s| s.parse::<f64>().ok())
                .map(|ms| ms as i32)
                .unwrap_or(response_time);
            (Some(0), None, "up", ping_time)
        }
        _ => (None, Some("Ping failed or timed out".to_string()), "down", response_time),
    }
}

/// Heartbeat (dead man's switch) — alerts if no ping received within 2x interval.
async fn check_heartbeat(monitor: &MonitorRow, pool: &PgPool) {
    let expected_interval = Duration::from_secs(monitor.check_interval.max(60) as u64);
    let last_check = monitor.last_checked_at.unwrap_or_else(chrono::Utc::now);
    let elapsed = chrono::Utc::now() - last_check;

    let max_silence = chrono::Duration::from_std(expected_interval * 2)
        .unwrap_or(chrono::Duration::minutes(10));

    if elapsed > max_silence {
        // Missed heartbeat
        if monitor.status != "down" {
            if let Err(e) = sqlx::query(
                "INSERT INTO monitor_checks (monitor_id, status_code, response_time, error) VALUES ($1, NULL, 0, $2)",
            )
            .bind(monitor.id)
            .bind("Heartbeat missed")
            .execute(pool)
            .await {
                tracing::error!("Failed to insert heartbeat miss check for {}: {e}", monitor.name);
            }

            if let Err(e) = sqlx::query(
                "UPDATE monitors SET status = 'down', last_checked_at = NOW(), last_response_time = 0, last_status_code = NULL WHERE id = $1",
            )
            .bind(monitor.id)
            .execute(pool)
            .await {
                tracing::error!("Failed to update heartbeat monitor status for {}: {e}", monitor.name);
            }

            if let Err(e) = sqlx::query(
                "INSERT INTO incidents (monitor_id, cause, alerted) VALUES ($1, $2, TRUE)",
            )
            .bind(monitor.id)
            .bind("Heartbeat missed — no ping received")
            .execute(pool)
            .await {
                tracing::error!("Failed to create heartbeat incident for {}: {e}", monitor.name);
            }

            tracing::warn!("Monitor {} ({}) heartbeat missed", monitor.name, monitor.url);
            crate::services::system_log::log_event(
                pool,
                "warning",
                "uptime",
                &format!("Heartbeat missed: {} ({})", monitor.name, monitor.url),
                Some("No heartbeat received within expected interval"),
            ).await;
            send_alerts(pool, monitor, &format!("{} heartbeat missed — no ping received", monitor.name)).await;
        }
    }
}

/// HTTP check with optional keyword verification and custom headers.
async fn check_http(monitor: &MonitorRow, client: &reqwest::Client) -> (Option<i32>, Option<String>, &'static str, i32) {
    let start = Instant::now();
    let mut builder = client.get(&monitor.url);

    // Apply custom headers if present
    if let Some(ref headers_json) = monitor.custom_headers {
        if let Some(headers_map) = headers_json.as_object() {
            for (key, value) in headers_map {
                if let Some(v) = value.as_str() {
                    if let (Ok(name), Ok(val)) = (
                        reqwest::header::HeaderName::from_bytes(key.as_bytes()),
                        reqwest::header::HeaderValue::from_str(v),
                    ) {
                        builder = builder.header(name, val);
                    }
                }
            }
        }
    }

    let result = builder.send().await;
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

    // Get PagerDuty key from alert_rules
    let pagerduty_key: Option<String> = sqlx::query_scalar(
        "SELECT notify_pagerduty_key FROM alert_rules WHERE user_id = $1 AND server_id IS NULL"
    ).bind(monitor.user_id).fetch_optional(pool).await.ok().flatten().flatten();

    let channels = crate::services::notifications::NotifyChannels {
        email,
        slack_url: monitor.alert_slack_url.clone(),
        discord_url: monitor.alert_discord_url.clone(),
        pagerduty_key,
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

/// GAP 3: Auto-create a managed incident when a monitor goes down.
async fn create_auto_incident(pool: &PgPool, monitor: &MonitorRow, cause: &str) -> Result<(), String> {
    // Find admin user for the monitor
    let user: Option<(uuid::Uuid,)> = sqlx::query_as(
        "SELECT user_id FROM monitors WHERE id = $1"
    )
    .bind(monitor.id)
    .fetch_optional(pool).await.ok().flatten();

    let user_id = match user {
        Some((id,)) => id,
        None => return Ok(()),
    };

    // Create managed incident
    let incident_id: uuid::Uuid = match sqlx::query_scalar(
        "INSERT INTO managed_incidents (user_id, title, status, severity, description, visible_on_status_page) \
         VALUES ($1, $2, 'investigating', 'major', $3, TRUE) RETURNING id"
    )
    .bind(user_id)
    .bind(format!("{} is down", monitor.name))
    .bind(cause)
    .fetch_one(pool).await {
        Ok(id) => id,
        Err(e) => {
            tracing::error!("Failed to create managed incident: {e}");
            return Err(e.to_string());
        }
    };

    // Create initial update
    let _ = sqlx::query(
        "INSERT INTO incident_updates (incident_id, status, message, author_email) \
         VALUES ($1, 'investigating', $2, 'system')"
    )
    .bind(incident_id)
    .bind(format!("Auto-detected: {cause}"))
    .execute(pool).await;

    // Link to status page components via monitor
    let _ = sqlx::query(
        "INSERT INTO managed_incident_components (incident_id, component_id) \
         SELECT $1, cm.component_id FROM status_page_component_monitors cm WHERE cm.monitor_id = $2 \
         ON CONFLICT DO NOTHING"
    )
    .bind(incident_id).bind(monitor.id)
    .execute(pool).await;

    tracing::info!("Auto-incident created for monitor {} (incident {})", monitor.name, incident_id);
    Ok(())
}

/// GAP 3: Auto-resolve managed incident when monitor recovers.
async fn resolve_auto_incident(pool: &PgPool, monitor: &MonitorRow) -> Result<(), String> {
    // Find unresolved managed incidents with matching title pattern
    let incidents: Vec<(uuid::Uuid,)> = sqlx::query_as(
        "SELECT id FROM managed_incidents WHERE title = $1 AND status != 'resolved' AND status != 'postmortem'"
    )
    .bind(format!("{} is down", monitor.name))
    .fetch_all(pool).await.unwrap_or_default();

    for (incident_id,) in &incidents {
        // Post resolved update
        let _ = sqlx::query(
            "INSERT INTO incident_updates (incident_id, status, message, author_email) \
             VALUES ($1, 'resolved', 'Monitor recovered automatically', 'system')"
        )
        .bind(incident_id)
        .execute(pool).await;

        // Resolve the incident
        let _ = sqlx::query(
            "UPDATE managed_incidents SET status = 'resolved', resolved_at = NOW(), updated_at = NOW() WHERE id = $1"
        )
        .bind(incident_id)
        .execute(pool).await;
    }

    if !incidents.is_empty() {
        tracing::info!("Auto-resolved {} managed incident(s) for monitor {}", incidents.len(), monitor.name);
    }
    Ok(())
}

/// GAP 19: Notify status page subscribers of monitor events.
async fn notify_status_subscribers(pool: &PgPool, monitor_name: &str, status: &str, message: &str) {
    let emails: Vec<(String,)> = sqlx::query_as(
        "SELECT email FROM status_page_subscribers WHERE verified = TRUE AND notify_incidents = TRUE"
    )
    .fetch_all(pool).await.unwrap_or_default();

    if emails.is_empty() {
        return;
    }

    let subject = format!("[Status Update] {} — {}", monitor_name, status);

    for (email,) in &emails {
        crate::services::notifications::send_notification(
            pool,
            &crate::services::notifications::NotifyChannels {
                email: Some(email.clone()),
                slack_url: None,
                discord_url: None,
                pagerduty_key: None,
            },
            &subject,
            message,
            message,
        ).await;
    }
}
