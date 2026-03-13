mod client;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "dockpanel",
    about = "DockPanel CLI — self-hosted server management",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Show server status (CPU, memory, disk, uptime)
    Status,
    /// List all sites
    Sites,
    /// List databases
    Db,
    /// List Docker apps
    Apps,
    /// Check service health
    Services,
    /// Show SSL certificate status for a domain
    Ssl {
        /// Domain to check
        domain: String,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let token = match client::load_token() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("\x1b[31merror:\x1b[0m {e}");
            std::process::exit(1);
        }
    };

    let result = match cli.command {
        Commands::Status => cmd_status(&token).await,
        Commands::Sites => cmd_sites(&token).await,
        Commands::Db => cmd_db(&token).await,
        Commands::Apps => cmd_apps(&token).await,
        Commands::Services => cmd_services(&token).await,
        Commands::Ssl { domain } => cmd_ssl(&token, &domain).await,
    };

    if let Err(e) = result {
        eprintln!("\x1b[31merror:\x1b[0m {e}");
        std::process::exit(1);
    }
}

async fn cmd_status(token: &str) -> Result<(), String> {
    let info = client::agent_get("/system/info", token).await?;

    let hostname = info["hostname"].as_str().unwrap_or("unknown");
    let os = info["os"].as_str().unwrap_or("unknown");
    let kernel = info["kernel"].as_str().unwrap_or("unknown");
    let cpu_model = info["cpu_model"].as_str().unwrap_or("unknown");
    let cpu_count = info["cpu_count"].as_u64().unwrap_or(0);
    let cpu_usage = info["cpu_usage"].as_f64().unwrap_or(0.0);
    let mem_total = info["mem_total_mb"].as_u64().unwrap_or(0);
    let mem_used = info["mem_used_mb"].as_u64().unwrap_or(0);
    let mem_pct = info["mem_usage_pct"].as_f64().unwrap_or(0.0);
    let disk_total = info["disk_total_gb"].as_f64().unwrap_or(0.0);
    let disk_used = info["disk_used_gb"].as_f64().unwrap_or(0.0);
    let disk_pct = info["disk_usage_pct"].as_f64().unwrap_or(0.0);
    let uptime = info["uptime_secs"].as_u64().unwrap_or(0);
    let load1 = info["load_avg_1"].as_f64().unwrap_or(0.0);
    let load5 = info["load_avg_5"].as_f64().unwrap_or(0.0);
    let load15 = info["load_avg_15"].as_f64().unwrap_or(0.0);
    let procs = info["process_count"].as_u64().unwrap_or(0);

    let days = uptime / 86400;
    let hours = (uptime % 86400) / 3600;
    let mins = (uptime % 3600) / 60;

    println!("\x1b[1mServer Status\x1b[0m");
    println!("  Hostname:    {hostname}");
    println!("  OS:          {os}");
    println!("  Kernel:      {kernel}");
    println!("  Uptime:      {days}d {hours}h {mins}m");
    println!();
    println!("\x1b[1mCPU\x1b[0m");
    println!("  Model:       {cpu_model}");
    println!("  Cores:       {cpu_count}");
    println!("  Usage:       {}{:.1}%\x1b[0m", usage_color(cpu_usage), cpu_usage);
    println!("  Load:        {load1:.2} / {load5:.2} / {load15:.2}");
    println!();
    println!("\x1b[1mMemory\x1b[0m");
    println!(
        "  Used:        {}{mem_used} MB\x1b[0m / {mem_total} MB ({mem_pct:.1}%)",
        usage_color(mem_pct)
    );
    println!();
    println!("\x1b[1mDisk\x1b[0m");
    println!(
        "  Used:        {}{disk_used:.1} GB\x1b[0m / {disk_total:.1} GB ({disk_pct:.1}%)",
        usage_color(disk_pct)
    );
    println!();
    println!("  Processes:   {procs}");

    Ok(())
}

async fn cmd_sites(token: &str) -> Result<(), String> {
    // List nginx site configs by checking known config directories
    let info = client::agent_get("/system/info", token).await?;
    let _ = info; // just to verify connectivity

    // Read site configs from /etc/nginx/sites-enabled/
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
            site.domain, site.runtime, if site.ssl { "yes" } else { "no" }
        );
    }

    println!("\n{} site(s)", sites.len());
    Ok(())
}

