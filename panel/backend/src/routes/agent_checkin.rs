use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use std::time::Instant;
use subtle::ConstantTimeEq;

use crate::error::{internal_error, err, ApiError};
use crate::AppState;

#[derive(serde::Deserialize)]
#[allow(dead_code)]
pub struct CheckinRequest {
    pub server_id: String,
    pub os_info: Option<String>,
    pub hostname: Option<String>,
    pub cpu_cores: Option<i32>,
    pub ram_mb: Option<i32>,
    pub disk_gb: Option<i32>,
    pub agent_version: Option<String>,
    pub disk_used_gb: Option<i32>,
    pub disk_usage_pct: Option<f32>,
    // Live metrics (stored in server record for dashboard display)
    pub cpu_usage: Option<f32>,
    pub mem_used_mb: Option<i64>,
    pub uptime_secs: Option<i64>,
    /// Unix timestamp (seconds) from the agent. Replay prevention:
    /// requests older than 120 seconds are rejected.
    pub timestamp: Option<i64>,
    /// SHA-256 hex fingerprint of the agent's inbound TLS cert. Captured on
    /// first checkin (TOFU), validated on subsequent checkins. Mismatch is
    /// treated as a potential MITM or re-provisioned agent; the admin must
    /// rotate the pin via POST /api/servers/{id}/rotate-cert-pin.
    pub cert_fingerprint: Option<String>,
}

