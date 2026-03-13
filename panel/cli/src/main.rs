mod client;

use clap::{Parser, Subcommand};
use serde_json::json;

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
    /// Manage nginx sites
    Sites {
        #[command(subcommand)]
        command: Option<SitesCmd>,
    },
    /// Manage databases
    Db {
        #[command(subcommand)]
        command: Option<DbCmd>,
    },
    /// Manage Docker apps
    Apps {
        #[command(subcommand)]
        command: Option<AppsCmd>,
    },
    /// Check service health
    Services,
    /// SSL certificate management
    Ssl {
        #[command(subcommand)]
        command: SslCmd,
    },
    /// Backup management
    Backup {
        #[command(subcommand)]
        command: BackupCmd,
    },
    /// View system and site logs
    Logs {
        /// Domain for site-specific logs
        #[arg(long, short = 'd')]
        domain: Option<String>,
        /// Log type (syslog, nginx, auth, php, mysql)
        #[arg(long, short = 't', default_value = "syslog")]
        r#type: String,
        /// Number of lines
        #[arg(long, short = 'n', default_value = "50")]
        lines: u32,
        /// Filter text
        #[arg(long, short = 'f')]
        filter: Option<String>,
        /// Search pattern (regex)
        #[arg(long, short = 's')]
        search: Option<String>,
    },
    /// Security overview and management
    Security {
        #[command(subcommand)]
        command: Option<SecurityCmd>,
    },
    /// Show top processes by CPU usage
    Top,
    /// PHP version management
    Php {
        #[command(subcommand)]
        command: Option<PhpCmd>,
    },
}

#[derive(Subcommand)]
enum SitesCmd {
    /// Create a new site
    Create {
        /// Domain name
        domain: String,
        /// Runtime type: static, php, or proxy
        #[arg(long, default_value = "static")]
        runtime: String,
        /// Proxy port (required for --runtime proxy)
        #[arg(long)]
        proxy_port: Option<u16>,
        /// Enable SSL (auto-provision with Let's Encrypt)
        #[arg(long)]
        ssl: bool,
        /// Email for Let's Encrypt (required with --ssl)
        #[arg(long)]
        ssl_email: Option<String>,
    },
    /// Delete a site
    Delete {
        /// Domain name
        domain: String,
    },
    /// Show site details
    Info {
        /// Domain name
        domain: String,
    },
}

#[derive(Subcommand)]
enum DbCmd {
    /// Create a new database
    Create {
        /// Database name
        name: String,
        /// Engine: mysql, mariadb, or postgres
        #[arg(long)]
        engine: String,
        /// Root/admin password
        #[arg(long)]
        password: String,
        /// Host port
        #[arg(long)]
        port: u16,
    },
    /// Delete a database
    Delete {
        /// Container ID
        container_id: String,
    },
}

#[derive(Subcommand)]
enum AppsCmd {
    /// List available app templates
    Templates,
    /// Deploy an app from a template
    Deploy {
        /// Template ID
        template: String,
        /// App name
        #[arg(long)]
        name: String,
        /// Host port
        #[arg(long)]
        port: u16,
    },
    /// Stop a running app
    Stop {
        /// Container ID
        container_id: String,
    },
    /// Start a stopped app
    Start {
        /// Container ID
        container_id: String,
    },
    /// Restart an app
    Restart {
        /// Container ID
        container_id: String,
    },
    /// Remove an app
    Remove {
        /// Container ID
        container_id: String,
    },
    /// View app logs
    Logs {
        /// Container ID
        container_id: String,
    },
    /// Deploy from a docker-compose.yml file
    Compose {
        /// Path to docker-compose.yml
        file: String,
    },
}

