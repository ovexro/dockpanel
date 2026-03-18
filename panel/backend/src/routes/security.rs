use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};

use crate::auth::AdminUser;
use crate::error::{err, agent_error, ApiError};
use crate::services::activity;
use crate::AppState;

/// GET /api/security/overview — Security overview.
pub async fn overview(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = state
        .agent
        .get("/security/overview")
        .await
        .map_err(|e| agent_error("Security overview", e))?;

    Ok(Json(result))
}

/// GET /api/security/firewall — Firewall status and rules.
pub async fn firewall_status(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = state
        .agent
        .get("/security/firewall")
        .await
        .map_err(|e| agent_error("Firewall status", e))?;

    Ok(Json(result))
}

#[derive(serde::Deserialize)]
pub struct FirewallRuleRequest {
    pub port: u16,
    pub proto: Option<String>,
    pub action: Option<String>,
    pub from: Option<String>,
}

/// POST /api/security/firewall/rules — Add firewall rule.
pub async fn add_firewall_rule(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Json(body): Json<FirewallRuleRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.port == 0 {
        return Err(err(StatusCode::BAD_REQUEST, "Port must be between 1 and 65535"));
    }

    let port = body.port;
    let proto = body.proto.unwrap_or_else(|| "tcp".to_string());
    if !["tcp", "udp", "tcp/udp"].contains(&proto.as_str()) {
        return Err(err(StatusCode::BAD_REQUEST, "Protocol must be tcp, udp, or tcp/udp"));
    }
    let action = body.action.unwrap_or_else(|| "allow".to_string());
    if !["allow", "deny", "reject"].contains(&action.as_str()) {
        return Err(err(StatusCode::BAD_REQUEST, "Action must be allow, deny, or reject"));
    }

    let agent_body = serde_json::json!({
        "port": port,
        "proto": proto,
        "action": action,
        "from": body.from,
    });

    let result = state
        .agent
        .post("/security/firewall/rules", Some(agent_body))
        .await
        .map_err(|e| agent_error("Add firewall rule", e))?;

    let rule_name = format!("{port}/{proto}");
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "firewall.add",
        Some("firewall"), Some(&rule_name), None, None,
    ).await;

    Ok(Json(result))
}

/// DELETE /api/security/firewall/rules/{number} — Delete firewall rule.
pub async fn delete_firewall_rule(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(number): Path<usize>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let agent_path = format!("/security/firewall/rules/{}", number);
    state
        .agent
        .delete(&agent_path)
        .await
        .map_err(|e| agent_error("Delete firewall rule", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "firewall.delete",
        Some("firewall"), Some(&format!("rule #{number}")), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /api/security/fail2ban — Fail2ban status.
pub async fn fail2ban_status(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = state
        .agent
        .get("/security/fail2ban")
        .await
        .map_err(|e| agent_error("Fail2ban status", e))?;

    Ok(Json(result))
}

/// POST /api/security/ssh/disable-password
pub async fn ssh_disable_password(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.agent.post("/security/ssh/disable-password", None).await
        .map_err(|e| agent_error("SSH config", e))?;
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "security.ssh_disable_password",
        None, None, None, None,
    ).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/security/ssh/enable-password
pub async fn ssh_enable_password(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.agent.post("/security/ssh/enable-password", None).await
        .map_err(|e| agent_error("SSH config", e))?;
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "security.ssh_enable_password",
        None, None, None, None,
    ).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/security/ssh/disable-root
pub async fn ssh_disable_root(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.agent.post("/security/ssh/disable-root", None).await
        .map_err(|e| agent_error("SSH config", e))?;
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "security.ssh_disable_root",
        None, None, None, None,
    ).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/security/ssh/change-port
pub async fn ssh_change_port(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.agent.post("/security/ssh/change-port", Some(body)).await
        .map_err(|e| agent_error("SSH config", e))?;
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "security.ssh_change_port",
        None, None, None, None,
    ).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/security/fail2ban/unban
pub async fn fail2ban_unban_ip(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.agent.post("/security/fail2ban/unban", Some(body)).await
        .map_err(|e| agent_error("Fail2Ban", e))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/security/fail2ban/ban
pub async fn fail2ban_ban_ip(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.agent.post("/security/fail2ban/ban", Some(body)).await
        .map_err(|e| agent_error("Fail2Ban", e))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /api/security/fail2ban/{jail}/banned
pub async fn fail2ban_banned(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Path(jail): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = state.agent.get(&format!("/security/fail2ban/{jail}/banned")).await
        .map_err(|e| agent_error("Fail2Ban", e))?;
    Ok(Json(result))
}

/// POST /api/security/fix — Apply a recommended security fix.
pub async fn apply_security_fix(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = state.agent.post("/security/fix", Some(body.clone())).await
        .map_err(|e| agent_error("Security fix", e))?;
    let fix_type = body.get("fix_type").and_then(|v| v.as_str()).unwrap_or("unknown");
    let target = body.get("target").and_then(|v| v.as_str()).unwrap_or("unknown");
    activity::log_activity(
        &state.db, claims.sub, &claims.email, &format!("security.fix.{fix_type}"),
        Some("security"), Some(target), None, None,
    ).await;
    Ok(Json(result))
}
