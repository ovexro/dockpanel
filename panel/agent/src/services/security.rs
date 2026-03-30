use serde::Serialize;
use crate::safe_cmd::safe_command;

#[derive(Serialize)]
pub struct LoginEntry {
    pub time: String,
    pub user: String,
    pub ip: String,
    pub method: String,
    pub success: bool,
}

#[derive(Serialize)]
pub struct FirewallStatus {
    pub active: bool,
    pub default_policy: String,
    pub rules: Vec<FirewallRule>,
}

#[derive(Serialize)]
pub struct FirewallRule {
    pub number: usize,
    pub to: String,
    pub action: String,
    pub from: String,
}

#[derive(Serialize)]
pub struct Fail2banStatus {
    pub running: bool,
    pub jails: Vec<JailInfo>,
}

#[derive(Serialize)]
pub struct JailInfo {
    pub name: String,
    pub banned_count: u32,
}

#[derive(Serialize)]
pub struct SecurityOverview {
    pub firewall_active: bool,
    pub firewall_rules_count: usize,
    pub fail2ban_running: bool,
    pub fail2ban_banned_total: u32,
    pub ssh_port: u16,
    pub ssh_password_auth: bool,
    pub ssh_root_login: bool,
    pub ssl_certs_count: usize,
}

/// Run `ufw status verbose` and parse the output into a `FirewallStatus`.
pub async fn get_firewall_status() -> Result<FirewallStatus, String> {
    let output = match tokio::time::timeout(
        std::time::Duration::from_secs(30),
        safe_command("ufw").args(["status", "verbose"]).output(),
    )
    .await
    {
        Ok(Ok(o)) => o,
        _ => {
            // ufw not installed, timed out, or errored — return inactive status
            return Ok(FirewallStatus {
                active: false,
                default_policy: String::new(),
                rules: Vec::new(),
            });
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Check if active
    let active = stdout.contains("Status: active");

    // Parse default policy — line like "Default: deny (incoming), allow (outgoing), disabled (routed)"
    let default_policy = stdout
        .lines()
        .find(|l| l.starts_with("Default:"))
        .map(|l| l.trim_start_matches("Default:").trim().to_string())
        .unwrap_or_default();

    // Parse rules — they appear after the "---" separator line
    let mut rules = Vec::new();
    let mut in_rules = false;
    let mut rule_num: usize = 0;

    for line in stdout.lines() {
        if line.starts_with("--") {
            in_rules = true;
            continue;
        }
        if !in_rules {
            continue;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Skip "(v6)" duplicate lines — IPv6 rules
        if trimmed.contains("(v6)") {
            continue;
        }

        rule_num += 1;

        // Typical line formats:
        //   22/tcp                     ALLOW IN    Anywhere
        //   80/tcp                     ALLOW IN    192.168.1.0/24
        //   443                        DENY IN     Anywhere
        // Split on whitespace and parse
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() >= 3 {
            let to = parts[0].to_string();
            // Action is typically "ALLOW" or "DENY", possibly followed by "IN"/"OUT"
            let action = if parts.len() >= 3 && (parts[2] == "IN" || parts[2] == "OUT") {
                format!("{} {}", parts[1], parts[2])
            } else {
                parts[1].to_string()
            };
            // From is the last part(s)
            let from_idx = if parts.len() >= 3 && (parts[2] == "IN" || parts[2] == "OUT") {
                3
            } else {
                2
            };
            let from = if from_idx < parts.len() {
                parts[from_idx..].join(" ")
            } else {
                "Anywhere".to_string()
            };

            rules.push(FirewallRule {
                number: rule_num,
                to,
                action,
                from,
            });
        }
    }

    Ok(FirewallStatus {
        active,
        default_policy,
        rules,
    })
}

/// Add a firewall rule via `ufw`.
///
/// `action` should be "allow" or "deny".
/// `proto` should be "tcp" or "udp".
/// If `from` is provided, adds a source-restricted rule.
pub async fn add_firewall_rule(
    port: u16,
    proto: &str,
    action: &str,
    from: Option<&str>,
) -> Result<(), String> {
    // Validate action
    let action_lower = action.to_lowercase();
    if action_lower != "allow" && action_lower != "deny" {
        return Err(format!("Invalid action '{action}': must be 'allow' or 'deny'"));
    }

    // Validate proto
    let proto_lower = proto.to_lowercase();
    if proto_lower != "tcp" && proto_lower != "udp" {
        return Err(format!("Invalid protocol '{proto}': must be 'tcp' or 'udp'"));
    }

    let port_proto = format!("{port}/{proto_lower}");

    let mut args: Vec<String> = vec![action_lower];

    if let Some(source) = from {
        // Validate source IP — basic check for alphanumeric, dots, colons, slashes
        if source.is_empty()
            || !source
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == ':' || c == '/')
        {
            return Err(format!("Invalid source address: {source}"));
        }
        args.push("from".into());
        args.push(source.to_string());
        args.push("to".into());
        args.push("any".into());
        args.push("port".into());
        args.push(port.to_string());
        args.push("proto".into());
        args.push(proto_lower);
    } else {
        args.push(port_proto);
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        safe_command("ufw").args(&args).output(),
    )
    .await
    .map_err(|_| "ufw command timed out".to_string())?
    .map_err(|_| "ufw is not installed".to_string())?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(format!("ufw failed: {stderr} {stdout}"));
    }

    Ok(())
}

/// Delete a firewall rule by its number.
pub async fn remove_firewall_rule(rule_num: usize) -> Result<(), String> {
    if rule_num == 0 {
        return Err("Rule number must be >= 1".into());
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        safe_command("ufw")
            .args(["--force", "delete", &rule_num.to_string()])
            .output(),
    )
    .await
    .map_err(|_| "ufw command timed out".to_string())?
    .map_err(|_| "ufw is not installed".to_string())?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(format!("ufw delete failed: {stderr} {stdout}"));
    }

    Ok(())
}

