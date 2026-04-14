use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::auth::AdminUser;
use crate::error::{internal_error, err, ApiError};
use crate::services::activity;
use crate::AppState;

// ---------------------------------------------------------------------------
// SSRF protection: validate webhook URLs
// ---------------------------------------------------------------------------

async fn validate_webhook_url(url: &str) -> Result<(), String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("webhook_url is required".to_string());
    }
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err("webhook_url must use http or https".to_string());
    }

    // Extract host from URL (strip scheme, take up to next / or :)
    let after_scheme = if url.starts_with("https://") { &url[8..] } else { &url[7..] };
    let host = after_scheme
        .split('/')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");

    if host.is_empty() {
        return Err("webhook_url has no hostname".to_string());
    }

    // Resolve hostname to IP addresses and check each one.
    // This prevents bypasses via hex IPs (0x7f000001), decimal IPs (2130706433),
    // IPv6 (::1), DNS names that resolve to localhost (e.g. localtest.me),
    // and cloud metadata endpoints (169.254.169.254).
    let lookup_host = format!("{}:80", host.trim_matches(|c| c == '[' || c == ']'));
    match tokio::net::lookup_host(&lookup_host).await {
        Ok(addrs) => {
            for addr in addrs {
                let ip = addr.ip();
                if ip.is_loopback() || ip.is_unspecified() {
                    return Err("webhook_url resolves to loopback address".to_string());
                }
                match ip {
                    std::net::IpAddr::V4(v4) => {
                        if v4.is_private() || v4.is_link_local() || v4.octets()[0] == 169 {
                            return Err(
                                "webhook_url resolves to private/link-local address".to_string(),
                            );
                        }
                    }
                    std::net::IpAddr::V6(v6) => {
                        if v6.is_loopback() {
                            return Err(
                                "webhook_url resolves to loopback address".to_string(),
                            );
                        }
                    }
                }
            }
        }
        Err(_) => {
            return Err("webhook_url hostname could not be resolved".to_string());
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Models
// ---------------------------------------------------------------------------

#[derive(serde::Serialize, sqlx::FromRow)]
#[allow(dead_code)]
pub struct Extension {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub description: String,
    pub author: String,
    pub version: String,
    pub webhook_url: String,
    #[serde(skip_serializing)]
    pub webhook_secret: String,
    #[serde(skip_serializing)]
    pub api_key_hash: Option<String>,
    pub api_key_prefix: Option<String>,
    pub enabled: bool,
    pub event_subscriptions: String,
    pub api_scopes: String,
    pub last_webhook_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_webhook_status: Option<i32>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct ExtensionEvent {
    pub id: Uuid,
    pub extension_id: Uuid,
    pub event_type: String,
    pub payload: String,
    pub response_status: Option<i32>,
    pub response_body: Option<String>,
    pub duration_ms: Option<i32>,
    pub delivered_at: chrono::DateTime<chrono::Utc>,
}

// ---------------------------------------------------------------------------
// Request bodies
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
pub struct CreateExtensionRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default = "default_version")]
    pub version: String,
    pub webhook_url: String,
    #[serde(default)]
    pub event_subscriptions: String,
    #[serde(default)]
    pub api_scopes: String,
}

fn default_version() -> String {
    "1.0.0".to_string()
}

#[derive(serde::Deserialize)]
pub struct UpdateExtensionRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub webhook_url: Option<String>,
    pub enabled: Option<bool>,
    pub event_subscriptions: Option<String>,
    pub api_scopes: Option<String>,
}

// ---------------------------------------------------------------------------
// GET /api/extensions — List all extensions (admin only).
// ---------------------------------------------------------------------------

