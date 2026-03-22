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

/// Maximum concurrent webhook deliveries across all extensions.
static ACTIVE_DELIVERIES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
const MAX_CONCURRENT_DELIVERIES: u32 = 20;

/// Emit an event to all subscribed extensions (fire-and-forget).
pub async fn emit_event(pool: &PgPool, event_type: &str, data: serde_json::Value) {
    // Rate limit: skip if too many deliveries in flight
    let active = ACTIVE_DELIVERIES.load(std::sync::atomic::Ordering::Relaxed);
    if active >= MAX_CONCURRENT_DELIVERIES {
        tracing::warn!("extension webhook rate limit hit ({active} in-flight), skipping {event_type}");
        return;
    }

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

        ACTIVE_DELIVERIES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
                    ACTIVE_DELIVERIES.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
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

            ACTIVE_DELIVERIES.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        });
    }
}

/// Helper: emit event from route handlers (fire-and-forget spawn).
/// GAP 7+21: Also forwards events to webhook gateway endpoints that have
/// routes with filter_path="/event" matching this event_type.
pub fn fire_event(pool: &PgPool, event_type: &str, data: serde_json::Value) {
    let pool = pool.clone();
    let event_type = event_type.to_string();
    tokio::spawn(async move {
        emit_event(&pool, &event_type, data.clone()).await;

        // Bridge to webhook gateway: find routes with filter_path="/event" and matching filter_value
        let routes: Vec<(uuid::Uuid, String, serde_json::Value, i32, i32)> = sqlx::query_as(
            "SELECT r.id, r.destination_url, r.extra_headers, r.retry_count, r.retry_delay_secs \
             FROM webhook_routes r JOIN webhook_endpoints e ON e.id = r.endpoint_id AND e.enabled = TRUE \
             WHERE r.enabled = TRUE AND r.filter_path = '/event' AND r.filter_value = $1"
        )
        .bind(&event_type)
        .fetch_all(&pool).await.unwrap_or_default();

        if routes.is_empty() { return; }

        let payload = serde_json::json!({
            "event": &event_type,
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "data": data,
        });
        let payload_str = serde_json::to_string(&payload).unwrap_or_default();

        for (route_id, dest_url, extra_headers, retry_count, retry_delay) in routes {
            let payload_clone = payload_str.clone();
            let dest = dest_url.clone();
            let headers = extra_headers.clone();
            let pool_clone = pool.clone();

            tokio::spawn(async move {
                let mut last_status = 0i32;
                for attempt in 0..=(retry_count.max(0).min(5)) {
                    if attempt > 0 {
                        tokio::time::sleep(std::time::Duration::from_secs(
                            retry_delay as u64 * (1 << (attempt - 1).min(4))
                        )).await;
                    }

                    let mut req = http_client()
                        .post(&dest)
                        .header("Content-Type", "application/json");

                    if let Some(obj) = headers.as_object() {
                        for (k, v) in obj {
                            if let Some(val) = v.as_str() {
                                req = req.header(k.as_str(), val);
                            }
                        }
                    }

                    match req.body(payload_clone.clone()).send().await {
                        Ok(resp) => {
                            last_status = resp.status().as_u16() as i32;
                            if last_status >= 200 && last_status < 300 { break; }
                        }
                        Err(_) => { last_status = 0; }
                    }
                }

                let _ = sqlx::query(
                    "UPDATE webhook_routes SET total_forwarded = total_forwarded + 1, \
                     last_forwarded_at = NOW(), last_status = $2 WHERE id = $1"
                )
                .bind(route_id).bind(last_status)
                .execute(&pool_clone).await;
            });
        }
    });
}
