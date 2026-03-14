mod client;
mod commands;

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
    /// Run server diagnostics (nginx, resources, SSL, security, logs)
    Diagnose,
    /// Export server configuration as YAML (Infrastructure as Code)
    Export {
        /// Output file (default: stdout)
        #[arg(long, short = 'o')]
        output: Option<String>,
    },
    /// Apply server configuration from YAML file
    Apply {
        /// Path to YAML config file
        file: String,
        /// Show what would change without applying
        #[arg(long)]
        dry_run: bool,
        /// Email for Let's Encrypt SSL provisioning
        #[arg(long)]
        email: Option<String>,
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
        /// Domain for auto reverse proxy
        #[arg(long)]
        domain: Option<String>,
        /// Email for Let's Encrypt SSL (requires --domain)
        #[arg(long)]
        ssl_email: Option<String>,
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
        Commands::Status => commands::status::cmd_status(&token).await,
        Commands::Sites { command } => match command {
            None => commands::sites::cmd_sites_list(&token).await,
            Some(SitesCmd::Create {
                domain,
                runtime,
                proxy_port,
                ssl,
                ssl_email,
            }) => commands::sites::cmd_sites_create(&token, &domain, &runtime, proxy_port, ssl, ssl_email.as_deref()).await,
            Some(SitesCmd::Delete { domain }) => commands::sites::cmd_sites_delete(&token, &domain).await,
            Some(SitesCmd::Info { domain }) => commands::sites::cmd_sites_info(&token, &domain).await,
        },
        Commands::Db { command } => match command {
            None => commands::db::cmd_db_list(&token).await,
            Some(DbCmd::Create {
                name,
                engine,
                password,
                port,
            }) => commands::db::cmd_db_create(&token, &name, &engine, &password, port).await,
            Some(DbCmd::Delete { container_id }) => commands::db::cmd_db_delete(&token, &container_id).await,
        },
        Commands::Apps { command } => match command {
            None => commands::apps::cmd_apps_list(&token).await,
            Some(AppsCmd::Templates) => commands::apps::cmd_apps_templates(&token).await,
            Some(AppsCmd::Deploy {
                template,
                name,
                port,
                domain,
                ssl_email,
            }) => commands::apps::cmd_apps_deploy(&token, &template, &name, port, domain.as_deref(), ssl_email.as_deref()).await,
            Some(AppsCmd::Stop { container_id }) => commands::apps::cmd_apps_action(&token, &container_id, "stop").await,
            Some(AppsCmd::Start { container_id }) => commands::apps::cmd_apps_action(&token, &container_id, "start").await,
            Some(AppsCmd::Restart { container_id }) => commands::apps::cmd_apps_action(&token, &container_id, "restart").await,
            Some(AppsCmd::Remove { container_id }) => commands::apps::cmd_apps_remove(&token, &container_id).await,
            Some(AppsCmd::Logs { container_id }) => commands::apps::cmd_apps_logs(&token, &container_id).await,
            Some(AppsCmd::Compose { file }) => commands::apps::cmd_apps_compose(&token, &file).await,
        },
        Commands::Services => commands::status::cmd_services(&token).await,
        Commands::Ssl { command } => match command {
            SslCmd::Status { domain } => commands::ssl::cmd_ssl_status(&token, &domain).await,
            SslCmd::Provision {
                domain,
                email,
                runtime,
                proxy_port,
            } => commands::ssl::cmd_ssl_provision(&token, &domain, &email, &runtime, proxy_port).await,
        },
        Commands::Backup { command } => match command {
            BackupCmd::Create { domain } => commands::backup::cmd_backup_create(&token, &domain).await,
            BackupCmd::List { domain } => commands::backup::cmd_backup_list(&token, &domain).await,
            BackupCmd::Restore { domain, filename } => {
                commands::backup::cmd_backup_restore(&token, &domain, &filename).await
            }
            BackupCmd::Delete { domain, filename } => {
                commands::backup::cmd_backup_delete(&token, &domain, &filename).await
            }
        },
        Commands::Logs {
            domain,
            r#type,
            lines,
            filter,
            search,
        } => commands::logs::cmd_logs(&token, domain.as_deref(), &r#type, lines, filter.as_deref(), search.as_deref()).await,
        Commands::Security { command } => match command {
            None => commands::security::cmd_security_overview(&token).await,
            Some(SecurityCmd::Scan) => commands::security::cmd_security_scan(&token).await,
            Some(SecurityCmd::Firewall { command }) => match command {
                None => commands::security::cmd_firewall_list(&token).await,
                Some(FirewallCmd::Add {
                    port,
                    proto,
                    action,
                    from,
                }) => commands::security::cmd_firewall_add(&token, port, &proto, &action, from.as_deref()).await,
                Some(FirewallCmd::Remove { number }) => commands::security::cmd_firewall_remove(&token, number).await,
            },
        },
        Commands::Top => commands::status::cmd_top(&token).await,
        Commands::Php { command } => match command {
            None => commands::php::cmd_php_list(&token).await,
            Some(PhpCmd::Install { version }) => commands::php::cmd_php_install(&token, &version).await,
        },
        Commands::Diagnose => commands::status::cmd_diagnose(&token).await,
        Commands::Export { output } => commands::iac::cmd_export(&token, output.as_deref()).await,
        Commands::Apply { file, dry_run, email } => commands::iac::cmd_apply(&token, &file, dry_run, email.as_deref()).await,
    };

    if let Err(e) = result {
        eprintln!("\x1b[31merror:\x1b[0m {e}");
        std::process::exit(1);
    }
}
