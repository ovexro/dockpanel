//! Telemetry collector background service.
//!
//! Responsibilities:
//! 1. Periodically collects system diagnostics from the agent
//! 2. Stores error/warning events in the telemetry_events table
//! 3. Sends unsent events to the configured remote endpoint (opt-in)
//! 4. Cleans up old events based on retention settings
//! 5. Checks GitHub Releases for DockPanel updates

use crate::services::agent::AgentClient;
use sqlx::PgPool;
use std::time::Duration;

const CHECK_INTERVAL: Duration = Duration::from_secs(3600); // 1 hour
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(21600); // 6 hours
const RETENTION_DAYS: i64 = 30;
const MAX_BATCH_SIZE: i64 = 50;
const GITHUB_RELEASES_URL: &str =
    "https://api.github.com/repos/ovexro/dockpanel/releases/latest";

/// Record a telemetry event (callable from anywhere in the backend).
pub async fn record_event(
    pool: &PgPool,
    event_type: &str,
    category: &str,
    message: &str,
    context: serde_json::Value,
) {
    let result = sqlx::query(
        "INSERT INTO telemetry_events (event_type, category, message, context) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(event_type)
    .bind(category)
    .bind(message)
    .bind(&context)
    .execute(pool)
    .await;

    if let Err(e) = result {
        tracing::warn!("Failed to record telemetry event: {e}");
    }
}

/// Main telemetry loop — runs as a supervised background service.
pub async fn run(
    pool: PgPool,
    agent: AgentClient,
    mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
) {
    // Stagger start to avoid thundering herd with other background services
    tokio::time::sleep(Duration::from_secs(120)).await;

    let mut check_interval = tokio::time::interval(CHECK_INTERVAL);
    let mut update_interval = tokio::time::interval(UPDATE_CHECK_INTERVAL);
    // Skip the first immediate tick for the update check
    update_interval.tick().await;

    loop {
        tokio::select! {
            _ = check_interval.tick() => {
                collect_and_process(&pool, &agent).await;
            }
            _ = update_interval.tick() => {
                check_for_updates(&pool).await;
            }
            _ = shutdown_rx.recv() => {
                tracing::info!("Telemetry collector shutting down");
                break;
            }
        }
    }
}

/// Collect diagnostics, send if enabled, clean up old events.
async fn collect_and_process(pool: &PgPool, agent: &AgentClient) {
    // 1. Collect agent health snapshot
    collect_agent_health(pool, agent).await;

    // 2. Check if telemetry sending is enabled
    let enabled = get_setting(pool, "telemetry_enabled").await == "true";
    if enabled {
        let endpoint = get_setting(pool, "telemetry_endpoint").await;
        if !endpoint.is_empty() {
            send_pending_events(pool, &endpoint).await;
        }
    }

    // 3. Retention cleanup
    cleanup_old_events(pool).await;
}

/// Collect agent health info and record any issues as telemetry events.
async fn collect_agent_health(pool: &PgPool, agent: &AgentClient) {
    // Check if agent is reachable via health endpoint
    match agent.get("/health").await {
        Ok(_) => {
            // Agent is healthy — no event needed
        }
        Err(e) => {
            record_event(
                pool,
                "error",
                "agent",
                &format!("Agent unreachable: {e}"),
                serde_json::json!({}),
            )
            .await;
        }
    }

    // Check key services via agent diagnostics
    if let Ok(report) = agent.get("/diagnostics").await {
        // Record any critical/error findings
        if let Some(checks) = report.get("checks").and_then(|c| c.as_array()) {
            for check in checks {
                let severity = check
                    .get("severity")
                    .and_then(|s| s.as_str())
                    .unwrap_or("");
                if severity == "critical" || severity == "error" {
                    let title = check
                        .get("title")
                        .and_then(|t| t.as_str())
                        .unwrap_or("Unknown issue");
                    let category = check
                        .get("category")
                        .and_then(|c| c.as_str())
                        .unwrap_or("general");

                    // Deduplicate: don't record the same issue within 6 hours
                    let exists: bool = sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM telemetry_events \
                         WHERE category = $1 AND message = $2 \
                         AND created_at > NOW() - INTERVAL '6 hours')",
                    )
                    .bind(category)
                    .bind(title)
                    .fetch_one(pool)
                    .await
                    .unwrap_or(false);

                    if !exists {
                        record_event(pool, "error", category, title, check.clone())
                            .await;
                    }
                }
            }
        }
    }
}

