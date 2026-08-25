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
            // Do NOT follow redirects: the notify_*_url values are vetted by
            // validate_url_not_internal only at write time, so a public URL that
            // 3xx-redirects to http://127.0.0.1 / 169.254.169.254 would otherwise
            // exfiltrate to / probe an internal address. Parity with
            // webhook_gateway.rs::http_client.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            // Panic (rather than unwrap_or_default) if the builder fails: the default
            // client FOLLOWS redirects, which would silently drop the SSRF control.
            .expect("build notification http client")
    })
}

/// POST a JSON payload to a user-supplied Slack/Discord/webhook URL, re-validating
/// it against SSRF at send time (DNS-rebinding defense — the write-time check can be
/// defeated by a low-TTL record that flips to an internal IP before the alert fires).
/// The shared http_client() additionally refuses redirects. Mirrors
/// webhook_gateway.rs::forward_to_route. The re-validation is bounded by a 3s timeout so
/// a slow/hostile resolver cannot serialize the alert path; fail-closed on error/timeout.
async fn post_user_webhook(client: &reqwest::Client, url: &str, payload: serde_json::Value) {
    match tokio::time::timeout(
        Duration::from_secs(3),
        crate::helpers::validate_url_not_internal(url),
    )
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            tracing::warn!("Notification webhook blocked at send time (SSRF/DNS-rebind?): {e}");
            return;
        }
        Err(_) => {
            tracing::warn!("Notification webhook URL validation timed out; skipping send");
            return;
        }
    }
    let _ = client
        .post(url)
        .json(&payload)
        .timeout(Duration::from_secs(10))
        .send()
        .await;
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
            post_user_webhook(client, url, serde_json::json!({ "text": text })).await;
        }
    }

    // Discord webhook — supports custom template via notif_template_discord
    if let Some(ref url) = channels.discord_url {
        if !url.is_empty() {
            let content = format_message(pool, "discord", subject, message, severity).await;
            post_user_webhook(client, url, serde_json::json!({ "content": content })).await;
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
            post_user_webhook(client, url, serde_json::json!({
                "title": subject,
                "message": custom_message,
                "severity": severity,
                "timestamp": chrono::Utc::now().to_rfc3339(),
                "source": "dockpanel"
            })).await;
        }
    }
}

/// Resolve panel base URL from settings → env → fallback.
/// Used to build "Open runbook" links in notification payloads.
async fn panel_base_url(pool: &PgPool) -> String {
    if let Ok(Some((url,))) = sqlx::query_as::<_, (String,)>(
        "SELECT value FROM settings WHERE key = 'base_url'",
    )
    .fetch_optional(pool)
    .await
    {
        if !url.is_empty() {
            return url.trim_end_matches('/').to_string();
        }
    }
    std::env::var("BASE_URL")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|s| s.trim_end_matches('/').to_string())
        .unwrap_or_default()
}

/// Phase 4 W3: build the runbook excerpt + URL pair for a given alert type.
///
/// Returns `(None, None)` when no runbook exists (DB row or const default).
/// Returns `(Some excerpt, Some url)` when the runbook is loadable AND a
/// `base_url` is configured; `(Some excerpt, None)` when no `base_url` is set
/// (excerpt still useful for chat/webhook channels, URL omitted).
///
/// Used by both `try_fire_alert` (initial page) and
/// `services::alert_engine::check_escalations` (re-pages on escalation)
/// so escalation notifications carry the same runbook payload as the
/// original fire (W2 consistency repair).
pub async fn load_runbook_payload(
    pool: &PgPool,
    alert_type: &str,
) -> (Option<String>, Option<String>) {
    let runbook = crate::services::alert_runbooks::get_runbook(pool, alert_type).await;
    let excerpt = runbook
        .as_ref()
        .map(|r| crate::services::alert_runbooks::excerpt(&r.runbook_md, 280));
    let url = if runbook.is_some() {
        let base = panel_base_url(pool).await;
        if base.is_empty() {
            None
        } else {
            Some(format!("{base}/alerts/runbooks/{alert_type}"))
        }
    } else {
        None
    };
    (excerpt, url)
}

