use serde::Serialize;
use tokio::process::Command;

#[derive(Serialize, Clone)]
pub struct DiagnosticFinding {
    pub id: String,
    pub category: String,
    pub severity: String, // critical, warning, info
    pub title: String,
    pub description: String,
    pub fix_available: bool,
    pub fix_id: Option<String>,
}

#[derive(Serialize)]
pub struct DiagnosticReport {
    pub findings: Vec<DiagnosticFinding>,
    pub summary: DiagnosticSummary,
}

#[derive(Serialize)]
pub struct DiagnosticSummary {
    pub critical: usize,
    pub warning: usize,
    pub info: usize,
    pub total: usize,
}

/// Run all diagnostic checks and return a consolidated report.
pub async fn run_diagnostics() -> DiagnosticReport {
    let (nginx, resources, services, ssl, logs, security) = tokio::join!(
        check_nginx(),
        check_resources(),
        check_services(),
        check_ssl_expiry(),
        check_log_patterns(),
        check_security(),
    );

    let mut findings = Vec::new();
    findings.extend(nginx);
    findings.extend(resources);
    findings.extend(services);
    findings.extend(ssl);
    findings.extend(logs);
    findings.extend(security);

    let critical = findings.iter().filter(|f| f.severity == "critical").count();
    let warning = findings.iter().filter(|f| f.severity == "warning").count();
    let info = findings.iter().filter(|f| f.severity == "info").count();

    DiagnosticReport {
        summary: DiagnosticSummary {
            critical,
            warning,
            info,
            total: findings.len(),
        },
        findings,
    }
}

// ── Nginx Checks ─────────────────────────────────────────────────────────

async fn check_nginx() -> Vec<DiagnosticFinding> {
    let mut findings = Vec::new();

    // 1. nginx -t  (config validation)
    let test_output = match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        Command::new("nginx").arg("-t").output(),
    )
    .await
    {
        Ok(Ok(o)) => Some(o),
        _ => {
            findings.push(DiagnosticFinding {
                id: "nginx-not-found".into(),
                category: "nginx".into(),
                severity: "critical".into(),
                title: "Nginx not installed or unreachable".into(),
                description: "Could not run nginx -t to validate configuration.".into(),
                fix_available: false,
                fix_id: None,
            });
            None
        }
    };

    if let Some(out) = test_output {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let has_fatal = stderr.contains("[emerg]") || stderr.contains("[error]")
            || stderr.contains("[crit]") || stderr.contains("[alert]");
        let warn_lines: Vec<&str> = stderr.lines().filter(|l| l.contains("[warn]")).collect();

        if has_fatal {
            // Real failure — nginx config is broken
            let error_lines: Vec<&str> = stderr.lines()
                .filter(|l| l.contains("[emerg]") || l.contains("[error]") || l.contains("[crit]"))
                .collect();
            findings.push(DiagnosticFinding {
                id: "nginx-config-invalid".into(),
                category: "nginx".into(),
                severity: "critical".into(),
                title: "Nginx configuration is invalid".into(),
                description: format!("nginx -t: {}", error_lines.iter().take(3).cloned().collect::<Vec<_>>().join(" ")),
                fix_available: false,
                fix_id: None,
            });
        } else if !warn_lines.is_empty() {
            // Warnings only — nginx works fine, cosmetic issues
            findings.push(DiagnosticFinding {
                id: "nginx-config-warnings".into(),
                category: "nginx".into(),
                severity: "info".into(),
                title: format!("Nginx has {} configuration warning{}", warn_lines.len(), if warn_lines.len() == 1 { "" } else { "s" }),
                description: warn_lines.iter().take(3).map(|w| {
                    w.split("[warn]").last().unwrap_or(w).trim().to_string()
                }).collect::<Vec<_>>().join("; "),
                fix_available: false,
                fix_id: None,
            });
        } else if !out.status.success() {
            // Unknown failure
            findings.push(DiagnosticFinding {
                id: "nginx-config-invalid".into(),
                category: "nginx".into(),
                severity: "critical".into(),
                title: "Nginx configuration is invalid".into(),
                description: format!("nginx -t exited with code {}: {}", out.status.code().unwrap_or(-1), stderr.lines().take(3).collect::<Vec<_>>().join(" ")),
                fix_available: false,
                fix_id: None,
            });
        }
    }

    // 2. Check for sites-enabled configs without matching root directory
    if let Ok(mut entries) = tokio::fs::read_dir("/etc/nginx/sites-enabled").await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            if filename == "default" || filename == "dockpanel-panel.conf" || filename == "dockpanel.dev.conf" {
                continue;
            }

            if let Ok(content) = tokio::fs::read_to_string(&path).await {
                // Extract root directive
                for line in content.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with("root ") && !trimmed.starts_with("root /var/www/html") {
                        let root_path = trimmed
                            .strip_prefix("root ")
                            .unwrap_or("")
                            .trim_end_matches(';')
                            .trim();
                        if !root_path.is_empty() && !tokio::fs::metadata(root_path).await.is_ok() {
                            let domain = filename.trim_end_matches(".conf");
                            findings.push(DiagnosticFinding {
                                id: format!("nginx-missing-root-{domain}"),
                                category: "nginx".into(),
                                severity: "warning".into(),
                                title: format!("Missing document root for {domain}"),
                                description: format!("Root directory {root_path} does not exist"),
                                fix_available: true,
                                fix_id: Some(format!("create-root:{root_path}")),
                            });
                        }
                        break;
                    }
                }

                // Check for duplicate server_name across configs
                // (lightweight: just flag if server_name is "_")
                if content.lines().any(|l| {
                    let t = l.trim();
                    t.starts_with("server_name") && t.contains(" _ ")
                }) {
                    findings.push(DiagnosticFinding {
                        id: format!("nginx-default-server-{filename}"),
                        category: "nginx".into(),
                        severity: "info".into(),
                        title: format!("Default server block in {filename}"),
                        description: "Config uses catch-all server_name _".into(),
                        fix_available: false,
                        fix_id: None,
                    });
                }
            }
        }
    }

    findings
}

