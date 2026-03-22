use sqlx::PgPool;
use std::sync::OnceLock;
use std::time::Duration;
use uuid::Uuid;

/// Shared HTTP client for webhook notifications (reuses connections).
fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_default()
    })
}

/// Notification channels for delivering alerts.
pub struct NotifyChannels {
    pub email: Option<String>,
    pub slack_url: Option<String>,
    pub discord_url: Option<String>,
    pub pagerduty_key: Option<String>,
}

/// Send a notification via all configured channels.
pub async fn send_notification(
    pool: &PgPool,
    channels: &NotifyChannels,
    subject: &str,
    message: &str,
    body_html: &str,
) {
    let client = http_client();

    // Email
    if let Some(ref email) = channels.email {
        if let Err(e) = crate::services::email::send_email(pool, email, subject, body_html).await {
            tracing::warn!("Alert email failed: {e}");
        }
    }

    // Slack webhook
    if let Some(ref url) = channels.slack_url {
        if !url.is_empty() {
            let _ = client
                .post(url)
                .json(&serde_json::json!({ "text": format!("*{subject}*\n{message}") }))
                .timeout(Duration::from_secs(10))
                .send()
                .await;
        }
    }

    // Discord webhook
    if let Some(ref url) = channels.discord_url {
        if !url.is_empty() {
            let _ = client
                .post(url)
                .json(&serde_json::json!({ "content": format!("**{subject}**\n{message}") }))
                .timeout(Duration::from_secs(10))
                .send()
                .await;
        }
    }

    // PagerDuty Events API v2
    if let Some(ref key) = channels.pagerduty_key {
        if !key.is_empty() {
            let severity = if subject.contains("FAIL") || subject.contains("down") || subject.contains("critical") {
                "critical"
            } else if subject.contains("warning") {
                "warning"
            } else if subject.contains("Resolved") || subject.contains("back up") {
                "info"
            } else {
                "error"
            };
            let event_action = if subject.contains("Resolved") || subject.contains("back up") {
                "resolve"
            } else {
                "trigger"
            };
            let _ = client
                .post("https://events.pagerduty.com/v2/enqueue")
                .json(&serde_json::json!({
                    "routing_key": key,
                    "event_action": event_action,
                    "payload": {
                        "summary": subject,
                        "source": "DockPanel",
                        "severity": severity,
                        "custom_details": { "message": message },
                    },
                }))
                .timeout(Duration::from_secs(10))
                .send()
                .await;
        }
    }
}

