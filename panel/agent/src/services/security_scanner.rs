use serde::Serialize;
use sha2::Digest;
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

/// Run a full security scan: file integrity, malware, ports, SSL.
pub async fn run_full_scan() -> ScanResult {
    let (integrity, malware, ports, ssl) = tokio::join!(
        scan_file_integrity(),
        scan_malware(),
        scan_open_ports(),
        scan_ssl_expiry(),
    );

    let mut findings = Vec::new();
    findings.extend(malware);
    findings.extend(ports);
    findings.extend(ssl);

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
