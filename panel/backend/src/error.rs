use axum::{http::StatusCode, Json};

pub type ApiError = (StatusCode, Json<serde_json::Value>);

pub fn err(status: StatusCode, msg: &str) -> ApiError {
    (status, Json(serde_json::json!({ "error": msg })))
}

/// Log the real error internally but return a generic message to clients.
/// Includes an incident ID for correlation.
pub fn agent_error(context: &str, e: impl std::fmt::Display) -> ApiError {
    let incident_id = uuid::Uuid::new_v4();
    tracing::error!(incident_id = %incident_id, error = %e, "{context}");
    err(StatusCode::BAD_GATEWAY, &format!("Operation failed. Reference: {incident_id}"))
}

pub fn require_admin(role: &str) -> Result<(), ApiError> {
    if role != "admin" {
        Err(err(StatusCode::FORBIDDEN, "Admin access required"))
    } else {
        Ok(())
    }
}

pub fn paginate(limit: Option<i64>, offset: Option<i64>) -> (i64, i64) {
    let limit = limit.unwrap_or(100).max(1).min(200);
    let offset = offset.unwrap_or(0).max(0);
    (limit, offset)
}