/// Get notification channels for a user from their alert_rules.
/// Checks server-specific rules first, falls back to global (server_id IS NULL).
pub async fn get_user_channels(
    pool: &PgPool,
    user_id: Uuid,
    server_id: Option<Uuid>,
) -> Option<NotifyChannels> {
    // Try server-specific rules first, then global
    let rule: Option<(bool, Option<String>, Option<String>, Option<String>)> = if let Some(sid) = server_id {
        let specific: Option<(bool, Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT notify_email, notify_slack_url, notify_discord_url, notify_pagerduty_key \
             FROM alert_rules WHERE user_id = $1 AND server_id = $2",
        )
        .bind(user_id)
        .bind(sid)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();

        if specific.is_some() {
            specific
        } else {
            sqlx::query_as(
                "SELECT notify_email, notify_slack_url, notify_discord_url, notify_pagerduty_key \
                 FROM alert_rules WHERE user_id = $1 AND server_id IS NULL",
            )
            .bind(user_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
        }
    } else {
        sqlx::query_as(
            "SELECT notify_email, notify_slack_url, notify_discord_url, notify_pagerduty_key \
             FROM alert_rules WHERE user_id = $1 AND server_id IS NULL",
        )
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
    };

    let (notify_email, slack_url, discord_url, pagerduty_key) = rule?;

    // Look up user email if email notifications are enabled
    let email = if notify_email {
        sqlx::query_scalar::<_, String>("SELECT email FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
    } else {
        None
    };

    Some(NotifyChannels {
        email,
        slack_url,
        discord_url,
        pagerduty_key,
    })
}

/// Check if an alert type is enabled for a user.
pub async fn is_alert_enabled(
    pool: &PgPool,
    user_id: Uuid,
    server_id: Option<Uuid>,
    alert_type: &str,
) -> bool {
    let column = match alert_type {
        "cpu" => "alert_cpu",
        "memory" => "alert_memory",
        "disk" => "alert_disk",
        "offline" => "alert_offline",
        "backup_failure" => "alert_backup_failure",
        "ssl_expiry" => "alert_ssl_expiry",
        "service_down" => "alert_service_health",
        _ => return true,
    };

    // Try server-specific, then global
    let query = format!(
        "SELECT {column} FROM alert_rules WHERE user_id = $1 AND server_id {}",
        if server_id.is_some() {
            "= $2"
        } else {
            "IS NULL"
        }
    );

    let result: Option<(bool,)> = if let Some(sid) = server_id {
        // Server-specific first
        let specific: Option<(bool,)> = sqlx::query_as(&query)
            .bind(user_id)
            .bind(sid)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();

        if specific.is_some() {
            specific
        } else {
            let global_query = format!(
                "SELECT {column} FROM alert_rules WHERE user_id = $1 AND server_id IS NULL"
            );
            sqlx::query_as(&global_query)
                .bind(user_id)
                .fetch_optional(pool)
                .await
                .ok()
                .flatten()
        }
    } else {
        sqlx::query_as(&query)
            .bind(user_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
    };

    // Default to true if no rules exist (alerts enabled by default)
    result.map(|r| r.0).unwrap_or(true)
}

/// Get threshold settings for a user/server.
pub async fn get_thresholds(
    pool: &PgPool,
    user_id: Uuid,
    server_id: Option<Uuid>,
) -> (i32, i32, i32, i32, i32, i32, String) {
    // (cpu_threshold, cpu_duration, mem_threshold, mem_duration, disk_threshold, cooldown, ssl_days)
    let row: Option<(i32, i32, i32, i32, i32, i32, String)> = if let Some(sid) = server_id {
        let specific: Option<(i32, i32, i32, i32, i32, i32, String)> = sqlx::query_as(
            "SELECT cpu_threshold, cpu_duration, memory_threshold, memory_duration, \
             disk_threshold, cooldown_minutes, ssl_warning_days \
             FROM alert_rules WHERE user_id = $1 AND server_id = $2",
        )
        .bind(user_id)
        .bind(sid)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();

        if specific.is_some() {
            specific
        } else {
            sqlx::query_as(
                "SELECT cpu_threshold, cpu_duration, memory_threshold, memory_duration, \
                 disk_threshold, cooldown_minutes, ssl_warning_days \
                 FROM alert_rules WHERE user_id = $1 AND server_id IS NULL",
            )
            .bind(user_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
        }
    } else {
        None
    };

    row.unwrap_or((90, 5, 90, 5, 85, 60, "30,14,7,3,1".to_string()))
}

/// Fire an alert: check cooldown, record in alerts table, send notification.
/// Convenience wrapper that ignores errors (for callers that don't need retry).
pub async fn fire_alert(
    pool: &PgPool,
    user_id: Uuid,
    server_id: Option<Uuid>,
    site_id: Option<Uuid>,
    alert_type: &str,
    severity: &str,
    title: &str,
    message: &str,
) {
    let _ = try_fire_alert(pool, user_id, server_id, site_id, alert_type, severity, title, message).await;
}

/// Fire an alert with Result return for retry support.
pub async fn try_fire_alert(
    pool: &PgPool,
    user_id: Uuid,
    server_id: Option<Uuid>,
    site_id: Option<Uuid>,
    alert_type: &str,
    severity: &str,
    title: &str,
    message: &str,
) -> Result<(), String> {
    // Check if this alert type is enabled
    if !is_alert_enabled(pool, user_id, server_id, alert_type).await {
        return Ok(());
    }

    // Record in alerts table
    sqlx::query(
        "INSERT INTO alerts (user_id, server_id, site_id, alert_type, severity, title, message) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(user_id)
    .bind(server_id)
    .bind(site_id)
    .bind(alert_type)
    .bind(severity)
    .bind(title)
    .bind(message)
    .execute(pool)
    .await
    .map_err(|e| format!("Failed to record alert: {e}"))?;

    // Also store in panel notification center (bell icon) — notify all admins
    notify_panel(pool, None, title, message, severity, "alert", None).await;

    // Send notification
    if let Some(channels) = get_user_channels(pool, user_id, server_id).await {
        let subject = format!("DockPanel Alert: {title}");
        let html = format!(
            "<div style=\"font-family:sans-serif;max-width:600px;margin:0 auto\">\
             <h2 style=\"color:{}\">{title}</h2>\
             <p>{message}</p>\
             <p style=\"color:#6b7280;font-size:14px\">Time: {}</p>\
             </div>",
            match severity {
                "critical" => "#ef4444",
                "warning" => "#f59e0b",
                _ => "#3b82f6",
            },
            chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC"),
        );
        send_notification(pool, &channels, &subject, message, &html).await;
    }

    Ok(())
}

/// Insert notification into the panel notification center (bell icon).
/// Pass user_id = None to notify all admins.
pub async fn notify_panel(
    db: &sqlx::PgPool,
    user_id: Option<uuid::Uuid>,
    title: &str,
    message: &str,
    severity: &str,
    category: &str,
    link: Option<&str>,
) {
    if let Some(uid) = user_id {
        let _ = sqlx::query(
            "INSERT INTO panel_notifications (user_id, title, message, severity, category, link) VALUES ($1, $2, $3, $4, $5, $6)"
        ).bind(uid).bind(title).bind(message).bind(severity).bind(category).bind(link)
        .execute(db).await;
    } else {
        let admins: Vec<(uuid::Uuid,)> = sqlx::query_as("SELECT id FROM users WHERE role = 'admin'")
            .fetch_all(db).await.unwrap_or_default();
        for (admin_id,) in &admins {
            let _ = sqlx::query(
                "INSERT INTO panel_notifications (user_id, title, message, severity, category, link) VALUES ($1, $2, $3, $4, $5, $6)"
            ).bind(admin_id).bind(title).bind(message).bind(severity).bind(category).bind(link)
            .execute(db).await;
        }
    }
}

/// Resolve a firing alert and send recovery notification.
pub async fn resolve_alert(
    pool: &PgPool,
    user_id: Uuid,
    server_id: Option<Uuid>,
    site_id: Option<Uuid>,
    alert_type: &str,
    title: &str,
    message: &str,
) {
    // Resolve firing alerts of this type
    let query = if server_id.is_some() {
        "UPDATE alerts SET status = 'resolved', resolved_at = NOW() \
         WHERE user_id = $1 AND server_id = $2 AND alert_type = $3 AND status = 'firing'"
    } else if site_id.is_some() {
        "UPDATE alerts SET status = 'resolved', resolved_at = NOW() \
         WHERE user_id = $1 AND site_id = $2 AND alert_type = $3 AND status = 'firing'"
    } else {
        return;
    };

    let Some(id) = server_id.or(site_id) else {
        tracing::warn!("resolve_alert called with no server_id or site_id");
        return;
    };
    let _ = sqlx::query(query)
        .bind(user_id)
        .bind(id)
        .bind(alert_type)
        .execute(pool)
        .await;

    // Send recovery notification
    if let Some(channels) = get_user_channels(pool, user_id, server_id).await {
        let subject = format!("DockPanel Resolved: {title}");
        let html = format!(
            "<div style=\"font-family:sans-serif;max-width:600px;margin:0 auto\">\
             <h2 style=\"color:#10b981\">{title}</h2>\
             <p>{message}</p>\
             <p style=\"color:#6b7280;font-size:14px\">Time: {}</p>\
             </div>",
            chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC"),
        );
        send_notification(pool, &channels, &subject, message, &html).await;
    }

    // Panel notification center
    notify_panel(pool, Some(user_id), &format!("Resolved: {}", title), message, "info", "alert", None).await;
}
