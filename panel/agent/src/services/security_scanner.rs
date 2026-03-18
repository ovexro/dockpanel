use serde::Serialize;
use sha2::Digest;
use std::collections::HashMap;
use tokio::process::Command;

#[derive(Serialize)]
pub struct ScanResult {
    pub findings: Vec<Finding>,
    pub file_hashes: Vec<FileHash>,
}

#[derive(Serialize, Clone)]
pub struct Finding {
    pub check_type: String,
    pub severity: String,
    pub title: String,
    pub description: String,
    pub file_path: Option<String>,
    pub remediation: Option<String>,
}

#[derive(Serialize)]
pub struct FileHash {
    pub path: String,
    pub hash: String,
    pub size: u64,
}

/// Critical system files to track for integrity changes.
const INTEGRITY_FILES: &[&str] = &[
    "/etc/passwd",
    "/etc/shadow",
    "/etc/sudoers",
    "/etc/ssh/sshd_config",
    "/etc/hosts",
    "/etc/crontab",
    "/etc/nginx/nginx.conf",
];

/// Common malware patterns in PHP files.
const MALWARE_PATTERNS: &[(&str, &str)] = &[
    (r"eval\s*\(\s*base64_decode", "eval(base64_decode()) — obfuscated code execution"),
    (r"eval\s*\(\s*gzinflate", "eval(gzinflate()) — compressed payload execution"),
    (r"eval\s*\(\s*str_rot13", "eval(str_rot13()) — obfuscated code"),
    (r"eval\s*\(\s*\$_(?:GET|POST|REQUEST|COOKIE)", "eval() with user input — remote code execution"),
    (r"preg_replace\s*\(.*/e", "preg_replace with /e modifier — code execution"),
    (r"assert\s*\(\s*\$_", "assert() with user input — code injection"),
    (r"system\s*\(\s*\$_", "system() with user input — command injection"),
    (r"passthru\s*\(\s*\$_", "passthru() with user input — command injection"),
    (r"shell_exec\s*\(\s*\$_", "shell_exec() with user input — command injection"),
    (r"exec\s*\(\s*\$_", "exec() with user input — command injection"),
];

/// Suspicious filenames that indicate web shells.
const SUSPICIOUS_FILES: &[&str] = &[
    "c99.php", "r57.php", "wso.php", "b374k.php", "alfa.php",
    "webshell.php", "shell.php", "cmd.php", "backdoor.php",
    ".htaccess.bak", "wp-config.php.bak",
];

/// Run a full security scan: file integrity, malware, ports, SSL, container vulns, security headers.
pub async fn run_full_scan() -> ScanResult {
    let (integrity, malware, ports, ssl, container_vulns, headers) = tokio::join!(
        scan_file_integrity(),
        scan_malware(),
        scan_open_ports(),
        scan_ssl_expiry(),
        scan_container_vulnerabilities(),
        scan_security_headers(),
    );

    let mut findings = Vec::new();
    findings.extend(malware);
    findings.extend(ports);
    findings.extend(ssl);
    findings.extend(container_vulns);
    findings.extend(headers);

    ScanResult {
        findings,
        file_hashes: integrity,
    }
}

/// Compute SHA-256 hashes of critical system files.
async fn scan_file_integrity() -> Vec<FileHash> {
    let mut hashes = Vec::new();

    for path in INTEGRITY_FILES {
        match tokio::fs::metadata(path).await {
            Ok(meta) => {
                if let Ok(contents) = tokio::fs::read(path).await {
                    let mut hasher = sha2::Sha256::new();
                    hasher.update(&contents);
                    let hash = hex::encode(hasher.finalize());
                    hashes.push(FileHash {
                        path: path.to_string(),
                        hash,
                        size: meta.len(),
                    });
                }
            }
            Err(_) => {} // File doesn't exist, skip
        }
    }

    hashes
}

/// Scan web directories for malware patterns and suspicious files.
async fn scan_malware() -> Vec<Finding> {
    let mut findings = Vec::new();

    // Scan site directories for suspicious filenames
    let web_roots = ["/var/www", "/etc/dockpanel/sites"];
    for root in &web_roots {
        if let Ok(output) = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            Command::new("find")
                .args([root, "-maxdepth", "5", "-type", "f", "-name", "*.php"])
                .output(),
        )
        .await
        {
            if let Ok(out) = output {
                let stdout = String::from_utf8_lossy(&out.stdout);
                for line in stdout.lines() {
                    let filename = line.rsplit('/').next().unwrap_or("");

                    // Check suspicious filenames
                    if SUSPICIOUS_FILES.contains(&filename) {
                        findings.push(Finding {
                            check_type: "malware".into(),
                            severity: "critical".into(),
                            title: format!("Suspicious file: {filename}"),
                            description: format!("Known web shell filename detected at {line}"),
                            file_path: Some(line.to_string()),
                            remediation: Some("Inspect and remove this file immediately".into()),
                        });
                    }
                }
            }
        }
    }

    // Scan for malware patterns with grep
    for root in &web_roots {
        for (pattern, desc) in MALWARE_PATTERNS {
            if let Ok(output) = tokio::time::timeout(
                std::time::Duration::from_secs(30),
                Command::new("grep")
                    .args(["-rlE", pattern, "--include=*.php", root])
                    .output(),
            )
            .await
            {
                if let Ok(out) = output {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    for file in stdout.lines().take(10) {
                        // Limit to 10 matches per pattern
                        if !file.is_empty() {
                            findings.push(Finding {
                                check_type: "malware".into(),
                                severity: "critical".into(),
                                title: desc.to_string(),
                                description: format!("Malware pattern found in {file}"),
                                file_path: Some(file.to_string()),
                                remediation: Some("Review this file for malicious code".into()),
                            });
                        }
                    }
                }
            }
        }
    }

    findings
}