// ── Resource Checks ──────────────────────────────────────────────────────

async fn check_resources() -> Vec<DiagnosticFinding> {
    let mut findings = Vec::new();

    // Disk usage
    if let Ok(Ok(output)) = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        Command::new("df").args(["--output=pcent", "/"]).output(),
    )
    .await
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Some(pct_line) = stdout.lines().nth(1) {
            if let Ok(pct) = pct_line.trim().trim_end_matches('%').parse::<u32>() {
                if pct >= 95 {
                    findings.push(DiagnosticFinding {
                        id: "disk-critical".into(),
                        category: "resources".into(),
                        severity: "critical".into(),
                        title: format!("Disk usage critical: {pct}%"),
                        description: "Root filesystem is almost full. Services may fail.".into(),
                        fix_available: true,
                        fix_id: Some("clean-logs".into()),
                    });
                } else if pct >= 85 {
                    findings.push(DiagnosticFinding {
                        id: "disk-warning".into(),
                        category: "resources".into(),
                        severity: "warning".into(),
                        title: format!("Disk usage high: {pct}%"),
                        description: "Root filesystem is getting full. Consider cleanup.".into(),
                        fix_available: true,
                        fix_id: Some("clean-logs".into()),
                    });
                }
            }
        }
    }

    // Memory usage
    if let Ok(Ok(output)) = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        Command::new("free").args(["-m"]).output(),
    )
    .await
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Some(mem_line) = stdout.lines().find(|l| l.starts_with("Mem:")) {
            let parts: Vec<&str> = mem_line.split_whitespace().collect();
            if parts.len() >= 3 {
                let total: f64 = parts[1].parse().unwrap_or(1.0);
                let used: f64 = parts[2].parse().unwrap_or(0.0);
                let pct = (used / total * 100.0) as u32;
                if pct >= 95 {
                    findings.push(DiagnosticFinding {
                        id: "memory-critical".into(),
                        category: "resources".into(),
                        severity: "critical".into(),
                        title: format!("Memory usage critical: {pct}%"),
                        description: format!("{used:.0} MB used of {total:.0} MB. OOM killer may activate."),
                        fix_available: false,
                        fix_id: None,
                    });
                } else if pct >= 85 {
                    findings.push(DiagnosticFinding {
                        id: "memory-warning".into(),
                        category: "resources".into(),
                        severity: "warning".into(),
                        title: format!("Memory usage high: {pct}%"),
                        description: format!("{used:.0} MB used of {total:.0} MB"),
                        fix_available: false,
                        fix_id: None,
                    });
                }
            }
        }
    }

    // Swap usage
    if let Ok(Ok(output)) = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        Command::new("free").args(["-m"]).output(),
    )
    .await
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Some(swap_line) = stdout.lines().find(|l| l.starts_with("Swap:")) {
            let parts: Vec<&str> = swap_line.split_whitespace().collect();
            if parts.len() >= 2 {
                let total: f64 = parts[1].parse().unwrap_or(0.0);
                if total == 0.0 {
                    findings.push(DiagnosticFinding {
                        id: "no-swap".into(),
                        category: "resources".into(),
                        severity: "info".into(),
                        title: "No swap configured".into(),
                        description: "System has no swap space. Low-memory situations may cause OOM kills.".into(),
                        fix_available: false,
                        fix_id: None,
                    });
                }
            }
        }
    }

    // Load average vs CPU count
    if let Ok(loadavg) = tokio::fs::read_to_string("/proc/loadavg").await {
        let parts: Vec<&str> = loadavg.split_whitespace().collect();
        if let (Some(load_str), Ok(cpus)) = (parts.first(), num_cpus()) {
            if let Ok(load) = load_str.parse::<f64>() {
                if load > cpus as f64 * 2.0 {
                    findings.push(DiagnosticFinding {
                        id: "load-critical".into(),
                        category: "resources".into(),
                        severity: "critical".into(),
                        title: format!("System load very high: {load:.1} (on {cpus} CPUs)"),
                        description: "Load average is more than 2x the CPU count.".into(),
                        fix_available: false,
                        fix_id: None,
                    });
                } else if load > cpus as f64 {
                    findings.push(DiagnosticFinding {
                        id: "load-warning".into(),
                        category: "resources".into(),
                        severity: "warning".into(),
                        title: format!("System load elevated: {load:.1} (on {cpus} CPUs)"),
                        description: "Load average exceeds CPU count.".into(),
                        fix_available: false,
                        fix_id: None,
                    });
                }
            }
        }
    }

    findings
}