struct SiteInfo {
    domain: String,
    runtime: String,
    ssl: bool,
}

fn list_nginx_sites() -> Vec<SiteInfo> {
    let mut sites = Vec::new();
    let sites_dir = std::path::Path::new("/etc/nginx/sites-enabled");

    let dir = match std::fs::read_dir(sites_dir) {
        Ok(d) => d,
        Err(_) => return sites,
    };

    for entry in dir.flatten() {
        let path = entry.path();
        let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        // Skip panel config and default
        if filename == "dockpanel-panel.conf"
            || filename == "dockpanel.dev.conf"
            || filename == "default"
        {
            continue;
        }

        if let Ok(content) = std::fs::read_to_string(&path) {
            // Extract primary domain (first server_name value)
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

    // Deduplicate (HTTP + HTTPS blocks for same domain)
    sites.sort_by(|a, b| a.domain.cmp(&b.domain));
    sites.dedup_by(|a, b| a.domain == b.domain);

    sites
}

async fn cmd_db(token: &str) -> Result<(), String> {
    let dbs = client::agent_get("/databases", token).await?;
    let dbs = dbs.as_array().ok_or("Expected array from /databases")?;

    if dbs.is_empty() {
        println!("No databases.");
        return Ok(());
    }

    println!(
        "\x1b[1m{:<20} {:<12} {:<8} {:<12}\x1b[0m",
        "NAME", "ENGINE", "PORT", "STATUS"
    );

    for db in dbs {
        let name = db["name"].as_str().unwrap_or("-");
        let engine = db["engine"].as_str().unwrap_or("-");
        let port = db["port"].as_u64().unwrap_or(0);
        let status = db["status"].as_str().unwrap_or("-");

        let color = if status == "running" {
            "\x1b[32m"
        } else {
            "\x1b[31m"
        };

        println!(
            "{:<20} {:<12} {:<8} {color}{:<12}\x1b[0m",
            name, engine, port, status
        );
    }

    println!("\n{} database(s)", dbs.len());
    Ok(())
}

async fn cmd_apps(token: &str) -> Result<(), String> {
    let apps = client::agent_get("/apps", token).await?;
    let apps = apps.as_array().ok_or("Expected array from /apps")?;

    if apps.is_empty() {
        println!("No Docker apps deployed.");
        return Ok(());
    }

    println!(
        "\x1b[1m{:<20} {:<15} {:<8} {:<12}\x1b[0m",
        "NAME", "TEMPLATE", "PORT", "STATUS"
    );

    for app in apps {
        let name = app["name"].as_str().unwrap_or("-");
        let template = app["template"].as_str().unwrap_or("-");
        let port = app["port"].as_u64().map(|p| p.to_string()).unwrap_or("-".to_string());
        let status = app["status"].as_str().unwrap_or("-");

        let color = if status == "running" {
            "\x1b[32m"
        } else {
            "\x1b[31m"
        };

        println!(
            "{:<20} {:<15} {:<8} {color}{:<12}\x1b[0m",
            name, template, port, status
        );
    }

    println!("\n{} app(s)", apps.len());
    Ok(())
}

async fn cmd_services(token: &str) -> Result<(), String> {
    let svcs = client::agent_get("/services/health", token).await?;
    let svcs = svcs.as_array().ok_or("Expected array from /services/health")?;

    println!("\x1b[1m{:<25} {:<15}\x1b[0m", "SERVICE", "STATUS");

    for svc in svcs {
        let name = svc["name"].as_str().unwrap_or("-");
        let status = svc["status"].as_str().unwrap_or("-");

        let color = match status {
            "running" => "\x1b[32m",
            "stopped" | "failed" => "\x1b[31m",
            "disabled" | "not_installed" => "\x1b[90m",
            _ => "\x1b[33m",
        };

        println!("{:<25} {color}{:<15}\x1b[0m", name, status);
    }

    Ok(())
}

async fn cmd_ssl(token: &str, domain: &str) -> Result<(), String> {
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

fn usage_color(pct: f64) -> &'static str {
    if pct > 90.0 {
        "\x1b[31m" // red
    } else if pct > 70.0 {
        "\x1b[33m" // yellow
    } else {
        "\x1b[32m" // green
    }
}