/// Get fail2ban status: list of active jails and banned IPs count per jail.
pub async fn get_fail2ban_status() -> Result<Fail2banStatus, String> {
    let output = match tokio::time::timeout(
        std::time::Duration::from_secs(30),
        safe_command("fail2ban-client").arg("status").output(),
    )
    .await
    {
        Ok(Ok(o)) if o.status.success() => o,
        _ => {
            // fail2ban not installed, timed out, or not running
            return Ok(Fail2banStatus {
                running: false,
                jails: Vec::new(),
            });
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse jail list from output like:
    //   `- Jail list:	sshd, nginx-http-auth`
    let jail_names: Vec<String> = stdout
        .lines()
        .find(|l| l.contains("Jail list:"))
        .map(|l| {
            l.split("Jail list:")
                .nth(1)
                .unwrap_or("")
                .trim()
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    // For each jail, get banned count
    let mut jails = Vec::new();
    for name in &jail_names {
        let jail_output = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            safe_command("fail2ban-client")
                .args(["status", name])
                .output(),
        )
        .await
        .map_err(|_| format!("Jail query for {name} timed out"))?
        .map_err(|e| format!("Failed to query jail {name}: {e}"))?;

        let jail_stdout = String::from_utf8_lossy(&jail_output.stdout);

        // Parse "Currently banned:" line
        let banned_count = jail_stdout
            .lines()
            .find(|l| l.contains("Currently banned:"))
            .and_then(|l| {
                l.split("Currently banned:")
                    .nth(1)
                    .and_then(|v| v.trim().parse::<u32>().ok())
            })
            .unwrap_or(0);

        jails.push(JailInfo {
            name: name.clone(),
            banned_count,
        });
    }

    Ok(Fail2banStatus {
        running: true,
        jails,
    })
}

/// Read SSH configuration values from /etc/ssh/sshd_config.
/// Returns (port, password_auth_enabled, root_login_enabled).
pub async fn parse_ssh_config() -> (u16, bool, bool) {
    let content = match tokio::fs::read_to_string("/etc/ssh/sshd_config").await {
        Ok(c) => c,
        Err(_) => return (22, true, true), // defaults
    };

    let mut port: u16 = 22;
    let mut password_auth = true;
    let mut root_login = true;

    for line in content.lines() {
        let trimmed = line.trim();
        // Skip comments and empty lines
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }

        match parts[0] {
            "Port" => {
                if let Ok(p) = parts[1].parse::<u16>() {
                    port = p;
                }
            }
            "PasswordAuthentication" => {
                password_auth = parts[1].eq_ignore_ascii_case("yes");
            }
            "PermitRootLogin" => {
                root_login = !parts[1].eq_ignore_ascii_case("no");
            }
            _ => {}
        }
    }

    (port, password_auth, root_login)
}

/// Count SSL certificate directories in /etc/dockpanel/ssl/.
async fn count_ssl_certs() -> usize {
    let mut count = 0;
    let mut entries = match tokio::fs::read_dir("/etc/dockpanel/ssl").await {
        Ok(e) => e,
        Err(_) => return 0,
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        if let Ok(ft) = entry.file_type().await {
            if ft.is_dir() {
                count += 1;
            }
        }
    }

    count
}

/// Aggregate security overview: firewall, fail2ban, SSH, SSL.
pub async fn get_security_overview() -> Result<SecurityOverview, String> {
    let (firewall, fail2ban, ssh, ssl) = tokio::join!(
        get_firewall_status(),
        get_fail2ban_status(),
        parse_ssh_config(),
        count_ssl_certs(),
    );

    let fw = firewall.unwrap_or(FirewallStatus {
        active: false,
        default_policy: String::new(),
        rules: Vec::new(),
    });

    let f2b = fail2ban.unwrap_or(Fail2banStatus {
        running: false,
        jails: Vec::new(),
    });

    let banned_total: u32 = f2b.jails.iter().map(|j| j.banned_count).sum();

    Ok(SecurityOverview {
        firewall_active: fw.active,
        firewall_rules_count: fw.rules.len(),
        fail2ban_running: f2b.running,
        fail2ban_banned_total: banned_total,
        ssh_port: ssh.0,
        ssh_password_auth: ssh.1,
        ssh_root_login: ssh.2,
        ssl_certs_count: ssl,
    })
}

/// Disable SSH password authentication (set PasswordAuthentication no in sshd_config).
pub async fn disable_ssh_password_auth() -> Result<(), String> {
    modify_sshd_config("PasswordAuthentication", "no").await?;
    restart_sshd().await
}

/// Enable SSH password authentication.
pub async fn enable_ssh_password_auth() -> Result<(), String> {
    modify_sshd_config("PasswordAuthentication", "yes").await?;
    restart_sshd().await
}

/// Disable root SSH login (set PermitRootLogin no in sshd_config).
pub async fn disable_ssh_root_login() -> Result<(), String> {
    modify_sshd_config("PermitRootLogin", "no").await?;
    restart_sshd().await
}

/// Change SSH port.
pub async fn change_ssh_port(port: u16) -> Result<(), String> {
    if port == 0 || port > 65535 {
        return Err("Invalid port".into());
    }
    modify_sshd_config("Port", &port.to_string()).await?;
    // Add firewall rule for new port before restarting
    let _ = safe_command("ufw").args(["allow", &format!("{port}/tcp")]).output().await;
    restart_sshd().await
}

/// Modify a single directive in /etc/ssh/sshd_config.
/// If the directive exists (commented or not), replace it. Otherwise append.
async fn modify_sshd_config(key: &str, value: &str) -> Result<(), String> {
    let config_path = "/etc/ssh/sshd_config";
    let content = tokio::fs::read_to_string(config_path).await
        .map_err(|e| format!("Failed to read sshd_config: {e}"))?;

    let mut found = false;
    let mut new_lines: Vec<String> = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        // Match both active and commented-out directives
        if trimmed.starts_with(key) || trimmed.starts_with(&format!("#{key}")) || trimmed.starts_with(&format!("# {key}")) {
            new_lines.push(format!("{key} {value}"));
            found = true;
        } else {
            new_lines.push(line.to_string());
        }
    }

    if !found {
        new_lines.push(format!("{key} {value}"));
    }

    let new_content = new_lines.join("\n") + "\n";

    // Atomic write
    let tmp_path = format!("{config_path}.tmp");
    tokio::fs::write(&tmp_path, &new_content).await
        .map_err(|e| format!("Failed to write sshd_config: {e}"))?;
    tokio::fs::rename(&tmp_path, config_path).await
        .map_err(|e| format!("Failed to rename sshd_config: {e}"))?;

    tracing::info!("SSH config updated: {key} {value}");
    Ok(())
}

/// Restart sshd service.
async fn restart_sshd() -> Result<(), String> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        safe_command("systemctl").args(["restart", "sshd"]).output(),
    ).await
        .map_err(|_| "sshd restart timed out".to_string())?
        .map_err(|e| format!("Failed to restart sshd: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("sshd restart failed: {stderr}"));
    }
    tracing::info!("sshd restarted");
    Ok(())
}