fn num_cpus() -> Result<usize, String> {
    std::fs::read_to_string("/proc/cpuinfo")
        .map_err(|e| e.to_string())
        .map(|content| content.matches("processor\t:").count().max(1))
}

// ── Service Health Checks ────────────────────────────────────────────────

async fn check_services() -> Vec<DiagnosticFinding> {
    let mut findings = Vec::new();

    let critical_services = [
        ("nginx", "Web server"),
        ("docker", "Docker engine"),
    ];

    for (service, label) in &critical_services {
        let active = match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            Command::new("systemctl")
                .args(["is-active", "--quiet", service])
                .status(),
        )
        .await
        {
            Ok(Ok(status)) => status.success(),
            _ => false,
        };

        if !active {
            findings.push(DiagnosticFinding {
                id: format!("service-down-{service}"),
                category: "services".into(),
                severity: "critical".into(),
                title: format!("{label} is not running"),
                description: format!("systemctl reports {service} is not active"),
                fix_available: true,
                fix_id: Some(format!("restart-service:{service}")),
            });
        }
    }

    // Check optional services
    let optional_services = [
        ("fail2ban", "Intrusion prevention"),
        ("ufw", "Firewall"),
    ];

    for (service, label) in &optional_services {
        let active = match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            Command::new("systemctl")
                .args(["is-active", "--quiet", service])
                .status(),
        )
        .await
        {
            Ok(Ok(status)) => status.success(),
            _ => false,
        };

        if !active {
            findings.push(DiagnosticFinding {
                id: format!("service-inactive-{service}"),
                category: "services".into(),
                severity: "info".into(),
                title: format!("{label} ({service}) is not active"),
                description: format!("{service} is not running — recommended for security"),
                fix_available: true,
                fix_id: Some(format!("restart-service:{service}")),
            });
        }
    }

    findings
}

