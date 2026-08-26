use serde::Serialize;
use sha2::Digest;
use std::collections::HashMap;
use crate::safe_cmd::safe_command;

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

/// A hardcoded-secret detection pattern for [`scan_secrets`]. `pattern` is a POSIX ERE
/// (matched by `grep -E`, whose DFA engine has no backtracking — so no ReDoS risk even on
/// adversarial input); `ci` runs it case-insensitively. The matched secret VALUE is
/// deliberately never copied into a Finding — only the file, line, and label are recorded —
/// so a scan result cannot itself leak (or, once backed up, archive) a live credential.
struct SecretPattern {
    pattern: &'static str,
    label: &'static str,
    severity: &'static str,
    ci: bool,
}

/// High-precision hardcoded-credential signatures. Curated for low false-positive rate:
/// vendor-prefixed key formats plus a labeled `key = "value"` heuristic that only fires on
/// a quoted literal from a secret-shaped alphabet (so `password = getenv(...)` / `"${VAR}"`
/// interpolations don't match).
const SECRET_PATTERNS: &[SecretPattern] = &[
    SecretPattern { pattern: r#"-----BEGIN [A-Z ]*PRIVATE KEY"#, label: "private key (PEM)", severity: "critical", ci: false },
    SecretPattern { pattern: r#"AKIA[0-9A-Z]{16}"#, label: "AWS access key ID", severity: "critical", ci: false },
    SecretPattern { pattern: r#"AIza[0-9A-Za-z_-]{35}"#, label: "Google API key", severity: "critical", ci: false },
    SecretPattern { pattern: r#"sk_live_[0-9a-zA-Z]{24,}"#, label: "Stripe live secret key", severity: "critical", ci: false },
    SecretPattern { pattern: r#"gh[pousr]_[0-9A-Za-z]{36,}"#, label: "GitHub token", severity: "critical", ci: false },
    SecretPattern { pattern: r#"SG\.[A-Za-z0-9_-]{22}\.[A-Za-z0-9_-]{43}"#, label: "SendGrid API key", severity: "critical", ci: false },
    SecretPattern { pattern: r#"xox[baprs]-[0-9A-Za-z-]{10,}"#, label: "Slack token", severity: "warning", ci: false },
    SecretPattern { pattern: r#"eyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}"#, label: "hardcoded JWT", severity: "warning", ci: false },
    SecretPattern { pattern: r#"(mysql|postgres|postgresql|mongodb|redis|amqp)://[^:@/[:space:]]*:[^@/[:space:]]+@"#, label: "database URI with embedded password", severity: "warning", ci: true },
    SecretPattern { pattern: r#"(api[_-]?key|api[_-]?secret|secret[_-]?key|access[_-]?token|auth[_-]?token|client[_-]?secret|passwd|password)['"]?[[:space:]]*(=>?|:)[[:space:]]*['"][A-Za-z0-9_./+=-]{8,}['"]"#, label: "hardcoded credential assignment", severity: "warning", ci: true },
];

/// File types scanned for secrets — source + config where credentials get hardcoded.
/// `.env*` is deliberately NOT included: an env file is the CORRECT home for a secret (and
/// DockPanel's nginx blocks dotfiles from being served), so flagging one would be pure noise.
const SECRET_INCLUDE_GLOBS: &[&str] = &[
    "*.php", "*.js", "*.jsx", "*.ts", "*.tsx", "*.mjs", "*.cjs", "*.py", "*.rb",
    "*.go", "*.java", "*.cs", "*.yml", "*.yaml", "*.json", "*.xml", "*.ini",
    "*.conf", "*.properties", "*.sh", "*.inc", "*.pem", "*.key",
];

/// Directories skipped when scanning for secrets — third-party code (not the tenant's own
/// secrets, and huge) plus VCS/build/cache noise.
const SECRET_EXCLUDE_DIRS: &[&str] = &[
    "node_modules", "vendor", ".git", ".svn", "bower_components",
    "cache", ".cache", "dist", "build",
];

/// Individual files skipped — machine-generated lockfiles and minified bundles: large and
/// only ever a source of false positives.
const SECRET_EXCLUDE_FILES: &[&str] = &[
    "package-lock.json", "yarn.lock", "composer.lock", "*.min.js", "*.min.css",
];

/// Run a full security scan: file integrity, malware, ports, SSL, container vulns, security
/// headers, and hardcoded webroot secrets.
pub async fn run_full_scan() -> ScanResult {
    let (integrity, malware, ports, ssl, container_vulns, headers, secrets) = tokio::join!(
        scan_file_integrity(),
        scan_malware(),
        scan_open_ports(),
        scan_ssl_expiry(),
        scan_container_vulnerabilities(),
        scan_security_headers(),
        scan_secrets(),
    );

    let mut findings = Vec::new();
    findings.extend(malware);
    findings.extend(ports);
    findings.extend(ssl);
    findings.extend(container_vulns);
    findings.extend(headers);
    findings.extend(secrets);

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
            safe_command("find")
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
                safe_command("grep")
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

    // Expected ports on a web/panel server. This list means "ports DockPanel
    // itself puts here", so an entry outlives its listener as a blind spot:
    // 9090 was dropped in s305 with the loopback command-forwarding listener
    // that bound it. Anything answering on 9090 now is genuinely not ours and
    // gets the warning it should. (Upgrade impact: a box running something else
    // there — Prometheus and Cockpit both default to 9090 — will see one new
    // "Unexpected open port" finding. That is the scanner working, not a false
    // positive.)
    let expected_ports: &[u16] = &[22, 25, 80, 443, 993, 995, 3080, 5432, 5450];

    let output = match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        safe_command("ss").args(["-tlnp"]).output(),
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
            safe_command("openssl")
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
            safe_command("openssl")
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
                safe_command("openssl")
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
                remediation: Some("Renew the SSL certificate from the panel".into()),
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
            safe_command("grype")
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
            safe_command("docker")
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

/// Scan every site's webroot for hardcoded secrets (API keys, tokens, private keys,
/// credentials) that belong in an environment variable instead. Findings surface in the
/// existing Security tab; the offending secret VALUE is never recorded (see [`SecretPattern`]).
async fn scan_secrets() -> Vec<Finding> {
    scan_secrets_in(&["/var/www"]).await
}

/// Core of [`scan_secrets`], parameterized on the roots so it is unit-testable against a
/// fixture directory. Runs one bounded `grep` per pattern; `-r` (not `-R`) means grep does
/// NOT descend into symlinked directories, so a tenant-planted symlink cannot redirect the
/// scan outside its webroot (cf. the s247 sandbox-escape class).
async fn scan_secrets_in(roots: &[&str]) -> Vec<Finding> {
    let mut findings = Vec::new();

    for root in roots {
        if !std::path::Path::new(root).is_dir() {
            continue;
        }

        for pat in SECRET_PATTERNS {
            let mut cmd = safe_command("grep");
            // `-m 1`: stop after the FIRST match per file — one match is enough to flag the
            // file, and it caps grep's buffered stdout so a tenant-controlled file with millions
            // of matching lines can't balloon the root agent's memory (matches `scan_malware`'s
            // bounded `-l` profile). `kill_on_drop`: kill a timed-out grep when the future is
            // dropped, since `safe_command` sets none (Lesson #76 — no orphaned child on timeout).
            cmd.args(["-rnE", "-m", "1"]);
            cmd.kill_on_drop(true);
            if pat.ci {
                cmd.arg("-i");
            }
            for inc in SECRET_INCLUDE_GLOBS {
                cmd.arg(format!("--include={inc}"));
            }
            for exc in SECRET_EXCLUDE_DIRS {
                cmd.arg(format!("--exclude-dir={exc}"));
            }
            for exf in SECRET_EXCLUDE_FILES {
                cmd.arg(format!("--exclude={exf}"));
            }
            // `-e` marks the pattern explicitly so a leading `-` (the PEM header) is never
            // mistaken for a grep flag.
            cmd.arg("-e").arg(pat.pattern).arg(root);

            let output = match tokio::time::timeout(
                std::time::Duration::from_secs(30),
                cmd.output(),
            )
            .await
            {
                Ok(Ok(o)) => o,
                _ => continue, // timed out, grep missing, or spawn failure — skip this pattern
            };

            // grep exits 1 on "no matches" (empty stdout) and 2 on error; both yield nothing
            // to parse. We read only stdout and never surface the matched line's content.
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines().take(50) {
                // Cap at 50 files/pattern to bound a pathological webroot. With `-m 1` each line
                // is a distinct file, so on a multi-tenant box this covers more sites before the
                // budget is exhausted (the crown-jewel vendor patterns each have their own cap).
                if let Some((file, lineno)) = parse_grep_match(line) {
                    findings.push(Finding {
                        check_type: "secret_leak".into(),
                        severity: pat.severity.into(),
                        title: format!("Exposed secret: {}", pat.label),
                        description: format!(
                            "A {} was found in {file} at line {lineno}. \
                             The secret value itself is intentionally not recorded by the scanner.",
                            pat.label
                        ),
                        file_path: Some(file),
                        remediation: Some(
                            "Move the secret out of the webroot into an environment variable \
                             (a .env file is not served by DockPanel's nginx) and rotate the \
                             exposed credential."
                                .into(),
                        ),
                    });
                }
            }
        }
    }

    findings
}

/// Parse one `grep -rn` output line (`<path>:<lineno>:<content>`) into `(path, lineno)`,
/// DISCARDING the content so a matched secret value can never propagate into a Finding.
/// Splits at the first `:<digits>:` separator; returns None if the line has none. Kept
/// dependency-free (no regex crate in the agent) and pure so it is unit-testable.
fn parse_grep_match(line: &str) -> Option<(String, u32)> {
    let mut search_from = 0;
    while let Some(rel) = line[search_from..].find(':') {
        let colon = search_from + rel;
        let after = &line[colon + 1..];
        let digit_len = after.chars().take_while(|c| c.is_ascii_digit()).count();
        if digit_len > 0 && after[digit_len..].starts_with(':') {
            // This IS grep's `PATH:LINENO:` separator (the leftmost `:<digits>:` boundary is
            // always at or before the true separator, so `line[..colon]` is always a prefix of
            // the path — never CONTENT). COMMIT to it and stop: never continue scanning past a
            // real separator, or a `:<digits>:` inside CONTENT could pull the matched secret
            // into the returned path. An overflowing line number saturates rather than making
            // the parse "fail" and skip forward (the redaction-leak fix).
            let lineno = after[..digit_len].parse::<u32>().unwrap_or(u32::MAX);
            return Some((line[..colon].to_string(), lineno));
        }
        search_from = colon + 1;
    }
    None
}

#[cfg(test)]
mod secret_scan_tests {
    use super::*;

    #[test]
    fn parse_grep_match_extracts_path_and_line() {
        assert_eq!(
            parse_grep_match("/var/www/a.php:42:$key = \"AKIA...\";"),
            Some(("/var/www/a.php".to_string(), 42))
        );
        // Content after the line number may itself contain colons — still parsed correctly.
        assert_eq!(
            parse_grep_match("/var/www/b.js:7:const u = \"redis://x:y@h\";"),
            Some(("/var/www/b.js".to_string(), 7))
        );
        // No `:<digits>:` separator anywhere.
        assert_eq!(parse_grep_match("no separator here"), None);
        assert_eq!(parse_grep_match("/path/no/lineno:notdigits:x"), None);
        // REDACTION REGRESSION: an overflowing line number (>u32::MAX, i.e. a >4GB file) must
        // NOT make the parser skip the true separator and walk into CONTENT — that would pull
        // the matched secret into the returned "path". It must saturate and stop.
        let overflow =
            parse_grep_match("/var/www/a.php:4294967296:$k=\"AKIA1234567890ABCDEF\":1:x");
        assert_eq!(overflow, Some(("/var/www/a.php".to_string(), u32::MAX)));
        assert!(
            !overflow.unwrap().0.contains("AKIA"),
            "a secret value must never appear in the parsed path"
        );
        // A colon INSIDE the path is handled — the separator is the first `:<digits>:`.
        assert_eq!(
            parse_grep_match("/var/www/od:d.php:12:secret"),
            Some(("/var/www/od:d.php".to_string(), 12))
        );
    }

    #[tokio::test]
    async fn scan_secrets_flags_and_redacts() {
        // Secret VALUES planted in the fixture — every one must be ABSENT from the findings.
        const AWS_ID: &str = "AKIA1234567890ABCDEF";
        const PW_VAL: &str = "Sup3rSecretValue123";

        let base = format!("/tmp/dockpanel-sectest-{}", uuid::Uuid::new_v4());
        let site = format!("{base}/site");
        let node_modules = format!("{site}/node_modules/pkg");
        std::fs::create_dir_all(&node_modules).unwrap();

        // The one file that should produce findings.
        std::fs::write(
            format!("{site}/app.php"),
            format!(
                "<?php\n\
                 $aws = \"{AWS_ID}\";\n\
                 $config = ['password' => '{PW_VAL}'];\n\
                 $dsn = \"mysql://appuser:{PW_VAL}@db.example.com/app\";\n\
                 // -----BEGIN RSA PRIVATE KEY-----\n"
            ),
        )
        .unwrap();
        // A .env file is the CORRECT home for a secret — must NOT be scanned.
        std::fs::write(format!("{site}/.env"), format!("AWS_KEY={AWS_ID}\n")).unwrap();
        // Third-party code under node_modules — must be skipped by --exclude-dir.
        std::fs::write(format!("{node_modules}/evil.php"), format!("<?php $k=\"{AWS_ID}\";")).unwrap();

        let findings = scan_secrets_in(&[site.as_str()]).await;
        std::fs::remove_dir_all(&base).ok();

        // Every finding is a secret_leak pointing only into app.php.
        assert!(!findings.is_empty(), "expected secret findings, got none");
        assert!(findings.iter().all(|f| f.check_type == "secret_leak"));
        assert!(
            findings.iter().all(|f| f.file_path.as_deref().is_some_and(|p| p.ends_with("app.php"))),
            ".env and node_modules must not be scanned"
        );

        // The four distinct signatures in app.php were all detected.
        let labels: Vec<&str> = findings.iter().map(|f| f.title.as_str()).collect();
        for expect in [
            "AWS access key ID",
            "hardcoded credential assignment",
            "database URI with embedded password",
            "private key (PEM)",
        ] {
            assert!(labels.iter().any(|l| l.contains(expect)), "missing finding: {expect}");
        }

        // REDACTION INVARIANT: no secret value appears anywhere in any finding.
        for f in &findings {
            assert!(!f.title.contains(AWS_ID) && !f.title.contains(PW_VAL));
            assert!(!f.description.contains(AWS_ID) && !f.description.contains(PW_VAL));
            let fp = f.file_path.clone().unwrap_or_default();
            assert!(!fp.contains(AWS_ID) && !fp.contains(PW_VAL));
        }
    }
}