#[derive(Subcommand)]
enum SslCmd {
    /// Check certificate status
    Status {
        /// Domain name
        domain: String,
    },
    /// Provision Let's Encrypt certificate
    Provision {
        /// Domain name
        domain: String,
        /// Email for Let's Encrypt
        #[arg(long)]
        email: String,
        /// Site runtime type: static, php, or proxy
        #[arg(long, default_value = "static")]
        runtime: String,
        /// Proxy port (for proxy runtime)
        #[arg(long)]
        proxy_port: Option<u16>,
    },
}

#[derive(Subcommand)]
enum BackupCmd {
    /// Create a backup
    Create {
        /// Domain name
        domain: String,
    },
    /// List backups for a domain
    List {
        /// Domain name
        domain: String,
    },
    /// Restore from a backup
    Restore {
        /// Domain name
        domain: String,
        /// Backup filename
        filename: String,
    },
    /// Delete a backup
    Delete {
        /// Domain name
        domain: String,
        /// Backup filename
        filename: String,
    },
}

#[derive(Subcommand)]
enum SecurityCmd {
    /// Run a security scan
    Scan,
    /// Firewall management
    Firewall {
        #[command(subcommand)]
        command: Option<FirewallCmd>,
    },
}

#[derive(Subcommand)]
enum FirewallCmd {
    /// Add a firewall rule
    Add {
        /// Port number
        #[arg(long)]
        port: u16,
        /// Protocol: tcp or udp
        #[arg(long, default_value = "tcp")]
        proto: String,
        /// Action: allow or deny
        #[arg(long, default_value = "allow")]
        action: String,
        /// Source IP/CIDR
        #[arg(long)]
        from: Option<String>,
    },
    /// Remove a firewall rule by number
    Remove {
        /// Rule number
        number: u32,
    },
}

