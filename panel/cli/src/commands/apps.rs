use crate::client;
use serde_json::json;

pub async fn cmd_apps_list(token: &str, output: &str, filter: Option<&str>) -> Result<(), String> {
    let apps = client::agent_get("/apps", token).await?;
    let apps = apps.as_array().ok_or("Expected array from /apps")?;

    // Apply filter (matches name or domain)
    let filtered: Vec<_> = if let Some(f) = filter {
        let f_lower = f.to_lowercase();
        apps.iter()
            .filter(|app| {
                let name = app["name"].as_str().unwrap_or("").to_lowercase();
                let domain = app["domain"].as_str().unwrap_or("").to_lowercase();
                name.contains(&f_lower) || domain.contains(&f_lower)
            })
            .collect()
    } else {
        apps.iter().collect()
    };

    if output == "json" {
        let json_arr: Vec<_> = filtered.into_iter().cloned().collect();
        println!("{}", serde_json::to_string_pretty(&json_arr).unwrap_or_default());
        return Ok(());
    }

    if filtered.is_empty() {
        println!("No Docker apps deployed.");
        return Ok(());
    }

    println!(
        "\x1b[1m{:<14} {:<20} {:<15} {:<25} {:<8} {:<12}\x1b[0m",
        "CONTAINER", "NAME", "TEMPLATE", "DOMAIN", "PORT", "STATUS"
    );

    for app in &filtered {
        let cid = app["container_id"].as_str().unwrap_or("-");
        let short_id = &cid[..cid.len().min(12)];
        let name = app["name"].as_str().unwrap_or("-");
        let template = app["template"].as_str().unwrap_or("-");
        let domain = app["domain"].as_str().unwrap_or("-");
        let port = app["port"]
            .as_u64()
            .map(|p| p.to_string())
            .unwrap_or("-".to_string());
        let status = app["status"].as_str().unwrap_or("-");

        let color = if status == "running" {
            "\x1b[32m"
        } else {
            "\x1b[31m"
        };

        println!(
            "{:<14} {:<20} {:<15} {:<25} {:<8} {color}{:<12}\x1b[0m",
            short_id, name, template, domain, port, status
        );
    }

    println!("\n{} app(s)", filtered.len());
    Ok(())
}

pub async fn cmd_apps_templates(token: &str, output: &str) -> Result<(), String> {
    let templates = client::agent_get("/apps/templates", token).await?;

    if output == "json" {
        println!("{}", serde_json::to_string_pretty(&templates).unwrap_or_default());
        return Ok(());
    }

    let templates = templates
        .as_array()
        .ok_or("Expected array from /apps/templates")?;

    if templates.is_empty() {
        println!("No templates available.");
        return Ok(());
    }

    println!(
        "\x1b[1m{:<20} {:<30} {:<15}\x1b[0m",
        "ID", "IMAGE", "DEFAULT PORT"
    );

    for t in templates {
        let id = t["id"].as_str().unwrap_or("-");
        let image = t["image"].as_str().unwrap_or("-");
        let ports = t["ports"]
            .as_array()
            .and_then(|p| p.first())
            .and_then(|p| p.as_u64())
            .map(|p| p.to_string())
            .unwrap_or("-".to_string());

        println!("{:<20} {:<30} {:<15}", id, image, ports);
    }

    println!("\n{} template(s)", templates.len());
    Ok(())
}

