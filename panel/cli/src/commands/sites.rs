use crate::client;
use serde_json::json;

pub struct SiteInfo {
    pub domain: String,
    pub runtime: String,
    pub ssl: bool,
}

pub fn list_nginx_sites() -> Vec<SiteInfo> {
    let mut sites = Vec::new();
    let sites_dir = std::path::Path::new("/etc/nginx/sites-enabled");

    let dir = match std::fs::read_dir(sites_dir) {
        Ok(d) => d,
        Err(_) => return sites,
    };

    for entry in dir.flatten() {
        let path = entry.path();
        let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        if filename == "dockpanel-panel.conf"
            || filename == "dockpanel.dev.conf"
            || filename == "default"
        {
            continue;
        }

        if let Ok(content) = std::fs::read_to_string(&path) {
            let domain = content
                .lines()
                .find(|l| l.trim().starts_with("server_name"))
                .and_then(|l| l.trim().strip_prefix("server_name"))
                .and_then(|l| {
                    l.trim()
                        .trim_end_matches(';')
                        .split_whitespace()
                        .next()
                        .map(|s| s.to_string())
                })
                .unwrap_or_else(|| filename.replace(".conf", ""));

            if domain == "_" {
                continue;
            }

            let ssl = content.contains("ssl_certificate");
            let runtime = if content.contains("proxy_pass") {
                "proxy"
            } else if content.contains("fastcgi_pass") || content.contains("php") {
                "php"
            } else {
                "static"
            };

            sites.push(SiteInfo {
                domain,
                runtime: runtime.to_string(),
                ssl,
            });
        }
    }

    sites.sort_by(|a, b| a.domain.cmp(&b.domain));
    sites.dedup_by(|a, b| a.domain == b.domain);
    sites
}

pub async fn cmd_sites_list(token: &str) -> Result<(), String> {
    let info = client::agent_get("/system/info", token).await?;
    let _ = info;

    let sites = list_nginx_sites();

    if sites.is_empty() {
        println!("No sites configured.");
        return Ok(());
    }

    println!(
        "\x1b[1m{:<30} {:<10} {:<6}\x1b[0m",
        "DOMAIN", "TYPE", "SSL"
    );

    for site in &sites {
        println!(
            "{:<30} {:<10} {:<6}",
            site.domain,
            site.runtime,
            if site.ssl { "yes" } else { "no" }
        );
    }

    println!("\n{} site(s)", sites.len());
    Ok(())
}

pub async fn cmd_sites_create(
    token: &str,
    domain: &str,
    runtime: &str,
    proxy_port: Option<u16>,
    ssl: bool,
    ssl_email: Option<&str>,
) -> Result<(), String> {
    if runtime == "proxy" && proxy_port.is_none() {
        return Err("--proxy-port is required for proxy runtime".to_string());
    }
    if ssl && ssl_email.is_none() {
        return Err("--ssl-email is required when using --ssl".to_string());
    }

    let mut body = json!({
        "runtime": runtime,
    });

    if let Some(port) = proxy_port {
        body["proxy_port"] = json!(port);
    }

    println!("Creating site {domain} ({runtime})...");
    let result = client::agent_put(&format!("/nginx/sites/{domain}"), &body, token).await?;

    if result["success"].as_bool() == Some(true) {
        println!("\x1b[32m✓\x1b[0m Site created: {domain}");
    } else {
        let msg = result["message"].as_str().unwrap_or("Unknown error");
        return Err(format!("Failed to create site: {msg}"));
    }

    if ssl {
        println!("Provisioning SSL certificate...");
        let mut ssl_body = json!({
            "email": ssl_email.unwrap(),
            "runtime": runtime,
        });
        if let Some(port) = proxy_port {
            ssl_body["proxy_port"] = json!(port);
        }
        match client::agent_post(&format!("/ssl/provision/{domain}"), &ssl_body, token).await {
            Ok(r) => {
                if r["success"].as_bool() == Some(true) {
                    let expiry = r["expiry"].as_str().unwrap_or("unknown");
                    println!("\x1b[32m✓\x1b[0m SSL provisioned (expires: {expiry})");
                }
            }
            Err(e) => {
                eprintln!("\x1b[33mwarning:\x1b[0m SSL provisioning failed: {e}");
                eprintln!("  Site created without SSL. Provision manually with:");
                eprintln!("  dockpanel ssl provision {domain} --email {}", ssl_email.unwrap_or("you@example.com"));
            }
        }
    }

    Ok(())
}

pub async fn cmd_sites_delete(token: &str, domain: &str) -> Result<(), String> {
    println!("Deleting site {domain}...");
    let result = client::agent_delete(&format!("/nginx/sites/{domain}"), token).await?;

    if result["success"].as_bool() == Some(true) {
        println!("\x1b[32m✓\x1b[0m Site deleted: {domain}");
    } else {
        let msg = result["message"].as_str().unwrap_or("Unknown error");
        return Err(format!("Failed to delete site: {msg}"));
    }

    Ok(())
}

pub async fn cmd_sites_info(token: &str, domain: &str) -> Result<(), String> {
    let info = client::agent_get(&format!("/nginx/sites/{domain}"), token).await?;

    println!("\x1b[1mSite: {domain}\x1b[0m");
    println!(
        "  Config:      {}",
        if info["config_exists"].as_bool() == Some(true) {
            "\x1b[32mexists\x1b[0m"
        } else {
            "\x1b[31mnot found\x1b[0m"
        }
    );
    println!(
        "  SSL:         {}",
        if info["ssl_enabled"].as_bool() == Some(true) {
            "\x1b[32menabled\x1b[0m"
        } else {
            "\x1b[90mdisabled\x1b[0m"
        }
    );

    if let Some(cert) = info["ssl_cert_path"].as_str() {
        println!("  Certificate: {cert}");
    }
    if let Some(expiry) = info["ssl_expiry"].as_str() {
        println!("  Expires:     {expiry}");
    }

    Ok(())
}