/// Public wrapper for routes to trigger manual send.
pub async fn send_pending_events_public(pool: &PgPool, endpoint: &str) {
    send_pending_events(pool, endpoint).await;
}

/// Public wrapper for routes to trigger manual update check.
pub async fn check_for_updates_public(pool: &PgPool) {
    check_for_updates(pool).await;
}

/// Send unsent telemetry events to the remote endpoint.
async fn send_pending_events(pool: &PgPool, endpoint: &str) {
    // Validate endpoint URL
    if !endpoint.starts_with("https://") {
        tracing::warn!("Telemetry endpoint must use HTTPS, skipping send");
        return;
    }

    // Fetch unsent events
    let events: Vec<(uuid::Uuid, String, String, String, serde_json::Value, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as(
            "SELECT id, event_type, category, message, context, created_at \
             FROM telemetry_events WHERE sent_at IS NULL \
             ORDER BY created_at ASC LIMIT $1",
        )
        .bind(MAX_BATCH_SIZE)
        .fetch_all(pool)
        .await
        .unwrap_or_default();

    if events.is_empty() {
        return;
    }

    // Get installation ID (anonymous)
    let installation_id = get_or_create_installation_id(pool).await;

    // Get system info snapshot
    let system_info = serde_json::json!({
        "dockpanel_version": env!("CARGO_PKG_VERSION"),
        "installation_id": installation_id,
    });

    // Build batch payload — strip any PII
    let batch: Vec<serde_json::Value> = events
        .iter()
        .map(|(id, event_type, category, message, context, created_at)| {
            serde_json::json!({
                "id": id.to_string(),
                "event_type": event_type,
                "category": category,
                "message": message,
                "context": strip_pii(context),
                "created_at": created_at.to_rfc3339(),
            })
        })
        .collect();

    let payload = serde_json::json!({
        "system": system_info,
        "events": batch,
        "sent_at": chrono::Utc::now().to_rfc3339(),
    });

    // Send with timeout
    let client = reqwest::Client::new();
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        client
            .post(endpoint)
            .header("Content-Type", "application/json")
            .header("User-Agent", format!("DockPanel/{}", env!("CARGO_PKG_VERSION")))
            .json(&payload)
            .send(),
    )
    .await;

    match result {
        Ok(Ok(resp)) if resp.status().is_success() => {
            // Mark events as sent
            let ids: Vec<uuid::Uuid> = events.iter().map(|(id, ..)| *id).collect();
            let _ = sqlx::query(
                "UPDATE telemetry_events SET sent_at = NOW() WHERE id = ANY($1)",
            )
            .bind(&ids)
            .execute(pool)
            .await;

            tracing::info!("Telemetry: sent {} events to endpoint", ids.len());
        }
        Ok(Ok(resp)) => {
            tracing::warn!(
                "Telemetry endpoint returned status {}: {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            );
        }
        Ok(Err(e)) => {
            tracing::warn!("Telemetry send failed: {e}");
        }
        Err(_) => {
            tracing::warn!("Telemetry send timed out");
        }
    }
}

/// Strip PII from context JSON before sending remotely.
pub fn strip_pii(context: &serde_json::Value) -> serde_json::Value {
    match context {
        serde_json::Value::Object(map) => {
            let mut clean = serde_json::Map::new();
            let pii_keys = [
                "ip",
                "email",
                "username",
                "password",
                "token",
                "secret",
                "domain",
                "hostname",
                "path",
                "user_id",
                "api_key",
                "auth",
                "session",
                "cookie",
                "phone",
                "address",
                "cert",
                "private_key",
                "url",
            ];
            for (key, value) in map {
                if pii_keys.iter().any(|k| key.to_lowercase().contains(k)) {
                    clean.insert(key.clone(), serde_json::Value::String("[redacted]".to_string()));
                } else {
                    clean.insert(key.clone(), strip_pii(value));
                }
            }
            serde_json::Value::Object(clean)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(strip_pii).collect())
        }
        other => other.clone(),
    }
}