/// Phase 4 W2: Send a notification with runbook attachment.
/// Used by `try_fire_alert` and `check_escalations` (W3) — non-alert
/// callers stay on `send_notification`.
///
/// Per-channel handling:
/// - email: full markdown rendered via pulldown-cmark, appended to body_html
/// - slack/discord: link + 280-char excerpt appended to message
/// - pagerduty: runbook_url + runbook_excerpt added to custom_details
/// - webhook: runbook_url + runbook_excerpt as top-level keys
pub async fn send_notification_with_runbook(
    pool: &PgPool,
    channels: &NotifyChannels,
    subject: &str,
    message: &str,
    body_html: &str,
    runbook_excerpt: Option<&str>,
    runbook_url: Option<&str>,
) {
    let client = http_client();
    let severity = derive_severity(subject);

    // Email — appends rendered runbook HTML to body_html.
    if let Some(ref email) = channels.email {
        let email_template: Option<(String,)> = sqlx::query_as(
            "SELECT value FROM settings WHERE key = 'notif_template_email'",
        )
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();

        let runbook_html = runbook_excerpt
            .map(|md| render_runbook_html(md))
            .unwrap_or_default();
        let runbook_url_str = runbook_url.unwrap_or("");

        let html = if let Some((tmpl,)) = email_template {
            if !tmpl.is_empty() {
                tmpl.replace("{{title}}", subject)
                    .replace("{{message}}", message)
                    .replace("{{severity}}", severity)
                    .replace("{{timestamp}}", &chrono::Utc::now().to_rfc3339())
                    .replace("{{runbook_excerpt}}", &runbook_html)
                    .replace("{{runbook_url}}", runbook_url_str)
            } else {
                append_runbook_to_html(body_html, &runbook_html, runbook_url_str)
            }
        } else {
            append_runbook_to_html(body_html, &runbook_html, runbook_url_str)
        };

        if let Err(e) = crate::services::email::send_email(pool, email, subject, &html).await {
            tracing::warn!("Alert email failed: {e}");
        }
    }

    // Slack — append `*Runbook:* <url|view>\n_excerpt_` to message.
    if let Some(ref url) = channels.slack_url {
        if !url.is_empty() {
            let mut text = format_message(pool, "slack", subject, message, severity).await;
            if let Some(excerpt) = runbook_excerpt {
                if let Some(rurl) = runbook_url {
                    text.push_str(&format!("\n\n*Runbook:* <{rurl}|view>\n_{excerpt}_"));
                } else {
                    text.push_str(&format!("\n\n*Runbook:* _{excerpt}_"));
                }
            }
            post_user_webhook(client, url, serde_json::json!({ "text": text })).await;
        }
    }

    // Discord — append **Runbook:** [view](url)\n*excerpt*
    if let Some(ref url) = channels.discord_url {
        if !url.is_empty() {
            let mut content = format_message(pool, "discord", subject, message, severity).await;
            if let Some(excerpt) = runbook_excerpt {
                if let Some(rurl) = runbook_url {
                    content.push_str(&format!("\n\n**Runbook:** [view]({rurl})\n*{excerpt}*"));
                } else {
                    content.push_str(&format!("\n\n**Runbook:** *{excerpt}*"));
                }
            }
            post_user_webhook(client, url, serde_json::json!({ "content": content })).await;
        }
    }

    // PagerDuty — extend custom_details with runbook fields.
    if let Some(ref key) = channels.pagerduty_key {
        if !key.is_empty() {
            let event_action = if subject.contains("Resolved") || subject.contains("back up") {
                "resolve"
            } else {
                "trigger"
            };
            let mut custom_details = serde_json::json!({ "message": message });
            if let Some(excerpt) = runbook_excerpt {
                custom_details["runbook_excerpt"] = serde_json::json!(excerpt);
            }
            if let Some(rurl) = runbook_url {
                custom_details["runbook_url"] = serde_json::json!(rurl);
            }
            let _ = client
                .post("https://events.pagerduty.com/v2/enqueue")
                .json(&serde_json::json!({
                    "routing_key": key,
                    "event_action": event_action,
                    "payload": {
                        "summary": subject,
                        "source": "DockPanel",
                        "severity": severity,
                        "custom_details": custom_details,
                    },
                }))
                .timeout(Duration::from_secs(10))
                .send()
                .await;
        }
    }

    // Generic webhook — top-level runbook keys.
    if let Some(ref url) = channels.webhook_url {
        if !url.is_empty() {
            let custom_message = format_message(pool, "webhook", subject, message, severity).await;
            let mut payload = serde_json::json!({
                "title": subject,
                "message": custom_message,
                "severity": severity,
                "timestamp": chrono::Utc::now().to_rfc3339(),
                "source": "dockpanel",
            });
            if let Some(excerpt) = runbook_excerpt {
                payload["runbook_excerpt"] = serde_json::json!(excerpt);
            }
            if let Some(rurl) = runbook_url {
                payload["runbook_url"] = serde_json::json!(rurl);
            }
            post_user_webhook(client, url, payload).await;
        }
    }
}

