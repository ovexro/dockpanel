use axum::{
    extract::{Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{require_admin, ApiError};
use crate::AppState;

#[derive(Deserialize)]
pub struct LogParams {
    pub level: Option<String>,
    pub source: Option<String>,
    pub since: Option<String>,  // "1h", "24h", "7d", "30d"
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct SystemLog {
    pub id: Uuid,
    pub level: String,
    pub source: String,
    pub message: String,
    pub details: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Parse a duration string like "1h", "24h", "7d", "30d" into a PostgreSQL interval string.
fn parse_since(since: &str) -> Option<&'static str> {
    match since {
        "1h" => Some("1 hour"),
        "24h" => Some("24 hours"),
        "7d" => Some("7 days"),
        "30d" => Some("30 days"),
        _ => None,
    }
}

/// GET /api/system-logs — List system log entries (admin only).
pub async fn list(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Query(params): Query<LogParams>,
) -> Result<Json<Vec<SystemLog>>, ApiError> {
    require_admin(&claims.role)?;

    let limit = params.limit.unwrap_or(50).max(1).min(200);
    let offset = params.offset.unwrap_or(0).max(0);

    // Build query dynamically based on filters
    let mut conditions: Vec<String> = Vec::new();
    let mut bind_idx = 1u32;

    if let Some(ref level) = params.level {
        if ["error", "warning", "info"].contains(&level.as_str()) {
            conditions.push(format!("level = ${bind_idx}"));
            bind_idx += 1;
        }
    }

    if let Some(ref source) = params.source {
        if !source.is_empty() {
            conditions.push(format!("source = ${bind_idx}"));
            bind_idx += 1;
        }
    }

    if let Some(ref since) = params.since {
        if let Some(interval) = parse_since(since) {
            conditions.push(format!("created_at > NOW() - INTERVAL '{interval}'"));
        }
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    let sql = format!(
        "SELECT id, level, source, message, details, created_at \
         FROM system_logs {where_clause} \
         ORDER BY created_at DESC \
         LIMIT ${bind_idx} OFFSET ${}",
        bind_idx + 1
    );

    // We need to bind parameters in the right order
    let mut query = sqlx::query_as::<_, SystemLog>(&sql);

    if let Some(ref level) = params.level {
        if ["error", "warning", "info"].contains(&level.as_str()) {
            query = query.bind(level);
        }
    }

    if let Some(ref source) = params.source {
        if !source.is_empty() {
            query = query.bind(source);
        }
    }

    query = query.bind(limit).bind(offset);

    let logs = query
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    Ok(Json(logs))
}

#[derive(Deserialize)]
pub struct CountParams {
    pub since: Option<String>,  // "1h", "24h", "7d", "30d"
}

#[derive(Serialize)]
pub struct LogCounts {
    pub error: i64,
    pub warning: i64,
    pub info: i64,
}

/// GET /api/system-logs/count — Count log entries by level (admin only).
pub async fn count(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Query(params): Query<CountParams>,
) -> Result<Json<LogCounts>, ApiError> {
    require_admin(&claims.role)?;

    let interval = params.since.as_deref()
        .and_then(parse_since)
        .unwrap_or("24 hours");

    let rows: Vec<(String, i64)> = sqlx::query_as(
        &format!(
            "SELECT level, COUNT(*) FROM system_logs \
             WHERE created_at > NOW() - INTERVAL '{interval}' \
             GROUP BY level"
        ),
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let mut counts = LogCounts { error: 0, warning: 0, info: 0 };
    for (level, cnt) in rows {
        match level.as_str() {
            "error" => counts.error = cnt,
            "warning" => counts.warning = cnt,
            "info" => counts.info = cnt,
            _ => {}
        }
    }

    Ok(Json(counts))
}
