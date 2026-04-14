use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Html,
    Json,
};

use crate::auth::{AdminUser, ServerScope};
use crate::error::{internal_error, err, agent_error, ApiError};
use crate::services::{activity, security_hardening};
use crate::AppState;

/// GET /api/security/overview — Security overview.
pub async fn overview(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent
        .get("/security/overview")
        .await
        .map_err(|e| agent_error("Security overview", e))?;

    Ok(Json(result))
}

/// GET /api/security/firewall — Firewall status and rules.
pub async fn firewall_status(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent
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
    ServerScope(_server_id, agent): ServerScope,
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

    let result = agent
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
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let agent_path = format!("/security/firewall/rules/{}", number);
    agent
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
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent
        .get("/security/fail2ban")
        .await
        .map_err(|e| agent_error("Fail2ban status", e))?;

    Ok(Json(result))
}

/// POST /api/security/ssh/disable-password
pub async fn ssh_disable_password(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    agent.post("/security/ssh/disable-password", None).await
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
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    agent.post("/security/ssh/enable-password", None).await
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
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    agent.post("/security/ssh/disable-root", None).await
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
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    agent.post("/security/ssh/change-port", Some(body)).await
        .map_err(|e| agent_error("SSH config", e))?;
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "security.ssh_change_port",
        None, None, None, None,
    ).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/security/fail2ban/unban
