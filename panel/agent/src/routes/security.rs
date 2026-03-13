use axum::{
    extract::Path,
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
use serde::Deserialize;

use super::AppState;
use crate::services::security;
use crate::services::security_scanner;

type ApiErr = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: &str) -> ApiErr {
    (status, Json(serde_json::json!({ "error": msg })))
}

#[derive(Deserialize)]
struct AddRuleRequest {
    port: u16,
    proto: String,
    action: String,
    from: Option<String>,
}

/// GET /security/overview
async fn overview() -> Result<Json<security::SecurityOverview>, ApiErr> {
    security::get_security_overview()
        .await
        .map(Json)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))
}

/// GET /security/firewall
async fn firewall_status() -> Result<Json<security::FirewallStatus>, ApiErr> {
    security::get_firewall_status()
        .await
        .map(Json)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))
}

/// POST /security/firewall/rules
async fn add_rule(
    Json(body): Json<AddRuleRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    security::add_firewall_rule(body.port, &body.proto, &body.action, body.from.as_deref())
        .await
        .map_err(|e| {
            if e.contains("Invalid") {
                err(StatusCode::BAD_REQUEST, &e)
            } else {
                err(StatusCode::INTERNAL_SERVER_ERROR, &e)
            }
        })?;

    Ok(Json(serde_json::json!({ "success": true })))
}

/// DELETE /security/firewall/rules/{number}
async fn delete_rule(Path(number): Path<usize>) -> Result<Json<serde_json::Value>, ApiErr> {
    security::remove_firewall_rule(number)
        .await
        .map_err(|e| {
            if e.contains("must be") {
                err(StatusCode::BAD_REQUEST, &e)
            } else {
                err(StatusCode::INTERNAL_SERVER_ERROR, &e)
            }
        })?;

    Ok(Json(serde_json::json!({ "success": true })))
}

/// GET /security/fail2ban
async fn fail2ban_status() -> Result<Json<security::Fail2banStatus>, ApiErr> {
    security::get_fail2ban_status()
        .await
        .map(Json)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))
}

/// POST /security/scan — Run a full security scan.
async fn run_scan() -> Json<security_scanner::ScanResult> {
    Json(security_scanner::run_full_scan().await)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/security/overview", get(overview))
        .route("/security/firewall", get(firewall_status))
        .route("/security/firewall/rules", post(add_rule))
        .route("/security/firewall/rules/{number}", delete(delete_rule))
        .route("/security/fail2ban", get(fail2ban_status))
        .route("/security/scan", post(run_scan))
}