/// Unban an IP from a specific fail2ban jail.
pub async fn fail2ban_unban(jail: &str, ip: &str) -> Result<(), String> {
    // Validate jail name (alphanumeric + hyphens only)
    if !jail.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("Invalid jail name".into());
    }
    // Validate IP (basic check)
    if ip.is_empty() || !ip.chars().all(|c| c.is_ascii_digit() || c == '.' || c == ':') {
        return Err("Invalid IP address".into());
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        safe_command("fail2ban-client").args(["set", jail, "unbanip", ip]).output(),
    ).await
        .map_err(|_| "fail2ban-client timed out".to_string())?
        .map_err(|e| format!("fail2ban-client failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Unban failed: {stderr}"));
    }
    tracing::info!("Unbanned {ip} from jail {jail}");
    Ok(())
}

/// Ban an IP in a specific fail2ban jail.
pub async fn fail2ban_ban(jail: &str, ip: &str) -> Result<(), String> {
    if !jail.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("Invalid jail name".into());
    }
    if ip.is_empty() || !ip.chars().all(|c| c.is_ascii_digit() || c == '.' || c == ':') {
        return Err("Invalid IP address".into());
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        safe_command("fail2ban-client").args(["set", jail, "banip", ip]).output(),
    ).await
        .map_err(|_| "fail2ban-client timed out".to_string())?
        .map_err(|e| format!("fail2ban-client failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Ban failed: {stderr}"));
    }
    tracing::info!("Banned {ip} in jail {jail}");
    Ok(())
}