#[derive(Subcommand)]
enum PhpCmd {
    /// Install a PHP version
    Install {
        /// PHP version (8.1, 8.2, 8.3, 8.4)
        version: String,
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
        Commands::Sites { command } => match command {
            None => cmd_sites_list(&token).await,
            Some(SitesCmd::Create {
                domain,
                runtime,
                proxy_port,
                ssl,
                ssl_email,
            }) => cmd_sites_create(&token, &domain, &runtime, proxy_port, ssl, ssl_email.as_deref()).await,
            Some(SitesCmd::Delete { domain }) => cmd_sites_delete(&token, &domain).await,
            Some(SitesCmd::Info { domain }) => cmd_sites_info(&token, &domain).await,
        },
        Commands::Db { command } => match command {
            None => cmd_db_list(&token).await,
            Some(DbCmd::Create {
                name,
                engine,
                password,
                port,
            }) => cmd_db_create(&token, &name, &engine, &password, port).await,
            Some(DbCmd::Delete { container_id }) => cmd_db_delete(&token, &container_id).await,
        },
        Commands::Apps { command } => match command {
            None => cmd_apps_list(&token).await,
            Some(AppsCmd::Templates) => cmd_apps_templates(&token).await,
            Some(AppsCmd::Deploy {
                template,
                name,
                port,
            }) => cmd_apps_deploy(&token, &template, &name, port).await,
            Some(AppsCmd::Stop { container_id }) => cmd_apps_action(&token, &container_id, "stop").await,
            Some(AppsCmd::Start { container_id }) => cmd_apps_action(&token, &container_id, "start").await,
            Some(AppsCmd::Restart { container_id }) => cmd_apps_action(&token, &container_id, "restart").await,
            Some(AppsCmd::Remove { container_id }) => cmd_apps_remove(&token, &container_id).await,
            Some(AppsCmd::Logs { container_id }) => cmd_apps_logs(&token, &container_id).await,
            Some(AppsCmd::Compose { file }) => cmd_apps_compose(&token, &file).await,
        },
        Commands::Services => cmd_services(&token).await,
        Commands::Ssl { command } => match command {
            SslCmd::Status { domain } => cmd_ssl_status(&token, &domain).await,
            SslCmd::Provision {
                domain,
                email,
                runtime,
                proxy_port,
            } => cmd_ssl_provision(&token, &domain, &email, &runtime, proxy_port).await,
        },
        Commands::Backup { command } => match command {
            BackupCmd::Create { domain } => cmd_backup_create(&token, &domain).await,
            BackupCmd::List { domain } => cmd_backup_list(&token, &domain).await,
            BackupCmd::Restore { domain, filename } => {
                cmd_backup_restore(&token, &domain, &filename).await
            }
            BackupCmd::Delete { domain, filename } => {
                cmd_backup_delete(&token, &domain, &filename).await
            }
        },
        Commands::Logs {
            domain,
            r#type,
            lines,
            filter,
            search,
        } => cmd_logs(&token, domain.as_deref(), &r#type, lines, filter.as_deref(), search.as_deref()).await,
        Commands::Security { command } => match command {
            None => cmd_security_overview(&token).await,
            Some(SecurityCmd::Scan) => cmd_security_scan(&token).await,
            Some(SecurityCmd::Firewall { command }) => match command {
                None => cmd_firewall_list(&token).await,
                Some(FirewallCmd::Add {
                    port,
                    proto,
                    action,
                    from,
                }) => cmd_firewall_add(&token, port, &proto, &action, from.as_deref()).await,
                Some(FirewallCmd::Remove { number }) => cmd_firewall_remove(&token, number).await,
            },
        },
        Commands::Top => cmd_top(&token).await,
        Commands::Php { command } => match command {
            None => cmd_php_list(&token).await,
            Some(PhpCmd::Install { version }) => cmd_php_install(&token, &version).await,
        },
    };

    if let Err(e) = result {
        eprintln!("\x1b[31merror:\x1b[0m {e}");
        std::process::exit(1);
    }
}

// ── Status ──────────────────────────────────────────────────────────────

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
    println!(
        "  Usage:       {}{:.1}%\x1b[0m",
        usage_color(cpu_usage),
        cpu_usage
    );
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

// ── Sites ───────────────────────────────────────────────────────────────

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

async fn cmd_sites_list(token: &str) -> Result<(), String> {
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

async fn cmd_sites_create(
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

async fn cmd_sites_delete(token: &str, domain: &str) -> Result<(), String> {
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

async fn cmd_sites_info(token: &str, domain: &str) -> Result<(), String> {
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

// ── Databases ───────────────────────────────────────────────────────────

async fn cmd_db_list(token: &str) -> Result<(), String> {
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

async fn cmd_db_create(
    token: &str,
    name: &str,
    engine: &str,
    password: &str,
    port: u16,
) -> Result<(), String> {
    match engine {
        "mysql" | "mariadb" | "postgres" => {}
        _ => return Err(format!("Invalid engine '{engine}'. Use: mysql, mariadb, or postgres")),
    }

    println!("Creating {engine} database '{name}' on port {port}...");
    let body = json!({
        "name": name,
        "engine": engine,
        "password": password,
        "port": port,
    });

    let result = client::agent_post("/databases", &body, token).await?;

    if result["success"].as_bool() == Some(true) {
        let cid = result["container_id"].as_str().unwrap_or("unknown");
        println!("\x1b[32m✓\x1b[0m Database created");
        println!("  Name:         {name}");
        println!("  Engine:       {engine}");
        println!("  Port:         {port}");
        println!("  Container:    {}", &cid[..cid.len().min(12)]);
    } else {
        let msg = result["message"].as_str().unwrap_or("Unknown error");
        return Err(format!("Failed to create database: {msg}"));
    }

    Ok(())
}

async fn cmd_db_delete(token: &str, container_id: &str) -> Result<(), String> {
    println!("Deleting database container {container_id}...");
    let result = client::agent_delete(&format!("/databases/{container_id}"), token).await?;

    if result["success"].as_bool() == Some(true) {
        println!("\x1b[32m✓\x1b[0m Database deleted");
    } else {
        let msg = result["message"].as_str().unwrap_or("Unknown error");
        return Err(format!("Failed to delete database: {msg}"));
    }

    Ok(())
}

// ── Docker Apps ─────────────────────────────────────────────────────────

async fn cmd_apps_list(token: &str) -> Result<(), String> {
    let apps = client::agent_get("/apps", token).await?;
    let apps = apps.as_array().ok_or("Expected array from /apps")?;

    if apps.is_empty() {
        println!("No Docker apps deployed.");
        return Ok(());
    }

    println!(
        "\x1b[1m{:<14} {:<20} {:<15} {:<8} {:<12}\x1b[0m",
        "CONTAINER", "NAME", "TEMPLATE", "PORT", "STATUS"
    );

    for app in apps {
        let cid = app["container_id"].as_str().unwrap_or("-");
        let short_id = &cid[..cid.len().min(12)];
        let name = app["name"].as_str().unwrap_or("-");
        let template = app["template"].as_str().unwrap_or("-");
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
            "{:<14} {:<20} {:<15} {:<8} {color}{:<12}\x1b[0m",
            short_id, name, template, port, status
        );
    }

    println!("\n{} app(s)", apps.len());
    Ok(())
}

async fn cmd_apps_templates(token: &str) -> Result<(), String> {
    let templates = client::agent_get("/apps/templates", token).await?;
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

async fn cmd_apps_deploy(
    token: &str,
    template: &str,
    name: &str,
    port: u16,
) -> Result<(), String> {
    println!("Deploying app '{name}' from template '{template}' on port {port}...");

    let body = json!({
        "template_id": template,
        "name": name,
        "port": port,
    });

    let result = client::agent_post("/apps/deploy", &body, token).await?;

    if result["success"].as_bool() == Some(true) {
        let cid = result["container_id"].as_str().unwrap_or("unknown");
        println!("\x1b[32m✓\x1b[0m App deployed");
        println!("  Name:         {name}");
        println!("  Port:         {port}");
        println!("  Container:    {}", &cid[..cid.len().min(12)]);
    } else {
        let msg = result["message"].as_str().unwrap_or("Unknown error");
        return Err(format!("Failed to deploy app: {msg}"));
    }

    Ok(())
}

async fn cmd_apps_action(token: &str, container_id: &str, action: &str) -> Result<(), String> {
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

async fn cmd_apps_remove(token: &str, container_id: &str) -> Result<(), String> {
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

async fn cmd_apps_logs(token: &str, container_id: &str) -> Result<(), String> {
    let result = client::agent_get(&format!("/apps/{container_id}/logs"), token).await?;

    let logs = result["logs"].as_str().unwrap_or("");
    if logs.is_empty() {
        println!("No logs available.");
    } else {
        print!("{logs}");
    }

    Ok(())
}

async fn cmd_apps_compose(token: &str, file: &str) -> Result<(), String> {
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

// ── Services ────────────────────────────────────────────────────────────

async fn cmd_services(token: &str) -> Result<(), String> {
    let svcs = client::agent_get("/services/health", token).await?;
    let svcs = svcs
        .as_array()
        .ok_or("Expected array from /services/health")?;

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

// ── SSL ─────────────────────────────────────────────────────────────────

async fn cmd_ssl_status(token: &str, domain: &str) -> Result<(), String> {
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

async fn cmd_ssl_provision(
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

// ── Backups ─────────────────────────────────────────────────────────────

async fn cmd_backup_create(token: &str, domain: &str) -> Result<(), String> {
    println!("Creating backup for {domain}...");

    let result = client::agent_post_empty(&format!("/backups/{domain}/create"), token).await?;

    let filename = result["filename"].as_str().unwrap_or("unknown");
    let size = result["size_bytes"].as_u64().unwrap_or(0);
    let size_mb = size as f64 / 1_048_576.0;

    println!("\x1b[32m✓\x1b[0m Backup created");
    println!("  File:    {filename}");
    println!("  Size:    {size_mb:.1} MB");

    Ok(())
}

async fn cmd_backup_list(token: &str, domain: &str) -> Result<(), String> {
    let backups = client::agent_get(&format!("/backups/{domain}/list"), token).await?;
    let backups = backups.as_array().ok_or("Expected array from /backups")?;

    if backups.is_empty() {
        println!("No backups for {domain}.");
        return Ok(());
    }

    println!(
        "\x1b[1m{:<40} {:<12} {:<20}\x1b[0m",
        "FILENAME", "SIZE", "CREATED"
    );

    for b in backups {
        let filename = b["filename"].as_str().unwrap_or("-");
        let size = b["size_bytes"].as_u64().unwrap_or(0);
        let size_mb = size as f64 / 1_048_576.0;
        let created = b["created"].as_str().unwrap_or("-");

        println!(
            "{:<40} {:<12} {:<20}",
            filename,
            format!("{size_mb:.1} MB"),
            created
        );
    }

    println!("\n{} backup(s)", backups.len());
    Ok(())
}

async fn cmd_backup_restore(token: &str, domain: &str, filename: &str) -> Result<(), String> {
    println!("Restoring {domain} from {filename}...");

    let result = client::agent_post_empty(
        &format!("/backups/{domain}/restore/{filename}"),
        token,
    )
    .await?;

    if result["success"].as_bool() == Some(true) {
        println!("\x1b[32m✓\x1b[0m Backup restored");
    } else {
        let msg = result["message"].as_str().unwrap_or("Unknown error");
        return Err(format!("Failed to restore backup: {msg}"));
    }

    Ok(())
}

async fn cmd_backup_delete(token: &str, domain: &str, filename: &str) -> Result<(), String> {
    println!("Deleting backup {filename}...");

    let result = client::agent_delete(
        &format!("/backups/{domain}/{filename}"),
        token,
    )
    .await?;

    if result["success"].as_bool() == Some(true) {
        println!("\x1b[32m✓\x1b[0m Backup deleted");
    } else {
        let msg = result["message"].as_str().unwrap_or("Unknown error");
        return Err(format!("Failed to delete backup: {msg}"));
    }

    Ok(())
}

// ── Logs ────────────────────────────────────────────────────────────────

async fn cmd_logs(
    token: &str,
    domain: Option<&str>,
    log_type: &str,
    lines: u32,
    filter: Option<&str>,
    search: Option<&str>,
) -> Result<(), String> {
    // Search mode
    if let Some(pattern) = search {
        let query = format!("/logs/search?pattern={}&type={log_type}", urlenc(pattern));
        let result = client::agent_get(&query, token).await?;
        let entries = result.as_array().ok_or("Expected array from /logs/search")?;

        if entries.is_empty() {
            println!("No matches found.");
        } else {
            for entry in entries {
                if let Some(line) = entry.as_str() {
                    println!("{line}");
                }
            }
            println!("\n{} match(es)", entries.len());
        }
        return Ok(());
    }

    // Domain-specific or system logs
    let path = if let Some(domain) = domain {
        let mut p = format!("/logs/{domain}?type={log_type}&lines={lines}");
        if let Some(f) = filter {
            p.push_str(&format!("&filter={}", urlenc(f)));
        }
        p
    } else {
        let mut p = format!("/logs?type={log_type}&lines={lines}");
        if let Some(f) = filter {
            p.push_str(&format!("&filter={}", urlenc(f)));
        }
        p
    };

    let result = client::agent_get(&path, token).await?;
    let entries = result.as_array().ok_or("Expected array from /logs")?;

    if entries.is_empty() {
        println!("No log entries.");
    } else {
        for entry in entries {
            if let Some(line) = entry.as_str() {
                println!("{line}");
            }
        }
    }

    Ok(())
}

// ── Security ────────────────────────────────────────────────────────────

async fn cmd_security_overview(token: &str) -> Result<(), String> {
    let overview = client::agent_get("/security/overview", token).await?;

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

async fn cmd_security_scan(token: &str) -> Result<(), String> {
    println!("Running security scan...");

    let result = client::agent_post_empty("/security/scan", token).await?;

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

async fn cmd_firewall_list(token: &str) -> Result<(), String> {
    let fw = client::agent_get("/security/firewall", token).await?;

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

async fn cmd_firewall_add(
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

async fn cmd_firewall_remove(token: &str, number: u32) -> Result<(), String> {
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

// ── Top (processes) ─────────────────────────────────────────────────────

async fn cmd_top(token: &str) -> Result<(), String> {
    let procs = client::agent_get("/system/processes", token).await?;
    let procs = procs
        .as_array()
        .ok_or("Expected array from /system/processes")?;

    println!(
        "\x1b[1m{:<8} {:<30} {:<10} {:<10}\x1b[0m",
        "PID", "NAME", "CPU %", "MEM MB"
    );

    for p in procs {
        let pid = p["pid"].as_u64().unwrap_or(0);
        let name = p["name"].as_str().unwrap_or("-");
        let cpu = p["cpu_pct"].as_f64().unwrap_or(0.0);
        let mem = p["mem_mb"].as_f64().unwrap_or(0.0);

        let cpu_color = usage_color(cpu);
        println!(
            "{:<8} {:<30} {cpu_color}{:<10.1}\x1b[0m {:<10.1}",
            pid, name, cpu, mem
        );
    }

    Ok(())
}

// ── PHP ─────────────────────────────────────────────────────────────────

async fn cmd_php_list(token: &str) -> Result<(), String> {
    let result = client::agent_get("/php/versions", token).await?;
    let versions = result["versions"]
        .as_array()
        .ok_or("Expected versions array from /php/versions")?;

    println!(
        "\x1b[1m{:<10} {:<12} {:<12} {:<30}\x1b[0m",
        "VERSION", "INSTALLED", "FPM", "SOCKET"
    );

    for v in versions {
        let version = v["version"].as_str().unwrap_or("-");
        let installed = v["installed"].as_bool().unwrap_or(false);
        let fpm = v["fpm_running"].as_bool().unwrap_or(false);
        let socket = v["socket"].as_str().unwrap_or("-");

        let inst_color = if installed { "\x1b[32m" } else { "\x1b[90m" };
        let fpm_color = if fpm { "\x1b[32m" } else { "\x1b[90m" };

        println!(
            "{:<10} {inst_color}{:<12}\x1b[0m {fpm_color}{:<12}\x1b[0m {:<30}",
            version,
            if installed { "yes" } else { "no" },
            if fpm { "running" } else { "stopped" },
            if installed { socket } else { "-" }
        );
    }

    Ok(())
}

async fn cmd_php_install(token: &str, version: &str) -> Result<(), String> {
    match version {
        "8.1" | "8.2" | "8.3" | "8.4" => {}
        _ => return Err(format!("Invalid PHP version '{version}'. Supported: 8.1, 8.2, 8.3, 8.4")),
    }

    println!("Installing PHP {version}...");

    let body = json!({ "version": version });
    let result = client::agent_post("/php/install", &body, token).await?;

    if result["success"].as_bool() == Some(true) {
        let msg = result["message"].as_str().unwrap_or("Installed successfully");
        println!("\x1b[32m✓\x1b[0m {msg}");
    } else {
        let msg = result["message"].as_str().unwrap_or("Unknown error");
        return Err(format!("Failed to install PHP {version}: {msg}"));
    }

    Ok(())
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn usage_color(pct: f64) -> &'static str {
    if pct > 90.0 {
        "\x1b[31m" // red
    } else if pct > 70.0 {
        "\x1b[33m" // yellow
    } else {
        "\x1b[32m" // green
    }
}

fn urlenc(s: &str) -> String {
    s.replace('%', "%25")
        .replace(' ', "%20")
        .replace('&', "%26")
        .replace('=', "%3D")
        .replace('+', "%2B")
        .replace('#', "%23")
}