pub async fn cmd_apps_deploy(
    token: &str,
    template: &str,
    name: &str,
    port: u16,
    domain: Option<&str>,
    ssl_email: Option<&str>,
) -> Result<(), String> {
    if ssl_email.is_some() && domain.is_none() {
        return Err("--ssl-email requires --domain".to_string());
    }

    println!("Deploying app '{name}' from template '{template}' on port {port}...");

    let mut body = json!({
        "template_id": template,
        "name": name,
        "port": port,
    });

    if let Some(domain) = domain {
        body["domain"] = json!(domain);
    }
    if let Some(email) = ssl_email {
        body["ssl_email"] = json!(email);
    }

    let result = client::agent_post("/apps/deploy", &body, token).await?;

    if result["success"].as_bool() == Some(true) {
        let cid = result["container_id"].as_str().unwrap_or("unknown");
        println!("\x1b[32m✓\x1b[0m App deployed");
        println!("  Name:         {name}");
        println!("  Port:         {port}");
        println!("  Container:    {}", &cid[..cid.len().min(12)]);

        if let Some(domain) = result["domain"].as_str() {
            println!("  Domain:       {domain}");
        }
        if result["proxy"].as_bool() == Some(true) {
            println!("  Proxy:        \x1b[32mconfigured\x1b[0m");
        }
        if let Some(warning) = result["proxy_warning"].as_str() {
            eprintln!("  \x1b[33mProxy warning:\x1b[0m {warning}");
        }
        if result["ssl"].as_bool() == Some(true) {
            println!("  SSL:          \x1b[32mprovisioned\x1b[0m");
        }
        if let Some(warning) = result["ssl_warning"].as_str() {
            eprintln!("  \x1b[33mSSL warning:\x1b[0m {warning}");
        }
    } else {
        let msg = result["message"].as_str().unwrap_or("Unknown error");
        return Err(format!("Failed to deploy app: {msg}"));
    }

    Ok(())
}

pub async fn cmd_apps_action(token: &str, container_id: &str, action: &str) -> Result<(), String> {
    let short_id = &container_id[..container_id.len().min(12)];
    println!("{action}ing container {short_id}...");

    let result = client::agent_post_empty(
        &format!("/apps/{container_id}/{action}"),
        token,
    )
    .await?;

    if result["success"].as_bool() == Some(true) {
        let past = match action {
            "stop" => "stopped",
            "start" => "started",
            "restart" => "restarted",
            _ => action,
        };
        println!("\x1b[32m✓\x1b[0m Container {past}");
    } else {
        let msg = result["message"].as_str().unwrap_or("Unknown error");
        return Err(format!("Failed to {action} container: {msg}"));
    }

    Ok(())
}

pub async fn cmd_apps_remove(token: &str, container_id: &str) -> Result<(), String> {
    let short_id = &container_id[..container_id.len().min(12)];
    println!("Removing container {short_id}...");

    let result = client::agent_delete(&format!("/apps/{container_id}"), token).await?;

    if result["success"].as_bool() == Some(true) {
        println!("\x1b[32m✓\x1b[0m Container removed");
    } else {
        let msg = result["message"].as_str().unwrap_or("Unknown error");
        return Err(format!("Failed to remove container: {msg}"));
    }

    Ok(())
}

pub async fn cmd_apps_logs(token: &str, container_id: &str) -> Result<(), String> {
    let result = client::agent_get(&format!("/apps/{container_id}/logs"), token).await?;

    let logs = result["logs"].as_str().unwrap_or("");
    if logs.is_empty() {
        println!("No logs available.");
    } else {
        print!("{logs}");
    }

    Ok(())
}

pub async fn cmd_apps_compose(token: &str, file: &str) -> Result<(), String> {
    let yaml = std::fs::read_to_string(file)
        .map_err(|e| format!("Cannot read {file}: {e}"))?;

    println!("Deploying from {file}...");

    let body = json!({ "yaml": yaml });
    let result = client::agent_post("/apps/compose/deploy", &body, token).await?;

    if let Some(services) = result["services"].as_array() {
        for svc in services {
            let name = svc["name"].as_str().unwrap_or("-");
            println!("\x1b[32m✓\x1b[0m Service deployed: {name}");
        }
    }

    if let Some(failed) = result["failed"].as_array() {
        for f in failed {
            let name = f["name"].as_str().unwrap_or("-");
            let err = f["error"].as_str().unwrap_or("unknown");
            eprintln!("\x1b[31m✗\x1b[0m Service failed: {name} — {err}");
        }
    }

    Ok(())
}