/// Get list of banned IPs for a specific jail.
pub async fn fail2ban_banned_ips(jail: &str) -> Result<Vec<String>, String> {
    if !jail.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("Invalid jail name".into());
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        safe_command("fail2ban-client").args(["status", jail]).output(),
    ).await
        .map_err(|_| "fail2ban-client timed out".to_string())?
        .map_err(|e| format!("fail2ban-client failed: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse "Banned IP list:" line
    let ips = stdout.lines()
        .find(|l| l.contains("Banned IP list:"))
        .map(|l| {
            l.split("Banned IP list:")
                .nth(1).unwrap_or("").trim()
                .split_whitespace()
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(ips)
}

/// Parse recent SSH login attempts from /var/log/auth.log.
pub async fn get_login_audit() -> Result<Vec<LoginEntry>, String> {
    let content = tokio::fs::read_to_string("/var/log/auth.log")
        .await
        .or_else(|_| std::fs::read_to_string("/var/log/auth.log"))
        .unwrap_or_default();

    let mut entries = Vec::new();

    // Parse lines like:
    // Mar 18 12:34:56 host sshd[1234]: Accepted publickey for user from 1.2.3.4 port 5678
    // Mar 18 12:34:56 host sshd[1234]: Failed password for user from 1.2.3.4 port 5678
    // Mar 18 12:34:56 host sshd[1234]: Failed password for invalid user admin from 1.2.3.4 port 5678
    for line in content.lines().rev().take(500) {
        if !line.contains("sshd[") {
            continue;
        }

        let success = line.contains("Accepted");
        let failed = line.contains("Failed password") || line.contains("Failed publickey");

        if !success && !failed {
            continue;
        }

        // Extract IP
        let ip = line
            .split(" from ")
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
            .unwrap_or("unknown")
            .to_string();

        // Extract user
        let user = if line.contains("invalid user") {
            line.split("invalid user ")
                .nth(1)
                .and_then(|s| s.split(" from").next())
                .unwrap_or("unknown")
                .to_string()
        } else {
            line.split(" for ")
                .nth(1)
                .and_then(|s| s.split(" from").next())
                .unwrap_or("unknown")
                .to_string()
        };

        // Extract timestamp (first 15 chars: "Mar 18 12:34:56")
        let time = if line.len() >= 15 {
            &line[..15]
        } else {
            "unknown"
        };

        // Extract method
        let method = if line.contains("publickey") {
            "publickey"
        } else if line.contains("password") {
            "password"
        } else {
            "unknown"
        };

        entries.push(LoginEntry {
            time: time.to_string(),
            user,
            ip,
            method: method.to_string(),
            success,
        });

        if entries.len() >= 50 {
            break;
        }
    }

    Ok(entries)
}

/// Create a Fail2Ban jail for the DockPanel panel login endpoint.
/// Monitors nginx access log for repeated 401 responses to /api/auth/login.
pub async fn setup_panel_jail() -> Result<(), String> {
    // 1. Create filter file
    let filter = r#"[Definition]
failregex = ^<HOST> .* "POST /api/auth/login HTTP/.*" 401
ignoreregex =
"#;
    tokio::fs::write("/etc/fail2ban/filter.d/dockpanel.conf", filter).await
        .map_err(|e| format!("Failed to write filter: {e}"))?;

    // 2. Create jail config
    // Find the nginx access log for the panel
    let jail = r#"[dockpanel]
enabled = true
filter = dockpanel
port = http,https
logpath = /var/log/nginx/*.access.log
maxretry = 5
findtime = 600
bantime = 3600
"#;
    tokio::fs::write("/etc/fail2ban/jail.d/dockpanel.conf", jail).await
        .map_err(|e| format!("Failed to write jail config: {e}"))?;

    // 3. Restart fail2ban
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        safe_command("systemctl").args(["restart", "fail2ban"]).output(),
    ).await
        .map_err(|_| "fail2ban restart timed out".to_string())?
        .map_err(|e| format!("Failed to restart fail2ban: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("fail2ban restart failed: {stderr}"));
    }

    tracing::info!("DockPanel Fail2Ban jail created and activated");
    Ok(())
}

/// Check if the DockPanel Fail2Ban jail is configured.
pub async fn panel_jail_status() -> bool {
    std::path::Path::new("/etc/fail2ban/jail.d/dockpanel.conf").exists()
}

/// Apply a recommended fix for a security finding.
pub async fn apply_fix(fix_type: &str, target: &str) -> Result<String, String> {
    match fix_type {
        "block_port" => {
            // Block an unexpected open port
            let port: u16 = target.parse().map_err(|_| "Invalid port".to_string())?;
            add_firewall_rule(port, "tcp", "deny", None).await?;
            Ok(format!("Port {port}/tcp blocked"))
        }
        "disable_password_auth" => {
            disable_ssh_password_auth().await?;
            Ok("SSH password authentication disabled".into())
        }
        "disable_root_login" => {
            disable_ssh_root_login().await?;
            Ok("SSH root login disabled".into())
        }
        "remove_file" => {
            // Remove a suspicious file (malware) — canonicalize to prevent symlink attacks
            let canonical = std::fs::canonicalize(target)
                .map_err(|e| format!("Cannot resolve path {target}: {e}"))?;
            let canonical_str = canonical.to_string_lossy();
            if !canonical_str.starts_with("/var/www/") {
                return Err("Can only remove files under /var/www".into());
            }
            tokio::fs::remove_file(&canonical).await
                .map_err(|e| format!("Failed to remove {}: {e}", canonical_str))?;
            Ok(format!("File removed: {}", canonical_str))
        }
        "quarantine_file" => {
            let canonical = std::fs::canonicalize(target)
                .map_err(|e| format!("Cannot resolve path {target}: {e}"))?;
            let canonical_str = canonical.to_string_lossy();
            if !canonical_str.starts_with("/var/www/") {
                return Err("Can only quarantine files under /var/www".into());
            }
            let target = canonical_str.as_ref();
            let quarantine_dir = "/var/lib/dockpanel/quarantine";
            std::fs::create_dir_all(quarantine_dir).ok();
            let filename = std::path::Path::new(target)
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or("unknown");
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let quarantine_path = format!("{quarantine_dir}/{timestamp}_{filename}");
            tokio::fs::rename(target, &quarantine_path)
                .await
                .map_err(|e| format!("Failed to quarantine {target}: {e}"))?;
            Ok(format!("File quarantined: {target} -> {quarantine_path}"))
        }
        _ => Err(format!("Unknown fix type: {fix_type}")),
    }
}