pub async fn fail2ban_unban_ip(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    agent.post("/security/fail2ban/unban", Some(body)).await
        .map_err(|e| agent_error("Fail2Ban", e))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/security/fail2ban/ban
pub async fn fail2ban_ban_ip(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    agent.post("/security/fail2ban/ban", Some(body)).await
        .map_err(|e| agent_error("Fail2Ban", e))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /api/security/fail2ban/{jail}/banned
pub async fn fail2ban_banned(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Path(jail): Path<String>,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    if jail.is_empty() || !jail.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid jail name"));
    }
    let result = agent.get(&format!("/security/fail2ban/{jail}/banned")).await
        .map_err(|e| agent_error("Fail2Ban", e))?;
    Ok(Json(result))
}

/// GET /api/security/login-audit — Recent login attempts (panel + SSH).
pub async fn login_audit(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Panel logins from activity_logs
    let panel_logins: Vec<(String, String, Option<String>, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as(
            "SELECT action, COALESCE(details, ''), target_name, created_at FROM activity_logs \
             WHERE action IN ('auth.login', 'auth.login_failed', 'auth.2fa_verify') \
             ORDER BY created_at DESC LIMIT 50",
        )
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    let panel: Vec<serde_json::Value> = panel_logins
        .iter()
        .map(|(action, details, target, time)| {
            serde_json::json!({
                "type": "panel",
                "action": action,
                "details": details,
                "user": target,
                "time": time,
                "success": !action.contains("failed"),
            })
        })
        .collect();

    // SSH logins from agent (parse auth.log)
    let ssh = match agent.get("/security/login-audit").await {
        Ok(result) => result
            .get("entries")
            .cloned()
            .unwrap_or(serde_json::json!([])),
        Err(_) => serde_json::json!([]),
    };

    Ok(Json(serde_json::json!({
        "panel": panel,
        "ssh": ssh,
    })))
}

/// POST /api/security/panel-jail/setup — Create DockPanel Fail2Ban jail.
pub async fn setup_panel_jail(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    agent.post("/security/panel-jail/setup", None).await
        .map_err(|e| agent_error("Panel jail", e))?;
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "security.panel_jail_setup",
        None, None, None, None,
    ).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /api/security/panel-jail/status — Check if panel jail exists.
pub async fn panel_jail_status(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent.get("/security/panel-jail/status").await
        .map_err(|e| agent_error("Panel jail", e))?;
    Ok(Json(result))
}

/// POST /api/security/fix — Apply a recommended security fix.
pub async fn apply_security_fix(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent.post("/security/fix", Some(body.clone())).await
        .map_err(|e| agent_error("Security fix", e))?;
    let fix_type = body.get("fix_type").and_then(|v| v.as_str()).unwrap_or("unknown");
    let target = body.get("target").and_then(|v| v.as_str()).unwrap_or("unknown");
    let ip = crate::routes::client_ip(&headers);
    activity::log_activity(
        &state.db, claims.sub, &claims.email, &format!("security.fix.{fix_type}"),
        Some("security"), Some(target), None, ip.as_deref(),
    ).await;
    Ok(Json(result))
}

/// GET /api/security/report — Generate security compliance report (HTML).
pub async fn compliance_report(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Html<String>, ApiError> {
    // Fetch latest scan
    let scan: Option<(uuid::Uuid, String, i32, i32, i32, i32, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT id, status, findings_count, critical_count, warning_count, info_count, completed_at \
         FROM security_scans WHERE status = 'completed' ORDER BY completed_at DESC LIMIT 1"
    ).fetch_optional(&state.db).await
        .map_err(|e| internal_error("compliance report scan lookup", e))?;

    // Fetch overview from agent
    let overview = agent.get("/security/overview").await.ok();

    // Fetch findings if scan exists
    let findings: Vec<(String, String, String, Option<String>, Option<String>)> = if let Some((scan_id, ..)) = &scan {
        sqlx::query_as(
            "SELECT severity, title, description, file_path, remediation FROM security_findings \
             WHERE scan_id = $1 ORDER BY CASE severity WHEN 'critical' THEN 0 WHEN 'warning' THEN 1 ELSE 2 END"
        ).bind(scan_id).fetch_all(&state.db).await.unwrap_or_default()
    } else {
        Vec::new()
    };

    let score = scan.as_ref().map(|(_, _, _f, c, w, _, _)| {
        let s = 100 - (c * 20) - (w * 5);
        if s < 0 { 0 } else { s }
    }).unwrap_or(-1);

    let scan_date = scan.as_ref().and_then(|(_, _, _, _, _, _, completed)| completed.as_ref())
        .map(|d| d.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| "No scan available".into());

    let fw_active = overview.as_ref().and_then(|o| o.get("firewall_active")).and_then(|v| v.as_bool()).unwrap_or(false);
    let f2b_running = overview.as_ref().and_then(|o| o.get("fail2ban_running")).and_then(|v| v.as_bool()).unwrap_or(false);
    let ssh_pw = overview.as_ref().and_then(|o| o.get("ssh_password_auth")).and_then(|v| v.as_bool()).unwrap_or(true);
    let ssh_root = overview.as_ref().and_then(|o| o.get("ssh_root_login")).and_then(|v| v.as_bool()).unwrap_or(true);
    let ssh_port = overview.as_ref().and_then(|o| o.get("ssh_port")).and_then(|v| v.as_u64()).unwrap_or(22);
    let ssl_count = overview.as_ref().and_then(|o| o.get("ssl_certs_count")).and_then(|v| v.as_u64()).unwrap_or(0);

    let (total, critical, warning, _info) = scan.as_ref()
        .map(|(_, _, f, c, w, i, _)| (*f, *c, *w, *i))
        .unwrap_or((0, 0, 0, 0));

    let findings_html: String = findings.iter().map(|(severity, title, description, _file, remediation)| {
        let color = match severity.as_str() {
            "critical" => "#ef4444",
            "warning" => "#f59e0b",
            _ => "#3b82f6",
        };
        format!(
            "<tr><td style=\"padding:8px;border-bottom:1px solid #333;\"><span style=\"color:{color};font-weight:bold;\">{severity}</span></td>\
             <td style=\"padding:8px;border-bottom:1px solid #333;\">{title}</td>\
             <td style=\"padding:8px;border-bottom:1px solid #333;\">{description}</td>\
             <td style=\"padding:8px;border-bottom:1px solid #333;\">{}</td></tr>",
            remediation.as_deref().unwrap_or(""),
        )
    }).collect();

    let score_color = if score >= 80 { "#22c55e" } else if score >= 50 { "#f59e0b" } else { "#ef4444" };

    let no_findings = if findings.is_empty() { "<p style=\"color:#525252;\">No findings — run a security scan first.</p>" } else { "" };

    let html = format!(r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>DockPanel Security Report</title>
<style>
body {{ font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif; max-width: 900px; margin: 0 auto; padding: 40px 20px; background: #0a0a0a; color: #e5e5e5; }}
h1 {{ color: #22c55e; font-size: 24px; }}
h2 {{ color: #a3a3a3; font-size: 16px; text-transform: uppercase; letter-spacing: 2px; margin-top: 40px; border-bottom: 1px solid #333; padding-bottom: 8px; }}
.score {{ font-size: 72px; font-weight: bold; color: {score_color}; }}
.grid {{ display: grid; grid-template-columns: repeat(3, 1fr); gap: 16px; margin: 20px 0; }}
.card {{ background: #1a1a1a; border: 1px solid #333; border-radius: 8px; padding: 16px; }}
.card .label {{ font-size: 12px; color: #737373; text-transform: uppercase; letter-spacing: 1px; }}
.card .value {{ font-size: 20px; font-weight: bold; margin-top: 4px; }}
.pass {{ color: #22c55e; }}
.fail {{ color: #ef4444; }}
table {{ width: 100%; border-collapse: collapse; background: #1a1a1a; border: 1px solid #333; border-radius: 8px; overflow: hidden; }}
th {{ text-align: left; padding: 8px; background: #111; color: #737373; font-size: 12px; text-transform: uppercase; letter-spacing: 1px; }}
.footer {{ margin-top: 40px; color: #525252; font-size: 12px; text-align: center; }}
</style></head><body>
<h1>DockPanel Security Report</h1>
<p style="color:#737373;">Generated: {scan_date}</p>

<h2>Security Score</h2>
<p class="score">{score}/100</p>

<h2>Infrastructure Status</h2>
<div class="grid">
<div class="card"><div class="label">Firewall (UFW)</div><div class="value {fw_class}">{fw_label}</div></div>
<div class="card"><div class="label">Fail2Ban</div><div class="value {f2b_class}">{f2b_label}</div></div>
<div class="card"><div class="label">SSH Password Auth</div><div class="value {ssh_pw_class}">{ssh_pw_label}</div></div>
<div class="card"><div class="label">SSH Root Login</div><div class="value {ssh_root_class}">{ssh_root_label}</div></div>
<div class="card"><div class="label">SSH Port</div><div class="value">{ssh_port}</div></div>
<div class="card"><div class="label">SSL Certificates</div><div class="value">{ssl_count}</div></div>
</div>

<h2>Scan Summary</h2>
<div class="grid">
<div class="card"><div class="label">Total Findings</div><div class="value">{total}</div></div>
<div class="card"><div class="label">Critical</div><div class="value fail">{critical}</div></div>
<div class="card"><div class="label">Warning</div><div class="value" style="color:#f59e0b;">{warning}</div></div>
</div>

<h2>Findings Detail</h2>
<table>
<thead><tr><th>Severity</th><th>Finding</th><th>Description</th><th>Remediation</th></tr></thead>
<tbody>{findings_html}</tbody>
</table>
{no_findings}

<div class="footer">
<p>Generated by DockPanel Security Scanner</p>
</div>
</body></html>"#,
        fw_class = if fw_active { "pass" } else { "fail" },
        fw_label = if fw_active { "Active" } else { "Inactive" },
        f2b_class = if f2b_running { "pass" } else { "fail" },
        f2b_label = if f2b_running { "Running" } else { "Stopped" },
        ssh_pw_class = if !ssh_pw { "pass" } else { "fail" },
        ssh_pw_label = if !ssh_pw { "Disabled" } else { "Enabled" },
        ssh_root_class = if !ssh_root { "pass" } else { "fail" },
        ssh_root_label = if !ssh_root { "Disabled" } else { "Enabled" },
    );

    Ok(Html(html))
}

// ═══════════════════════════════════════════════════════════════════════
// Security Hardening Endpoints (Features 9, 10, 11, 8)
// ═══════════════════════════════════════════════════════════════════════

/// GET /api/security/lockdown — Get lockdown state.
pub async fn lockdown_status(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let lockdown = security_hardening::get_lockdown_state(&state.db).await;
    Ok(Json(lockdown))
}

/// POST /api/security/lockdown/activate — Manual lockdown (admin only).
pub async fn lockdown_activate(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let reason = body.get("reason").and_then(|r| r.as_str()).unwrap_or("Manual admin lockdown");

    security_hardening::activate_lockdown(&state.db, "admin", reason).await;
    security_hardening::audit_log(
        &state.db, "lockdown.manual", Some(&claims.email), None,
        Some("system"), None, Some(reason), None, "critical",
    ).await;
    security_hardening::alert_lockdown(&state.db, reason, &format!("admin:{}", claims.email)).await;

    Ok(Json(serde_json::json!({ "status": "locked", "reason": reason })))
}

/// POST /api/security/lockdown/deactivate — Unlock (admin only).
pub async fn lockdown_deactivate(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    security_hardening::deactivate_lockdown(&state.db, &claims.email).await;
    security_hardening::audit_log(
        &state.db, "lockdown.deactivate", Some(&claims.email), None,
        Some("system"), None, None, None, "info",
    ).await;

    Ok(Json(serde_json::json!({ "status": "unlocked" })))
}

/// POST /api/security/panic — Emergency panic button (Feature 11).
/// Kills all terminal sessions, blocks non-admins, disables registration.
pub async fn panic_button(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let reason = "PANIC BUTTON pressed by admin";

    // Activate lockdown
    security_hardening::activate_lockdown(&state.db, "panic", reason).await;

    // Tell agent to kill all terminal sessions
    let _ = agent.post("/security/kill-terminals", Some(serde_json::json!({}))).await;

    // Disable self-registration at DB level
    let _ = sqlx::query("INSERT INTO settings (key, value) VALUES ('self_registration_enabled', 'false') ON CONFLICT (key) DO UPDATE SET value = 'false'")
        .execute(&state.db).await;

    // Audit + alert
    security_hardening::audit_log(
        &state.db, "panic", Some(&claims.email), None,
        Some("system"), None, Some(reason), None, "critical",
    ).await;
    security_hardening::alert_lockdown(&state.db, reason, &format!("panic:{}", claims.email)).await;

    Ok(Json(serde_json::json!({
        "status": "panic_activated",
        "terminals_killed": true,
        "registration_disabled": true,
        "lockdown_active": true,
    })))
}

/// POST /api/security/forensic-snapshot — Capture forensic state (Feature 10).
pub async fn forensic_snapshot(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent
        .get("/security/forensic-snapshot")
        .await
        .map_err(|e| agent_error("Forensic snapshot", e))?;

    security_hardening::audit_log(
        &state.db, "forensic.snapshot", Some(&claims.email), None,
        Some("system"), None, Some("Forensic snapshot captured"), None, "info",
    ).await;

    Ok(Json(result))
}

/// GET /api/security/audit-log — Query immutable audit log.
pub async fn audit_log_list(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let limit: i64 = params.get("limit").and_then(|v| v.parse().ok()).unwrap_or(100).min(500);
    let offset: i64 = params.get("offset").and_then(|v| v.parse().ok()).unwrap_or(0);
    let severity = params.get("severity").map(|s| s.as_str());

    let rows: Vec<(uuid::Uuid, String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, String, chrono::DateTime<chrono::Utc>)> = if let Some(sev) = severity {
        sqlx::query_as(
            "SELECT id, event_type, actor_email, actor_ip, target_type, target_name, details, geo_country, geo_city, geo_isp, severity, created_at \
             FROM security_audit_log WHERE severity = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3"
        )
        .bind(sev).bind(limit).bind(offset)
        .fetch_all(&state.db).await
    } else {
        sqlx::query_as(
            "SELECT id, event_type, actor_email, actor_ip, target_type, target_name, details, geo_country, geo_city, geo_isp, severity, created_at \
             FROM security_audit_log ORDER BY created_at DESC LIMIT $1 OFFSET $2"
        )
        .bind(limit).bind(offset)
        .fetch_all(&state.db).await
    }.map_err(|e| internal_error("audit log list", e))?;

    let result: Vec<serde_json::Value> = rows.iter().map(|r| {
        serde_json::json!({
            "id": r.0, "event_type": r.1, "actor_email": r.2, "actor_ip": r.3,
            "target_type": r.4, "target_name": r.5, "details": r.6,
            "geo_country": r.7, "geo_city": r.8, "geo_isp": r.9,
            "severity": r.10, "created_at": r.11,
        })
    }).collect();

    Ok(Json(result))
}

/// GET /api/security/recordings — List terminal session recordings (Feature 5).
pub async fn recordings_list(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let dir = "/var/lib/dockpanel/recordings";
    let mut recordings = Vec::new();

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                recordings.push(serde_json::json!({
                    "filename": entry.file_name().to_string_lossy(),
                    "size_bytes": meta.len(),
                    "created": meta.created().ok().map(|t| {
                        chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339()
                    }),
                }));
            }
        }
    }

    recordings.sort_by(|a, b| b["created"].as_str().cmp(&a["created"].as_str()));
    Ok(Json(serde_json::json!({ "recordings": recordings })))
}

/// POST /api/security/users/{id}/approve — Approve a pending user (Feature 8).
pub async fn approve_user(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(user_id): Path<uuid::Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = sqlx::query(
        "UPDATE users SET approved = TRUE, approved_at = NOW(), approved_by = $1 WHERE id = $2"
    )
    .bind(claims.sub)
    .bind(user_id)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("approve user", e))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "User not found"));
    }

    security_hardening::audit_log(
        &state.db, "user.approve", Some(&claims.email), None,
        Some("user"), Some(&user_id.to_string()), None, None, "info",
    ).await;

    Ok(Json(serde_json::json!({ "status": "approved" })))
}

/// GET /api/security/pending-users — List users awaiting approval (Feature 8).
pub async fn pending_users(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let users: Vec<(uuid::Uuid, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT id, email, created_at FROM users WHERE approved = FALSE ORDER BY created_at DESC LIMIT 500"
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("pending users", e))?;

    let result: Vec<serde_json::Value> = users.iter().map(|(id, email, created)| {
        serde_json::json!({ "id": id, "email": email, "created_at": created })
    }).collect();

    Ok(Json(result))
}
