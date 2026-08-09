use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{err, internal_error, require_admin, ApiError};
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

/// The levels this API can filter and count.
const LEVELS: [&str; 3] = ["error", "warning", "info"];

/// A source that writes a METRIC into this table rather than a log line.
///
/// `backup_policy_executor` inserts one row per 60-second tick recording total
/// backup bytes, and `backup_orchestrator` reads them back as a 30-day time
/// series — so the rows have a consumer and must keep being written. But they
/// are not events an operator reads: on this panel they outnumbered genuine
/// entries 43,272 to 10, which at the page's 50-row window put every real entry
/// older than ~50 minutes below the fold and made the log unusable by hand.
///
/// So the default listing hides them. The source is still SELECTABLE — picking
/// it explicitly returns them — because a filter the operator cannot switch off
/// is the same defect pointed the other way.
const METRIC_SOURCE: &str = "backup_storage";

/// The condition that hides the metric rows, applied only when the caller has
/// not asked for a specific source.
///
/// Derived from [`METRIC_SOURCE`] rather than spelled again, so the name the
/// listing hides and the name the dropdown offers cannot drift apart.
///
/// ⚠ This is a NARROWING: it makes the endpoint return less than before, and a
/// narrowing that goes too far fails silently — the response still looks like a
/// healthy list either way. `tests/system-logs-scope-pin-e2e.sh` pins both
/// directions: a non-metric row must still be returned, and the metric source
/// must still be reachable when named.
fn hide_metric() -> String {
    format!("source <> '{METRIC_SOURCE}'")
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

    // Build query dynamically based on filters.
    //
    // ⚠ An unrecognised filter value is REFUSED, not ignored. Every one of these
    // used to fall through to "push no condition", which does not return an error
    // and does not return an empty set — it returns the UNFILTERED list. To the
    // caller that is indistinguishable from "your filter matched a great many
    // rows", so a typo silently widened the answer while looking like it worked.
    let mut conditions: Vec<String> = Vec::new();
    let mut bind_idx = 1u32;

    let level = params.level.as_deref().filter(|s| !s.is_empty());
    if let Some(level) = level {
        if !LEVELS.contains(&level) {
            return Err(err(
                StatusCode::BAD_REQUEST,
                &format!("Unknown level '{level}' — expected one of {}", LEVELS.join(", ")),
            ));
        }
        conditions.push(format!("level = ${bind_idx}"));
        bind_idx += 1;
    }

    let source = params.source.as_deref().filter(|s| !s.is_empty());
    match source {
        Some(_) => {
            conditions.push(format!("source = ${bind_idx}"));
            bind_idx += 1;
        }
        // No source named — hide the metric rows. See `METRIC_SOURCE`.
        None => conditions.push(hide_metric()),
    }

    if let Some(since) = params.since.as_deref().filter(|s| !s.is_empty()) {
        let interval = parse_since(since).ok_or_else(|| {
            err(
                StatusCode::BAD_REQUEST,
                &format!("Unknown range '{since}' — expected 1h, 24h, 7d or 30d"),
            )
        })?;
        conditions.push(format!("created_at > NOW() - INTERVAL '{interval}'"));
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

    // Bind in the same order the conditions were pushed.
    let mut query = sqlx::query_as::<_, SystemLog>(&sql);
    if let Some(level) = level {
        query = query.bind(level);
    }
    if let Some(source) = source {
        query = query.bind(source);
    }
    query = query.bind(limit).bind(offset);

    // ⚠ This used to be `.unwrap_or_default()`, which renders a database failure
    // as an empty list — "there are no logs" and "we could not read the logs"
    // became the same screen, on the one page an operator opens to find out what
    // went wrong. Same class as the canary reader's silence-as-safety.
    let logs = query
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("list system logs", e))?;

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

    let interval = match params.since.as_deref().filter(|s| !s.is_empty()) {
        None => "24 hours",
        Some(since) => parse_since(since).ok_or_else(|| {
            err(
                StatusCode::BAD_REQUEST,
                &format!("Unknown range '{since}' — expected 1h, 24h, 7d or 30d"),
            )
        })?,
    };

    // The tiles must count the same population the default listing shows, or the
    // "Info" tile reports one metric sample per minute — 1,440 a day, forever —
    // and reads as activity. See `METRIC_SOURCE`.
    let hide = hide_metric();
    let rows: Vec<(String, i64)> = sqlx::query_as(
        &format!(
            "SELECT level, COUNT(*) FROM system_logs \
             WHERE created_at > NOW() - INTERVAL '{interval}' AND {hide} \
             GROUP BY level"
        ),
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("count system logs", e))?;

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
