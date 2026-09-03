use serde::Serialize;
use crate::safe_cmd::safe_command;

/// Named so the route can match it instead of guessing at its wording.
///
/// Both firewall handlers used to sort their errors with an ad-hoc substring
/// test — one looked for "Invalid", the other for "must be" — so this sentence,
/// which neither author had in mind, fell to the 5xx arm and was replaced by an
/// incident id before the operator saw it. It is reachable on every RHEL-family
/// box: the Firewall page lists rules read from firewalld, and every one of them
/// gets a working delete button whose backing command is ufw, which the panel
/// will never install there. Same shape, and same fix, as
/// [`crate::services::remote_backup::SSHPASS_MISSING`].
pub const UFW_MISSING: &str = "ufw is not installed";

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

/// Which `firewall-cmd` removal this entry needs, so `remove_firewall_rule`
/// can resolve a bare display number back to the right command — firewalld
/// has no ufw-style numbered-rule identity of its own.
enum FirewalldKind {
    Service(String),
    Port(String, String),
    /// The exact `--list-rich-rules` line, so removal round-trips through
    /// the same text firewalld itself considers canonical.
    Rich(String),
}

struct FirewalldEntry {
    rule: FirewallRule,
    kind: FirewalldKind,
}

/// Services, ports, AND rich rules (deny/source-restricted entries this
/// file's own `add_firewall_rule` writes), in one continuously-numbered
/// list — the numbers the Security page shows must be the same numbers
/// `remove_firewall_rule` resolves, or "delete rule 3" deletes the wrong
/// thing the moment a rich rule exists.
async fn firewalld_entries() -> Vec<FirewalldEntry> {
    let zone = fw_out(&["--get-default-zone"]).await.trim().to_string();
    let mut entries = Vec::new();
    let mut number = 0;

    for item in fw_out(&["--zone", zone.as_str(), "--list-services"]).await.split_whitespace() {
        number += 1;
        entries.push(FirewalldEntry {
            rule: FirewallRule {
                number,
                to: format!("{item} (service)"),
                action: "ALLOW IN".into(),
                from: "Anywhere".into(),
            },
            kind: FirewalldKind::Service(item.to_string()),
        });
    }
    for item in fw_out(&["--zone", zone.as_str(), "--list-ports"]).await.split_whitespace() {
        number += 1;
        let (port, proto) = item.split_once('/').unwrap_or((item, "tcp"));
        entries.push(FirewalldEntry {
            rule: FirewallRule {
                number,
                to: item.to_string(),
                action: "ALLOW IN".into(),
                from: "Anywhere".into(),
            },
            kind: FirewalldKind::Port(port.to_string(), proto.to_string()),
        });
    }
    for line in fw_out(&["--zone", zone.as_str(), "--list-rich-rules"]).await.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        number += 1;
        let (to, action, from) = parse_rich_rule_display(line);
        entries.push(FirewalldEntry {
            rule: FirewallRule { number, to, action, from },
            kind: FirewalldKind::Rich(line.to_string()),
        });
    }
    entries
}

/// Display-friendly (to, action, from) for one `--list-rich-rules` line.
/// Not a general rich-rule parser — only understands the port+protocol(+
/// source) shape `add_firewall_rule` itself ever writes via `rich_rule_spec`.
fn parse_rich_rule_display(line: &str) -> (String, String, String) {
    let action = if line.contains("reject") || line.contains(" drop") || line.ends_with("drop") {
        "DENY IN"
    } else {
        "ALLOW IN"
    };
    let to = match (quoted_after(line, "port=\""), quoted_after(line, "protocol=\"")) {
        (Some(p), Some(pr)) => format!("{p}/{pr}"),
        _ => "(rich rule)".to_string(),
    };
    let from = quoted_after(line, "address=\"").unwrap_or_else(|| "Anywhere".to_string());
    (to, action.to_string(), from)
}

