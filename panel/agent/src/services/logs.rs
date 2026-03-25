use std::path::Path;
use crate::safe_cmd::safe_command;

/// Resolve a log type string to the corresponding file path.
/// Validates domain names to prevent path traversal.
pub fn resolve_log_path(log_type: &str) -> Result<String, String> {
    // Check for site-specific log types: "nginx_access:{domain}" or "nginx_error:{domain}"
    if let Some(domain) = log_type.strip_prefix("nginx_access:") {
        validate_domain(domain)?;
        return Ok(format!("/var/log/nginx/{domain}.access.log"));
    }
    if let Some(domain) = log_type.strip_prefix("nginx_error:") {
        validate_domain(domain)?;
        return Ok(format!("/var/log/nginx/{domain}.error.log"));
    }

    // Site-specific PHP-FPM error logs
    if let Some(domain) = log_type.strip_prefix("php_error:") {
        validate_domain(domain)?;
        // PHP-FPM site errors land in the nginx error log by default
        return Ok(format!("/var/log/nginx/{domain}.error.log"));
    }

    match log_type {
        "nginx_access" => Ok("/var/log/nginx/access.log".into()),
        "nginx_error" => Ok("/var/log/nginx/error.log".into()),
        "syslog" => Ok("/var/log/syslog".into()),
        "auth" => Ok("/var/log/auth.log".into()),
        "php_fpm" => {
            // Find the active PHP-FPM log
            if let Ok(entries) = std::fs::read_dir("/var/log") {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with("php") && name.ends_with("-fpm.log") {
                        return Ok(format!("/var/log/{name}"));
                    }
                }
            }
            Ok("/var/log/php-fpm.log".into())
        }
        _ => Err(format!("Unknown log type: {log_type}")),
    }
}

/// Validate a domain name — no slashes, no `..`, only alphanumeric, hyphens, dots.
fn validate_domain(domain: &str) -> Result<(), String> {
    if domain.is_empty() {
        return Err("Domain cannot be empty".into());
    }
    if domain.contains('/') || domain.contains('\\') || domain.contains("..") {
        return Err("Invalid domain: path traversal denied".into());
    }
    if !domain
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
    {
        return Err("Invalid domain: only alphanumeric, hyphens, and dots allowed".into());
    }
    Ok(())
}

/// Read the last N lines from a log file, with an optional case-insensitive text filter.
pub async fn read_log(
    log_type: &str,
    lines: usize,
    filter: Option<&str>,
) -> Result<Vec<String>, String> {
    let path = resolve_log_path(log_type)?;

    if !Path::new(&path).exists() {
        return Err(format!("Log file not found: {path}"));
    }

    // Cap lines to a reasonable maximum
    let lines = lines.min(10_000);

    // Use tail to read last N lines
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        safe_command("tail")
            .args(["-n", &lines.to_string(), &path])
            .output(),
    )
    .await
    .map_err(|_| "Log read timed out".to_string())?
    .map_err(|e| format!("Failed to read log: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("tail failed: {stderr}"));
    }

    let content = String::from_utf8_lossy(&output.stdout);

    let result: Vec<String> = if let Some(filter_str) = filter {
        if filter_str.is_empty() {
            content.lines().map(String::from).collect()
        } else {
            let filter_lower = filter_str.to_lowercase();
            content
                .lines()
                .filter(|line| line.to_lowercase().contains(&filter_lower))
                .map(String::from)
                .collect()
        }
    } else {
        content.lines().map(String::from).collect()
    };

    Ok(result)
}

/// Search a log file with grep (supports regex).
/// Returns matching lines, capped at `max_results`.
pub async fn search_log(
    log_type: &str,
    pattern: &str,
    max_results: usize,
) -> Result<Vec<String>, String> {
    // Validate pattern: length limit to prevent ReDoS, reject null bytes
    if pattern.len() > 500 {
        return Err("Pattern too long (max 500 characters)".into());
    }
    if pattern.contains('\0') {
        return Err("Pattern contains invalid characters".into());
    }

    let path = resolve_log_path(log_type)?;

    if !Path::new(&path).exists() {
        return Err(format!("Log file not found: {path}"));
    }

    let max_results = max_results.min(5000);

    // Use grep -iF for case-insensitive fixed-string matching (no regex).
    // Fixed-string matching eliminates ReDoS risk from user-supplied patterns.
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        safe_command("grep")
            .args([
                "-iF",
                pattern,
                &path,
                "-m",
                &max_results.to_string(),
            ])
            .output(),
    )
    .await
    .map_err(|_| "Search timed out".to_string())?
    .map_err(|e| format!("grep failed: {e}"))?;

    // grep returns exit code 1 for "no matches" — not an error
    if !output.status.success() && output.status.code() != Some(1) {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("grep error: {stderr}"));
    }

    let content = String::from_utf8_lossy(&output.stdout);
    Ok(content.lines().map(String::from).collect())
}