/// Check GitHub Releases for a newer version.
async fn check_for_updates(pool: &PgPool) {
    let client = reqwest::Client::new();
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        client
            .get(GITHUB_RELEASES_URL)
            .header("User-Agent", format!("DockPanel/{}", env!("CARGO_PKG_VERSION")))
            .header("Accept", "application/vnd.github+json")
            .send(),
    )
    .await;

    let resp = match result {
        Ok(Ok(r)) if r.status().is_success() => r,
        Ok(Ok(r)) => {
            tracing::debug!("GitHub releases check returned {}", r.status());
            return;
        }
        Ok(Err(e)) => {
            tracing::debug!("GitHub releases check failed: {e}");
            return;
        }
        Err(_) => {
            tracing::debug!("GitHub releases check timed out");
            return;
        }
    };

    let body = match resp.text().await {
        Ok(b) => b,
        Err(_) => return,
    };

    let release: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => return,
    };

    let tag = release
        .get("tag_name")
        .and_then(|t| t.as_str())
        .unwrap_or("");
    let latest_version = tag.trim_start_matches('v');
    let current_version = env!("CARGO_PKG_VERSION");

    if latest_version.is_empty() || latest_version == current_version {
        // No update or same version — clear any stale update info
        let _ = sqlx::query(
            "DELETE FROM settings WHERE key IN ('update_available_version', 'update_release_notes', 'update_release_url', 'update_checked_at')",
        )
        .execute(pool)
        .await;
        return;
    }

    // Simple semver comparison (works for X.Y.Z format)
    let is_newer = compare_versions(latest_version, current_version);
    if !is_newer {
        return;
    }

    let release_notes = release
        .get("body")
        .and_then(|b| b.as_str())
        .unwrap_or("")
        .chars()
        .take(4000) // Limit stored release notes
        .collect::<String>();

    let release_url = release
        .get("html_url")
        .and_then(|u| u.as_str())
        .unwrap_or("");

    // Store update info in settings
    let pairs = [
        ("update_available_version", latest_version),
        ("update_release_notes", &release_notes),
        ("update_release_url", release_url),
    ];

    for (key, value) in &pairs {
        let _ = sqlx::query(
            "INSERT INTO settings (key, value) VALUES ($1, $2) \
             ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
        )
        .bind(key)
        .bind(value)
        .execute(pool)
        .await;
    }

    // Store check timestamp
    let _ = sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('update_checked_at', $1) \
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
    )
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await;

    tracing::info!(
        "DockPanel update available: v{current_version} -> v{latest_version}"
    );
}

/// Compare two semver version strings. Returns true if `a` is newer than `b`.
fn compare_versions(a: &str, b: &str) -> bool {
    let parse = |v: &str| -> Vec<u32> {
        v.split('.')
            .filter_map(|p| p.parse::<u32>().ok())
            .collect()
    };
    let va = parse(a);
    let vb = parse(b);
    for i in 0..3 {
        let pa = va.get(i).copied().unwrap_or(0);
        let pb = vb.get(i).copied().unwrap_or(0);
        if pa > pb {
            return true;
        }
        if pa < pb {
            return false;
        }
    }
    false
}

/// Clean up events older than retention period.
async fn cleanup_old_events(pool: &PgPool) {
    let _ = sqlx::query(
        "DELETE FROM telemetry_events WHERE created_at < NOW() - $1::interval",
    )
    .bind(format!("{RETENTION_DAYS} days"))
    .execute(pool)
    .await;
}

/// Get or create an anonymous installation ID.
async fn get_or_create_installation_id(pool: &PgPool) -> String {
    let existing: Option<(String,)> =
        sqlx::query_as("SELECT value FROM settings WHERE key = 'telemetry_installation_id'")
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();

    if let Some((id,)) = existing {
        return id;
    }

    let new_id = uuid::Uuid::new_v4().to_string();
    let _ = sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('telemetry_installation_id', $1) \
         ON CONFLICT (key) DO NOTHING",
    )
    .bind(&new_id)
    .execute(pool)
    .await;

    new_id
}

/// Helper to read a setting value.
async fn get_setting(pool: &PgPool, key: &str) -> String {
    sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key = $1")
        .bind(key)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
}
