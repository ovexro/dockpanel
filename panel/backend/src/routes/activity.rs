use axum::{
    extract::{Query, State},
    Json,
};
use uuid::Uuid;

use crate::auth::AdminUser;
use crate::error::{internal_error, ApiError};
use crate::AppState;

#[derive(serde::Deserialize)]
pub struct ActivityQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub action: Option<String>,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct ActivityLog {
    pub id: Uuid,
    pub user_id: Option<Uuid>,
    pub user_email: String,
    pub action: String,
    pub target_type: Option<String>,
    pub target_name: Option<String>,
    pub details: Option<String>,
    pub ip_address: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// GET /api/activity — List activity logs (admin only).
pub async fn list(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Query(params): Query<ActivityQuery>,
) -> Result<Json<Vec<ActivityLog>>, ApiError> {

    let limit = params.limit.unwrap_or(50).min(200);
    let offset = params.offset.unwrap_or(0);

    let logs: Vec<ActivityLog> = if let Some(ref action) = params.action {
        // Support category filtering (e.g., "site" matches "site.create", "site.delete")
        let pattern = format!("{}%", action);
        sqlx::query_as(
            "SELECT * FROM activity_logs WHERE action LIKE $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3",
        )
        .bind(&pattern)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("list activity", e))?
    } else {
        sqlx::query_as(
            "SELECT * FROM activity_logs ORDER BY created_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("list activity", e))?
    };

    Ok(Json(logs))
}