pub async fn list(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<Vec<Extension>>, ApiError> {
    let rows: Vec<Extension> = sqlx::query_as(
        "SELECT id, user_id, name, description, author, version, webhook_url, \
         webhook_secret, api_key_hash, api_key_prefix, enabled, event_subscriptions, \
         api_scopes, last_webhook_at, last_webhook_status, created_at, updated_at \
         FROM extensions ORDER BY created_at DESC",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("list extensions", e))?;

    Ok(Json(rows))
}

// ---------------------------------------------------------------------------
// POST /api/extensions — Create a new extension (admin only).
// Returns the API key + webhook secret ONCE.
// ---------------------------------------------------------------------------

pub async fn create(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Json(body): Json<CreateExtensionRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let name = body.name.trim();
    if name.is_empty() || name.len() > 100 {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Name must be 1-100 characters",
        ));
    }

    if let Err(msg) = validate_webhook_url(&body.webhook_url).await {
        return Err(err(StatusCode::BAD_REQUEST, &msg));
    }

    // Generate dpx_ API key (same pattern as dp_ keys)
    let raw = Uuid::new_v4().to_string().replace('-', "")
        + &Uuid::new_v4().to_string().replace('-', "");
    let api_key = format!("dpx_{raw}");
    let api_key_prefix = api_key[..12].to_string();

    let mut hasher = Sha256::new();
    hasher.update(api_key.as_bytes());
    let api_key_hash = hex::encode(hasher.finalize());

    // Generate webhook secret
    let webhook_secret = format!(
        "whsec_{}",
        Uuid::new_v4().to_string().replace('-', "")
    );

    let ext_id = Uuid::new_v4();

    sqlx::query(
        "INSERT INTO extensions \
         (id, user_id, name, description, author, version, webhook_url, webhook_secret, \
          api_key_hash, api_key_prefix, enabled, event_subscriptions, api_scopes) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, TRUE, $11, $12)",
    )
    .bind(ext_id)
    .bind(claims.sub)
    .bind(name)
    .bind(&body.description)
    .bind(&body.author)
    .bind(&body.version)
    .bind(body.webhook_url.trim())
    .bind(&webhook_secret)
    .bind(&api_key_hash)
    .bind(&api_key_prefix)
    .bind(&body.event_subscriptions)
    .bind(&body.api_scopes)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("create extensions", e))?;

    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "extension.created",
        Some("extension"),
        Some(name),
        None,
        None,
    )
    .await;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": ext_id,
            "name": name,
            "api_key": api_key,
            "api_key_prefix": api_key_prefix,
            "webhook_secret": webhook_secret,
            "message": "Save these credentials — they won't be shown again.",
        })),
    ))
}

// ---------------------------------------------------------------------------
// PUT /api/extensions/{id} — Update an extension (admin only).
// ---------------------------------------------------------------------------

pub async fn update(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateExtensionRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Verify extension exists
    let existing: Option<(String,)> =
        sqlx::query_as("SELECT name FROM extensions WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("update extensions", e))?;

    if existing.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Extension not found"));
    }

    // Build dynamic UPDATE
    let mut sets = Vec::new();
    let mut idx = 1u32;

    // We'll build a simple approach: update all provided fields
    if let Some(ref name) = body.name {
        let name = name.trim();
        if name.is_empty() || name.len() > 100 {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "Name must be 1-100 characters",
            ));
        }
        sets.push(("name", serde_json::Value::String(name.to_string())));
    }
    if let Some(ref desc) = body.description {
        sets.push((
            "description",
            serde_json::Value::String(desc.clone()),
        ));
    }
    if let Some(ref url) = body.webhook_url {
        if let Err(msg) = validate_webhook_url(url).await {
            return Err(err(StatusCode::BAD_REQUEST, &msg));
        }
        sets.push((
            "webhook_url",
            serde_json::Value::String(url.trim().to_string()),
        ));
    }
    if let Some(enabled) = body.enabled {
        sets.push(("enabled", serde_json::Value::Bool(enabled)));
    }
    if let Some(ref subs) = body.event_subscriptions {
        sets.push((
            "event_subscriptions",
            serde_json::Value::String(subs.clone()),
        ));
    }
    if let Some(ref scopes) = body.api_scopes {
        sets.push((
            "api_scopes",
            serde_json::Value::String(scopes.clone()),
        ));
    }

    if sets.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "No fields to update"));
    }

    // Build query dynamically
    let mut query_parts = Vec::new();
    for (col, _) in &sets {
        idx += 1;
        query_parts.push(format!("{col} = ${idx}"));
    }
    query_parts.push("updated_at = NOW()".to_string());

    let sql = format!(
        "UPDATE extensions SET {} WHERE id = $1",
        query_parts.join(", ")
    );

    let mut q = sqlx::query(&sql).bind(id);
    for (_, val) in &sets {
        match val {
            serde_json::Value::String(s) => q = q.bind(s.as_str()),
            serde_json::Value::Bool(b) => q = q.bind(*b),
            _ => {}
        }
    }

    q.execute(&state.db)
        .await
        .map_err(|e| internal_error("update extensions", e))?;

    let updated_name = body
        .name
        .as_deref()
        .unwrap_or_else(|| existing.as_ref().unwrap().0.as_str());

    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "extension.updated",
        Some("extension"),
        Some(updated_name),
        None,
        None,
    )
    .await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// DELETE /api/extensions/{id} — Delete an extension (admin only).
// ---------------------------------------------------------------------------

pub async fn remove(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Fetch name for activity log
    let ext_info: Option<(String,)> =
        sqlx::query_as("SELECT name FROM extensions WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("remove extensions", e))?;

    let ext_name = ext_info
        .map(|(n,)| n)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Extension not found"))?;

    // Delete events first (FK cascade would handle this, but be explicit)
    let _ = sqlx::query("DELETE FROM extension_events WHERE extension_id = $1")
        .bind(id)
        .execute(&state.db)
        .await;

    let result = sqlx::query("DELETE FROM extensions WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("remove extensions", e))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Extension not found"));
    }

    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "extension.deleted",
        Some("extension"),
        Some(&ext_name),
        None,
        None,
    )
    .await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// POST /api/extensions/{id}/test — Send a test event to an extension's webhook.