/// Scan open ports and flag unexpected services.
async fn scan_open_ports() -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut seen_ports = std::collections::HashSet::new();

    // Expected ports on a web/panel server
    let expected_ports: &[u16] = &[22, 25, 80, 443, 993, 995, 3080, 5432, 5450, 9090];

    let output = match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        Command::new("ss").args(["-tlnp"]).output(),
    )
    .await
    {
        Ok(Ok(o)) => o,
        _ => return findings,
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 5 {
            continue;
        }

        // Local address is at index 3: *:port, 0.0.0.0:port, [::]:port, 127.0.0.1:port
        let local_addr = parts[3];
        let port_str = local_addr.rsplit(':').next().unwrap_or("");
        let port: u16 = match port_str.parse() {
            Ok(p) => p,
            Err(_) => continue,
        };

        // Skip expected, high (ephemeral), loopback-only, and already-seen ports
        if expected_ports.contains(&port) || port >= 32768 || !seen_ports.insert(port) {
            continue;
        }

        // Skip loopback-only listeners (127.0.0.1 / [::1])
        if local_addr.starts_with("127.") || local_addr.starts_with("[::1]") {
            continue;
        }

        // Skip Docker-managed ports (docker-proxy process)
        let process = parts.last().unwrap_or(&"");
        if process.contains("docker-proxy") || process.contains("containerd") {
            continue;
        }

        findings.push(Finding {
            check_type: "open_port".into(),
            severity: "warning".into(),
            title: format!("Unexpected open port: {port}"),
            description: format!("Port {port} is listening ({process})"),
            file_path: None,
            remediation: Some(format!(
                "If this port is not needed, close it with: ufw deny {port}/tcp"
            )),
        });
    }

    findings
}

