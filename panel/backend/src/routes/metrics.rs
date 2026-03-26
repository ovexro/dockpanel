use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{internal_error, err, ApiError};
use crate::AppState;

#[derive(serde::Deserialize)]
pub struct MetricsQuery {
    pub metric_type: Option<String>,
    pub hours: Option<i32>,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct MetricPoint {
    pub value: f64,
    pub recorded_at: chrono::DateTime<chrono::Utc>,
}

/// GET /api/servers/{id}/metrics — Get performance metrics history.
pub async fn server_metrics(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(server_id): Path<Uuid>,
    Query(params): Query<MetricsQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Verify ownership
    let exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM servers WHERE id = $1 AND user_id = $2",
    )
    .bind(server_id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("server metrics", e))?;

    if exists.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Server not found"));
    }

    let hours = params.hours.unwrap_or(24).min(168).max(1);
    let metric_type = params.metric_type.as_deref().unwrap_or("cpu");

    let points: Vec<MetricPoint> = sqlx::query_as(
        "SELECT value, recorded_at FROM metrics \
         WHERE server_id = $1 AND metric_type = $2 \
         AND recorded_at > NOW() - ($3 || ' hours')::interval \
         ORDER BY recorded_at ASC",
    )
    .bind(server_id)
    .bind(metric_type)
    .bind(hours.to_string())
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("server metrics", e))?;

    // Also compute summary stats
    let stats: Option<(f64, f64, f64)> = sqlx::query_as(
        "SELECT AVG(value), MIN(value), MAX(value) FROM metrics \
         WHERE server_id = $1 AND metric_type = $2 \
         AND recorded_at > NOW() - ($3 || ' hours')::interval",
    )
    .bind(server_id)
    .bind(metric_type)
    .bind(hours.to_string())
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("server metrics", e))?;

    let (avg, min, max) = stats.unwrap_or((0.0, 0.0, 0.0));

    Ok(Json(serde_json::json!({
        "metric_type": metric_type,
        "hours": hours,
        "points": points,
        "summary": {
            "avg": (avg * 10.0).round() / 10.0,
            "min": (min * 10.0).round() / 10.0,
            "max": (max * 10.0).round() / 10.0,
            "count": points.len(),
        },
    })))
}