/// Render markdown to safe HTML using pulldown-cmark. Wrapped in catch_unwind
/// defensively — admin-authored input is trusted but the parser is third-party.
fn render_runbook_html(md: &str) -> String {
    use std::panic::{catch_unwind, AssertUnwindSafe};
    let result = catch_unwind(AssertUnwindSafe(|| {
        let parser = pulldown_cmark::Parser::new(md);
        let mut html = String::with_capacity(md.len() * 2);
        pulldown_cmark::html::push_html(&mut html, parser);
        html
    }));
    match result {
        Ok(html) => html,
        Err(_) => {
            tracing::warn!("pulldown-cmark panicked rendering runbook; falling back to raw text");
            html_escape(md)
        }
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn append_runbook_to_html(body_html: &str, runbook_html: &str, runbook_url: &str) -> String {
    if runbook_html.is_empty() {
        return body_html.to_string();
    }
    let link = if runbook_url.is_empty() {
        String::new()
    } else {
        format!(
            "<p style=\"margin:16px 0 8px\"><a href=\"{runbook_url}\" \
             style=\"color:#3b82f6;text-decoration:none;font-weight:600\">Open runbook in panel →</a></p>"
        )
    };
    format!(
        "{body_html}\
         <hr style=\"margin:24px 0;border:none;border-top:1px solid #e5e7eb\"/>\
         <h3 style=\"font-family:sans-serif;color:#111827;margin:0 0 12px\">Runbook</h3>\
         <div style=\"font-family:sans-serif;color:#374151;line-height:1.5\">{runbook_html}</div>\
         {link}"
    )
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

/// Phase 4 W3: look up the escalation_policy_id for a user/server pair.
/// Mirrors `get_user_channels` row-resolution: server-specific row wins,
/// global (server_id IS NULL) row is the fallback, no row at all → None.
pub async fn get_user_policy_id(
    pool: &PgPool,
    user_id: Uuid,
    server_id: Option<Uuid>,
) -> Option<Uuid> {
    if let Some(sid) = server_id {
        let specific: Option<(Option<Uuid>,)> = sqlx::query_as(
            "SELECT escalation_policy_id FROM alert_rules \
             WHERE user_id = $1 AND server_id = $2",
        )
        .bind(user_id)
        .bind(sid)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
        if let Some((Some(pid),)) = specific {
            return Some(pid);
        }
        // Specific row existed but policy NULL → preserve "no policy"; only
        // fall back to global when the specific row is absent entirely.
        if specific.is_some() {
            return None;
        }
    }
    let global: Option<(Option<Uuid>,)> = sqlx::query_as(
        "SELECT escalation_policy_id FROM alert_rules \
         WHERE user_id = $1 AND server_id IS NULL",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    global.and_then(|(p,)| p)
}

/// Phase 4 W3: load + decode an `escalation_policies` row's `steps` array.
/// Returns the parsed `Vec<EscalationStep>` or empty vec on any failure.
pub async fn load_escalation_steps(
    pool: &PgPool,
    policy_id: Uuid,
) -> Vec<crate::models::EscalationStep> {
    let row: Option<(serde_json::Value,)> = sqlx::query_as(
        "SELECT steps FROM escalation_policies WHERE id = $1",
    )
    .bind(policy_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    let Some((steps_json,)) = row else { return Vec::new(); };
    serde_json::from_value(steps_json).unwrap_or_else(|e| {
        tracing::warn!("Failed to decode escalation_policy {policy_id} steps: {e}");
        Vec::new()
    })
}

/// Phase 4 W3: dispatch a single escalation step's payload.
///
/// Resolves the step's `route` against on-call schedules and user IDs,
/// then sends `send_notification_with_runbook` for each resolved
/// channel-set. `alert_owner_id` is the user_id on the alerts row —
/// used as the fallback "channels of record" for `all_channels` routes
/// and synthetic-webhook routes.
pub async fn dispatch_escalation_step(
    pool: &PgPool,
    alert_owner_id: Uuid,
    alert_owner_server_id: Option<Uuid>,
    alert_type: &str,
    step: &crate::models::EscalationStep,
    subject: &str,
    message: &str,
    body_html: &str,
    runbook_excerpt: Option<&str>,
    runbook_url: Option<&str>,
) {
    let route = &step.route;
    if let Some(url) = route.strip_prefix("webhook:") {
        // Direct webhook bypass — synthesize a NotifyChannels with only
        // the webhook_url populated.
        let synthetic = NotifyChannels {
            email: None,
            slack_url: None,
            discord_url: None,
            pagerduty_key: None,
            webhook_url: Some(url.to_string()),
            muted_types: String::new(),
        };
        send_notification_with_runbook(
            pool,
            &synthetic,
            subject,
            message,
            body_html,
            runbook_excerpt,
            runbook_url,
        )
        .await;
        return;
    }

    if route == "all_channels" {
        // Fan out to the alert's owner — preserves pre-W3 default behaviour.
        fanout_to_user(
            pool,
            alert_owner_id,
            alert_owner_server_id,
            alert_type,
            subject,
            message,
            body_html,
            runbook_excerpt,
            runbook_url,
        )
        .await;
        return;
    }

    // on_call_schedule:<uuid> or user:<uuid> → routes resolve to user IDs.
    let users = crate::services::on_call::route_to_user_ids(pool, route).await;
    if users.is_empty() {
        // Fail OPEN to the alert's owner rather than dropping the page. An
        // unresolvable route (deleted schedule, emptied members, deleted routed
        // user, malformed shape) used to silently swallow the notification —
        // and since try_fire_alert dispatches step 0 and returns, that swallowed
        // the *initial* page for a critical alert too. A page to the owner
        // instead of the rota is a degraded delivery; no page at all is a
        // missed outage.
        tracing::warn!(
            "dispatch_escalation_step: route {route} resolved to no users — falling back to the alert owner's channels"
        );
        fanout_to_user(
            pool,
            alert_owner_id,
            alert_owner_server_id,
            alert_type,
            subject,
            message,
            body_html,
            runbook_excerpt,
            runbook_url,
        )
        .await;
        return;
    }
    let mut delivered = false;
    for uid in users {
        delivered |= fanout_to_user(
            pool,
            uid,
            alert_owner_server_id,
            alert_type,
            subject,
            message,
            body_html,
            runbook_excerpt,
            runbook_url,
        )
        .await;
    }

    if !delivered {
        // The route resolved to real user IDs, but none of them has any
        // notification configuration — a rota member who never opened alert
        // settings, or a `user:<uuid>` route to a since-deleted account. That is
        // indistinguishable from an unresolvable route in effect: nobody is
        // paged. Degrade to the owner for the same reason.
        tracing::warn!(
            "dispatch_escalation_step: no routed user for {route} has notification channels — falling back to the alert owner"
        );
        fanout_to_user(
            pool,
            alert_owner_id,
            alert_owner_server_id,
            alert_type,
            subject,
            message,
            body_html,
            runbook_excerpt,
            runbook_url,
        )
        .await;
    }
}

/// Every alert type an operator is able to suppress from external channels.
///
/// The Settings suppression grid renders exactly this vocabulary and both
/// write paths validate against it, so a stored list can never name something
/// the panel does not page about. It drifted once: the grid was a hand-written
/// ten and the panel had grown to twenty producers, so half the alert types
/// that page an operator had no per-type control at all — and the missing half
/// was the half nobody had thought about recently, which is the half that
/// pages you at three in the morning.
///
/// Membership is DELIBERATE, not derived from the producers. A type belongs
/// here only if its pages actually reach the per-type suppression check on the
/// fan-out. One producer writes its row with a direct INSERT that never
/// reaches the fan-out at all; a checkbox for it would name a thing it does
/// not govern, which is the defect this list exists to end, not to spread.
pub const SUPPRESSIBLE_ALERT_TYPES: &[&str] = &[
    "cpu",
    "memory",
    "disk",
    "disk_forecast",
    "memory_leak",
    "offline",
    "service_down",
    "container_down",
    "container_crashloop",
    "container_unhealthy",
    "gpu_utilization",
    "gpu_temperature",
    "gpu_vram",
    "backup_failure",
    "backup_verification_failed",
    "cron_failure",
    "ssl_expiry",
    "ssl_renewal_failure",
    "security",
];

/// Tokens of a stored suppression list that name no suppressible alert type.
///
/// Returned rather than silently dropped: a value that persists, echoes back
/// on the next read and suppresses nothing is indistinguishable from a working
/// mute, and the operator keeps being paged by a type their settings say is
/// off. The write paths turn a non-empty result into a rejection that names
/// the offending token.
pub fn unknown_suppressible_types(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .filter(|s| !SUPPRESSIBLE_ALERT_TYPES.contains(s))
        .map(|s| s.to_string())
        .collect()
}

/// Is this alert type on the user's suppression list (Gap #69)?
///
/// Single-sourced because the mute has to mean the same thing on both edges of
/// an alert's life. It was applied on the firing path and skipped on the resolve
/// path, so muting a type silenced the page and still delivered the recovery —
/// a suppression that only half-suppresses is worse than none, because the
/// operator sees "Resolved: X" for an X they were never told about.
fn is_type_muted(channels: &NotifyChannels, alert_type: &str) -> bool {
    !channels.muted_types.is_empty()
        && channels
            .muted_types
            .split(',')
            .map(|s| s.trim())
            .any(|t| t == alert_type)
}

/// Phase 4 W3: send to one user's channels with that user's own mute
/// preference applied. Used by `dispatch_escalation_step` to honour the
/// routed user's per-type mute even when escalation routes them in.
///
/// Returns whether this user was a SERVICEABLE destination. A deliberate mute
/// counts as serviceable (the operator chose silence, and an escalation must not
/// override it); only "this user has no notification configuration at all"
/// returns false, so a caller can degrade to someone who does.
async fn fanout_to_user(
    pool: &PgPool,
    user_id: Uuid,
    server_id: Option<Uuid>,
    alert_type: &str,
    subject: &str,
    message: &str,
    body_html: &str,
    runbook_excerpt: Option<&str>,
    runbook_url: Option<&str>,
) -> bool {
    let Some(channels) = get_user_channels(pool, user_id, server_id).await else {
        // `alert_rules` rows are only created when a user opens alert settings,
        // so a perfectly valid rota member can have none — this is the common
        // case, not a corrupt one.
        tracing::warn!(
            "No notification channels configured for user {user_id} — cannot deliver '{alert_type}' page to them"
        );
        return false;
    };
    if is_type_muted(&channels, alert_type) {
        tracing::debug!(
            "Alert type '{alert_type}' muted for routed user {user_id} — skipping external channels"
        );
        return true;
    }
    send_notification_with_runbook(
        pool,
        &channels,
        subject,
        message,
        body_html,
        runbook_excerpt,
        runbook_url,
    )
    .await;
    true
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
        "gpu_utilization" | "gpu_temperature" | "gpu_vram" => "alert_gpu",
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
            global_thresholds(pool, user_id).await
        }
    } else {
        // server_id = None means "the user's global rule" — that is exactly what
        // check_ssl_expiry asks for (SSL is site-scoped, not server-scoped). This
        // arm used to return None unconditionally, so the SSL ladder silently ran
        // on the hardcoded defaults and a user's configured ssl_warning_days was
        // never read.
        global_thresholds(pool, user_id).await
    };

    row.unwrap_or((90, 5, 90, 5, 85, 60, "30,14,7,3,1".to_string()))
}

/// The user's server-independent `alert_rules` row (`server_id IS NULL`).
async fn global_thresholds(
    pool: &PgPool,
    user_id: Uuid,
) -> Option<(i32, i32, i32, i32, i32, i32, String)> {
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

/// Get GPU-specific threshold settings for a user/server.
/// Returns (gpu_util_threshold, gpu_util_duration, gpu_temp_threshold, gpu_vram_threshold, cooldown).
pub async fn get_gpu_thresholds(
    pool: &PgPool,
    user_id: Uuid,
    server_id: Option<Uuid>,
) -> (i32, i32, i32, i32, i32) {
    let row: Option<(i32, i32, i32, i32, i32)> = if let Some(sid) = server_id {
        let specific: Option<(i32, i32, i32, i32, i32)> = sqlx::query_as(
            "SELECT gpu_util_threshold, gpu_util_duration, gpu_temp_threshold, \
             gpu_vram_threshold, cooldown_minutes \
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
                "SELECT gpu_util_threshold, gpu_util_duration, gpu_temp_threshold, \
                 gpu_vram_threshold, cooldown_minutes \
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

    row.unwrap_or((95, 5, 85, 95, 60))
}

/// Fire an alert: check cooldown, record in alerts table, send notification.
/// Convenience wrapper that ignores errors (for callers that don't need retry).
///
/// `state_key` names the entity this alert is about — the container name, the
/// service name, the GPU index — and MUST be the same key the caller writes to
/// `alert_state.state_key`. Pass `""` for a condition about the server as a
/// whole (disk, CPU, memory). It is a required argument rather than an
/// `Option` with a default precisely so that a new alert type cannot inherit
/// the unscoped behaviour by saying nothing: see `resolve_alert`, where the
/// absence of this key resolved every sibling alert on the server.
pub async fn fire_alert(
    pool: &PgPool,
    user_id: Uuid,
    server_id: Option<Uuid>,
    site_id: Option<Uuid>,
    alert_type: &str,
    state_key: &str,
    severity: &str,
    title: &str,
    message: &str,
) {
    let _ = try_fire_alert(pool, user_id, server_id, site_id, alert_type, state_key, severity, title, message).await;
}

/// The `state_key`s raised under `alert_type = "ssl_renewal_failure"`.
///
/// Both SSL loops raise FIVE different conditions under that one type, and
/// `fire_alert_deduped` below dedups on `(alert_type, site_id, state_key)`.
/// Feeding it the whole-server key made all five one subject, so whichever
/// fired first muted the other four for twelve hours — four of them `critical`.
///
/// The pairing that made this fatal is sequential, not coincidental.
/// `RenewalPlan::Refuse` alerts every twelve hours for as long as a DNS-01 zone
/// stays unreachable, and the downgrade it ends in fires only once that same
/// refusal reaches the certificate's last week. The warning is therefore always
/// inside the window that hid the critical, and the alert saying which names
/// stopped being covered could not be delivered to the only people who could
/// ever receive it.
///
/// Keyed per CONDITION, not per certificate: `site_id` already names the
/// certificate, and the two loops must still collapse to ONE alert when they
/// reach the same condition on the same site — which is the flood control the
/// dedup was added for and which these keys preserve exactly.
pub mod ssl_renewal_key {
    /// The installed certificate was issued by somebody else, so renewing it
    /// would replace it. Announced, not failed.
    pub const DECLINED: &str = "declined";
    /// A renewal was attempted and failed.
    pub const FAILED: &str = "failed";
    /// A renewal could not be attempted at all — a configuration problem, so
    /// deliberately a different sentence from FAILED.
    pub const BLOCKED: &str = "blocked";
    /// A DNS-01 renewal refused while there is still time to fix the zone.
    pub const DNS01_DECLINED: &str = "dns01_declined";
    /// A DNS-01 certificate downgraded to a single name in its last week.
    pub const DNS01_DOWNGRADED: &str = "dns01_downgraded";
}

/// Fire an alert unless one of the same type already fired for the same site
/// inside `within_hours`.
///
/// The loops that watch certificates run every two minutes. An unconditional
/// alert from inside one of them is not a warning, it is a flood — and a flood
/// is how a real alert gets muted. Deduplicating against the alerts table keeps
/// it to one per site per window without introducing a second piece of state
/// that can disagree with the first.
pub async fn fire_alert_deduped(
    pool: &PgPool,
    user_id: Uuid,
    server_id: Option<Uuid>,
    site_id: Option<Uuid>,
    alert_type: &str,
    state_key: &str,
    severity: &str,
    title: &str,
    message: &str,
    within_hours: i64,
) {
    // within_hours is a typed i64 — safe to interpolate, and it keeps Postgres
    // from having to infer a parameter type inside an interval expression.
    //
    // The dedup window is scoped by `state_key` too: two different certificates
    // on the same site are two different subjects, and suppressing the second
    // because the first fired is the same conflation `resolve_alert` used to
    // make in the other direction.
    let recent: Option<(i64,)> = sqlx::query_as(&format!(
        "SELECT COUNT(*) FROM alerts \
         WHERE alert_type = $1 AND site_id IS NOT DISTINCT FROM $2 AND state_key = $3 \
         AND created_at > NOW() - INTERVAL '{within_hours} hours'",
    ))
    .bind(alert_type)
    .bind(site_id)
    .bind(state_key)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    if recent.map(|r| r.0).unwrap_or(0) > 0 {
        return;
    }

    fire_alert(pool, user_id, server_id, site_id, alert_type, state_key, severity, title, message).await;
}

/// Fire an alert with Result return for retry support.
pub async fn try_fire_alert(
    pool: &PgPool,
    user_id: Uuid,
    server_id: Option<Uuid>,
    site_id: Option<Uuid>,
    alert_type: &str,
    state_key: &str,
    severity: &str,
    title: &str,
    message: &str,
) -> Result<(), String> {
    // Check if this alert type is enabled
    if !is_alert_enabled(pool, user_id, server_id, alert_type).await {
        return Ok(());
    }

    // Record in alerts table. `state_key` mirrors `alert_state.state_key` so the
    // resolve path can target this exact entity's row rather than every row of
    // this type on the server.
    sqlx::query(
        "INSERT INTO alerts (user_id, server_id, site_id, alert_type, state_key, severity, title, message) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(user_id)
    .bind(server_id)
    .bind(site_id)
    .bind(alert_type)
    .bind(state_key)
    .bind(severity)
    .bind(title)
    .bind(message)
    .execute(pool)
    .await
    .map_err(|e| format!("Failed to record alert: {e}"))?;

    // Also store in panel notification center (bell icon) — notify all admins
    notify_panel(pool, None, title, message, severity, "alert", Some("/monitoring?tab=alerts")).await;

    // Build the notification payload once — both the NULL-policy fan-out and the
    // policy-driven fan-out reuse it.
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
    // Phase 4 W2 + W3: attach runbook to the notification payload.
    // Same helper is reused by check_escalations so escalation re-pages
    // carry the runbook excerpt + URL too.
    let (runbook_excerpt, runbook_url) = load_runbook_payload(pool, alert_type).await;

    // Phase 4 W3: if the alert_rules row attaches an escalation policy,
    // page only the channels routed by step 0 (e.g. the current on-call
    // user). NULL policy_id preserves the pre-W3 behaviour exactly.
    let policy_id = get_user_policy_id(pool, user_id, server_id).await;
    if let Some(pid) = policy_id {
        let steps = load_escalation_steps(pool, pid).await;
        if let Some(step0) = steps.first() {
            dispatch_escalation_step(
                pool,
                user_id,
                server_id,
                alert_type,
                step0,
                &subject,
                message,
                &html,
                runbook_excerpt.as_deref(),
                runbook_url.as_deref(),
            )
            .await;
            return Ok(());
        }
        tracing::warn!(
            "Alert rule references escalation_policy {pid} with empty/invalid steps — falling back to default channel fan-out"
        );
    }

    // NULL policy (or fallback for malformed policy) → pre-W3 behaviour:
    // page the alert owner's channels with their own mute prefs applied.
    fanout_to_user(
        pool,
        user_id,
        server_id,
        alert_type,
        &subject,
        message,
        &html,
        runbook_excerpt.as_deref(),
        runbook_url.as_deref(),
    )
    .await;

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
    if let Some(uid) = user_id {
        insert_and_broadcast(db, uid, title, message, severity, category, link).await;
    } else {
        let admins: Vec<(uuid::Uuid,)> = sqlx::query_as("SELECT id FROM users WHERE role = 'admin'")
            .fetch_all(db).await.unwrap_or_default();
        for (admin_id,) in &admins {
            insert_and_broadcast(db, *admin_id, title, message, severity, category, link).await;
        }
    }
}

/// One recipient's copy: write the row, then broadcast what was written.
///
/// Two things here used to be wrong in ways nothing could report.
///
/// The INSERT was `let _ = …`, so a notification that failed to store — a
/// constraint, a full disk, a pool timeout — was indistinguishable from one
/// that stored fine. The one class of message whose entire job is to tell you
/// something went wrong was the class that failed silently. It still does not
/// return an error (a caller reporting a deploy result must not fail because
/// the notification did not store), but it now says so at `error!`.
///
/// And the SSE payload was built from the arguments, so it carried no `id`, no
/// `created_at` and no `read_at` — every field a client needs to render the row
/// or to mark it read. That is why the only subscriber in the frontend threw the
/// body away and re-fetched a count instead. The INSERT now RETURNs the row's
/// identity and the payload carries it, so a live list can prepend what arrives
/// instead of asking the server what it just sent.
async fn insert_and_broadcast(
    db: &sqlx::PgPool,
    user_id: uuid::Uuid,
    title: &str,
    message: &str,
    severity: &str,
    category: &str,
    link: Option<&str>,
) {
    let inserted: Result<(uuid::Uuid, chrono::DateTime<chrono::Utc>), _> = sqlx::query_as(
        "INSERT INTO panel_notifications (user_id, title, message, severity, category, link) \
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id, created_at"
    ).bind(user_id).bind(title).bind(message).bind(severity).bind(category).bind(link)
    .fetch_one(db).await;

    let (id, created_at) = match inserted {
        Ok(row) => row,
        Err(e) => {
            tracing::error!(
                "notification not stored for {user_id} ({category}/{severity}): {title} — {e}"
            );
            return;
        }
    };

    if let Some(tx) = NOTIF_TX.get() {
        let payload = serde_json::json!({
            "id": id,
            "title": title,
            "message": message,
            "severity": severity,
            "category": category,
            "link": link,
            "read_at": serde_json::Value::Null,
            "created_at": created_at,
        })
        .to_string();
        let _ = tx.send((user_id, payload));
    }
}

/// Resolve a firing alert and send recovery notification.
///
/// `state_key` MUST be the key the matching fire used, and the same one the
/// caller clears in `alert_state`. Scoping is not a refinement here, it is the
/// correctness property: `alert_state` is keyed
/// `(server_id, alert_type, state_key)` while this UPDATE was keyed only
/// `(user_id, server_id, alert_type)`, so one container recovering resolved the
/// alert rows of every other container down on that server. Those siblings kept
/// `alert_state.current_state = 'firing'`, and a transition-triggered engine
/// never re-announces a condition it believes it already announced — so the
/// remaining outages went silent permanently.
///
/// The recovery page is sent only if this call actually resolved something.
/// "Resolved: X" for a condition that was never firing is a false all-clear, and
/// two live callers (`auto_healer`'s service and disk heals) reach here without
/// checking first.
pub async fn resolve_alert(
    pool: &PgPool,
    user_id: Uuid,
    server_id: Option<Uuid>,
    site_id: Option<Uuid>,
    alert_type: &str,
    state_key: &str,
    title: &str,
    message: &str,
) {
    // Resolve firing alerts of this type FOR THIS ENTITY.
    //
    // The third arm exists because `monitors.site_id` is nullable: a monitor
    // that watches a bare URL fires with both ids NULL, and before `state_key`
    // there was no way to name what such an alert was about, so this function
    // could only refuse. A non-empty key IS the scope — it identifies one
    // subject inside one user's alerts — so that case is now resolvable. Both
    // ids NULL *and* an empty key is still a refusal: that would match every
    // server-wide alert the user has.
    let query = if server_id.is_some() {
        "UPDATE alerts SET status = 'resolved', resolved_at = NOW() \
         WHERE user_id = $1 AND server_id = $2 AND alert_type = $3 AND state_key = $4 AND status = 'firing'"
    } else if site_id.is_some() {
        "UPDATE alerts SET status = 'resolved', resolved_at = NOW() \
         WHERE user_id = $1 AND site_id = $2 AND alert_type = $3 AND state_key = $4 AND status = 'firing'"
    } else if !state_key.is_empty() {
        // `$2::uuid` is cast because this arm never references it for anything
        // else, and an unadorned parameter Postgres cannot type is precisely how
        // the slow-response INSERT stayed dead for months (see `uptime.rs`).
        // Keeping the bind keeps one `.bind()` chain for all three arms.
        "UPDATE alerts SET status = 'resolved', resolved_at = NOW() \
         WHERE user_id = $1 AND $2::uuid IS NULL AND alert_type = $3 AND state_key = $4 AND status = 'firing'"
    } else {
        tracing::warn!(
            "resolve_alert called for '{alert_type}' with no server_id, no site_id and no state_key — refusing to resolve unscoped"
        );
        return;
    };

    let resolved = match sqlx::query(query)
        .bind(user_id)
        .bind(server_id.or(site_id))
        .bind(alert_type)
        .bind(state_key)
        .execute(pool)
        .await
    {
        Ok(r) => r.rows_affected(),
        Err(e) => {
            // A failed UPDATE must not become a recovery page: the condition may
            // well still be firing, and this is the direction that reassures.
            tracing::warn!("resolve_alert UPDATE failed for '{alert_type}' key '{state_key}': {e}");
            return;
        }
    };

    if resolved == 0 {
        tracing::debug!(
            "resolve_alert: nothing firing for '{alert_type}' key '{state_key}' — no recovery notice sent"
        );
        return;
    }

    // Send recovery notification, honouring the same per-type mute the firing
    // path honours. A muted type that pages on recovery is still a page.
    if let Some(channels) = get_user_channels(pool, user_id, server_id).await {
        if is_type_muted(&channels, alert_type) {
            tracing::debug!("Alert type '{alert_type}' muted for user {user_id} — skipping recovery notice");
        } else {
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
    }

    // Panel notification center
    notify_panel(pool, Some(user_id), &format!("Resolved: {}", title), message, "info", "alert", Some("/monitoring?tab=alerts")).await;
}
