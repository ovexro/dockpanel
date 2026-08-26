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

/// Order a Let's Encrypt certificate for `domain`.
///
/// ⛔ THE ISSUER QUESTION IS NOT ASKED HERE, DELIBERATELY. It is asked by the
/// agent, inside the two functions every ACME order in the product passes
/// through — this command, `sites create --ssl`, `apply`, the panel's own doors
/// and anything else that ever reaches them. Asking again here would mean a
/// second copy of the "is this ours?" rule living in a crate that cannot import
/// the first, and the copy that drifts is the one standing over an operator's
/// purchased certificate.
///
/// What this command owes instead is `--force`: a way to say "yes, replace it"
/// once the agent has explained what would be lost. The refusal reaches the
/// operator verbatim — `client` lifts `error` out of a non-2xx body.
pub async fn cmd_ssl_provision(
    token: &str,
    domain: &str,
    email: &str,
    runtime: &str,
    proxy_port: Option<u16>,
    force: bool,
) -> Result<(), String> {
    println!("Provisioning SSL for {domain}...");

    let mut body = json!({
        "email": email,
        "runtime": runtime,
    });

    // Sent only when asked for. An absent field reads as `false` at the agent,
    // which is the safe default and also what an older agent does with it.
    if force {
        body["force"] = json!(true);
    }

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
