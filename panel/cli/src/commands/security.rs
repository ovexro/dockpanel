use crate::client;
use serde_json::json;

pub async fn cmd_security_overview(token: &str, output: &str) -> Result<(), String> {
    let overview = client::agent_get("/security/overview", token).await?;

    if output == "json" {
        println!("{}", serde_json::to_string_pretty(&overview).unwrap_or_default());
        return Ok(());
    }

    println!("\x1b[1mSecurity Overview\x1b[0m");

    let fw = overview["firewall_status"].as_str().unwrap_or("unknown");
    let fw_color = if fw == "active" { "\x1b[32m" } else { "\x1b[31m" };
    println!("  Firewall:    {fw_color}{fw}\x1b[0m");

    let f2b = overview["fail2ban_status"].as_str().unwrap_or("unknown");
    let f2b_color = if f2b == "active" { "\x1b[32m" } else { "\x1b[31m" };
    println!("  Fail2ban:    {f2b_color}{f2b}\x1b[0m");

    if let Some(ssl) = overview["ssl_coverage"].as_str() {
        println!("  SSL:         {ssl}");
    }

    if let Some(scan) = overview["scan_date"].as_str() {
        println!("  Last scan:   {scan}");
    }

    Ok(())
}

pub async fn cmd_security_scan(token: &str, output: &str) -> Result<(), String> {
    println!("Running security scan...");

    let result = client::agent_post_empty("/security/scan", token).await?;

    if output == "json" {
        println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default());
        return Ok(());
    }

    let risk = result["risk_level"].as_str().unwrap_or("unknown");
    let risk_color = match risk {
        "low" => "\x1b[32m",
        "medium" => "\x1b[33m",
        "high" | "critical" => "\x1b[31m",
        _ => "\x1b[90m",
    };

    println!("\x1b[1mScan Results\x1b[0m");
    println!("  Risk level:  {risk_color}{risk}\x1b[0m");

    if let Some(findings) = result["findings"].as_array() {
        if findings.is_empty() {
            println!("  \x1b[32mNo issues found.\x1b[0m");
        } else {
            println!();
            for finding in findings {
                let severity = finding["severity"].as_str().unwrap_or("info");
                let message = finding["message"].as_str().unwrap_or("-");
                let sev_color = match severity {
                    "critical" | "high" => "\x1b[31m",
                    "medium" => "\x1b[33m",
                    _ => "\x1b[90m",
                };
                println!("  {sev_color}[{severity}]\x1b[0m {message}");
            }
            println!("\n{} finding(s)", findings.len());
        }
    }

    Ok(())
}

pub async fn cmd_firewall_list(token: &str, output: &str) -> Result<(), String> {
    let fw = client::agent_get("/security/firewall", token).await?;

    if output == "json" {
        println!("{}", serde_json::to_string_pretty(&fw).unwrap_or_default());
        return Ok(());
    }

    let enabled = fw["enabled"].as_bool().unwrap_or(false);
    println!(
        "\x1b[1mFirewall:\x1b[0m {}",
        if enabled {
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
                "\n\x1b[1m{:<6} {:<8} {:<8} {:<10} {:<20}\x1b[0m",
                "#", "PORT", "PROTO", "ACTION", "FROM"
            );
            for (i, rule) in rules.iter().enumerate() {
                let port = rule["port"].as_u64().map(|p| p.to_string()).unwrap_or("-".to_string());
                let proto = rule["proto"].as_str().unwrap_or("-");
                let action = rule["action"].as_str().unwrap_or("-");
                let from = rule["from"].as_str().unwrap_or("anywhere");

                let color = if action == "allow" {
                    "\x1b[32m"
                } else {
                    "\x1b[31m"
                };

                println!(
                    "{:<6} {:<8} {:<8} {color}{:<10}\x1b[0m {:<20}",
                    i + 1,
                    port,
                    proto,
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
