use axum::{http::StatusCode, Json};

pub type ApiError = (StatusCode, Json<serde_json::Value>);

pub fn err(status: StatusCode, msg: &str) -> ApiError {
    (status, Json(serde_json::json!({ "error": msg })))
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
