use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use jsonwebtoken::{encode, EncodingKey, Header};

use crate::auth::AuthUser;
use crate::error::{err, require_admin, ApiError};
use crate::AppState;

#[derive(serde::Deserialize)]
pub struct LogQuery {
    #[serde(rename = "type")]
    pub log_type: Option<String>,
    pub lines: Option<u32>,
    pub filter: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct SearchQuery {
    #[serde(rename = "type")]
    pub log_type: Option<String>,
    pub pattern: Option<String>,
    pub max: Option<u32>,
}

#[derive(serde::Deserialize)]
pub struct StreamTokenQuery {
    pub site_id: Option<String>,
    #[serde(rename = "type")]
    pub log_type: Option<String>,
}

#[derive(serde::Serialize)]
struct StreamTicket {
    sub: String,
    purpose: String,
    exp: usize,
}

/// GET /api/logs — System-wide logs (admin only).
pub async fn system_logs(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Query(q): Query<LogQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let log_type = q.log_type.as_deref().unwrap_or("nginx_access");
    if !["nginx_access", "nginx_error", "syslog", "auth", "php_fpm"].contains(&log_type) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid log type"));
    }
    let lines = q.lines.unwrap_or(100).max(1).min(1000);
    let mut agent_path = format!("/logs?type={}&lines={}", log_type, lines);
    if let Some(ref filter) = q.filter {
        agent_path.push_str(&format!("&filter={}", urlencoding::encode(filter)));
    }

    let result = state
        .agent
        .get(&agent_path)
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Agent error: {e}")))?;

    Ok(Json(result))
}

/// GET /api/sites/{id}/logs — Site-specific logs.
pub async fn site_logs(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<uuid::Uuid>,
    Query(q): Query<LogQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT domain FROM sites WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(claims.sub)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let (domain,) = row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    let log_type = q.log_type.as_deref().unwrap_or("access");
    if !["access", "error"].contains(&log_type) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid site log type"));
    }
    let lines = q.lines.unwrap_or(100).max(1).min(1000);
    let mut agent_path = format!("/logs/{}?type={}&lines={}", domain, log_type, lines);
    if let Some(ref filter) = q.filter {
        agent_path.push_str(&format!("&filter={}", urlencoding::encode(filter)));
    }

    let result = state
        .agent
        .get(&agent_path)
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Agent error: {e}")))?;

    Ok(Json(result))
}

/// GET /api/logs/search — Search system logs with grep/regex (admin only).
pub async fn search_system_logs(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Query(q): Query<SearchQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let log_type = q.log_type.as_deref().unwrap_or("nginx_access");
    if !["nginx_access", "nginx_error", "syslog", "auth", "php_fpm"].contains(&log_type) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid log type"));
    }

    let pattern = q.pattern.as_deref().unwrap_or("");
    if pattern.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Pattern is required"));
    }

    let max = q.max.unwrap_or(500).max(1).min(5000);
    let agent_path = format!(
        "/logs/search?type={}&pattern={}&max={}",
        log_type,
        urlencoding::encode(pattern),
        max
    );

    let result = state
        .agent
        .get(&agent_path)
        .await
        .map_err(|e| err(StatusCode::UNPROCESSABLE_ENTITY, &format!("Search error: {e}")))?;

    Ok(Json(result))
}

/// GET /api/sites/{id}/logs/search — Search site logs with grep/regex.
pub async fn search_site_logs(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<uuid::Uuid>,
    Query(q): Query<SearchQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT domain FROM sites WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(claims.sub)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let (domain,) = row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    let log_type = q.log_type.as_deref().unwrap_or("access");
    let full_type = match log_type {
        "access" => format!("nginx_access:{domain}"),
        "error" => format!("nginx_error:{domain}"),
        _ => return Err(err(StatusCode::BAD_REQUEST, "Invalid site log type")),
    };

    let pattern = q.pattern.as_deref().unwrap_or("");
    if pattern.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Pattern is required"));
    }

    let max = q.max.unwrap_or(500).max(1).min(5000);
    let agent_path = format!(
        "/logs/search?type={}&pattern={}&max={}",
        urlencoding::encode(&full_type),
        urlencoding::encode(pattern),
        max
    );

    let result = state
        .agent
        .get(&agent_path)
        .await
        .map_err(|e| err(StatusCode::UNPROCESSABLE_ENTITY, &format!("Search error: {e}")))?;

    Ok(Json(result))
}

/// GET /api/logs/stream/token — Generate a short-lived JWT for WebSocket log streaming.
pub async fn stream_token(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Query(q): Query<StreamTokenQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // System-level streaming requires admin
    if q.site_id.is_none() && claims.role != "admin" {
        return Err(err(
            StatusCode::FORBIDDEN,
            "Admin access required for system log streaming",
        ));
    }

    let mut domain: Option<String> = None;

    // Resolve domain from site_id if provided
    if let Some(ref sid) = q.site_id {
        let site_id: uuid::Uuid = sid
            .parse()
            .map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid site_id"))?;

        let row: Option<(String,)> =
            sqlx::query_as("SELECT domain FROM sites WHERE id = $1 AND user_id = $2")
                .bind(site_id)
                .bind(claims.sub)
                .fetch_optional(&state.db)
                .await
                .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

        domain = row.map(|(d,)| d);
        if domain.is_none() {
            return Err(err(StatusCode::NOT_FOUND, "Site not found"));
        }
    }

    let ticket = StreamTicket {
        sub: claims.email,
        purpose: "log_stream".to_string(),
        exp: (chrono::Utc::now() + chrono::Duration::seconds(60)).timestamp() as usize,
    };

    let token = encode(
        &Header::default(),
        &ticket,
        &EncodingKey::from_secret(state.agent.token().as_bytes()),
    )
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(serde_json::json!({
        "token": token,
        "domain": domain,
        "type": q.log_type.as_deref().unwrap_or("nginx_access"),
    })))
}

/// GET /api/system/processes — Top processes (admin only).
pub async fn processes(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let result = state
        .agent
        .get("/system/processes")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Agent error: {e}")))?;

    Ok(Json(result))
}

/// GET /api/system/network — Network I/O stats (admin only).
pub async fn network(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let result = state
        .agent
        .get("/system/network")
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Agent error: {e}")))?;

    Ok(Json(result))
}