// ── SSL Expiry Checks ────────────────────────────────────────────────────

async fn check_ssl_expiry() -> Vec<DiagnosticFinding> {
    let mut findings = Vec::new();
    let ssl_dir = "/etc/dockpanel/ssl";

    let mut entries = match tokio::fs::read_dir(ssl_dir).await {
        Ok(e) => e,
        Err(_) => return findings,
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        if !entry.file_type().await.map(|ft| ft.is_dir()).unwrap_or(false) {
            continue;
        }

        let domain = entry.file_name().to_string_lossy().to_string();
        let cert_path = format!("{ssl_dir}/{domain}/fullchain.pem");

        // Check if cert expires within 7 days
        let check_7d = match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            Command::new("openssl")
                .args(["x509", "-checkend", "604800", "-noout", "-in", &cert_path])
                .output(),
        )
        .await
        {
            Ok(Ok(o)) => o,
            _ => continue,
        };

        if !check_7d.status.success() {
            // Get expiry date for the message
            let expiry = get_cert_expiry(&cert_path).await;
            findings.push(DiagnosticFinding {
                id: format!("ssl-expiring-{domain}"),
                category: "ssl".into(),
                severity: "critical".into(),
                title: format!("SSL certificate expiring soon: {domain}"),
                description: format!("Certificate expires {}", expiry.as_deref().unwrap_or("within 7 days")),
                fix_available: true,
                fix_id: Some(format!("renew-ssl:{domain}")),
            });
            continue;
        }

        // Check if cert expires within 30 days
        let check_30d = match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            Command::new("openssl")
                .args(["x509", "-checkend", "2592000", "-noout", "-in", &cert_path])
                .output(),
        )
        .await
        {
            Ok(Ok(o)) => o,
            _ => continue,
        };

        if !check_30d.status.success() {
            let expiry = get_cert_expiry(&cert_path).await;
            findings.push(DiagnosticFinding {
                id: format!("ssl-expiring-{domain}"),
                category: "ssl".into(),
                severity: "warning".into(),
                title: format!("SSL certificate expiring: {domain}"),
                description: format!("Certificate expires {}", expiry.as_deref().unwrap_or("within 30 days")),
                fix_available: true,
                fix_id: Some(format!("renew-ssl:{domain}")),
            });
        }
    }

    findings
}

async fn get_cert_expiry(cert_path: &str) -> Option<String> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        Command::new("openssl")
            .args(["x509", "-enddate", "-noout", "-in", cert_path])
            .output(),
    )
    .await
    .ok()?
    .ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .trim()
        .strip_prefix("notAfter=")
        .map(|s| s.to_string())
}

// ── Log Pattern Analysis ─────────────────────────────────────────────────