fn quoted_after(line: &str, marker: &str) -> Option<String> {
    let idx = line.find(marker)?;
    let rest = &line[idx + marker.len()..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// firewalld's equivalent of the ufw view: the default zone's target as the
/// policy, and its open services and ports as the rule list. Reported in the
/// same shape so the Security page needs no per-firewall branch.
async fn firewalld_status() -> Result<FirewallStatus, String> {
    let zone = fw_out(&["--get-default-zone"]).await.trim().to_string();
    let target = fw_out(&["--zone", &zone, "--get-target"]).await.trim().to_string();
    let rules = firewalld_entries().await.into_iter().map(|e| e.rule).collect();

    Ok(FirewallStatus {
        active: true,
        default_policy: format!(
            "{} (incoming), allow (outgoing) — firewalld zone '{zone}'",
            if target.eq_ignore_ascii_case("ACCEPT") { "allow" } else { "deny" }
        ),
        rules,
    })
}

async fn fw_out(args: &[&str]) -> String {
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        safe_command("firewall-cmd").args(args).output(),
    )
    .await
    .ok()
    .and_then(|r| r.ok())
    .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
    .unwrap_or_default()
}

/// Firewall state for the Security page.
///
/// This only ever ran `ufw status verbose`, so on the RHEL family — where
/// firewalld is the running firewall — it reported `active: false` and zero
/// rules for a box that was firewalled correctly, and the overview card said
/// the server had no firewall at all (s265).
pub async fn get_firewall_status() -> Result<FirewallStatus, String> {
    if crate::services::firewall::detect().await == crate::services::firewall::Firewall::Firewalld {
        return firewalld_status().await;
    }

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

    // Validate source IP — basic check for alphanumeric, dots, colons, slashes
    if let Some(source) = from {
        if source.is_empty()
            || !source
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == ':' || c == '/')
        {
            return Err(format!("Invalid source address: {source}"));
        }
    }

    // This used to always shell to `ufw`, which isn't installed anywhere in
    // the RHEL family — `setup.sh` targets it as a first-class platform, and
    // every one of these mutating actions 424'd there while the read-only
    // status view above already spoke firewalld correctly (same s265 split
    // `get_firewall_status`/`change_ssh_port` were already fixed for).
    if crate::services::firewall::detect().await == crate::services::firewall::Firewall::Firewalld {
        let ok = if action_lower == "allow" && from.is_none() {
            crate::services::firewall::add_port(&port.to_string(), &proto_lower).await
        } else {
            crate::services::firewall::add_rich_rule(&port.to_string(), &proto_lower, &action_lower, from).await
        };
        return if ok {
            Ok(())
        } else {
            Err("firewall-cmd failed to add the rule".to_string())
        };
    }

    let port_proto = format!("{port}/{proto_lower}");
    let mut args: Vec<String> = vec![action_lower];

    if let Some(source) = from {
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
    .map_err(|_| UFW_MISSING.to_string())?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(format!("ufw failed: {stderr} {stdout}"));
    }

    Ok(())
}

/// Delete a firewall rule by its display number — the same number
/// `get_firewall_status`/`firewalld_status` reported it as.
pub async fn remove_firewall_rule(rule_num: usize) -> Result<(), String> {
    if rule_num == 0 {
        return Err("Rule number must be >= 1".into());
    }

    if crate::services::firewall::detect().await == crate::services::firewall::Firewall::Firewalld {
        let entries = firewalld_entries().await;
        let entry = entries
            .into_iter()
            .find(|e| e.rule.number == rule_num)
            .ok_or_else(|| format!("No firewall rule numbered {rule_num}"))?;
        let ok = match &entry.kind {
            FirewalldKind::Service(name) => crate::services::firewall::remove_service(name).await,
            FirewalldKind::Port(port, proto) => crate::services::firewall::remove_port(port, proto).await,
            FirewalldKind::Rich(raw) => crate::services::firewall::remove_rich_rule_raw(raw).await,
        };
        return if ok {
            Ok(())
        } else {
            Err(format!("firewall-cmd failed to remove rule {rule_num}"))
        };
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        safe_command("ufw")
            .args(["--force", "delete", &rule_num.to_string()])
            .output(),
    )
    .await
    .map_err(|_| "ufw command timed out".to_string())?
    .map_err(|_| UFW_MISSING.to_string())?;

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

/// Expand one `Include` argument (a single path, or a glob with one `*`) to
/// the files it matches, sorted lexically — the order OpenSSH itself
/// processes a glob's matches in.
async fn expand_include_arg(dir: &std::path::Path, arg: &str) -> Vec<std::path::PathBuf> {
    let path = if arg.starts_with('/') {
        std::path::PathBuf::from(arg)
    } else {
        dir.join(arg)
    };
    let Some(name) = path.file_name().and_then(|f| f.to_str()) else {
        return Vec::new();
    };
    if !name.contains('*') {
        return if tokio::fs::metadata(&path).await.is_ok() {
            vec![path]
        } else {
            Vec::new()
        };
    }
    let parent = path.parent().unwrap_or(std::path::Path::new("/"));
    let (prefix, suffix) = name.split_once('*').unwrap_or((name, ""));
    let mut matches = Vec::new();
    if let Ok(mut entries) = tokio::fs::read_dir(parent).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if let Some(fname) = entry.file_name().to_str() {
                if fname.len() >= prefix.len() + suffix.len()
                    && fname.starts_with(prefix)
                    && fname.ends_with(suffix)
                {
                    matches.push(entry.path());
                }
            }
        }
    }
    matches.sort();
    matches
}

