use axum::{
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use super::AppState;
use crate::services::diagnostics;

type ApiErr = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: &str) -> ApiErr {
    (status, Json(serde_json::json!({ "error": msg })))
}

/// GET /diagnostics — Run all diagnostic checks.
async fn run_diagnostics() -> Json<diagnostics::DiagnosticReport> {
    Json(diagnostics::run_diagnostics().await)
}

#[derive(Deserialize)]
struct FixRequest {
    fix_id: String,
}

/// POST /diagnostics/fix — Apply a one-click fix.
async fn apply_fix(
    Json(body): Json<FixRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    // Validate fix_id format (action:target)
    if body.fix_id.is_empty() || body.fix_id.len() > 256 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid fix_id"));
    }

    diagnostics::apply_fix(&body.fix_id)
        .await
        .map(|msg| Json(serde_json::json!({ "success": true, "message": msg })))
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/diagnostics", get(run_diagnostics))
        .route("/diagnostics/fix", post(apply_fix))
}