async fn check_log_patterns() -> Vec<DiagnosticFinding> {
    let mut findings = Vec::new();

    // Check nginx error log for repeated errors (last 200 lines)
    if let Ok(Ok(output)) = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        Command::new("tail")
            .args(["-n", "200", "/var/log/nginx/error.log"])
            .output(),
    )
    .await
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().collect();

        // Count 502 Bad Gateway errors
        let bad_gateway = lines.iter().filter(|l| l.contains("connect() failed") || l.contains("upstream prematurely closed")).count();
        if bad_gateway >= 5 {
            findings.push(DiagnosticFinding {
                id: "log-502-errors".into(),
                category: "logs".into(),
                severity: "warning".into(),
                title: format!("{bad_gateway} upstream connection failures in recent logs"),
                description: "Nginx is having trouble connecting to upstream services (PHP-FPM, proxy targets).".into(),
                fix_available: false,
                fix_id: None,
            });
        }

        // Count permission denied errors
        let perm_denied = lines.iter().filter(|l| l.contains("Permission denied") || l.contains("forbidden")).count();
        if perm_denied >= 3 {
            findings.push(DiagnosticFinding {
                id: "log-permission-errors".into(),
                category: "logs".into(),
                severity: "warning".into(),
                title: format!("{perm_denied} permission errors in nginx logs"),
                description: "Check file permissions on site document roots.".into(),
                fix_available: false,
                fix_id: None,
            });
        }
    }

    // Check auth.log for brute force patterns (last 500 lines)
    if let Ok(Ok(output)) = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        Command::new("tail")
            .args(["-n", "500", "/var/log/auth.log"])
            .output(),
    )
    .await
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let failed_ssh = stdout
            .lines()
            .filter(|l| l.contains("Failed password") || l.contains("Invalid user"))
            .count();

        if failed_ssh >= 50 {
            findings.push(DiagnosticFinding {
                id: "log-ssh-bruteforce".into(),
                category: "logs".into(),
                severity: "critical".into(),
                title: format!("{failed_ssh} failed SSH login attempts in recent logs"),
                description: "Possible brute force attack. Ensure fail2ban is active.".into(),
                fix_available: true,
                fix_id: Some("restart-service:fail2ban".into()),
            });
        } else if failed_ssh >= 10 {
            findings.push(DiagnosticFinding {
                id: "log-ssh-failures".into(),
                category: "logs".into(),
                severity: "warning".into(),
                title: format!("{failed_ssh} failed SSH login attempts in recent logs"),
                description: "Monitor for brute force patterns.".into(),
                fix_available: false,
                fix_id: None,
            });
        }
    }

    // Check syslog for OOM killer activity
    if let Ok(Ok(output)) = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        Command::new("tail")
            .args(["-n", "500", "/var/log/syslog"])
            .output(),
    )
    .await
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let oom_count = stdout.lines().filter(|l| l.contains("oom-kill") || l.contains("Out of memory")).count();
        if oom_count > 0 {
            findings.push(DiagnosticFinding {
                id: "log-oom-kills".into(),
                category: "logs".into(),
                severity: "critical".into(),
                title: format!("{oom_count} OOM killer event(s) in recent syslog"),
                description: "The system ran out of memory and killed processes. Consider adding swap or upgrading RAM.".into(),
                fix_available: false,
                fix_id: None,
            });
        }
    }

    findings
}

// ── Security Checks ──────────────────────────────────────────────────────

async fn check_security() -> Vec<DiagnosticFinding> {
    let mut findings = Vec::new();

    // SSH password authentication enabled?
    if let Ok(content) = tokio::fs::read_to_string("/etc/ssh/sshd_config").await {
        let password_auth = content
            .lines()
            .filter(|l| !l.trim().starts_with('#'))
            .find(|l| l.trim().starts_with("PasswordAuthentication"))
            .map(|l| l.split_whitespace().nth(1).unwrap_or("yes"))
            .unwrap_or("yes");

        if password_auth.eq_ignore_ascii_case("yes") {
            findings.push(DiagnosticFinding {
                id: "ssh-password-auth".into(),
                category: "security".into(),
                severity: "warning".into(),
                title: "SSH password authentication is enabled".into(),
                description: "Key-based authentication is more secure. Consider disabling PasswordAuthentication.".into(),
                fix_available: false,
                fix_id: None,
            });
        }

        // SSH on default port 22?
        let port = content
            .lines()
            .filter(|l| !l.trim().starts_with('#'))
            .find(|l| l.trim().starts_with("Port "))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(22);

        if port == 22 {
            findings.push(DiagnosticFinding {
                id: "ssh-default-port".into(),
                category: "security".into(),
                severity: "info".into(),
                title: "SSH running on default port 22".into(),
                description: "Changing SSH port reduces automated scanning noise (not a security fix, but reduces log spam).".into(),
                fix_available: false,
                fix_id: None,
            });
        }
    }

    // Firewall active?
    if let Ok(Ok(output)) = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        Command::new("ufw").args(["status"]).output(),
    )
    .await
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if !stdout.contains("Status: active") {
            findings.push(DiagnosticFinding {
                id: "firewall-inactive".into(),
                category: "security".into(),
                severity: "warning".into(),
                title: "Firewall (ufw) is not active".into(),
                description: "All ports are exposed to the internet without a firewall.".into(),
                fix_available: false,
                fix_id: None,
            });
        }
    }

    // Unattended upgrades?
    let unattended = tokio::fs::metadata("/etc/apt/apt.conf.d/20auto-upgrades")
        .await
        .is_ok();
    if !unattended {
        findings.push(DiagnosticFinding {
            id: "no-auto-updates".into(),
            category: "security".into(),
            severity: "info".into(),
            title: "Automatic security updates not configured".into(),
            description: "Consider enabling unattended-upgrades for automatic security patches.".into(),
            fix_available: false,
            fix_id: None,
        });
    }

    findings
}