/// `sshd_config` and its `Include`d drop-ins, linearized into the order
/// sshd itself evaluates directives in — each `Include` line is spliced in
/// place with the lines of every file it matches, one level deep (the
/// stock `Include /etc/ssh/sshd_config.d/*.conf` never nests further in
/// practice; a deeper Include inside a drop-in falls through as an
/// unrecognized line, same as any other keyword this parser doesn't know).
/// OpenSSH's own rule is "the first obtained value wins", so callers must
/// scan this in order and stop at the first match per keyword — reading
/// only `/etc/ssh/sshd_config` silently ignores every drop-in that
/// overrides it, which is exactly what a distro cloud-init image and this
/// project's own `port.conf`-style admin drop-ins both do.
async fn linearized_sshd_lines(main_path: &std::path::Path) -> Vec<(std::path::PathBuf, String)> {
    let mut out = Vec::new();
    let Ok(content) = tokio::fs::read_to_string(main_path).await else {
        return out;
    };
    let dir = main_path.parent().unwrap_or(std::path::Path::new("/etc/ssh"));

    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            let mut parts = trimmed.splitn(2, char::is_whitespace);
            let keyword = parts.next().unwrap_or("");
            if keyword.eq_ignore_ascii_case("include") {
                for arg in parts.next().unwrap_or("").split_whitespace() {
                    for matched in expand_include_arg(dir, arg).await {
                        if let Ok(inc) = tokio::fs::read_to_string(&matched).await {
                            for inc_line in inc.lines() {
                                out.push((matched.clone(), inc_line.to_string()));
                            }
                        }
                    }
                }
                continue;
            }
        }
        out.push((main_path.to_path_buf(), line.to_string()));
    }
    out
}

/// Read SSH configuration values from /etc/ssh/sshd_config and its Includes.
/// Returns (port, password_auth_enabled, root_login_enabled).
pub async fn parse_ssh_config() -> (u16, bool, bool) {
    parse_ssh_config_at(std::path::Path::new("/etc/ssh/sshd_config")).await
}

