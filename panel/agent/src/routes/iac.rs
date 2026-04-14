use crate::safe_cmd::safe_command;
use axum::{http::StatusCode, routing::get, Json, Router};
use serde::Serialize;

use super::AppState;
use crate::services::{database, docker_apps, security};

#[derive(Serialize)]
struct ServerExport {
    version: String,
    sites: Vec<SiteExport>,
    databases: Vec<DbExport>,
    apps: Vec<AppExport>,
    crons: Vec<CronExport>,
    php: Vec<PhpExport>,
    firewall: FirewallExport,
}

#[derive(Serialize)]
struct SiteExport {
    domain: String,
    runtime: String,
    ssl: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    proxy_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    php_version: Option<String>,
}

#[derive(Serialize)]
struct DbExport {
    name: String,
    engine: String,
    port: u16,
}

#[derive(Serialize)]
struct AppExport {
    name: String,
    template: String,
    port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    domain: Option<String>,
}

#[derive(Serialize)]
struct CronExport {
    id: String,
    schedule: String,
    command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
}

#[derive(Serialize)]
struct PhpExport {
    version: String,
    installed: bool,
    fpm_running: bool,
}

#[derive(Serialize)]
struct FirewallExport {
    enabled: bool,
    rules: Vec<FirewallRuleExport>,
}

#[derive(Serialize)]
struct FirewallRuleExport {
    to: String,
    action: String,
}

/// GET /iac/export — Export full server configuration as structured JSON.
async fn export() -> Result<Json<ServerExport>, (StatusCode, Json<serde_json::Value>)> {
    let _map_err = |e: String| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
    };

    // 1. Scan nginx sites from /etc/nginx/sites-enabled/
    let sites = scan_nginx_sites();

    // 2. List databases
    let databases = database::list_databases()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|db| DbExport {
            name: db.name,
            engine: db.engine,
            port: db.port,
        })
        .collect();

    // 3. List Docker apps
    let apps = docker_apps::list_deployed_apps()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|app| {
            let clean_name = app
                .name
                .strip_prefix("dockpanel-app-")
                .unwrap_or(&app.name)
                .to_string();
            AppExport {
                name: clean_name,
                template: app.template,
                port: app.port,
                domain: app.domain,
            }
        })
        .collect();

    // 4. Read crontab entries
    let crons = read_crontab_entries().await;

    // 5. List PHP versions
    let php = check_php_versions().await;

    // 6. Get firewall status
    let firewall = match security::get_firewall_status().await {
        Ok(fw) => FirewallExport {
            enabled: fw.active,
            rules: fw
                .rules
                .into_iter()
                .map(|r| FirewallRuleExport {
                    to: r.to,
                    action: r.action,
                })
                .collect(),
        },
        Err(_) => FirewallExport {
            enabled: false,
            rules: vec![],
        },
    };

    Ok(Json(ServerExport {
        version: "1".to_string(),
        sites,
        databases,
        apps,
        crons,
        php,
        firewall,
    }))
}

/// Scan /etc/nginx/sites-enabled/ and extract site configurations.
fn scan_nginx_sites() -> Vec<SiteExport> {
    let sites_dir = std::path::Path::new("/etc/nginx/sites-enabled");
    let dir = match std::fs::read_dir(sites_dir) {
        Ok(d) => d,
        Err(_) => return vec![],
    };

    let mut sites = Vec::new();

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

            let (runtime, proxy_port, php_version) = if content.contains("proxy_pass") {
                // Extract proxy port from "proxy_pass http://127.0.0.1:PORT"
                let port = content
                    .lines()
                    .find(|l| l.contains("proxy_pass"))
                    .and_then(|l| {
                        l.rsplit(':')
                            .next()
                            .and_then(|p| p.trim_end_matches(';').trim().parse::<u16>().ok())
                    });
                ("proxy".to_string(), port, None)
            } else if content.contains("fastcgi_pass") || content.contains("php") {
                // Extract PHP version from socket path
                let ver = content
                    .lines()
                    .find(|l| l.contains("php") && l.contains("fpm"))
                    .and_then(|l| {
                        l.split("php")
                            .nth(1)
                            .and_then(|s| s.split('-').next().map(|v| v.to_string()))
                    });
                ("php".to_string(), None, ver)
            } else {
                ("static".to_string(), None, None)
            };

            // Extract document root
            let root = content
                .lines()
                .find(|l| {
                    let trimmed = l.trim();
                    trimmed.starts_with("root ") && !trimmed.contains("acme")
                })
                .map(|l| {
                    l.trim()
                        .strip_prefix("root ")
                        .unwrap_or("")
                        .trim_end_matches(';')
                        .trim()
                        .to_string()
                })
                .filter(|r| !r.is_empty());

            sites.push(SiteExport {
                domain,
                runtime,
                ssl,
                proxy_port,
                root,
                php_version,
            });
        }
    }

    // Deduplicate (HTTP + HTTPS blocks for same domain)
    sites.sort_by(|a, b| a.domain.cmp(&b.domain));
    sites.dedup_by(|a, b| {
        if a.domain == b.domain {
            // Keep the one with SSL=true
            if a.ssl {
                b.ssl = true;
            }
            true
        } else {
            false
        }
    });

    sites
}

/// Read dockpanel cron entries from system crontab.
async fn read_crontab_entries() -> Vec<CronExport> {
    let output = safe_command("crontab")
        .arg("-l")
        .output()
        .await;

    let crontab = match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => return vec![],
    };

    let marker = "# dockpanel:";
    crontab
        .lines()
        .filter(|line| line.contains(marker))
        .filter_map(|line| {
            let marker_pos = line.find(marker)?;
            let before = line[..marker_pos].trim();
            let after = &line[marker_pos + marker.len()..];

            let (id, label) = match after.find(' ') {
                Some(pos) => (after[..pos].to_string(), Some(after[pos + 1..].trim().to_string())),
                None => (after.trim().to_string(), None),
            };

            // Split "schedule command" — first 5 fields are schedule, rest is command
            let parts: Vec<&str> = before.splitn(6, ' ').collect();
            if parts.len() < 6 {
                return None;
            }

            let schedule = parts[..5].join(" ");
            let command = parts[5].to_string();

            Some(CronExport {
                id,
                schedule,
                command,
                label: label.filter(|l| !l.is_empty()),
            })
        })
        .collect()
}

/// Check PHP version install/running status.
async fn check_php_versions() -> Vec<PhpExport> {
    let versions = ["8.1", "8.2", "8.3", "8.4"];
    let mut results = Vec::new();

    for v in &versions {
        let installed = safe_command("dpkg")
            .args(["-s", &format!("php{v}-fpm")])
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);

        let fpm_running = if installed {
            safe_command("systemctl")
                .args(["is-active", "--quiet", &format!("php{v}-fpm")])
                .output()
                .await
                .map(|o| o.status.success())
                .unwrap_or(false)
        } else {
            false
        };

        // Only include installed versions in export
        if installed {
            results.push(PhpExport {
                version: v.to_string(),
                installed,
                fpm_running,
            });
        }
    }

    results
}

pub fn router() -> Router<AppState> {
    Router::new().route("/iac/export", get(export))
}
