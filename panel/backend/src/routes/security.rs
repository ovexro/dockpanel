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