async fn parse_ssh_config_at(main_path: &std::path::Path) -> (u16, bool, bool) {
    let mut port: u16 = 22;
    let mut password_auth = true;
    let mut root_login = true;
    let (mut port_set, mut pw_set, mut root_set) = (false, false, false);

    for (_, line) in linearized_sshd_lines(main_path).await {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }

        match parts[0] {
            "Port" if !port_set => {
                if let Ok(p) = parts[1].parse::<u16>() {
                    port = p;
                    port_set = true;
                }
            }
            "PasswordAuthentication" if !pw_set => {
                password_auth = parts[1].eq_ignore_ascii_case("yes");
                pw_set = true;
            }
            "PermitRootLogin" if !root_set => {
                root_login = !parts[1].eq_ignore_ascii_case("no");
                root_set = true;
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
    if port == 0 {
        return Err("Invalid port".into());
    }
    modify_sshd_config("Port", &port.to_string()).await?;
    // Add a firewall rule for the new port before restarting sshd — in
    // whichever firewall is running. This used to be a discarded `ufw allow`,
    // so on a firewalld box it changed nothing and the next SSH connection
    // had nowhere to land.
    if !crate::services::firewall::allow_tcp(&port.to_string()).await {
        return Err(format!(
            "Could not open port {port} in the firewall — refusing to move SSH there, \
             it would lock you out. Open it manually, then retry."
        ));
    }
    crate::services::firewall::reload().await;
    restart_sshd().await
}

/// Modify a single directive across /etc/ssh/sshd_config and its Includes.
/// If the directive exists (commented or not) ANYWHERE in the Include chain,
/// replace it in the file that actually governs it. Otherwise append to the
/// main file.
///
/// Writing to the main file unconditionally — the old behavior — could
/// report success while sshd kept the old value: OpenSSH's `Include` is
/// processed inline, at the point it appears, and "the first obtained value
/// wins" — so a drop-in matched by `Include /etc/ssh/sshd_config.d/*.conf`
/// (which every stock sshd_config ships near the top) overrides anything the
/// main file sets afterward. Live on this exact box: `Port` in
/// `sshd_config.d/port.conf` and `PasswordAuthentication` in
/// `50-cloud-init.conf` both win over the main file's own commented
/// defaults; a write that only ever touched `sshd_config` would silently do
/// nothing to what sshd actually enforces.
async fn modify_sshd_config(key: &str, value: &str) -> Result<(), String> {
    modify_sshd_config_at(std::path::Path::new("/etc/ssh/sshd_config"), key, value).await
}

async fn modify_sshd_config_at(main_path: &std::path::Path, key: &str, value: &str) -> Result<(), String> {
    let lines = linearized_sshd_lines(main_path).await;
    if lines.is_empty() && tokio::fs::metadata(main_path).await.is_err() {
        return Err(format!("Failed to read {}: not found", main_path.display()));
    }

    // The file that governs `key` today: the first one, in sshd's own
    // Include-resolution order, with an active directive for it — falling
    // back to the first with a commented one if none is active anywhere.
    let mut target: Option<std::path::PathBuf> = None;
    for (path, line) in &lines {
        let trimmed = line.trim();
        if trimmed.starts_with(key) {
            target = Some(path.clone());
            break;
        }
        if target.is_none()
            && (trimmed.starts_with(&format!("#{key}")) || trimmed.starts_with(&format!("# {key}")))
        {
            target = Some(path.clone());
        }
    }
    let target_path = target.unwrap_or_else(|| main_path.to_path_buf());

    let content = tokio::fs::read_to_string(&target_path).await
        .map_err(|e| format!("Failed to read {}: {e}", target_path.display()))?;

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
    let tmp_path = target_path.with_file_name(format!(
        "{}.tmp",
        target_path.file_name().and_then(|f| f.to_str()).unwrap_or("sshd_config")
    ));
    tokio::fs::write(&tmp_path, &new_content).await
        .map_err(|e| format!("Failed to write {}: {e}", target_path.display()))?;
    tokio::fs::rename(&tmp_path, &target_path).await
        .map_err(|e| format!("Failed to rename {}: {e}", target_path.display()))?;

    tracing::info!("SSH config updated: {key} {value} (in {})", target_path.display());
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
    // RHEL-family boxes (a first-class `setup.sh` target) log the same
    // sshd events to `/var/log/secure`, not `/var/log/auth.log` — reading
    // only the latter and swallowing the read error into an empty Vec
    // (the old behavior) made a RHEL host indistinguishable from a
    // genuinely quiet one: both reported HTTP 200 with zero entries. A
    // missing file is now a real error, so the backend's fleet wrapper
    // (routes/security.rs, which already distinguishes `reached: false`
    // from `reached: true, entries: 0`) reports it correctly.
    let path = crate::services::logs::resolve_auth_log_path();
    let content = tokio::fs::read_to_string(path).await
        .map_err(|e| format!("Failed to read {path}: {e}"))?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dp-sshtest-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // ── parse_ssh_config_at / modify_sshd_config_at (s454: Include-blindness) ──

    #[tokio::test]
    async fn parse_ssh_config_reads_plain_main_file() {
        let dir = scratch_dir("plain");
        let main = dir.join("sshd_config");
        std::fs::write(&main, "Port 2222\nPasswordAuthentication no\nPermitRootLogin no\n").unwrap();

        let (port, pw, root) = parse_ssh_config_at(&main).await;
        assert_eq!(port, 2222);
        assert!(!pw);
        assert!(!root);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Reproduces the exact live-box shape found by s454's fan-out:
    /// `Include` appears BEFORE the main file's own commented `#Port 22`
    /// default, and a drop-in sets the real value. OpenSSH's rule is "the
    /// first obtained value wins" — reading only the main file reported 22
    /// on a box that was actually listening on 1571.
    #[tokio::test]
    async fn parse_ssh_config_include_drop_in_wins_over_later_main_default() {
        let dir = scratch_dir("include");
        let dropdir = dir.join("sshd_config.d");
        std::fs::create_dir_all(&dropdir).unwrap();
        std::fs::write(dropdir.join("port.conf"), "Port 1571\n").unwrap();
        let main = dir.join("sshd_config");
        std::fs::write(&main, format!("Include {}/*.conf\n\n#Port 22\n", dropdir.display())).unwrap();

        let (port, _, _) = parse_ssh_config_at(&main).await;
        assert_eq!(port, 1571, "the Include'd drop-in must win over the main file's own later, commented default");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn parse_ssh_config_first_included_file_wins_lexically() {
        let dir = scratch_dir("firstwins");
        let dropdir = dir.join("sshd_config.d");
        std::fs::create_dir_all(&dropdir).unwrap();
        std::fs::write(dropdir.join("10-first.conf"), "Port 3000\n").unwrap();
        std::fs::write(dropdir.join("20-second.conf"), "Port 4000\n").unwrap();
        let main = dir.join("sshd_config");
        std::fs::write(&main, format!("Include {}/*.conf\n", dropdir.display())).unwrap();

        let (port, _, _) = parse_ssh_config_at(&main).await;
        assert_eq!(port, 3000, "OpenSSH resolves a glob's matches in lexical order; the first obtained value wins");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn parse_ssh_config_missing_main_file_returns_defaults() {
        let dir = scratch_dir("missing");
        let main = dir.join("does-not-exist");
        let (port, pw, root) = parse_ssh_config_at(&main).await;
        assert_eq!((port, pw, root), (22, true, true));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The write-side counterpart of the Include-blindness bug: editing only
    /// ever touched the main file, so on this exact box a `Port`/
    /// `PasswordAuthentication` change via the panel would report success
    /// while sshd — governed by the drop-in — kept the old value.
    #[tokio::test]
    async fn modify_sshd_config_writes_to_the_file_that_actually_governs_the_key() {
        let dir = scratch_dir("write-governs");
        let dropdir = dir.join("sshd_config.d");
        std::fs::create_dir_all(&dropdir).unwrap();
        let port_conf = dropdir.join("port.conf");
        std::fs::write(&port_conf, "Port 1571\n").unwrap();
        let main = dir.join("sshd_config");
        std::fs::write(&main, format!("Include {}/*.conf\n\n#Port 22\n", dropdir.display())).unwrap();

        modify_sshd_config_at(&main, "Port", "2022").await.unwrap();

        let port_conf_content = std::fs::read_to_string(&port_conf).unwrap();
        assert!(port_conf_content.contains("Port 2022"), "must edit the governing drop-in, got: {port_conf_content}");
        let main_content = std::fs::read_to_string(&main).unwrap();
        assert!(main_content.contains("#Port 22"), "the main file's own commented default must be left untouched — editing THAT would silently do nothing, since the drop-in still wins");

        let (port, _, _) = parse_ssh_config_at(&main).await;
        assert_eq!(port, 2022, "the reported effective port must reflect the write");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn modify_sshd_config_falls_back_to_main_file_when_nothing_governs_the_key() {
        let dir = scratch_dir("fallback");
        let main = dir.join("sshd_config");
        std::fs::write(&main, "# nothing here\n").unwrap();

        modify_sshd_config_at(&main, "PermitRootLogin", "no").await.unwrap();

        let content = std::fs::read_to_string(&main).unwrap();
        assert!(content.contains("PermitRootLogin no"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn modify_sshd_config_replaces_active_directive_in_governing_file_not_main() {
        // No comment anywhere — an ACTIVE directive in the drop-in, nothing
        // in main at all. Must still resolve to the drop-in, not append to main.
        let dir = scratch_dir("active-governs");
        let dropdir = dir.join("sshd_config.d");
        std::fs::create_dir_all(&dropdir).unwrap();
        let pw_conf = dropdir.join("50-cloud-init.conf");
        std::fs::write(&pw_conf, "PasswordAuthentication no\n").unwrap();
        let main = dir.join("sshd_config");
        std::fs::write(&main, format!("Include {}/*.conf\n", dropdir.display())).unwrap();

        modify_sshd_config_at(&main, "PasswordAuthentication", "yes").await.unwrap();

        let pw_content = std::fs::read_to_string(&pw_conf).unwrap();
        assert!(pw_content.contains("PasswordAuthentication yes"));
        let main_content = std::fs::read_to_string(&main).unwrap();
        assert!(!main_content.contains("PasswordAuthentication"), "must not append a second, shadowed directive into main");
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── firewalld rich-rule helpers ──────────────────────────────────────

    #[test]
    fn rich_rule_spec_matches_what_parse_rich_rule_display_reads_back() {
        let spec = crate::services::firewall::rich_rule_spec("8080", "tcp", "deny", None);
        let (to, action, from) = parse_rich_rule_display(&spec);
        assert_eq!(to, "8080/tcp");
        assert_eq!(action, "DENY IN");
        assert_eq!(from, "Anywhere");

        let spec_src = crate::services::firewall::rich_rule_spec("22", "tcp", "allow", Some("10.0.0.5"));
        let (to2, action2, from2) = parse_rich_rule_display(&spec_src);
        assert_eq!(to2, "22/tcp");
        assert_eq!(action2, "ALLOW IN");
        assert_eq!(from2, "10.0.0.5");
    }

    #[test]
    fn rich_rule_spec_picks_ipv6_family_from_a_colon_address() {
        let spec = crate::services::firewall::rich_rule_spec("443", "tcp", "deny", Some("fd00::1"));
        assert!(spec.contains(r#"family="ipv6""#), "got: {spec}");
    }
}