/// POST /api/agent/checkin — Agent reports system info and heartbeat.
/// Authenticated via Bearer token matching the server's agent_token.
pub async fn checkin(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CheckinRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Extract Bearer token
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "Missing authorization"))?;

    // Parse server ID
    let server_id: uuid::Uuid = body
        .server_id
        .parse()
        .map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid server_id"))?;

    // Verify token matches server record (hash-based, falls back to plaintext for unmigrated rows)
    let token_hash = crate::helpers::hash_agent_token(token);
    let existing: Option<(Option<String>, String)> =
        sqlx::query_as("SELECT agent_token_hash, agent_token FROM servers WHERE id = $1")
            .bind(server_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("checkin", e))?;

    let (stored_hash, stored_token) =
        existing.ok_or_else(|| err(StatusCode::NOT_FOUND, "Server not found"))?;

    // Constant-time comparison to prevent timing-based token brute force
    let token_valid: bool = match stored_hash {
        Some(ref hash) => hash.as_bytes().ct_eq(token_hash.as_bytes()).into(),
        None => stored_token.as_bytes().ct_eq(token.as_bytes()).into(),
    };
    if !token_valid {
        return Err(err(StatusCode::UNAUTHORIZED, "Invalid token"));
    }

    // Replay prevention: reject requests with stale timestamps (>120s drift).
    // Accepts requests without timestamp for backward compatibility with older agents.
    if let Some(ts) = body.timestamp {
        let now = chrono::Utc::now().timestamp();
        let drift = (now - ts).abs();
        if drift > 120 {
            tracing::warn!(
                "Checkin rejected for server {server_id}: timestamp drift {drift}s (limit 120s)"
            );
            return Err(err(StatusCode::BAD_REQUEST, "Request timestamp too old"));
        }
    }

    // Cert fingerprint pinning (Trust On First Use).
    // - If the agent sends a fingerprint and nothing is stored: capture it.
    // - If the stored fingerprint matches: no-op.
    // - If the stored fingerprint differs: treat as MITM and refuse checkin.
    //   The admin must POST /api/servers/{id}/rotate-cert-pin to re-capture.
    if let Some(fp_raw) = body.cert_fingerprint.as_deref() {
        let fp = fp_raw.trim().to_ascii_lowercase();
        let valid_format = fp.len() == 64 && fp.chars().all(|c| c.is_ascii_hexdigit());
        if !valid_format {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "cert_fingerprint must be a 64-char lowercase hex SHA-256",
            ));
        }
        let (stored,): (Option<String>,) =
            sqlx::query_as("SELECT cert_fingerprint FROM servers WHERE id = $1")
                .bind(server_id)
                .fetch_one(&state.db)
                .await
                .map_err(|e| internal_error("checkin cert lookup", e))?;
        match stored {
            None => {
                // TOFU capture
                sqlx::query("UPDATE servers SET cert_fingerprint = $1 WHERE id = $2")
                    .bind(&fp)
                    .bind(server_id)
                    .execute(&state.db)
                    .await
                    .map_err(|e| internal_error("checkin cert capture", e))?;
                tracing::info!("Captured cert fingerprint for server {server_id}: {fp}");
            }
            Some(existing) if existing.as_bytes().ct_eq(fp.as_bytes()).into() => {
                // Match — nothing to do.
            }
            Some(_) => {
                tracing::error!(
                    "Checkin REJECTED for server {server_id}: cert fingerprint mismatch (possible MITM or agent re-provisioned). Admin must rotate the pin."
                );
                return Err(err(
                    StatusCode::FORBIDDEN,
                    "cert fingerprint mismatch — rotate the pin to accept a new cert",
                ));
            }
        }
    }

    // Rate limit: max 120 requests per minute per server_id
    {
        let mut limits = state.agent_rate_limits.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let entry = limits.entry(server_id).or_insert((0, now));
        if now.duration_since(entry.1).as_secs() >= 60 {
            *entry = (1, now);
        } else {
            entry.0 += 1;
            if entry.0 > 120 {
                return Err(err(StatusCode::TOO_MANY_REQUESTS, "Rate limit exceeded"));
            }
        }
    }

    // Extract client IP
    let ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|s| s.trim().to_string())
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
        });

    // Compute memory usage percentage
    let mem_usage_pct = match (body.mem_used_mb, body.ram_mb) {
        (Some(used), Some(total)) if total > 0 => {
            Some((used as f32 / total as f32) * 100.0)
        }
        _ => None,
    };

    // Update server record
    sqlx::query(
        "UPDATE servers SET \
         status = 'online', \
         last_seen_at = NOW(), \
         ip_address = COALESCE($2, ip_address), \
         os_info = COALESCE($3, os_info), \
         cpu_cores = COALESCE($4, cpu_cores), \
         ram_mb = COALESCE($5, ram_mb), \
         disk_gb = COALESCE($6, disk_gb), \
         agent_version = COALESCE($7, agent_version), \
         cpu_usage = COALESCE($8, cpu_usage), \
         mem_used_mb = COALESCE($9, mem_used_mb), \
         uptime_secs = COALESCE($10, uptime_secs), \
         disk_used_gb = COALESCE($11, disk_used_gb), \
         disk_usage_pct = COALESCE($12, disk_usage_pct), \
         mem_usage_pct = COALESCE($13, mem_usage_pct) \
         WHERE id = $1",
    )
    .bind(server_id)
    .bind(&ip)
    .bind(&body.os_info)
    .bind(body.cpu_cores)
    .bind(body.ram_mb)
    .bind(body.disk_gb)
    .bind(&body.agent_version)
    .bind(body.cpu_usage)
    .bind(body.mem_used_mb)
    .bind(body.uptime_secs)
    .bind(body.disk_used_gb)
    .bind(body.disk_usage_pct)
    .bind(mem_usage_pct)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("checkin", e))?;

    // Store performance metrics time-series data
    if let Some(cpu) = body.cpu_usage {
        let _ = sqlx::query(
            "INSERT INTO metrics (server_id, metric_type, value) VALUES ($1, 'cpu', $2)",
        )
        .bind(server_id)
        .bind(cpu as f64)
        .execute(&state.db)
        .await;
    }
    if let Some(mem) = body.mem_used_mb {
        let _ = sqlx::query(
            "INSERT INTO metrics (server_id, metric_type, value) VALUES ($1, 'memory_mb', $2)",
        )
        .bind(server_id)
        .bind(mem as f64)
        .execute(&state.db)
        .await;
    }
    if let Some(disk_pct) = body.disk_usage_pct {
        let _ = sqlx::query(
            "INSERT INTO metrics (server_id, metric_type, value) VALUES ($1, 'disk_pct', $2)",
        )
        .bind(server_id)
        .bind(disk_pct as f64)
        .execute(&state.db)
        .await;
    }

    // Resolve any offline alert state (server just checked in = it's online)
    let _ = sqlx::query(
        "UPDATE alert_state SET current_state = 'ok', fired_at = NULL, last_notified_at = NULL \
         WHERE server_id = $1 AND alert_type = 'offline' AND current_state = 'firing'",
    )
    .bind(server_id)
    .execute(&state.db)
    .await;

    Ok(Json(serde_json::json!({ "ok": true })))
}
