use crate::safe_cmd::safe_command;
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

/// POST /security/kill-terminals — Kill all active terminal sessions (Feature 11: Panic).
async fn kill_terminals() -> Json<serde_json::Value> {
    // Reset the active terminal counter (sessions will close when PTY dies)
    let killed = super::terminal::ACTIVE_TERMINALS.swap(0, std::sync::atomic::Ordering::SeqCst);

    // Kill all PTY child processes owned by www-data (site terminals)
    let _ = safe_command("pkill")
        .args(["-u", "www-data", "-f", "bash"])
        .output().await;

    tracing::warn!("PANIC: Killed {} terminal sessions", killed);
    Json(serde_json::json!({ "killed": killed }))
}

/// GET /security/forensic-snapshot — Capture system state for forensics (Feature 10).
async fn forensic_snapshot() -> Result<Json<serde_json::Value>, ApiErr> {
    use crate::safe_cmd::safe_command;

    let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let dir = format!("/var/lib/dockpanel/forensics/snapshot-{ts}");
    let _ = std::fs::create_dir_all(&dir);

    // Capture running processes
    let ps = safe_command("ps").args(["auxf"]).output().await.ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    let _ = std::fs::write(format!("{dir}/processes.txt"), &ps);

    // Capture network connections
    let ss = safe_command("ss").args(["-tulnp"]).output().await.ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    let _ = std::fs::write(format!("{dir}/network.txt"), &ss);

    // Capture established connections
    let ss_est = safe_command("ss").args(["-tnp"]).output().await.ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    let _ = std::fs::write(format!("{dir}/connections.txt"), &ss_est);

    // Capture open files
    let lsof = safe_command("lsof").args(["-nP", "+L1"]).output().await.ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    let _ = std::fs::write(format!("{dir}/open_files.txt"), &lsof);

    // Capture recent journal
    let journal = safe_command("journalctl")
        .args(["--since", "1 hour ago", "--no-pager", "-q"])
        .output().await.ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    let _ = std::fs::write(format!("{dir}/journal.txt"), &journal);

    // Capture who is logged in
    let who = safe_command("who").output().await.ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    let _ = std::fs::write(format!("{dir}/who.txt"), &who);

    // Capture last logins
    let last = safe_command("last").args(["-20"]).output().await.ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    let _ = std::fs::write(format!("{dir}/last.txt"), &last);

    // Capture /etc/passwd current state
    let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
    let _ = std::fs::write(format!("{dir}/etc_passwd.txt"), &passwd);

    // Capture active terminal recordings
    let recordings = std::fs::read_dir("/var/lib/dockpanel/recordings")
        .map(|d| d.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).collect::<Vec<_>>())
        .unwrap_or_default();
    let _ = std::fs::write(format!("{dir}/active_recordings.txt"), recordings.join("\n"));

    tracing::info!("Forensic snapshot captured at {dir}");

    Ok(Json(serde_json::json!({
        "snapshot_dir": dir,
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "files": ["processes.txt", "network.txt", "connections.txt", "open_files.txt", "journal.txt", "who.txt", "last.txt", "etc_passwd.txt", "active_recordings.txt"],
    })))
}

/// POST /security/db-backup — Backup DockPanel's own PostgreSQL database (Feature 2).
async fn db_backup() -> Result<Json<serde_json::Value>, ApiErr> {
    use crate::safe_cmd::safe_command;

    let backup_dir = "/var/backups/dockpanel";
    let _ = std::fs::create_dir_all(backup_dir);

    let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let filename = format!("{backup_dir}/dockpanel-db-{ts}.sql.gz");

    // pg_dump via Docker exec, piped to gzip
    let output = safe_command("sh")
        .args(["-c", &format!(
            "docker exec dockpanel-postgres pg_dump -U dockpanel dockpanel | gzip > {filename}"
        )])
        .output()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("pg_dump failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!("pg_dump failed: {stderr}")));
    }

    // Cleanup old backups (keep last 7 days)
    if let Ok(entries) = std::fs::read_dir(backup_dir) {
        let mut files: Vec<_> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("dockpanel-db-"))
            .collect();
        files.sort_by_key(|e| std::cmp::Reverse(e.file_name()));
        for old in files.iter().skip(7) {
            let _ = std::fs::remove_file(old.path());
        }
    }

    let size = std::fs::metadata(&filename).map(|m| m.len()).unwrap_or(0);
    tracing::info!("DockPanel DB backup created: {filename} ({size} bytes)");

    Ok(Json(serde_json::json!({
        "filename": filename,
        "size_bytes": size,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    })))
}

/// POST /security/init — Initialize all security hardening features (called once after deploy).
async fn security_init() -> Result<Json<serde_json::Value>, ApiErr> {
    use crate::safe_cmd::safe_command;

    let mut results = Vec::new();

    // Set chattr +a on audit directory
    let _ = std::fs::create_dir_all("/var/lib/dockpanel/audit");
    let chattr = safe_command("chattr").args(["+a", "/var/lib/dockpanel/audit/"]).output().await;
    results.push(serde_json::json!({
        "action": "chattr +a /var/lib/dockpanel/audit/",
        "success": chattr.map(|o| o.status.success()).unwrap_or(false),
    }));

    // Ensure recording directory exists
    let _ = std::fs::create_dir_all("/var/lib/dockpanel/recordings");
    results.push(serde_json::json!({ "action": "create recordings dir", "success": true }));

    // Ensure forensics directory exists
    let _ = std::fs::create_dir_all("/var/lib/dockpanel/forensics");
    results.push(serde_json::json!({ "action": "create forensics dir", "success": true }));

    // Ensure DB backup directory exists
    let _ = std::fs::create_dir_all("/var/backups/dockpanel");
    results.push(serde_json::json!({ "action": "create db backup dir", "success": true }));

    Ok(Json(serde_json::json!({ "initialized": results })))
}

/// POST /security/canary/setup — Create canary files in sensitive directories (Feature 12).
async fn canary_setup() -> Result<Json<serde_json::Value>, ApiErr> {
    let canary_locations = [
        ("/etc/.dockpanel-canary", "System config directory canary"),
        ("/root/.dockpanel-canary", "Root home directory canary"),
        ("/home/.dockpanel-canary", "Home directories canary"),
        ("/var/www/.dockpanel-canary", "Web root canary"),
    ];

    let mut created = Vec::new();
    for (path, desc) in &canary_locations {
        let content = format!(
            "DOCKPANEL CANARY FILE — DO NOT TOUCH\n\
             Created: {}\n\
             Purpose: Intrusion detection tripwire\n\
             If you are reading this, security has been alerted.\n",
            chrono::Utc::now().to_rfc3339()
        );

        if std::fs::write(path, &content).is_ok() {
            // Set permissions: readable by all (so attackers trigger it), but hidden
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o444));
            created.push(serde_json::json!({ "path": path, "description": desc }));
            tracing::info!("Canary file created: {path}");
        }
    }

    Ok(Json(serde_json::json!({
        "canaries": created,
        "monitoring": "inotify-based monitoring recommended"
    })))
}

use std::os::unix::fs::PermissionsExt;

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
        // Security Hardening (post-incident features)
        .route("/security/init", post(security_init))
        .route("/security/kill-terminals", post(kill_terminals))
        .route("/security/forensic-snapshot", get(forensic_snapshot))
        .route("/security/db-backup", post(db_backup))
        .route("/security/canary/setup", post(canary_setup))
}
