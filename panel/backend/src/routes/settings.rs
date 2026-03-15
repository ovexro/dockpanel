use axum::{
    extract::State,
    http::StatusCode,
    Json,
};
use std::collections::HashMap;

use crate::auth::AdminUser;
use crate::error::{err, agent_error, ApiError};
use crate::services::activity;
use crate::AppState;

#[derive(sqlx::FromRow)]
struct SettingRow {
    key: String,
    value: String,
}

/// GET /api/settings — Returns all settings as a key/value map (admin only).
pub async fn list(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<HashMap<String, String>>, ApiError> {

    let rows: Vec<SettingRow> = sqlx::query_as("SELECT key, value FROM settings")
        .fetch_all(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let map: HashMap<String, String> = rows
        .into_iter()
        .map(|r| {
            if (r.key == "smtp_password" || r.key == "pdns_api_key") && !r.value.is_empty() {
                (r.key, "********".to_string())
            } else {
                (r.key, r.value)
            }
        })
        .collect();

    Ok(Json(map))
}

/// PUT /api/settings — Upsert settings from key/value map (admin only).
pub async fn update(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Json(body): Json<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, ApiError> {

    // Whitelist allowed setting keys
    let allowed_keys = [
        "panel_name", "smtp_host", "smtp_port", "smtp_username", "smtp_password",
        "smtp_from", "smtp_from_name", "smtp_encryption",
        "stripe_price_starter", "stripe_price_pro", "stripe_price_agency",
        "agent_latest_version", "agent_download_url", "agent_checksum",
        "pdns_api_url", "pdns_api_key",
    ];
    for key in body.keys() {
        if !allowed_keys.contains(&key.as_str()) {
            return Err(err(StatusCode::BAD_REQUEST, &format!("Unknown setting: {key}")));
        }
    }

    // Update all settings atomically in a transaction
    let mut tx = state.db.begin().await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    for (key, value) in &body {
        sqlx::query(
            "INSERT INTO settings (key, value, updated_at) VALUES ($1, $2, NOW()) \
             ON CONFLICT (key) DO UPDATE SET value = $2, updated_at = NOW()",
        )
        .bind(key)
        .bind(value)
        .execute(&mut *tx)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    }

    tx.commit().await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    tracing::info!("Settings updated by {}: {} keys", claims.email, body.len());

    // If SMTP keys were updated, push config to agent
    let smtp_keys = ["smtp_host", "smtp_port", "smtp_username", "smtp_password", "smtp_from", "smtp_from_name", "smtp_encryption"];
    if body.keys().any(|k| smtp_keys.contains(&k.as_str())) {
        // Fetch all SMTP settings to send complete config
        let rows: Vec<SettingRow> = sqlx::query_as("SELECT key, value FROM settings WHERE key LIKE 'smtp_%'")
            .fetch_all(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

        let map: HashMap<String, String> = rows.into_iter().map(|r| (r.key, r.value)).collect();

        let host = map.get("smtp_host").cloned().unwrap_or_default();
        if !host.is_empty() {
            let port_str = map.get("smtp_port").cloned().unwrap_or_else(|| "587".to_string());
            let port: u16 = port_str.parse().unwrap_or(587);

            let agent_body = serde_json::json!({
                "host": host,
                "port": port,
                "username": map.get("smtp_username").cloned().unwrap_or_default(),
                "password": map.get("smtp_password").cloned().unwrap_or_default(),
                "from": map.get("smtp_from").cloned().unwrap_or_default(),
                "from_name": map.get("smtp_from_name").cloned().unwrap_or_else(|| "DockPanel".to_string()),
                "encryption": map.get("smtp_encryption").cloned().unwrap_or_else(|| "starttls".to_string()),
            });

            if let Err(e) = state.agent.post("/smtp/configure", Some(agent_body)).await {
                tracing::warn!("Failed to configure SMTP on agent: {e}");
            }
        }
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/settings/smtp/test — Send a test email (admin only).
pub async fn test_email(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Json(body): Json<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, ApiError> {

    let to = body.get("to").cloned().unwrap_or_else(|| claims.email.clone());
    if to.is_empty() || !to.contains('@') {
        return Err(err(StatusCode::BAD_REQUEST, "Valid email address required"));
    }

    // Get stored from address
    let rows: Vec<SettingRow> = sqlx::query_as("SELECT key, value FROM settings WHERE key LIKE 'smtp_%'")
        .fetch_all(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let map: HashMap<String, String> = rows.into_iter().map(|r| (r.key, r.value)).collect();
    let from = map.get("smtp_from").cloned().unwrap_or_default();
    let from_name = map.get("smtp_from_name").cloned().unwrap_or_else(|| "DockPanel".to_string());

    if from.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "SMTP not configured — save SMTP settings first"));
    }

    let agent_body = serde_json::json!({
        "to": to,
        "from": from,
        "from_name": from_name,
    });

    let result = state.agent
        .post("/smtp/test", Some(agent_body))
        .await
        .map_err(|e| agent_error("SMTP test email", e))?;

    let message = result.get("message").and_then(|v| v.as_str()).unwrap_or("Email sent");

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "smtp.test",
        Some("settings"), None, Some(&to), None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true, "message": message })))
}

/// POST /api/settings/test-webhook — Test Slack/Discord webhook
pub async fn test_webhook(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Json(body): Json<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let url = body.get("url").ok_or_else(|| err(StatusCode::BAD_REQUEST, "URL required"))?;
    let service = body.get("service").unwrap_or(&"webhook".to_string()).clone();

    if url.is_empty() || !url.starts_with("https://") {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid webhook URL"));
    }

    let payload = if service == "slack" {
        serde_json::json!({ "text": "DockPanel test notification — your Slack webhook is working!" })
    } else {
        serde_json::json!({ "content": "DockPanel test notification — your Discord webhook is working!" })
    };

    let client = reqwest::Client::new();
    let resp = client.post(url).json(&payload).send().await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Webhook request failed: {e}")))?;

    if !resp.status().is_success() {
        return Err(err(StatusCode::BAD_GATEWAY, &format!("Webhook returned {}", resp.status())));
    }

    Ok(Json(serde_json::json!({ "ok": true, "message": format!("{} test sent", service) })))
}

/// GET /api/settings/health — System health check (admin only).
pub async fn health(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {

    // Check DB
    let db_status = match sqlx::query("SELECT 1").execute(&state.db).await {
        Ok(_) => "ok",
        Err(_) => "error",
    };

    // Check agent connectivity
    let agent_status = match state.agent.get("/health").await {
        Ok(_) => "ok",
        Err(_) => "error",
    };

    // System uptime from /proc/uptime
    let uptime = match tokio::fs::read_to_string("/proc/uptime").await {
        Ok(contents) => {
            let secs: f64 = contents
                .split_whitespace()
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0);
            let days = (secs / 86400.0) as u64;
            let hours = ((secs % 86400.0) / 3600.0) as u64;
            let minutes = ((secs % 3600.0) / 60.0) as u64;
            if days > 0 {
                format!("{days} days, {hours}h {minutes}m")
            } else {
                format!("{hours}h {minutes}m")
            }
        }
        Err(_) => "unknown".to_string(),
    };

    Ok(Json(serde_json::json!({
        "db": db_status,
        "agent": agent_status,
        "uptime": uptime,
    })))
}
