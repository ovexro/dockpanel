use axum::{
    extract::{Path, State},
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

/// GET /api/notifications — List notifications for current user (last 50).
pub async fn list(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<Vec<NotificationRow>>, ApiError> {
    let notifs: Vec<NotificationRow> = sqlx::query_as(
        "SELECT id, title, message, severity, category, link, read_at, created_at \
         FROM panel_notifications WHERE user_id = $1 ORDER BY created_at DESC LIMIT 50",
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("list notifications", e))?;

    Ok(Json(notifs))
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
    });

    Sse::new(live_stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(30))
            .text("keepalive"),
    )
}
