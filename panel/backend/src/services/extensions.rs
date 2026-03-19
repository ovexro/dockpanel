use sqlx::PgPool;
use std::sync::OnceLock;
use std::time::Instant;

static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn http_client() -> &'static reqwest::Client {
    HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .unwrap_or_default()
    })
}

/// Emit an event to all subscribed extensions (fire-and-forget).
pub async fn emit_event(pool: &PgPool, event_type: &str, data: serde_json::Value) {
    // Find all enabled extensions subscribed to this event
    let extensions: Vec<(uuid::Uuid, String, String)> = match sqlx::query_as(
        "SELECT id, webhook_url, webhook_secret FROM extensions WHERE enabled = TRUE",
    )
    .fetch_all(pool)
    .await
    {
        Ok(exts) => exts,
        Err(_) => return,
    };

    let delivery_id = uuid::Uuid::new_v4().to_string();
    let timestamp = chrono::Utc::now().to_rfc3339();

    let payload = serde_json::json!({
        "event": event_type,
        "timestamp": timestamp,
        "delivery_id": delivery_id,
        "data": data,
    });
    let payload_str = serde_json::to_string(&payload).unwrap_or_default();

    for (ext_id, webhook_url, webhook_secret) in extensions {
        let pool = pool.clone();
        let event_type = event_type.to_string();
        let payload_str = payload_str.clone();
        let delivery_id = delivery_id.clone();

        tokio::spawn(async move {
            let started = Instant::now();

            // Compute HMAC-SHA256 signature
            use hmac::{Hmac, Mac};
            use sha2::Sha256;
            type HmacSha256 = Hmac<Sha256>;
            let signature = match HmacSha256::new_from_slice(webhook_secret.as_bytes()) {
                Ok(mut mac) => {
                    mac.update(payload_str.as_bytes());
                    hex::encode(mac.finalize().into_bytes())
                }
                Err(_) => {
                    tracing::error!("HMAC key invalid for extension {ext_id}, skipping delivery");
                    let _ = sqlx::query(
                        "INSERT INTO extension_events (extension_id, event_type, payload, response_body, duration_ms) \
                         VALUES ($1, $2, $3, 'HMAC key error — delivery skipped', 0)"
                    ).bind(ext_id).bind(&event_type).bind(&payload_str).execute(&pool).await;
                    return; // Exit this spawned task, don't deliver unsigned webhook
                }
            };

            let result = http_client()
                .post(&webhook_url)
                .header("Content-Type", "application/json")
                .header("X-DockPanel-Event", &event_type)
                .header("X-DockPanel-Delivery", &delivery_id)
                .header("X-DockPanel-Signature", format!("sha256={signature}"))
                .body(payload_str.clone())
                .send()
                .await;

            let (status, body) = match result {
                Ok(resp) => {
                    let status = resp.status().as_u16() as i32;
                    let body = resp.text().await.unwrap_or_default();
                    (Some(status), body.chars().take(1024).collect::<String>())
                }
                Err(e) => (None, format!("Delivery failed: {e}")),
            };

            let duration = started.elapsed().as_millis() as i32;

            // Record delivery
            let _ = sqlx::query(
                "INSERT INTO extension_events (extension_id, event_type, payload, response_status, response_body, duration_ms) \
                 VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(ext_id)
            .bind(&event_type)
            .bind(&payload_str)
            .bind(status)
            .bind(&body)
            .bind(duration)
            .execute(&pool)
            .await;

            // Update extension last_webhook status
            let _ = sqlx::query(
                "UPDATE extensions SET last_webhook_at = NOW(), last_webhook_status = $1 WHERE id = $2",
            )
            .bind(status)
            .bind(ext_id)
            .execute(&pool)
            .await;

            if let Some(s) = status {
                if s >= 400 {
                    tracing::warn!(
                        "Extension webhook failed: ext={ext_id} event={event_type} status={s}"
                    );
                }
            }
        });
    }
}

/// Helper: emit event from route handlers (fire-and-forget spawn).
pub fn fire_event(pool: &PgPool, event_type: &str, data: serde_json::Value) {
    let pool = pool.clone();
    let event_type = event_type.to_string();
    tokio::spawn(async move {
        emit_event(&pool, &event_type, data).await;
    });
}
