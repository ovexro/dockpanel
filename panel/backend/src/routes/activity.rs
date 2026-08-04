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
    /// Which host the action happened on. `activity_logs.server_id` has been
    /// written since v2.58.0 and stamped by the auto-healer's own writers since
    /// v2.60.0, and until now it was read only by this file's sibling cooldown
    /// gates — never declared here, so `SELECT *` dropped it and the audit feed
    /// could not say which of N machines an action ran on. NULL for a panel-wide
    /// action that belongs to no single host.
    pub server_id: Option<Uuid>,
    /// Resolved from the join, so the operator reads a name rather than a UUID.
    /// NULL when the server row has since been deleted.
    pub server_name: Option<String>,
}

/// The audit feed's columns, with the host resolved to its name.
///
/// Spelled out rather than `SELECT *` because the join adds a column that is not
/// on `activity_logs`, and because a `*` here would silently start shipping any
/// column a future migration adds to the table.
const ACTIVITY_COLUMNS: &str = "a.id, a.user_id, a.user_email, a.action, a.target_type, \
     a.target_name, a.details, a.ip_address, a.created_at, a.server_id, s.name AS server_name \
     FROM activity_logs a LEFT JOIN servers s ON s.id = a.server_id";

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
        sqlx::query_as(&format!(
            "SELECT {ACTIVITY_COLUMNS} WHERE a.action LIKE $1 \
             ORDER BY a.created_at DESC LIMIT $2 OFFSET $3"
        ))
        .bind(&pattern)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("list activity", e))?
    } else {
        sqlx::query_as(&format!(
            "SELECT {ACTIVITY_COLUMNS} ORDER BY a.created_at DESC LIMIT $1 OFFSET $2"
        ))
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("list activity", e))?
    };

    Ok(Json(logs))
}
