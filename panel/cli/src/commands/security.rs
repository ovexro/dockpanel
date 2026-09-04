use crate::client;
use serde_json::json;

pub async fn cmd_security_overview(token: &str, output: &str) -> Result<(), String> {
    let overview = client::agent_get("/security/overview", token).await?;

    if output == "json" {
        println!("{}", serde_json::to_string_pretty(&overview).unwrap_or_default());
        return Ok(());
    }

    println!("\x1b[1mSecurity Overview\x1b[0m");

    let fw_active = overview["firewall_active"].as_bool().unwrap_or(false);
    let fw_color = if fw_active { "\x1b[32m" } else { "\x1b[31m" };
    println!("  Firewall:    {fw_color}{}\x1b[0m", if fw_active { "active" } else { "inactive" });

    let f2b_running = overview["fail2ban_running"].as_bool().unwrap_or(false);
    let f2b_color = if f2b_running { "\x1b[32m" } else { "\x1b[31m" };
    println!("  Fail2ban:    {f2b_color}{}\x1b[0m", if f2b_running { "active" } else { "inactive" });

    let root_login = overview["ssh_root_login"].as_bool().unwrap_or(false);
    let root_color = if root_login { "\x1b[31m" } else { "\x1b[32m" };
    println!("  SSH root:    {root_color}{}\x1b[0m", if root_login { "enabled" } else { "disabled" });

    let pw_auth = overview["ssh_password_auth"].as_bool().unwrap_or(false);
    let pw_color = if pw_auth { "\x1b[33m" } else { "\x1b[32m" };
    println!("  SSH password:{pw_color}{}\x1b[0m", if pw_auth { " enabled" } else { " disabled" });

    Ok(())
}

pub async fn cmd_security_scan(token: &str, output: &str) -> Result<(), String> {
    println!("Running security scan...");

    let result = client::agent_post_empty("/security/scan", token).await?;

    if output == "json" {
        println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default());
        return Ok(());
    }

    let empty = Vec::new();
    let findings = result["findings"].as_array().unwrap_or(&empty);

    // The agent's ScanResult carries only per-finding severities, no
    // precomputed overall risk_level — derive it from the highest severity
    // among the findings actually returned. security_scanner.rs's only
    // severity values are "critical"/"warning"/"info" (checked in source).
    let risk_rank = |s: &str| match s {
        "critical" => 3,
        "warning" => 2,
        "info" => 1,
        _ => 0,
    };
    let risk = findings
        .iter()
        .filter_map(|f| f["severity"].as_str())
        .max_by_key(|s| risk_rank(s))
        .unwrap_or("info");
    let risk_color = match risk {
        "critical" => "\x1b[31m",
        "warning" => "\x1b[33m",
        "info" => "\x1b[32m",
        _ => "\x1b[90m",
    };

    println!("\x1b[1mScan Results\x1b[0m");
    println!("  Risk level:  {risk_color}{risk}\x1b[0m");

    if findings.is_empty() {
        println!("  \x1b[32mNo issues found.\x1b[0m");
    } else {
        println!();
        for finding in findings {
            let severity = finding["severity"].as_str().unwrap_or("info");
            let title = finding["title"].as_str().unwrap_or("-");
            let sev_color = match severity {
                "critical" => "\x1b[31m",
                "warning" => "\x1b[33m",
                _ => "\x1b[90m",
            };
            println!("  {sev_color}[{severity}]\x1b[0m {title}");
        }
        println!("\n{} finding(s)", findings.len());
    }

    Ok(())
}

pub async fn cmd_firewall_list(token: &str, output: &str) -> Result<(), String> {
    let fw = client::agent_get("/security/firewall", token).await?;

    if output == "json" {
        println!("{}", serde_json::to_string_pretty(&fw).unwrap_or_default());
        return Ok(());
    }

    let active = fw["active"].as_bool().unwrap_or(false);
    println!(
        "\x1b[1mFirewall:\x1b[0m {}",
        if active {
            "\x1b[32menabled\x1b[0m"
        } else {
            "\x1b[31mdisabled\x1b[0m"
        }
    );

    if let Some(rules) = fw["rules"].as_array() {
        if rules.is_empty() {
            println!("  No rules configured.");
        } else {
            println!(
                "\n\x1b[1m{:<6} {:<12} {:<10} {:<20}\x1b[0m",
                "#", "TO", "ACTION", "FROM"
            );
            for rule in rules {
                let number = rule["number"].as_u64().map(|n| n.to_string()).unwrap_or("-".to_string());
                let to = rule["to"].as_str().unwrap_or("-");
                let action = rule["action"].as_str().unwrap_or("-");
                let from = rule["from"].as_str().unwrap_or("anywhere");

                let color = if action.to_lowercase().contains("allow") {
                    "\x1b[32m"
                } else {
                    "\x1b[31m"
                };

                println!(
                    "{:<6} {:<12} {color}{:<10}\x1b[0m {:<20}",
                    number,
                    to,
                    action,
                    from
                );
            }
            println!("\n{} rule(s)", rules.len());
        }
    }

    Ok(())
}

pub async fn cmd_firewall_add(
    token: &str,
    port: u16,
    proto: &str,
    action: &str,
    from: Option<&str>,
) -> Result<(), String> {
    match action {
        "allow" | "deny" => {}
        _ => return Err(format!("Invalid action '{action}'. Use: allow or deny")),
    }
    match proto {
        "tcp" | "udp" => {}
        _ => return Err(format!("Invalid protocol '{proto}'. Use: tcp or udp")),
    }

    let mut body = json!({
        "port": port,
        "proto": proto,
        "action": action,
    });

    if let Some(from) = from {
        body["from"] = json!(from);
    }

    let result = client::agent_post("/security/firewall/rules", &body, token).await?;

    if result["success"].as_bool() == Some(true) {
        println!(
            "\x1b[32m✓\x1b[0m Firewall rule added: {action} {proto}/{port}{}",
            from.map(|f| format!(" from {f}")).unwrap_or_default()
        );
    } else {
        let msg = result["message"].as_str().unwrap_or("Unknown error");
        return Err(format!("Failed to add rule: {msg}"));
    }

    Ok(())
}

pub async fn cmd_firewall_remove(token: &str, number: u32) -> Result<(), String> {
    println!("Removing firewall rule #{number}...");

    let result = client::agent_delete(&format!("/security/firewall/rules/{number}"), token).await?;

    if result["success"].as_bool() == Some(true) {
        println!("\x1b[32m✓\x1b[0m Firewall rule removed");
    } else {
        let msg = result["message"].as_str().unwrap_or("Unknown error");
        return Err(format!("Failed to remove rule: {msg}"));
    }

    Ok(())
}
