use sqlx::PgPool;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::broadcast;
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

// ── Real-time notification broadcast (SSE) ─────────────────────────────────

/// Global broadcast sender for real-time notification delivery.
/// Initialized once from main.rs at startup via `init_notif_broadcast`.
static NOTIF_TX: OnceLock<broadcast::Sender<(Uuid, String)>> = OnceLock::new();

/// Register the broadcast sender (called once from main.rs).
pub fn init_notif_broadcast(tx: broadcast::Sender<(Uuid, String)>) {
    NOTIF_TX.set(tx).ok();
}

/// Notification channels for delivering alerts.
pub struct NotifyChannels {
    pub email: Option<String>,
    pub slack_url: Option<String>,
    pub discord_url: Option<String>,
    pub pagerduty_key: Option<String>,
    pub webhook_url: Option<String>,
    /// Comma-separated alert types to suppress from external channels (Gap #69)
    pub muted_types: String,
}

/// Gap #70: Load a custom notification template from settings, or use default formatting.
async fn format_message(pool: &PgPool, channel: &str, subject: &str, message: &str, severity: &str) -> String {
    let key = format!("notif_template_{channel}");
    let template: Option<(String,)> = sqlx::query_as(
        "SELECT value FROM settings WHERE key = $1"
    ).bind(&key).fetch_optional(pool).await.ok().flatten();

    if let Some((tmpl,)) = template {
        if !tmpl.is_empty() {
            return tmpl.replace("{{title}}", subject)
                .replace("{{message}}", message)
                .replace("{{severity}}", severity)
                .replace("{{timestamp}}", &chrono::Utc::now().to_rfc3339());
        }
    }

    // Default format per channel
    match channel {
        "slack" => format!("*{subject}*\n{message}"),
        "discord" => format!("**{subject}**\n{message}"),
        _ => format!("{subject}\n\n{message}"),
    }
}

/// Derive severity string from subject line (for webhook/pagerduty payloads).
fn derive_severity(subject: &str) -> &'static str {
    if subject.contains("FAIL") || subject.contains("down") || subject.contains("critical") {
        "critical"
    } else if subject.contains("warning") {
        "warning"
    } else if subject.contains("Resolved") || subject.contains("back up") {
        "info"
    } else {
        "error"
    }
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
    let severity = derive_severity(subject);

    // Email — supports custom template via notif_template_email
    if let Some(ref email) = channels.email {
        let email_template: Option<(String,)> = sqlx::query_as(
            "SELECT value FROM settings WHERE key = 'notif_template_email'"
        ).fetch_optional(pool).await.ok().flatten();

        let html = if let Some((tmpl,)) = email_template {
            if !tmpl.is_empty() {
                tmpl.replace("{{title}}", subject)
                    .replace("{{message}}", message)
                    .replace("{{severity}}", severity)
                    .replace("{{timestamp}}", &chrono::Utc::now().to_rfc3339())
            } else {
                body_html.to_string()
            }
        } else {
            body_html.to_string()
        };

        if let Err(e) = crate::services::email::send_email(pool, email, subject, &html).await {
            tracing::warn!("Alert email failed: {e}");
        }
    }

    // Slack webhook — supports custom template via notif_template_slack
    if let Some(ref url) = channels.slack_url {
        if !url.is_empty() {
            let text = format_message(pool, "slack", subject, message, severity).await;
            let _ = client
                .post(url)
                .json(&serde_json::json!({ "text": text }))
                .timeout(Duration::from_secs(10))
                .send()
                .await;
        }
    }

    // Discord webhook — supports custom template via notif_template_discord
    if let Some(ref url) = channels.discord_url {
        if !url.is_empty() {
            let content = format_message(pool, "discord", subject, message, severity).await;
            let _ = client
                .post(url)
                .json(&serde_json::json!({ "content": content }))
                .timeout(Duration::from_secs(10))
                .send()
                .await;
        }
    }

    // PagerDuty Events API v2
    if let Some(ref key) = channels.pagerduty_key {
        if !key.is_empty() {
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

    // Generic webhook (GAP 31) — supports custom template via notif_template_webhook
    if let Some(ref url) = channels.webhook_url {
        if !url.is_empty() {
            let custom_message = format_message(pool, "webhook", subject, message, severity).await;
            let _ = client
                .post(url)
                .json(&serde_json::json!({
                    "title": subject,
                    "message": custom_message,
                    "severity": severity,
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                    "source": "dockpanel"
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
    let rule: Option<(bool, Option<String>, Option<String>, Option<String>, Option<String>, String)> = if let Some(sid) = server_id {
        let specific: Option<(bool, Option<String>, Option<String>, Option<String>, Option<String>, String)> = sqlx::query_as(
            "SELECT notify_email, notify_slack_url, notify_discord_url, notify_pagerduty_key, notify_webhook_url, muted_types \
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
                "SELECT notify_email, notify_slack_url, notify_discord_url, notify_pagerduty_key, notify_webhook_url, muted_types \
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
            "SELECT notify_email, notify_slack_url, notify_discord_url, notify_pagerduty_key, notify_webhook_url, muted_types \
             FROM alert_rules WHERE user_id = $1 AND server_id IS NULL",
        )
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
    };

    let (notify_email, slack_url, discord_url, pagerduty_key, webhook_url, muted_types) = rule?;

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
        webhook_url,
        muted_types,
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
        // Gap #69: Check if this alert type is muted from external channels
        let is_muted = if !channels.muted_types.is_empty() {
            let muted: Vec<&str> = channels.muted_types.split(',').map(|s| s.trim()).collect();
            muted.contains(&alert_type)
        } else {
            false
        };

        if !is_muted {
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
        } else {
            tracing::debug!("Alert type '{alert_type}' is muted — skipping external channels");
        }
    }

    Ok(())
}

/// Insert notification into the panel notification center (bell icon).
/// Pass user_id = None to notify all admins.
/// Also broadcasts via SSE for real-time delivery.
pub async fn notify_panel(
    db: &sqlx::PgPool,
    user_id: Option<uuid::Uuid>,
    title: &str,
    message: &str,
    severity: &str,
    category: &str,
    link: Option<&str>,
) {
    // Build JSON payload once for SSE broadcast
    let notif_json = serde_json::json!({
        "title": title,
        "message": message,
        "severity": severity,
        "category": category,
        "link": link,
    })
    .to_string();

    if let Some(uid) = user_id {
        let _ = sqlx::query(
            "INSERT INTO panel_notifications (user_id, title, message, severity, category, link) VALUES ($1, $2, $3, $4, $5, $6)"
        ).bind(uid).bind(title).bind(message).bind(severity).bind(category).bind(link)
        .execute(db).await;

        // Broadcast to SSE subscribers
        if let Some(tx) = NOTIF_TX.get() {
            let _ = tx.send((uid, notif_json));
        }
    } else {
        let admins: Vec<(uuid::Uuid,)> = sqlx::query_as("SELECT id FROM users WHERE role = 'admin'")
            .fetch_all(db).await.unwrap_or_default();
        for (admin_id,) in &admins {
            let _ = sqlx::query(
                "INSERT INTO panel_notifications (user_id, title, message, severity, category, link) VALUES ($1, $2, $3, $4, $5, $6)"
            ).bind(admin_id).bind(title).bind(message).bind(severity).bind(category).bind(link)
            .execute(db).await;

            // Broadcast to SSE subscribers
            if let Some(tx) = NOTIF_TX.get() {
                let _ = tx.send((*admin_id, notif_json.clone()));
            }
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
