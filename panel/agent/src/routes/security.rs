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

#[derive(Deserialize)]
struct SshPortRequest {
    port: u16,
}

/// POST /security/ssh/disable-password — Disable SSH password auth.
async fn ssh_disable_password() -> Result<Json<serde_json::Value>, ApiErr> {
    security::disable_ssh_password_auth().await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(serde_json::json!({ "success": true })))
}

/// POST /security/ssh/enable-password — Enable SSH password auth.
async fn ssh_enable_password() -> Result<Json<serde_json::Value>, ApiErr> {
    security::enable_ssh_password_auth().await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(serde_json::json!({ "success": true })))
}

/// POST /security/ssh/disable-root — Disable root SSH login.
async fn ssh_disable_root() -> Result<Json<serde_json::Value>, ApiErr> {
    security::disable_ssh_root_login().await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(serde_json::json!({ "success": true })))
}

/// POST /security/ssh/change-port — Change SSH port.
async fn ssh_change_port(Json(body): Json<SshPortRequest>) -> Result<Json<serde_json::Value>, ApiErr> {
    security::change_ssh_port(body.port).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(serde_json::json!({ "success": true })))
}

#[derive(Deserialize)]
struct BanRequest {
    jail: String,
    ip: String,
}

/// POST /security/fail2ban/unban
async fn fail2ban_unban(Json(body): Json<BanRequest>) -> Result<Json<serde_json::Value>, ApiErr> {
    security::fail2ban_unban(&body.jail, &body.ip).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(serde_json::json!({ "success": true })))
}

/// POST /security/fail2ban/ban
async fn fail2ban_ban(Json(body): Json<BanRequest>) -> Result<Json<serde_json::Value>, ApiErr> {
    security::fail2ban_ban(&body.jail, &body.ip).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(serde_json::json!({ "success": true })))
}

/// GET /security/fail2ban/{jail}/banned
async fn fail2ban_banned(Path(jail): Path<String>) -> Result<Json<serde_json::Value>, ApiErr> {
    let ips = security::fail2ban_banned_ips(&jail).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(serde_json::json!({ "ips": ips })))
}

#[derive(Deserialize)]
struct FixRequest {
    fix_type: String,
    target: String,
}

/// POST /security/fix — Apply a recommended security fix.
async fn apply_fix(Json(body): Json<FixRequest>) -> Result<Json<serde_json::Value>, ApiErr> {
    let result = security::apply_fix(&body.fix_type, &body.target).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(serde_json::json!({ "success": true, "message": result })))
}

/// GET /security/login-audit — Recent SSH login attempts from auth.log.
async fn login_audit() -> Result<Json<serde_json::Value>, ApiErr> {
    let entries = security::get_login_audit()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(serde_json::json!({ "entries": entries })))
}

/// POST /security/panel-jail/setup — Create DockPanel Fail2Ban jail.
async fn setup_panel_jail() -> Result<Json<serde_json::Value>, ApiErr> {
    security::setup_panel_jail().await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(serde_json::json!({ "success": true })))
}

/// GET /security/panel-jail/status — Check if panel jail exists.
async fn panel_jail_status() -> Json<serde_json::Value> {
    let active = security::panel_jail_status().await;
    Json(serde_json::json!({ "active": active }))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/security/overview", get(overview))
        .route("/security/firewall", get(firewall_status))
        .route("/security/firewall/rules", post(add_rule))
        .route("/security/firewall/rules/{number}", delete(delete_rule))
        .route("/security/fail2ban", get(fail2ban_status))
        .route("/security/scan", post(run_scan))
        .route("/security/ssh/disable-password", post(ssh_disable_password))
        .route("/security/ssh/enable-password", post(ssh_enable_password))
        .route("/security/ssh/disable-root", post(ssh_disable_root))
        .route("/security/ssh/change-port", post(ssh_change_port))
        .route("/security/fail2ban/unban", post(fail2ban_unban))
        .route("/security/fail2ban/ban", post(fail2ban_ban))
        .route("/security/fail2ban/{jail}/banned", get(fail2ban_banned))
        .route("/security/fix", post(apply_fix))
        .route("/security/login-audit", get(login_audit))
        .route("/security/panel-jail/setup", post(setup_panel_jail))
        .route("/security/panel-jail/status", get(panel_jail_status))
}
