use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
    Json,
};
use futures::stream::StreamExt;
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{internal_error, err, ApiError};
use crate::AppState;

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct NotificationRow {
    id: Uuid,
    title: String,
    message: String,
    severity: String,
    category: String,
    link: Option<String>,
    read_at: Option<chrono::DateTime<chrono::Utc>>,
    created_at: chrono::DateTime<chrono::Utc>,
}

/// Query parameters for `GET /api/notifications`.
///
/// The handler used to take none, and the SQL ended in a bare `LIMIT 50`. On
/// this panel that left 28 of 78 rows with no way to reach them: no offset, no
/// cursor, and nothing in the UI that could have asked for them. The badge,
/// meanwhile, counts unread without a limit — so the two numbers on screen were
/// answering different questions and could disagree by construction.
#[derive(serde::Deserialize)]
pub struct ListQuery {
    /// Page size. Clamped to 1..=200; absent means 50, which is what the
    /// hardcoded limit used to be, so an old client sees no change.
    limit: Option<i64>,
    /// Keyset cursor: return rows strictly older than this timestamp. Keyset
    /// rather than OFFSET because rows arrive at the head while you page.
    before: Option<chrono::DateTime<chrono::Utc>>,
    /// Exact `category` match, or absent for all.
    category: Option<String>,
    /// `true` restricts to unread. Absent or false means all.
    unread: Option<bool>,
}

/// GET /api/notifications — one page of the current user's notifications.
pub async fn list(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<NotificationRow>>, ApiError> {
    let limit = q.limit.unwrap_or(50).clamp(1, 200);

    // Every filter is bound, and the two optional ones are expressed as
    // "$n IS NULL OR column = $n" so the statement shape never changes with the
    // arguments. String-building a WHERE clause here would put a user-supplied
    // `category` next to SQL.
    let notifs: Vec<NotificationRow> = sqlx::query_as(
        "SELECT id, title, message, severity, category, link, read_at, created_at \
         FROM panel_notifications \
         WHERE user_id = $1 \
           AND ($2::timestamptz IS NULL OR created_at < $2) \
           AND ($3::text IS NULL OR category = $3) \
           AND ($4::bool IS NOT TRUE OR read_at IS NULL) \
         ORDER BY created_at DESC LIMIT $5",
    )
    .bind(claims.sub)
    .bind(q.before)
    .bind(q.category.as_deref())
    .bind(q.unread)
    .bind(limit)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("list notifications", e))?;

    Ok(Json(notifs))
}

/// GET /api/notifications/summary — totals the list cannot carry.
///
/// The page used to derive "N unread" from the rows it had loaded, while the
/// bell used the server's own COUNT. With a hard limit on one side and none on
/// the other, the header and the badge could state different numbers for the
/// same thing. Both now read from here.
pub async fn summary(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (total, unread): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COUNT(*) FILTER (WHERE read_at IS NULL) \
         FROM panel_notifications WHERE user_id = $1",
    )
    .bind(claims.sub)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("notification summary", e))?;

    let categories: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT category, COUNT(*), COUNT(*) FILTER (WHERE read_at IS NULL) \
         FROM panel_notifications WHERE user_id = $1 GROUP BY category ORDER BY category",
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("notification summary categories", e))?;

    Ok(Json(serde_json::json!({
        "total": total,
        "unread": unread,
        "categories": categories
            .into_iter()
            .map(|(name, total, unread)| serde_json::json!({
                "category": name, "total": total, "unread": unread,
            }))
            .collect::<Vec<_>>(),
    })))
}

/// GET /api/notifications/unread-count — Quick count for badge.
pub async fn unread_count(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM panel_notifications WHERE user_id = $1 AND read_at IS NULL",
    )
    .bind(claims.sub)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("unread count", e))?;

    Ok(Json(serde_json::json!({ "count": count })))
}

/// POST /api/notifications/{id}/read — Mark single notification as read.
pub async fn mark_read(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = sqlx::query(
        "UPDATE panel_notifications SET read_at = NOW() WHERE id = $1 AND user_id = $2 AND read_at IS NULL",
    )
    .bind(id)
    .bind(claims.sub)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("mark read", e))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Notification not found or already read"));
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/notifications/read-all — Mark all notifications as read.
pub async fn mark_all_read(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    sqlx::query(
        "UPDATE panel_notifications SET read_at = NOW() WHERE user_id = $1 AND read_at IS NULL",
    )
    .bind(claims.sub)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("mark all read", e))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// DELETE /api/notifications/{id} — Remove one notification.
///
/// There was no way to remove a notification at all: the only DELETE against
/// this table in the whole product is the retention sweep, which is time-based
/// and takes unread rows with it. So the operator's only tool for a feed full of
/// noise was to wait 30 days.
pub async fn delete_one(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = sqlx::query("DELETE FROM panel_notifications WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(claims.sub)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("delete notification", e))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Notification not found"));
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// DELETE /api/notifications/read — Clear everything already read.
///
/// Scoped to `read_at IS NOT NULL` on purpose: "clear" must never be able to
/// discard something the operator has not seen. `user_id = $1` is the other half
/// — one admin clearing their feed cannot touch another's copy of the same
/// broadcast, because the fan-out gives each recipient their own row.
pub async fn delete_read(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result =
        sqlx::query("DELETE FROM panel_notifications WHERE user_id = $1 AND read_at IS NOT NULL")
            .bind(claims.sub)
            .execute(&state.db)
            .await
            .map_err(|e| internal_error("clear read notifications", e))?;

    Ok(Json(
        serde_json::json!({ "ok": true, "deleted": result.rows_affected() }),
    ))
}

/// GET /api/notifications/stream — SSE stream for real-time notification delivery.
/// Filters events to only those belonging to the authenticated user.
pub async fn stream(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Sse<impl futures::Stream<Item = Result<Event, axum::BoxError>>> {
    let rx = state.notif_tx.subscribe();
    let user_id = claims.sub;

    let live_stream = BroadcastStream::new(rx).filter_map(move |result| {
        let user_id = user_id;
        async move {
            match result {
                Ok((uid, json)) if uid == user_id => {
                    Some(Ok(Event::default().data(json)))
                }
                Ok(_) => None,           // Not for this user
                Err(_) => None,           // Lagged or closed — skip
            }
        }
    })
    // `notif_tx` lives for the whole process, so without this the stream never
    // ends on its own — every admin tab with the panel open holds one of these
    // open indefinitely, which is exactly what blocks a graceful shutdown drain.
    .take_until(crate::helpers::shutdown_signal_fut(&state));

    Sse::new(live_stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(30))
            .text("keepalive"),
    )
}