/// Check SSL certificates for approaching expiry.
async fn scan_ssl_expiry() -> Vec<Finding> {
    let mut findings = Vec::new();

    let ssl_dir = "/etc/dockpanel/ssl";
    let mut entries = match tokio::fs::read_dir(ssl_dir).await {
        Ok(e) => e,
        Err(_) => return findings,
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        let ft = match entry.file_type().await {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if !ft.is_dir() {
            continue;
        }

        let domain = entry.file_name().to_string_lossy().to_string();
        let cert_path = format!("{ssl_dir}/{domain}/fullchain.pem");

        // Use openssl to check expiry
        let output = match tokio::time::timeout(
            std::time::Duration::from_secs(10),
            Command::new("openssl")
                .args(["x509", "-enddate", "-noout", "-in", &cert_path])
                .output(),
        )
        .await
        {
            Ok(Ok(o)) if o.status.success() => o,
            _ => continue,
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        // Output format: notAfter=Mar 15 12:00:00 2026 GMT
        let date_str = match stdout.trim().strip_prefix("notAfter=") {
            Some(d) => d,
            None => continue,
        };

        // Parse expiry date and check if within 30 days
        let check_output = match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            Command::new("openssl")
                .args([
                    "x509", "-checkend", "2592000", // 30 days in seconds
                    "-noout", "-in", &cert_path,
                ])
                .output(),
        )
        .await
        {
            Ok(Ok(o)) => o,
            _ => continue,
        };

        if !check_output.status.success() {
            // Certificate will expire within 30 days
            let check_7d = match tokio::time::timeout(
                std::time::Duration::from_secs(5),
                Command::new("openssl")
                    .args([
                        "x509", "-checkend", "604800", // 7 days
                        "-noout", "-in", &cert_path,
                    ])
                    .output(),
            )
            .await
            {
                Ok(Ok(o)) => o,
                _ => continue,
            };

            let severity = if !check_7d.status.success() {
                "critical"
            } else {
                "warning"
            };

            findings.push(Finding {
                check_type: "ssl_expiry".into(),
                severity: severity.into(),
                title: format!("SSL certificate expiring: {domain}"),
                description: format!("Certificate expires {date_str}"),
                file_path: Some(cert_path),
                remediation: Some("Renew the SSL certificate via the Sites panel".into()),
            });
        }
    }

    findings
}

/// Scan Docker images for known vulnerabilities.
/// Tries `grype` first (if installed), falls back to `docker scout` (if available).
async fn scan_container_vulnerabilities() -> Vec<Finding> {
    let docker = match bollard::Docker::connect_with_local_defaults() {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };

    // List DockPanel-managed containers
    let containers = docker
        .list_containers(Some(bollard::container::ListContainersOptions::<String> {
            all: false,
            filters: {
                let mut f = HashMap::new();
                f.insert(
                    "label".to_string(),
                    vec!["dockpanel.managed=true".to_string()],
                );
                f
            },
            ..Default::default()
        }))
        .await
        .unwrap_or_default();

    let mut findings = Vec::new();
    let mut scanned_images = std::collections::HashSet::new();

    for container in &containers {
        let image = match &container.image {
            Some(img) => img.clone(),
            None => continue,
        };

        if scanned_images.contains(&image) {
            continue;
        }
        scanned_images.insert(image.clone());

        // Try grype first
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            Command::new("grype")
                .args([&image, "-o", "json", "--only-fixed"])
                .output(),
        )
        .await;

        match result {
            Ok(Ok(output)) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Ok(report) = serde_json::from_str::<serde_json::Value>(&stdout) {
                    if let Some(matches) = report.get("matches").and_then(|m| m.as_array()) {
                        let critical = matches
                            .iter()
                            .filter(|m| {
                                m.get("vulnerability")
                                    .and_then(|v| v.get("severity"))
                                    .and_then(|s| s.as_str())
                                    == Some("Critical")
                            })
                            .count();
                        let high = matches
                            .iter()
                            .filter(|m| {
                                m.get("vulnerability")
                                    .and_then(|v| v.get("severity"))
                                    .and_then(|s| s.as_str())
                                    == Some("High")
                            })
                            .count();
                        let total = matches.len();

                        if critical > 0 || high > 0 {
                            findings.push(Finding {
                                check_type: "container_vuln".to_string(),
                                severity: if critical > 0 {
                                    "critical"
                                } else {
                                    "warning"
                                }
                                .to_string(),
                                title: format!(
                                    "Image {}: {} vulnerabilities ({} critical, {} high)",
                                    image, total, critical, high
                                ),
                                description: format!(
                                    "{total} known vulnerabilities with fixes available"
                                ),
                                file_path: Some(image.clone()),
                                remediation: Some(
                                    "Update the base image to the latest version and rebuild"
                                        .to_string(),
                                ),
                            });
                        }
                    }
                }
                continue; // grype worked, skip docker scout
            }
            _ => {} // grype not available or failed, try docker scout
        }

        // Fallback: docker scout (simpler output)
        let scout_result = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            Command::new("docker")
                .args(["scout", "quickview", &image])
                .output(),
        )
        .await;

        if let Ok(Ok(output)) = scout_result {
            let stdout = String::from_utf8_lossy(&output.stdout);
            // Parse "X critical, Y high, Z medium" from output
            let has_critical = stdout.contains("critical") && !stdout.contains("0C");
            let has_high = stdout.contains("high") && !stdout.contains("0H");

            if has_critical || has_high {
                findings.push(Finding {
                    check_type: "container_vuln".to_string(),
                    severity: if has_critical {
                        "critical"
                    } else {
                        "warning"
                    }
                    .to_string(),
                    title: format!("Image {} has known vulnerabilities", image),
                    description: stdout
                        .lines()
                        .find(|l| l.contains("critical") || l.contains("high"))
                        .unwrap_or("")
                        .trim()
                        .to_string(),
                    file_path: Some(image.clone()),
                    remediation: Some("Update the base image and rebuild".to_string()),
                });
            }
        }
    }

    findings
}

/// Check security headers on nginx-served sites.
async fn scan_security_headers() -> Vec<Finding> {
    let mut findings = Vec::new();

    // Get list of nginx sites
    let sites_dir = "/etc/nginx/sites-enabled";
    let mut entries = match tokio::fs::read_dir(sites_dir).await {
        Ok(e) => e,
        Err(_) => return findings,
    };

    let required_headers: &[(&str, &str)] = &[
        (
            "Strict-Transport-Security",
            "HSTS protects against protocol downgrade attacks",
        ),
        (
            "X-Content-Type-Options",
            "Prevents MIME-type sniffing",
        ),
        (
            "X-Frame-Options",
            "Prevents clickjacking attacks",
        ),
        (
            "Content-Security-Policy",
            "Prevents XSS and data injection attacks",
        ),
    ];

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let filename = path.file_name().and_then(|f| f.to_str()).unwrap_or("");

        // Skip non-site configs
        if !filename.ends_with(".conf") || filename == "dockpanel.dev.conf" {
            continue;
        }
        let domain = filename.trim_end_matches(".conf");

        let content = tokio::fs::read_to_string(&path).await.unwrap_or_default();

        for (header, description) in required_headers {
            if !content.contains(header) {
                findings.push(Finding {
                    check_type: "security_headers".to_string(),
                    severity: "info".to_string(),
                    title: format!("Missing {} header on {}", header, domain),
                    description: description.to_string(),
                    file_path: Some(path.to_string_lossy().to_string()),
                    remediation: Some(format!(
                        "Add 'add_header {} \"...\" always;' to the nginx config",
                        header
                    )),
                });
            }
        }
    }

    findings
}