// ── One-Click Fixes ──────────────────────────────────────────────────────

/// Apply a one-click fix by ID.
pub async fn apply_fix(fix_id: &str) -> Result<String, String> {
    let parts: Vec<&str> = fix_id.splitn(2, ':').collect();
    let action = parts[0];
    let target = parts.get(1).copied().unwrap_or("");

    match action {
        "restart-service" => {
            if target.is_empty() {
                return Err("No service specified".into());
            }
            // Validate service name
            if !target.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                return Err("Invalid service name".into());
            }
            let output = tokio::time::timeout(
                std::time::Duration::from_secs(30),
                Command::new("systemctl")
                    .args(["restart", target])
                    .output(),
            )
            .await
            .map_err(|_| "Restart timed out")?
            .map_err(|e| format!("Failed to run systemctl: {e}"))?;

            if output.status.success() {
                Ok(format!("Service {target} restarted successfully"))
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(format!("Failed to restart {target}: {stderr}"))
            }
        }
        "create-root" => {
            if target.is_empty() {
                return Err("No path specified".into());
            }
            // Validate path starts with expected prefix
            if !target.starts_with("/var/www/") && !target.starts_with("/etc/dockpanel/sites/") {
                return Err("Path must be under /var/www/ or /etc/dockpanel/sites/".into());
            }
            tokio::fs::create_dir_all(target)
                .await
                .map_err(|e| format!("Failed to create directory: {e}"))?;

            // Create a default index.html
            let index = format!("{target}/index.html");
            if !tokio::fs::metadata(&index).await.is_ok() {
                tokio::fs::write(&index, "<h1>Site coming soon</h1>\n")
                    .await
                    .ok();
            }

            // Set ownership to www-data
            let _ = Command::new("chown")
                .args(["-R", "www-data:www-data", target])
                .output()
                .await;

            Ok(format!("Created document root: {target}"))
        }
        "clean-logs" => {
            // Truncate large log files to free disk
            let log_files = [
                "/var/log/nginx/access.log",
                "/var/log/nginx/error.log",
                "/var/log/syslog",
            ];
            let mut freed = 0u64;
            for log in &log_files {
                if let Ok(meta) = tokio::fs::metadata(log).await {
                    if meta.len() > 100 * 1024 * 1024 {
                        // > 100MB
                        freed += meta.len();
                        tokio::fs::write(log, "").await.ok();
                    }
                }
            }

            // Also clean up old journal entries
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                Command::new("journalctl")
                    .args(["--vacuum-size=100M"])
                    .output(),
            )
            .await;

            if freed > 0 {
                Ok(format!(
                    "Cleaned {:.0} MB of log files",
                    freed as f64 / 1024.0 / 1024.0
                ))
            } else {
                Ok("No oversized log files found. Ran journal vacuum.".into())
            }
        }
        _ => Err(format!("Unknown fix action: {action}")),
    }
}
