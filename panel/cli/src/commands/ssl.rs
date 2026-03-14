use crate::client;
use serde_json::json;

pub async fn cmd_ssl_status(token: &str, domain: &str) -> Result<(), String> {
    let status = client::agent_get(&format!("/ssl/status/{domain}"), token).await?;

    let has_cert = status["has_cert"].as_bool().unwrap_or(false);

    if !has_cert {
        println!("No SSL certificate for {domain}");
        return Ok(());
    }

    let issuer = status["issuer"].as_str().unwrap_or("unknown");
    let expiry = status["not_after"].as_str().unwrap_or("unknown");
    let days = status["days_remaining"].as_i64().unwrap_or(0);

    let color = if days > 30 {
        "\x1b[32m"
    } else if days > 7 {
        "\x1b[33m"
    } else {
        "\x1b[31m"
    };

    println!("\x1b[1mSSL Certificate: {domain}\x1b[0m");
    println!("  Issuer:      {issuer}");
    println!("  Expires:     {expiry}");
    println!("  Remaining:   {color}{days} days\x1b[0m");

    Ok(())
}

pub async fn cmd_ssl_provision(
    token: &str,
    domain: &str,
    email: &str,
    runtime: &str,
    proxy_port: Option<u16>,
) -> Result<(), String> {
    println!("Provisioning SSL for {domain}...");

    let mut body = json!({
        "email": email,
        "runtime": runtime,
    });

    if let Some(port) = proxy_port {
        body["proxy_port"] = json!(port);
    }

    let result = client::agent_post(&format!("/ssl/provision/{domain}"), &body, token).await?;

    if result["success"].as_bool() == Some(true) {
        let cert = result["cert_path"].as_str().unwrap_or("unknown");
        let expiry = result["expiry"].as_str().unwrap_or("unknown");
        println!("\x1b[32m✓\x1b[0m SSL certificate provisioned");
        println!("  Domain:      {domain}");
        println!("  Certificate: {cert}");
        println!("  Expires:     {expiry}");
    } else {
        let msg = result["message"].as_str().unwrap_or("Unknown error");
        return Err(format!("Failed to provision SSL: {msg}"));
    }

    Ok(())
}
