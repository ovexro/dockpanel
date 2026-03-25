use axum::{
    extract::State,
    http::StatusCode,
    Json,
};
use std::collections::HashMap;

use crate::auth::{AdminUser, ServerScope};
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
            if (r.key == "smtp_password" || r.key == "pdns_api_key"
                || r.key == "oauth_google_client_secret"
                || r.key == "oauth_github_client_secret"
                || r.key == "oauth_gitlab_client_secret") && !r.value.is_empty() {
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
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, ApiError> {

    // Whitelist allowed setting keys
    let allowed_keys = [
        "panel_name", "smtp_host", "smtp_port", "smtp_username", "smtp_password",
        "smtp_from", "smtp_from_name", "smtp_encryption",
        "stripe_price_starter", "stripe_price_pro", "stripe_price_agency",
        "agent_latest_version", "agent_download_url", "agent_checksum",
        "pdns_api_url", "pdns_api_key",
        "auto_heal_enabled", "status_page_enabled", "enforce_2fa",
        "timezone", "logo_url", "accent_color",
        "email_footer", "events_webhook_url",
        "oauth_google_client_id", "oauth_google_client_secret",
        "oauth_github_client_id", "oauth_github_client_secret",
        "oauth_gitlab_client_id", "oauth_gitlab_client_secret",
        "oauth_auto_create", "hide_branding",
        "reverse_proxy",
        // Gap #70: Customizable notification templates
        "notif_template_email", "notif_template_slack",
        "notif_template_discord", "notif_template_webhook",
    ];
    for key in body.keys() {
        if !allowed_keys.contains(&key.as_str()) {
            return Err(err(StatusCode::BAD_REQUEST, &format!("Unknown setting: {key}")));
        }
    }

    // Validate logo_url
    if let Some(url) = body.get("logo_url") {
        if !url.is_empty() && !url.starts_with("https://") && !url.starts_with("http://") && !url.starts_with("/") {
            return Err(err(StatusCode::BAD_REQUEST, "logo_url must be an HTTP(S) URL or relative path"));
        }
    }

    // Validate accent_color
    if let Some(color) = body.get("accent_color") {
        if !color.is_empty() {
            let valid = color.starts_with('#') && color.len() <= 9 && color[1..].chars().all(|c| c.is_ascii_hexdigit());
            let valid = valid || color.starts_with("rgb") || color.starts_with("hsl");
            if !valid {
                return Err(err(StatusCode::BAD_REQUEST, "accent_color must be a valid hex color (#rrggbb), rgb(), or hsl()"));
            }
        }
    }

    // Update all settings atomically in a transaction
    let mut tx = state.db.begin().await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    // Sensitive keys that are masked in the GET response — skip if value is the mask sentinel
    const SENSITIVE_KEYS: &[&str] = &["smtp_password", "pdns_api_key"];

    for (key, value) in &body {
        // Don't overwrite real secrets with the mask placeholder
        if SENSITIVE_KEYS.contains(&key.as_str()) && value == "********" {
            continue;
        }
        if key.ends_with("_client_secret") && value == "********" {
            continue;
        }

        // Encrypt sensitive values before storing
        let store_value = if SENSITIVE_KEYS.contains(&key.as_str()) || key.ends_with("_client_secret") {
            if value.is_empty() {
                value.clone()
            } else {
                crate::services::secrets_crypto::encrypt_credential(value, &state.config.jwt_secret)
                    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Encryption failed: {e}")))?
            }
        } else {
            value.clone()
        };

        sqlx::query(
            "INSERT INTO settings (key, value, updated_at) VALUES ($1, $2, NOW()) \
             ON CONFLICT (key) DO UPDATE SET value = $2, updated_at = NOW()",
        )
        .bind(key)
        .bind(&store_value)
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

            // Decrypt smtp_password before sending to agent
            let smtp_password_raw = map.get("smtp_password").cloned().unwrap_or_default();
            let smtp_password = crate::services::secrets_crypto::decrypt_credential_or_legacy(
                &smtp_password_raw, &state.config.jwt_secret,
            );

            let agent_body = serde_json::json!({
                "host": host,
                "port": port,
                "username": map.get("smtp_username").cloned().unwrap_or_default(),
                "password": smtp_password,
                "from": map.get("smtp_from").cloned().unwrap_or_default(),
                "from_name": map.get("smtp_from_name").cloned().unwrap_or_else(|| "DockPanel".to_string()),
                "encryption": map.get("smtp_encryption").cloned().unwrap_or_else(|| "starttls".to_string()),
            });

            if let Err(e) = agent.post("/smtp/configure", Some(agent_body)).await {
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
    ServerScope(_server_id, agent): ServerScope,
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

    let result = agent
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

/// GET /api/branding — Public branding configuration (for login page + authenticated users).
/// Returns reseller branding if user belongs to one, otherwise global settings.
pub async fn branding(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Load global branding settings
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT key, value FROM settings WHERE key IN ('panel_name', 'logo_url', 'accent_color', 'hide_branding')"
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let map: HashMap<String, String> = rows.into_iter().collect();

    let global_name = map.get("panel_name").cloned().unwrap_or_else(|| "DockPanel".into());
    let global_logo = map.get("logo_url").cloned().unwrap_or_default();
    let global_accent = map.get("accent_color").cloned().unwrap_or_default();
    let global_hide = map.get("hide_branding").map(|v| v == "true").unwrap_or(false);

    // GAP 41: Check if authenticated user belongs to a reseller with custom branding
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .or_else(|| {
            headers
                .get(axum::http::header::COOKIE)
                .and_then(|v| v.to_str().ok())
                .and_then(|cookies| {
                    cookies.split(';').find_map(|s| s.trim().strip_prefix("token="))
                })
        });

    if let Some(token) = token {
        // Try to decode JWT — ignore errors (unauthenticated users just get global branding)
        let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
        validation.validate_exp = true;
        validation.leeway = 0;
        if let Ok(data) = jsonwebtoken::decode::<crate::auth::Claims>(
            token,
            &jsonwebtoken::DecodingKey::from_secret(state.config.jwt_secret.as_bytes()),
            &validation,
        ) {
            // Look up user's reseller_id and reseller branding
            let reseller_branding: Option<(Option<String>, Option<String>, Option<String>, bool)> = sqlx::query_as(
                "SELECT rp.logo_url, rp.accent_color, rp.panel_name, rp.hide_branding \
                 FROM reseller_profiles rp \
                 JOIN users u ON u.reseller_id = rp.user_id \
                 WHERE u.id = $1"
            )
            .bind(data.claims.sub)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();

            if let Some((logo, accent, name, hide)) = reseller_branding {
                if logo.is_some() || accent.is_some() || name.is_some() || hide {
                    // Check which OAuth providers are configured
                    let oauth_rows: Vec<(String, String)> = sqlx::query_as(
                        "SELECT key, value FROM settings WHERE key LIKE 'oauth_%_client_id' AND value != ''"
                    )
                    .fetch_all(&state.db)
                    .await
                    .unwrap_or_default();

                    let oauth_providers: Vec<String> = oauth_rows.iter()
                        .filter_map(|(k, _)| {
                            k.strip_prefix("oauth_")
                                .and_then(|s| s.strip_suffix("_client_id"))
                                .map(|s| s.to_string())
                        })
                        .collect();

                    return Ok(Json(serde_json::json!({
                        "panel_name": name.unwrap_or(global_name),
                        "logo_url": logo.unwrap_or(global_logo),
                        "accent_color": accent.unwrap_or(global_accent),
                        "hide_branding": hide,
                        "oauth_providers": oauth_providers,
                    })));
                }
            }
        }
    }

    // Check which OAuth providers are configured
    let oauth_rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT key, value FROM settings WHERE key LIKE 'oauth_%_client_id' AND value != ''"
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let oauth_providers: Vec<String> = oauth_rows.iter()
        .filter_map(|(k, _)| {
            k.strip_prefix("oauth_")
                .and_then(|s| s.strip_suffix("_client_id"))
                .map(|s| s.to_string())
        })
        .collect();

    Ok(Json(serde_json::json!({
        "panel_name": global_name,
        "logo_url": global_logo,
        "accent_color": global_accent,
        "hide_branding": global_hide,
        "oauth_providers": oauth_providers,
    })))
}

/// GET /api/settings/export — Export all panel settings, alert rules, monitors,
/// backup schedules, and backup policies as JSON (Gap #71).
pub async fn export_config(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let rows: Vec<SettingRow> = sqlx::query_as("SELECT key, value FROM settings")
        .fetch_all(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let map: HashMap<String, String> = rows
        .into_iter()
        .filter(|r| r.key != "smtp_password" && r.key != "pdns_api_key"
            && !r.key.ends_with("_client_secret"))
        .map(|r| (r.key, r.value))
        .collect();

    // Gap #71: Export alert rules (user's own rules only, exclude webhook secrets)
    let alert_rule_rows = sqlx::query(
        "SELECT server_id, cpu_threshold, cpu_duration, memory_threshold, memory_duration, \
         disk_threshold, alert_cpu, alert_memory, alert_disk, alert_offline, \
         alert_backup_failure, alert_ssl_expiry, alert_service_health, \
         ssl_warning_days, notify_email, cooldown_minutes, muted_types \
         FROM alert_rules WHERE user_id = $1 ORDER BY server_id NULLS FIRST"
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let alert_rules: Vec<serde_json::Value> = alert_rule_rows.iter().map(|r| {
        use sqlx::Row;
        serde_json::json!({
            "server_id": r.get::<Option<uuid::Uuid>, _>("server_id"),
            "cpu_threshold": r.get::<i32, _>("cpu_threshold"),
            "cpu_duration": r.get::<i32, _>("cpu_duration"),
            "memory_threshold": r.get::<i32, _>("memory_threshold"),
            "memory_duration": r.get::<i32, _>("memory_duration"),
            "disk_threshold": r.get::<i32, _>("disk_threshold"),
            "alert_cpu": r.get::<bool, _>("alert_cpu"),
            "alert_memory": r.get::<bool, _>("alert_memory"),
            "alert_disk": r.get::<bool, _>("alert_disk"),
            "alert_offline": r.get::<bool, _>("alert_offline"),
            "alert_backup_failure": r.get::<bool, _>("alert_backup_failure"),
            "alert_ssl_expiry": r.get::<bool, _>("alert_ssl_expiry"),
            "alert_service_health": r.get::<bool, _>("alert_service_health"),
            "ssl_warning_days": r.get::<String, _>("ssl_warning_days"),
            "notify_email": r.get::<bool, _>("notify_email"),
            "cooldown_minutes": r.get::<i32, _>("cooldown_minutes"),
            "muted_types": r.get::<String, _>("muted_types"),
        })
    }).collect();

    // Gap #71: Export monitors (name, url, type, interval, keyword — no secrets)
    let monitor_rows = sqlx::query(
        "SELECT name, url, monitor_type, check_interval, keyword \
         FROM monitors WHERE user_id = $1 ORDER BY name"
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let monitors: Vec<serde_json::Value> = monitor_rows.iter().map(|r| {
        use sqlx::Row;
        serde_json::json!({
            "name": r.get::<String, _>("name"),
            "url": r.get::<String, _>("url"),
            "monitor_type": r.get::<String, _>("monitor_type"),
            "check_interval": r.get::<i32, _>("check_interval"),
            "keyword": r.get::<Option<String>, _>("keyword"),
        })
    }).collect();

    // Gap #71: Export backup schedules
    let schedule_rows = sqlx::query(
        "SELECT site_id, schedule, retention_count, enabled FROM backup_schedules"
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let schedules: Vec<serde_json::Value> = schedule_rows.iter().map(|r| {
        use sqlx::Row;
        serde_json::json!({
            "site_id": r.get::<uuid::Uuid, _>("site_id"),
            "schedule": r.get::<String, _>("schedule"),
            "retention_count": r.get::<i32, _>("retention_count"),
            "enabled": r.get::<bool, _>("enabled"),
        })
    }).collect();

    // Gap #71: Export backup policies
    let policy_rows = sqlx::query(
        "SELECT name, schedule, backup_sites, backup_databases, backup_volumes, \
         retention_count, encrypt, verify_after_backup \
         FROM backup_policies WHERE user_id = $1 ORDER BY name"
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let policies: Vec<serde_json::Value> = policy_rows.iter().map(|r| {
        use sqlx::Row;
        serde_json::json!({
            "name": r.get::<String, _>("name"),
            "schedule": r.get::<String, _>("schedule"),
            "backup_sites": r.get::<bool, _>("backup_sites"),
            "backup_databases": r.get::<bool, _>("backup_databases"),
            "backup_volumes": r.get::<bool, _>("backup_volumes"),
            "retention_count": r.get::<i32, _>("retention_count"),
            "encrypt": r.get::<bool, _>("encrypt"),
            "verify_after_backup": r.get::<bool, _>("verify_after_backup"),
        })
    }).collect();

    Ok(Json(serde_json::json!({
        "settings": map,
        "alert_rules": alert_rules,
        "monitors": monitors,
        "backup_schedules": schedules,
        "backup_policies": policies,
        "exported_at": chrono::Utc::now().to_rfc3339(),
    })))
}

/// POST /api/settings/import — Import panel settings from JSON.
pub async fn import_config(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let settings_obj = body.get("settings").and_then(|s| s.as_object())
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Invalid format: missing 'settings' object"))?;

    // Filter imported settings through the same whitelist as update()
    let allowed_keys = [
        "panel_name", "smtp_host", "smtp_port", "smtp_username", "smtp_password",
        "smtp_from", "smtp_from_name", "smtp_encryption",
        "stripe_price_starter", "stripe_price_pro", "stripe_price_agency",
        "agent_latest_version", "agent_download_url", "agent_checksum",
        "pdns_api_url", "pdns_api_key",
        "auto_heal_enabled", "status_page_enabled", "enforce_2fa",
        "timezone", "logo_url", "accent_color",
        "email_footer", "events_webhook_url",
        "oauth_google_client_id", "oauth_google_client_secret",
        "oauth_github_client_id", "oauth_github_client_secret",
        "oauth_gitlab_client_id", "oauth_gitlab_client_secret",
        "oauth_auto_create", "hide_branding",
        "reverse_proxy",
        "notif_template_email", "notif_template_slack",
        "notif_template_discord", "notif_template_webhook",
    ];

    const SENSITIVE_KEYS: &[&str] = &["smtp_password", "pdns_api_key"];

    let mut imported = 0;
    let mut skipped = 0;
    for (key, value) in settings_obj {
        if !allowed_keys.contains(&key.as_str()) {
            skipped += 1;
            continue; // Skip disallowed keys
        }
        if let Some(val) = value.as_str() {
            // Encrypt sensitive values before storing (same logic as update())
            let store_value = if SENSITIVE_KEYS.contains(&key.as_str()) || key.ends_with("_client_secret") {
                if val.is_empty() {
                    val.to_string()
                } else {
                    crate::services::secrets_crypto::encrypt_credential(val, &state.config.jwt_secret)
                        .unwrap_or_else(|_| val.to_string())
                }
            } else {
                val.to_string()
            };

            sqlx::query(
                "INSERT INTO settings (key, value, updated_at) VALUES ($1, $2, NOW()) \
                 ON CONFLICT (key) DO UPDATE SET value = $2, updated_at = NOW()",
            )
            .bind(key)
            .bind(&store_value)
            .execute(&state.db)
            .await
            .ok();
            imported += 1;
        }
    }

    // Import alert rules
    let mut alert_rules_imported = 0i64;
    if let Some(rules) = body.get("alert_rules").and_then(|v| v.as_array()) {
        for rule in rules {
            let cpu_threshold = rule.get("cpu_threshold").and_then(|v| v.as_i64()).unwrap_or(90) as i32;
            let cpu_duration = rule.get("cpu_duration").and_then(|v| v.as_i64()).unwrap_or(5) as i32;
            let mem_threshold = rule.get("memory_threshold").and_then(|v| v.as_i64()).unwrap_or(90) as i32;
            let mem_duration = rule.get("memory_duration").and_then(|v| v.as_i64()).unwrap_or(5) as i32;
            let disk_threshold = rule.get("disk_threshold").and_then(|v| v.as_i64()).unwrap_or(90) as i32;
            let alert_cpu = rule.get("alert_cpu").and_then(|v| v.as_bool()).unwrap_or(true);
            let alert_memory = rule.get("alert_memory").and_then(|v| v.as_bool()).unwrap_or(true);
            let alert_disk = rule.get("alert_disk").and_then(|v| v.as_bool()).unwrap_or(true);
            let alert_offline = rule.get("alert_offline").and_then(|v| v.as_bool()).unwrap_or(true);
            let alert_backup_failure = rule.get("alert_backup_failure").and_then(|v| v.as_bool()).unwrap_or(false);
            let alert_ssl_expiry = rule.get("alert_ssl_expiry").and_then(|v| v.as_bool()).unwrap_or(false);
            let alert_service_health = rule.get("alert_service_health").and_then(|v| v.as_bool()).unwrap_or(false);
            let ssl_warning_days = rule.get("ssl_warning_days").and_then(|v| v.as_str()).unwrap_or("14");
            let notify_email = rule.get("notify_email").and_then(|v| v.as_bool()).unwrap_or(true);
            let cooldown = rule.get("cooldown_minutes").and_then(|v| v.as_i64()).unwrap_or(15) as i32;
            let muted_types = rule.get("muted_types").and_then(|v| v.as_str()).unwrap_or("");

            // Upsert: if server_id is null, update the global (server_id IS NULL) rule
            let server_id: Option<uuid::Uuid> = rule
                .get("server_id")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok());

            let result = if let Some(sid) = server_id {
                sqlx::query(
                    "INSERT INTO alert_rules (user_id, server_id, cpu_threshold, cpu_duration, \
                     memory_threshold, memory_duration, disk_threshold, alert_cpu, alert_memory, \
                     alert_disk, alert_offline, alert_backup_failure, alert_ssl_expiry, \
                     alert_service_health, ssl_warning_days, notify_email, cooldown_minutes, muted_types) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18) \
                     ON CONFLICT (user_id, server_id) DO UPDATE SET \
                     cpu_threshold=$3, cpu_duration=$4, memory_threshold=$5, memory_duration=$6, \
                     disk_threshold=$7, alert_cpu=$8, alert_memory=$9, alert_disk=$10, \
                     alert_offline=$11, alert_backup_failure=$12, alert_ssl_expiry=$13, \
                     alert_service_health=$14, ssl_warning_days=$15, notify_email=$16, \
                     cooldown_minutes=$17, muted_types=$18"
                )
                .bind(claims.sub).bind(sid)
                .bind(cpu_threshold).bind(cpu_duration)
                .bind(mem_threshold).bind(mem_duration)
                .bind(disk_threshold)
                .bind(alert_cpu).bind(alert_memory).bind(alert_disk)
                .bind(alert_offline).bind(alert_backup_failure)
                .bind(alert_ssl_expiry).bind(alert_service_health)
                .bind(ssl_warning_days).bind(notify_email)
                .bind(cooldown).bind(muted_types)
                .execute(&state.db).await
            } else {
                sqlx::query(
                    "INSERT INTO alert_rules (user_id, cpu_threshold, cpu_duration, \
                     memory_threshold, memory_duration, disk_threshold, alert_cpu, alert_memory, \
                     alert_disk, alert_offline, alert_backup_failure, alert_ssl_expiry, \
                     alert_service_health, ssl_warning_days, notify_email, cooldown_minutes, muted_types) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17) \
                     ON CONFLICT DO NOTHING"
                )
                .bind(claims.sub)
                .bind(cpu_threshold).bind(cpu_duration)
                .bind(mem_threshold).bind(mem_duration)
                .bind(disk_threshold)
                .bind(alert_cpu).bind(alert_memory).bind(alert_disk)
                .bind(alert_offline).bind(alert_backup_failure)
                .bind(alert_ssl_expiry).bind(alert_service_health)
                .bind(ssl_warning_days).bind(notify_email)
                .bind(cooldown).bind(muted_types)
                .execute(&state.db).await
            };
            if result.is_ok() {
                alert_rules_imported += 1;
            }
        }
    }

    // Import monitors
    let mut monitors_imported = 0i64;
    if let Some(monitors) = body.get("monitors").and_then(|v| v.as_array()) {
        for m in monitors {
            let name = m.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let url = m.get("url").and_then(|v| v.as_str()).unwrap_or("");
            let monitor_type = m.get("monitor_type").and_then(|v| v.as_str()).unwrap_or("http");
            let interval = m.get("check_interval").and_then(|v| v.as_i64()).unwrap_or(60) as i32;
            let keyword = m.get("keyword").and_then(|v| v.as_str());

            if !name.is_empty() && !url.is_empty() {
                let result = sqlx::query(
                    "INSERT INTO monitors (user_id, name, url, monitor_type, check_interval, keyword) \
                     VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
                )
                .bind(claims.sub)
                .bind(name)
                .bind(url)
                .bind(monitor_type)
                .bind(interval)
                .bind(keyword)
                .execute(&state.db)
                .await;
                if result.is_ok() {
                    monitors_imported += 1;
                }
            }
        }
    }

    // Import backup schedules
    let mut schedules_imported = 0i64;
    if let Some(schedules) = body.get("backup_schedules").and_then(|v| v.as_array()) {
        for s in schedules {
            let site_id: Option<uuid::Uuid> = s
                .get("site_id")
                .and_then(|v| v.as_str())
                .and_then(|v| v.parse().ok());
            let schedule = s.get("schedule").and_then(|v| v.as_str()).unwrap_or("0 2 * * *");
            let retention = s.get("retention_count").and_then(|v| v.as_i64()).unwrap_or(7) as i32;
            let enabled = s.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);

            if let Some(sid) = site_id {
                let result = sqlx::query(
                    "INSERT INTO backup_schedules (site_id, schedule, retention_count, enabled) \
                     VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
                )
                .bind(sid)
                .bind(schedule)
                .bind(retention)
                .bind(enabled)
                .execute(&state.db)
                .await;
                if result.is_ok() {
                    schedules_imported += 1;
                }
            }
        }
    }

    // Import backup policies
    let mut policies_imported = 0i64;
    if let Some(policies) = body.get("backup_policies").and_then(|v| v.as_array()) {
        for p in policies {
            let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let schedule = p.get("schedule").and_then(|v| v.as_str()).unwrap_or("0 2 * * *");
            let backup_sites = p.get("backup_sites").and_then(|v| v.as_bool()).unwrap_or(true);
            let backup_databases = p.get("backup_databases").and_then(|v| v.as_bool()).unwrap_or(true);
            let backup_volumes = p.get("backup_volumes").and_then(|v| v.as_bool()).unwrap_or(false);
            let retention = p.get("retention_count").and_then(|v| v.as_i64()).unwrap_or(7) as i32;
            let encrypt = p.get("encrypt").and_then(|v| v.as_bool()).unwrap_or(false);
            let verify = p.get("verify_after_backup").and_then(|v| v.as_bool()).unwrap_or(false);

            if !name.is_empty() {
                let result = sqlx::query(
                    "INSERT INTO backup_policies (user_id, name, schedule, backup_sites, backup_databases, \
                     backup_volumes, retention_count, encrypt, verify_after_backup) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) ON CONFLICT DO NOTHING",
                )
                .bind(claims.sub)
                .bind(name)
                .bind(schedule)
                .bind(backup_sites)
                .bind(backup_databases)
                .bind(backup_volumes)
                .bind(retention)
                .bind(encrypt)
                .bind(verify)
                .execute(&state.db)
                .await;
                if result.is_ok() {
                    policies_imported += 1;
                }
            }
        }
    }

    crate::services::activity::log_activity(
        &state.db, claims.sub, &claims.email, "settings.import",
        Some("settings"), None, None, None,
    ).await;

    Ok(Json(serde_json::json!({
        "ok": true,
        "imported": imported,
        "skipped": skipped,
        "alert_rules_imported": alert_rules_imported,
        "monitors_imported": monitors_imported,
        "schedules_imported": schedules_imported,
        "policies_imported": policies_imported,
    })))
}

/// GET /api/settings/health — System health check (admin only).
pub async fn health(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {

    // Check DB
    let db_status = match sqlx::query("SELECT 1").execute(&state.db).await {
        Ok(_) => "ok",
        Err(_) => "error",
    };

    // Check agent connectivity
    let agent_status = match agent.get("/health").await {
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