// ---------------------------------------------------------------------------

pub async fn test_webhook(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let ext: Option<(String, String, String)> = sqlx::query_as(
        "SELECT name, webhook_url, webhook_secret FROM extensions WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("test webhook", e))?;

    let (name, webhook_url, webhook_secret) =
        ext.ok_or_else(|| err(StatusCode::NOT_FOUND, "Extension not found"))?;

    let delivery_id = Uuid::new_v4().to_string();
    let timestamp = chrono::Utc::now().to_rfc3339();
    let payload = serde_json::json!({
        "event": "test",
        "timestamp": timestamp,
        "delivery_id": delivery_id,
        "data": {
            "message": "This is a test event from DockPanel.",
            "extension": name,
            "triggered_by": claims.email,
        },
    });
    let payload_str = serde_json::to_string(&payload).unwrap_or_default();

    // Compute HMAC-SHA256 signature
    use hmac::{Hmac, Mac};
    type HmacSha256 = Hmac<sha2::Sha256>;
    let signature = match HmacSha256::new_from_slice(webhook_secret.as_bytes()) {
        Ok(mut mac) => {
            mac.update(payload_str.as_bytes());
            hex::encode(mac.finalize().into_bytes())
        }
        Err(_) => {
            return Err(err(StatusCode::INTERNAL_SERVER_ERROR, "HMAC key is invalid — rotate the webhook secret"));
        }
    };

    let started = std::time::Instant::now();
    let result = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_default()
        .post(&webhook_url)
        .header("Content-Type", "application/json")
        .header("X-DockPanel-Event", "test")
        .header("X-DockPanel-Delivery", &delivery_id)
        .header("X-DockPanel-Signature", format!("sha256={signature}"))
        .body(payload_str.clone())
        .send()
        .await;

    let duration = started.elapsed().as_millis() as i32;

    let (status, body) = match result {
        Ok(resp) => {
            let s = resp.status().as_u16() as i32;
            let b = resp
                .text()
                .await
                .unwrap_or_default()
                .chars()
                .take(1024)
                .collect::<String>();
            (Some(s), b)
        }
        Err(e) => (None, format!("Request failed: {e}")),
    };

    // Record the test delivery
    let _ = sqlx::query(
        "INSERT INTO extension_events (extension_id, event_type, payload, response_status, response_body, duration_ms) \
         VALUES ($1, 'test', $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(&payload_str)
    .bind(status)
    .bind(&body)
    .bind(duration)
    .execute(&state.db)
    .await;

    // Update last webhook status
    let _ = sqlx::query(
        "UPDATE extensions SET last_webhook_at = NOW(), last_webhook_status = $1 WHERE id = $2",
    )
    .bind(status)
    .bind(id)
    .execute(&state.db)
    .await;

    let success = status.is_some_and(|s| s < 400);

    Ok(Json(serde_json::json!({
        "ok": success,
        "status": status,
        "duration_ms": duration,
        "response_body": body,
    })))
}

// ---------------------------------------------------------------------------
// POST /api/extensions/{id}/rotate-secret — Rotate webhook secret.
// ---------------------------------------------------------------------------

pub async fn rotate_secret(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Verify extension exists
    let ext: Option<(String,)> =
        sqlx::query_as("SELECT name FROM extensions WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("rotate secret", e))?;

    let ext_name = ext
        .map(|(n,)| n)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Extension not found"))?;

    let new_secret = format!("whsec_{}", Uuid::new_v4().to_string().replace('-', ""));

    sqlx::query("UPDATE extensions SET webhook_secret = $1, updated_at = NOW() WHERE id = $2")
        .bind(&new_secret)
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("rotate secret", e))?;

    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "extension.rotate_secret",
        Some("extension"),
        Some(&ext_name),
        None,
        None,
    )
    .await;

    Ok(Json(serde_json::json!({ "webhook_secret": new_secret })))
}

// ---------------------------------------------------------------------------
// GET /api/extensions/{id}/events — List recent events for an extension.
// ---------------------------------------------------------------------------

pub async fn events(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<ExtensionEvent>>, ApiError> {
    // Verify extension exists
    let exists: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM extensions WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("events", e))?;

    if exists.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Extension not found"));
    }

    let rows: Vec<ExtensionEvent> = sqlx::query_as(
        "SELECT id, extension_id, event_type, payload, response_status, response_body, \
         duration_ms, delivered_at \
         FROM extension_events WHERE extension_id = $1 \
         ORDER BY delivered_at DESC LIMIT 50",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("events", e))?;

    Ok(Json(rows))
}
