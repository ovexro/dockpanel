// Prometheus `/metrics` scrape endpoint and admin toggle.
//
// Disabled by default. The admin enables it and a scrape token is
// generated once; Prometheus is configured with bearer-token auth:
//
//   scrape_configs:
//     - job_name: 'dockpanel'
//       metrics_path: /api/metrics
//       bearer_token: dpms_...
//       static_configs:
//         - targets: ['panel.example.com']
//
// When disabled, /api/metrics returns 404 — the endpoint is not
// advertised. When enabled but the token is wrong or missing, returns
// 401. Constant-time comparison of the SHA-256 hashed token.

use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{err, internal_error, require_admin, ApiError};
use crate::services::prometheus_exporter;
use crate::AppState;

#[derive(Deserialize)]
pub struct ScrapeQuery {
    #[serde(default)]
    pub token: Option<String>,
}

/// GET /api/metrics — Prometheus scrape endpoint.
///
/// No JWT required; gated by scrape token. Returns 404 when disabled so
/// a panel with metrics off doesn't advertise a scrape surface at all.
pub async fn scrape(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ScrapeQuery>,
) -> Result<Response, ApiError> {
    let (enabled, token_hash) = read_settings(&state.db)
        .await
        .map_err(|e| internal_error("read prometheus settings", e))?;

    if !enabled {
        return Err(err(StatusCode::NOT_FOUND, "Not Found"));
    }

    let stored = match token_hash.as_deref() {
        Some(s) if !s.is_empty() => s,
        _ => return Err(err(StatusCode::UNAUTHORIZED, "No scrape token configured")),
    };

    let provided = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|s| s.to_string())
        .or(q.token)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "Scrape token required"))?;

    let mut hasher = Sha256::new();
    hasher.update(provided.as_bytes());
    let provided_hex = hex::encode(hasher.finalize());

    if provided_hex.as_bytes().ct_eq(stored.as_bytes()).unwrap_u8() != 1 {
        return Err(err(StatusCode::UNAUTHORIZED, "Invalid scrape token"));
    }

    let body = prometheus_exporter::render(&state.db).await;
    Ok((
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
        .into_response())
}

// ── Admin surface ───────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct PromSettings {
    pub enabled: bool,
    pub token_configured: bool,
    pub token_prefix: Option<String>,
}

/// GET /api/prometheus/settings — Admin view: is it on, does a token exist,
/// and what's its display prefix.
pub async fn get_settings(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<PromSettings>, ApiError> {
    require_admin(&claims.role)?;

    let (enabled, token_hash) = read_settings(&state.db)
        .await
        .map_err(|e| internal_error("read prometheus settings", e))?;
    let token_prefix: Option<String> = sqlx::query_scalar(
        "SELECT value FROM settings WHERE key = 'prometheus_token_prefix'",
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("read prometheus prefix", e))?;

    Ok(Json(PromSettings {
        enabled,
        token_configured: token_hash.as_deref().map(|s| !s.is_empty()).unwrap_or(false),
        token_prefix: token_prefix.filter(|s| !s.is_empty()),
    }))
}

#[derive(Deserialize)]
pub struct UpdateSettingsReq {
    pub enabled: bool,
    #[serde(default)]
    pub rotate_token: bool,
}

/// POST /api/prometheus/settings — enable/disable and/or rotate the scrape
/// token. On first-enable with no existing token, auto-generates one so the
/// admin doesn't have to flip two switches.
pub async fn update_settings(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<UpdateSettingsReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let existing_hash: Option<String> = sqlx::query_scalar(
        "SELECT value FROM settings WHERE key = 'prometheus_token_hash'",
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("read existing prometheus token", e))?;

    let need_new = body.rotate_token
        || (body.enabled && existing_hash.as_deref().unwrap_or("").is_empty());

    let mut new_token: Option<String> = None;
    if need_new {
        let token = generate_token();
        let prefix = &token[..14];
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        let hash = hex::encode(hasher.finalize());

        write_kv(&state.db, "prometheus_token_hash", &hash)
            .await
            .map_err(|e| internal_error("save prometheus token hash", e))?;
        write_kv(&state.db, "prometheus_token_prefix", prefix)
            .await
            .map_err(|e| internal_error("save prometheus token prefix", e))?;
        new_token = Some(token);
    }

    write_kv(
        &state.db,
        "prometheus_enabled",
        if body.enabled { "true" } else { "false" },
    )
    .await
    .map_err(|e| internal_error("save prometheus enabled", e))?;

    Ok(Json(serde_json::json!({
        "ok": true,
        "token": new_token,
        "message": if new_token.is_some() {
            "Save this token — it won't be shown again."
        } else {
            "Settings saved."
        },
    })))
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn generate_token() -> String {
    // 256 bits of entropy via two UUIDs. "dpms_" = DockPanel Metrics Scrape.
    format!(
        "dpms_{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple(),
    )
}

async fn read_settings(pool: &sqlx::PgPool) -> Result<(bool, Option<String>), sqlx::Error> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT key, value FROM settings \
         WHERE key IN ('prometheus_enabled', 'prometheus_token_hash')",
    )
    .fetch_all(pool)
    .await?;

    let mut enabled = false;
    let mut token_hash: Option<String> = None;
    for (k, v) in rows {
        match k.as_str() {
            "prometheus_enabled" => enabled = v == "true",
            "prometheus_token_hash" => token_hash = Some(v),
            _ => {}
        }
    }
    Ok((enabled, token_hash))
}

async fn write_kv(pool: &sqlx::PgPool, key: &str, value: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ($1, $2) \
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = NOW()",
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_prefix_is_stable_length() {
        let t = generate_token();
        assert!(t.starts_with("dpms_"));
        assert_eq!(&t[..14].len(), &14);
        // The next 9 chars (indices 5..14) must be hex — that's the prefix
        // stored for display in the UI.
        assert!(t[5..14].chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn token_is_sufficiently_long() {
        // "dpms_" + two UUIDs with dashes stripped = 5 + 32 + 32 = 69 chars.
        assert_eq!(generate_token().len(), 69);
    }
}
