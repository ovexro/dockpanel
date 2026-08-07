use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Html,
    Json,
};

use std::time::Duration;

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
    ServerScope(server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Panel logins from activity_logs.
    //
    // This reads `user_email` and `ip_address`, which is the only way this table
    // can identify anything: the login writers pass `None` for both `target_type`
    // and `target_name`, so the `target_name` this used to select as "user" was
    // NULL on every row ever written. The account is in `user_email` and the
    // origin is in `ip_address` — the two facts an operator looking at a failed
    // login actually needs, and neither was leaving the database.
    let panel_logins: Result<
        Vec<(
            String,
            String,
            String,
            Option<String>,
            chrono::DateTime<chrono::Utc>,
        )>,
        sqlx::Error,
    > = sqlx::query_as(
        "SELECT action, COALESCE(details, ''), user_email, ip_address, created_at \
             FROM activity_logs \
             WHERE action IN ('auth.login', 'auth.login_failed', 'auth.2fa_verify') \
             ORDER BY created_at DESC LIMIT 50",
    )
    .fetch_all(&state.db)
    .await;

    // Distinguish "no panel logins" from "we could not read the table". The same
    // reassuring-direction fault the SSH half had, one query up: an
    // `unwrap_or_default()` here turns a database error into an empty list, and
    // an empty login-attempt list is the single most calming thing this endpoint
    // can display. Reported as a flag rather than a hard failure so a broken
    // panel query does not also cost the operator the SSH half.
    let panel_query_failed = panel_logins.is_err();
    if let Err(e) = &panel_logins {
        tracing::warn!("login audit: panel login query failed — showing SSH half only: {e}");
    }
    let panel_logins = panel_logins.unwrap_or_default();

    let panel: Vec<serde_json::Value> = panel_logins
        .iter()
        .map(|(action, details, email, ip, time)| {
            serde_json::json!({
                "type": "panel",
                "action": action,
                "details": details,
                "user": email,
                "ip": ip,
                // `details` is "unknown_user" exactly when the attempt named an
                // email with no account — the enumeration signature. Surfaced as
                // its own flag so the UI does not have to parse a free-text field.
                "unknown_user": details == "unknown_user",
                "time": time,
                "success": !action.contains("failed"),
            })
        })
        .collect();

    // SSH logins, from every member of the fleet rather than from whichever one
    // the caller happens to have selected in the server picker.
    //
    // The two halves of this response are rendered as siblings — one table
    // headed "panel logins", one headed "SSH logins" — and until now they were
    // not comparable at all. The panel half is a query over `activity_logs`,
    // which is a single panel-wide table, so it has always meant every login to
    // this panel. The SSH half came from ONE agent. On a fleet that made "SSH
    // logins" silently mean "SSH logins on the server currently selected", and a
    // brute-force campaign against a member's sshd was invisible unless the
    // operator happened to have that member picked. Nothing on the page said so.
    //
    // The subject list is the one `panic_button` below settled on for the same
    // reason: `online_fleet()` — the list the security scanner, the image
    // scanner, the alert engine and the auto-healer all sweep — plus the
    // caller-selected server even when it is not currently `online`, because
    // that host is the one this endpoint has always answered for and must not
    // silently stop answering for.
    //
    // What "not asked" means here is narrower than it does on the panic button
    // but it is still a fact the operator needs: nobody read that machine's
    // auth.log, so its absence from the table is not evidence of anything. That
    // covers a member whose request failed, a member marked `offline`, and a
    // member whose agent will not resolve (which is what `online_fleet` skips).
    // The last two are deliberately not dialled — a dead host costs a full
    // request timeout — but they are named in the response rather than omitted
    // from it, which is precisely what the old `Err(_) => json!([])` refused to
    // do: it folded a host that could not be asked into a host with nothing to
    // report, and those read identically on screen while meaning opposite things.
    let census: Vec<(uuid::Uuid, String, String, bool)> = sqlx::query_as(
        "SELECT id, name, status, is_local FROM servers WHERE status != 'pending' \
         ORDER BY is_local DESC, name ASC",
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let mut targets: Vec<(uuid::Uuid, String, bool, crate::services::agent::AgentHandle)> = state
        .agents
        .online_fleet()
        .await
        .into_iter()
        .map(|m| (m.id, m.name, m.is_local, m.agent))
        .collect();
    if !targets.iter().any(|(id, ..)| *id == server_id) {
        let (name, is_local) = census
            .iter()
            .find(|(id, ..)| *id == server_id)
            .map(|(_, name, _, is_local)| (name.clone(), *is_local))
            .unwrap_or_else(|| ("selected server".to_string(), false));
        targets.push((server_id, name, is_local, agent));
    }

    // Concurrently, so a fleet of N hosts costs one round trip rather than N.
    // The agent route is a GET (`/security/login-audit` is registered with
    // `get`), so this cannot borrow the panic button's `post_long` verb — but it
    // takes the bound that call was given for the same reason. `AgentHandle::get`
    // is capped at 60s, which is a defensible ceiling for one deliberate action
    // and an indefensible one for a table that loads with a page: a single
    // wedged member would hold the whole security view for a minute and then
    // show it. Fifteen seconds is far more than tailing 500 lines of auth.log
    // needs, and a host named as unasked is more useful than a host reported
    // accurately a minute late.
    let results: Vec<Result<serde_json::Value, String>> =
        futures::future::join_all(targets.iter().map(|(_, _, _, agent)| {
            tokio::time::timeout(Duration::from_secs(15), agent.get("/security/login-audit"))
        }))
        .await
        .into_iter()
        .map(|r| match r {
            Ok(answer) => answer.map_err(|e| e.to_string()),
            Err(_) => Err("agent did not answer within 15s".to_string()),
        })
        .collect();

    let mut ssh: Vec<serde_json::Value> = Vec::new();
    let mut ssh_servers: Vec<serde_json::Value> = Vec::with_capacity(census.len().max(targets.len()));
    let mut unreachable: Vec<serde_json::Value> = Vec::new();
    let mut unreachable_names: Vec<String> = Vec::new();
    let mut reached: usize = 0;

    for ((id, name, is_local, _), res) in targets.iter().zip(results.into_iter()) {
        match res {
            Ok(v) => {
                let entries = v
                    .get("entries")
                    .and_then(|e| e.as_array())
                    .cloned()
                    .unwrap_or_default();
                let count = entries.len();
                for mut entry in entries {
                    // Stamp the host onto every row. `time` is a syslog stamp
                    // with no year in it, so rows from different machines cannot
                    // be merged into one trustworthy chronology — which makes
                    // the origin the only thing that lets a reader tell two
                    // identical-looking failed logins apart. Rows keep their
                    // per-host order and the hosts keep `online_fleet`'s order.
                    if let Some(obj) = entry.as_object_mut() {
                        obj.insert("server_id".to_string(), serde_json::json!(id));
                        obj.insert("server_name".to_string(), serde_json::json!(name));
                        obj.insert("is_local".to_string(), serde_json::json!(is_local));
                    }
                    ssh.push(entry);
                }
                reached += 1;
                ssh_servers.push(serde_json::json!({
                    "server_id": id,
                    "server_name": name,
                    "is_local": is_local,
                    "reached": true,
                    "entries": count,
                }));
            }
            Err(why) => {
                tracing::warn!("login audit: {name} ({id}) did not answer — its SSH logins are missing from this view: {why}");
                ssh_servers.push(serde_json::json!({
                    "server_id": id,
                    "server_name": name,
                    "is_local": is_local,
                    "reached": false,
                    "entries": null,
                    "error": why,
                }));
                unreachable.push(serde_json::json!({
                    "server_id": id, "server_name": name, "reason": why,
                }));
                unreachable_names.push(name.clone());
            }
        }
    }

    // The members that were never dialled at all. They belong in the same list
    // as the ones that failed, because from the reader's side the fact is
    // identical: that machine's auth.log was not read.
    for (id, name, status, is_local) in &census {
        if targets.iter().any(|(tid, ..)| tid == id) {
            continue;
        }
        let why = if status.as_str() == "online" {
            "no reachable agent configured for this server"
        } else {
            "server is not online — not asked"
        };
        ssh_servers.push(serde_json::json!({
            "server_id": id,
            "server_name": name,
            "is_local": is_local,
            "reached": false,
            "entries": null,
            "error": why,
        }));
        unreachable.push(serde_json::json!({
            "server_id": id, "server_name": name, "reason": why,
        }));
        unreachable_names.push(name.clone());
    }

    let ssh_servers_total = ssh_servers.len();
    let ssh_servers_unreachable = unreachable.len();

    Ok(Json(serde_json::json!({
        // Legacy pair — same names, same types. `ssh` is now the concatenation
        // of every host that answered, with each row carrying its origin, so a
        // client that only understands the old shape gets strictly more of the
        // data it was already rendering rather than a different kind of data.
        "panel": panel,
        "ssh": ssh,
        // Coverage. `ssh_partial` is the one field a client must not ignore: it
        // is true whenever at least one machine's auth.log went unread, which is
        // the difference between "no SSH logins" and "we could not ask".
        "ssh_partial": ssh_servers_unreachable > 0,
        "ssh_servers": ssh_servers,
        "ssh_servers_total": ssh_servers_total,
        "ssh_servers_reached": reached,
        "ssh_servers_unreachable": ssh_servers_unreachable,
        "ssh_unreachable_servers": unreachable,
        "ssh_unreachable_names": unreachable_names,
        // The panel half is a single panel-wide table and cannot be partial by
        // host; it can only fail outright, and this says whether it did.
        "panel_query_failed": panel_query_failed,
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
    ServerScope(server_id, agent): ServerScope,
) -> Result<Html<String>, ApiError> {
    // Fetch the latest scan OF THIS SERVER. The handler already scoped the
    // agent it asks for the live overview below, then paired that answer with
    // whichever server's scan finished most recently — a compliance document
    // built to be filed and forwarded, describing two machines as one.
    let scan: Option<(uuid::Uuid, String, i32, i32, i32, i32, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT id, status, findings_count, critical_count, warning_count, info_count, completed_at \
         FROM security_scans WHERE server_id = $1 AND status = 'completed' \
         ORDER BY completed_at DESC LIMIT 1"
    ).bind(server_id).fetch_optional(&state.db).await
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
    let mut lockdown = security_hardening::get_lockdown_state(&state.db).await;

    // `get_lockdown_state` returns whether a lockdown is on, who started it, when
    // and why — and none of the three columns that say what it actually DOES.
    // `terminals_disabled` is the one an operator has a live reason to ask about:
    // the panic button sets it, the terminal ticket mint now refuses on it, and
    // between those two facts there was no surface anywhere that would show the
    // state the emergency control claims to have set.
    //
    // Two fields, because they answer two different questions and conflating them
    // is the trap the column itself is built on. `terminals_disabled` is the
    // stored value: it reads TRUE on a panel that has never been locked down
    // (the migration's DEFAULT) and stays TRUE after an unlock (nothing clears
    // it), so on its own it is not the answer to "can anyone open a shell right
    // now". `terminals_blocked` is that answer, and it is computed by the same
    // function the mint calls, so the page and the gate cannot drift apart.
    if let Some(obj) = lockdown.as_object_mut() {
        let (active, terminals_disabled) =
            crate::routes::terminal::lockdown_terminal_state(&state.db).await;
        obj.insert(
            "terminals_disabled".to_string(),
            serde_json::json!(terminals_disabled),
        );
        obj.insert(
            "terminals_blocked".to_string(),
            serde_json::json!(active && terminals_disabled),
        );
    }

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
    ServerScope(server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let reason = "PANIC BUTTON pressed by admin";

    // Activate lockdown
    security_hardening::activate_lockdown(&state.db, "panic", reason).await;

    // Kill terminal sessions on EVERY member of the fleet, not just the one the
    // caller happens to have selected in the server picker.
    //
    // Every other leg of this handler is panel-global: `activate_lockdown`
    // writes the single `lockdown_state` row, `sessions_revoked_at` invalidates
    // every token the panel has ever minted, and the share sweep deletes every
    // `terminal_share_%` key there is. The kill was the one leg still scoped to
    // a single host, so on a fleet the panic button returned `lockdown_active:
    // true` and a terminal count while live root shells kept running on every
    // other member. The response was honest about the host it asked and silent
    // about the hosts it did not, which on an emergency control reads as a
    // complete result and is worse than no result at all.
    //
    // The subject list is `online_fleet()` — the same list the security scanner,
    // the image scanner, the alert engine and the auto-healer sweep — plus the
    // caller-selected server even when it is not currently `online`, because
    // that host was already being killed before this change and must not
    // silently stop being killed.
    //
    // What "unreachable" means here, and it means only one thing: NOBODY KILLED
    // ANYTHING ON THAT MACHINE, so any root shell an intruder is holding there
    // is still holding. That covers a member whose kill request failed, a member
    // marked `offline`, and a member whose agent will not resolve (which is
    // precisely what `online_fleet` skips). Those last two are deliberately not
    // dialled — a dead host costs a full request timeout each and this is the
    // button an operator presses in a hurry — but they are named in the
    // response rather than omitted from it. One unreachable member is enough to
    // make the legacy `agent_reached` field false for the whole call, so a
    // client that only understands the old single-host shape still shows its
    // "terminals may still be running" warning instead of a reassuring count.
    let census: Vec<(uuid::Uuid, String, String, bool)> = sqlx::query_as(
        "SELECT id, name, status, is_local FROM servers WHERE status != 'pending' \
         ORDER BY is_local DESC, name ASC",
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let mut targets: Vec<(uuid::Uuid, String, bool, crate::services::agent::AgentHandle)> = state
        .agents
        .online_fleet()
        .await
        .into_iter()
        .map(|m| (m.id, m.name, m.is_local, m.agent))
        .collect();
    if !targets.iter().any(|(id, ..)| *id == server_id) {
        let (name, is_local) = census
            .iter()
            .find(|(id, ..)| *id == server_id)
            .map(|(_, name, _, is_local)| (name.clone(), *is_local))
            .unwrap_or_else(|| ("selected server".to_string(), false));
        targets.push((server_id, name, is_local, agent));
    }

    // Concurrently, like the metrics collector: a fleet of N hosts must not cost
    // N round trips while an intruder holds a shell, and one wedged member must
    // not delay the kill on the others. The 15s bound is far more than a SIGKILL
    // sweep of a PID registry needs and far less than the client's 120s default;
    // during a panic an unconfirmed host reported NOW beats a confirmation two
    // minutes from now, and the error it produces points the operator at a
    // machine to go and check by hand rather than away from one.
    let results = futures::future::join_all(targets.iter().map(|(_, _, _, agent)| {
        agent.post_long("/security/kill-terminals", Some(serde_json::json!({})), 15)
    }))
    .await;

    let mut terminals_total: u64 = 0;
    let mut root_total: u64 = 0;
    let mut reached: usize = 0;
    let mut server_results: Vec<serde_json::Value> = Vec::with_capacity(census.len().max(targets.len()));
    let mut unreachable: Vec<serde_json::Value> = Vec::new();
    let mut unreachable_names: Vec<String> = Vec::new();

    for ((id, name, is_local, _), res) in targets.iter().zip(results.into_iter()) {
        match res {
            Ok(v) => {
                let killed = v.get("killed").and_then(|k| k.as_u64());
                let root = v.get("server_terminals_killed").and_then(|k| k.as_u64());
                terminals_total += killed.unwrap_or(0);
                root_total += root.unwrap_or(0);
                reached += 1;
                server_results.push(serde_json::json!({
                    "server_id": id,
                    "server_name": name,
                    "is_local": is_local,
                    "reached": true,
                    "terminals_killed": killed,
                    "server_terminals_killed": root,
                    "registered": v.get("registered").and_then(|k| k.as_u64()),
                }));
            }
            Err(e) => {
                let why = e.to_string();
                tracing::error!(
                    "PANIC: {name} ({id}) did not kill terminals — root shells may still be live there: {why}"
                );
                server_results.push(serde_json::json!({
                    "server_id": id,
                    "server_name": name,
                    "is_local": is_local,
                    "reached": false,
                    "terminals_killed": null,
                    "server_terminals_killed": null,
                    "error": why,
                }));
                unreachable.push(serde_json::json!({
                    "server_id": id, "server_name": name, "reason": why,
                }));
                unreachable_names.push(name.clone());
            }
        }
    }

    // The members that were never dialled at all. They belong in the same list
    // as the ones that failed, because from the operator's side the fact is
    // identical: that machine was not swept.
    for (id, name, status, is_local) in &census {
        if targets.iter().any(|(tid, ..)| tid == id) {
            continue;
        }
        let why = if status.as_str() == "online" {
            "no reachable agent configured for this server"
        } else {
            "server is not online — not dialled"
        };
        tracing::error!("PANIC: {name} ({id}) not swept ({why}) — root shells may still be live there");
        server_results.push(serde_json::json!({
            "server_id": id,
            "server_name": name,
            "is_local": is_local,
            "reached": false,
            "terminals_killed": null,
            "server_terminals_killed": null,
            "error": why,
        }));
        unreachable.push(serde_json::json!({
            "server_id": id, "server_name": name, "reason": why,
        }));
        unreachable_names.push(name.clone());
    }

    let servers_total = server_results.len();
    let servers_unreachable = unreachable.len();
    // Legacy field, now fleet-wide: true only if NOTHING was left unswept.
    let agent_reached = servers_unreachable == 0 && reached > 0;
    // Legacy fields, now fleet totals. Still `null` when not a single host
    // answered, which is the case the old frontend already renders as a warning.
    let (terminals_killed, server_terminals_killed) = if reached > 0 {
        (Some(terminals_total), Some(root_total))
    } else {
        (None, None)
    };

    // Revoke every issued session. Lockdown is enforced at the doors — login,
    // OAuth, passkey, site creation — and nothing tests it against a token that
    // has already been minted. Without this leg, the stolen admin session that
    // got an intruder their root shell keeps working for the rest of its two
    // hours, and they simply request another terminal. This logs the pressing
    // admin out too; during a panic that is the correct trade, and an admin can
    // log back in while lockdown keeps everyone else out.
    let revoked = sqlx::query(
        "INSERT INTO settings (key, value, updated_at) VALUES ('sessions_revoked_at', $1, NOW()) \
         ON CONFLICT (key) DO UPDATE SET value = $1, updated_at = NOW()",
    )
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&state.db)
    .await
    .is_ok();
    if revoked {
        *state.sessions_revoked_at.write().await = Some(chrono::Utc::now().timestamp());
    }

    // Disable self-registration at DB level
    let _ = sqlx::query("INSERT INTO settings (key, value) VALUES ('self_registration_enabled', 'false') ON CONFLICT (key) DO UPDATE SET value = 'false'")
        .execute(&state.db).await;

    // Revoke every terminal share. A share is an UNAUTHENTICATED public URL
    // (`GET /api/terminal/shared/{id}` carries no auth extractor) holding up to
    // 500KB of operator-supplied terminal output, and lockdown does not reach it
    // — lockdown is tested at the doors, and this is not a door. So without this
    // the panic button left the intruder's own share serving to the whole
    // internet for the rest of its hour, while the UI said "all sessions
    // revoked". `revoke_share`/`list_shares` exist and are correct, but have
    // never had a frontend caller, so there was no in-panel way to close one.
    let shares_revoked = sqlx::query("DELETE FROM settings WHERE key LIKE 'terminal_share_%'")
        .execute(&state.db)
        .await
        .map(|r| r.rows_affected())
        .unwrap_or(0);

    // Audit + alert. Both carry the un-swept host list, not just the bare
    // reason. The response below is the most detailed account of this event that
    // will ever exist and it is also the shortest-lived — the session revocation
    // above logs the pressing admin out, so the browser has one render in which
    // to show it and then the page is gone. The immutable audit row and the
    // out-of-band admin notification are the two places the answer to "which
    // machines did we fail to cover" survives long enough for the incident
    // review to read it.
    let outcome = if unreachable_names.is_empty() {
        format!(
            "{reason} — all {servers_total} server(s) swept, {terminals_total} terminal(s) killed ({root_total} server/root)"
        )
    } else {
        format!(
            "{reason} — {reached}/{servers_total} server(s) swept, {terminals_total} terminal(s) killed ({root_total} server/root). NOT SWEPT, root shells may still be live: {}",
            unreachable_names.join(", ")
        )
    };
    security_hardening::audit_log(
        &state.db, "panic", Some(&claims.email), None,
        Some("system"), None, Some(outcome.as_str()), None, "critical",
    ).await;
    security_hardening::alert_lockdown(&state.db, &outcome, &format!("panic:{}", claims.email)).await;

    Ok(Json(serde_json::json!({
        "status": "panic_activated",
        // Legacy trio — same names, same types, now fleet-wide in meaning.
        "agent_reached": agent_reached,
        "terminals_killed": terminals_killed,
        "server_terminals_killed": server_terminals_killed,
        // Fleet detail. `unreachable_servers` is the operational payload: each
        // entry is a machine on which root terminals may still be running.
        "servers_total": servers_total,
        "servers_reached": reached,
        "servers_unreachable": servers_unreachable,
        "server_results": server_results,
        "unreachable_servers": unreachable,
        "sessions_revoked": revoked,
        "shares_revoked": shares_revoked,
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
