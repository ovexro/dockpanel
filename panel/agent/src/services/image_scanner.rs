// Per-image vulnerability scanner using Anchore's grype.
//
// Surfaces results per Docker image so the panel can badge individual apps
// and gate deploys at a configurable severity threshold. Distinct from the
// full security scan (services::security_scanner) which lumps container vuln
// counts into a single all-server report.

use crate::safe_cmd::safe_command;
use serde::{Deserialize, Serialize};
use std::time::Duration;

// Scanner lives inside the DockPanel data dir so it works under the hardened
// agent sandbox (ProtectSystem=strict, ProtectHome=yes) without needing write
// access to /usr/local/bin or $HOME.
const GRYPE_DIR: &str = "/var/lib/dockpanel/scanners";
const GRYPE_BIN: &str = "/var/lib/dockpanel/scanners/grype";
const GRYPE_DB_CACHE: &str = "/var/lib/dockpanel/scanners/grype-db";

#[derive(Serialize, Clone)]
pub struct ImageScanResult {
    pub image: String,
    pub scanner: String,
    pub critical_count: u32,
    pub high_count: u32,
    pub medium_count: u32,
    pub low_count: u32,
    pub unknown_count: u32,
    pub vulnerabilities: Vec<Vuln>,
    pub scanned_at: String,
}

#[derive(Serialize, Clone)]
pub struct Vuln {
    pub cve: String,
    pub severity: String,
    pub package: String,
    pub installed_version: String,
    pub fixed_version: Option<String>,
    pub description: Option<String>,
}

/// True if the scanner binary is present at the managed path.
pub async fn is_installed() -> bool {
    tokio::fs::metadata(GRYPE_BIN).await.is_ok()
}

/// Install grype via Anchore's official installer script into the DockPanel
/// data directory so the hardened agent sandbox can read/write it.
pub async fn install_grype() -> Result<(), String> {
    if is_installed().await {
        return Ok(());
    }

    tokio::fs::create_dir_all(GRYPE_DIR)
        .await
        .map_err(|e| format!("create {GRYPE_DIR}: {e}"))?;

    // Anchore's installer writes the binary to the path passed via `-b`. We
    // pin it inside /var/lib/dockpanel (writable under systemd ProtectSystem=strict)
    // rather than /usr/local/bin.
    let cmd = format!(
        "curl -sSfL https://raw.githubusercontent.com/anchore/grype/main/install.sh \
         | sh -s -- -b {GRYPE_DIR}"
    );

    let output = tokio::time::timeout(
        Duration::from_secs(180),
        safe_command("sh").args(["-c", &cmd]).output(),
    )
    .await
    .map_err(|_| "grype install timed out after 180s".to_string())?
    .map_err(|e| format!("grype install failed to execute: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "grype install failed: {}",
            stderr.chars().take(300).collect::<String>()
        ));
    }

    // Prime the vulnerability database so the first user-facing scan does
    // not stall on a 200 MB download.
    let _ = tokio::time::timeout(
        Duration::from_secs(180),
        safe_command(GRYPE_BIN)
            .args(["db", "update"])
            .env("GRYPE_DB_CACHE_DIR", GRYPE_DB_CACHE)
            .output(),
    )
    .await;

    Ok(())
}

/// Remove grype binary and its cached vulnerability database.
pub async fn uninstall_grype() -> Result<(), String> {
    let _ = tokio::fs::remove_file(GRYPE_BIN).await;
    let _ = tokio::fs::remove_dir_all(GRYPE_DB_CACHE).await;
    Ok(())
}

/// Scan a single Docker image. Times out at 180s so a hung pull cannot wedge
/// the agent's request handler.
pub async fn scan_image(image: &str) -> Result<ImageScanResult, String> {
    if image.is_empty() || image.len() > 512 {
        return Err("Invalid image reference".to_string());
    }
    // Same character set Docker accepts for image refs. Reject anything that
    // could break out of the argv slot.
    if !image.chars().all(|c| {
        c.is_ascii_alphanumeric() || c == ':' || c == '/' || c == '.' || c == '-' || c == '_' || c == '@'
    }) {
        return Err("Image reference contains disallowed characters".to_string());
    }

    if !is_installed().await {
        return Err("Scanner not installed. Install grype first.".to_string());
    }

    let output = tokio::time::timeout(
        Duration::from_secs(180),
        safe_command(GRYPE_BIN)
            .args([image, "-o", "json"])
            .env("GRYPE_DB_CACHE_DIR", GRYPE_DB_CACHE)
            .output(),
    )
    .await
    .map_err(|_| "Image scan timed out after 180s".to_string())?
    .map_err(|e| format!("grype invocation failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "grype scan failed: {}",
            stderr.chars().take(400).collect::<String>()
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_grype_output(image, &stdout)
}

#[derive(Deserialize)]
struct GrypeReport {
    matches: Option<Vec<GrypeMatch>>,
}

#[derive(Deserialize)]
struct GrypeMatch {
    vulnerability: GrypeVuln,
    artifact: GrypeArtifact,
}

#[derive(Deserialize)]
struct GrypeVuln {
    id: String,
    severity: String,
    description: Option<String>,
    fix: Option<GrypeFix>,
}

#[derive(Deserialize)]
struct GrypeFix {
    versions: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct GrypeArtifact {
    name: String,
    version: String,
}

fn parse_grype_output(image: &str, json_str: &str) -> Result<ImageScanResult, String> {
    let report: GrypeReport = serde_json::from_str(json_str)
        .map_err(|e| format!("Failed to parse grype JSON: {e}"))?;

    let mut critical = 0u32;
    let mut high = 0u32;
    let mut medium = 0u32;
    let mut low = 0u32;
    let mut unknown = 0u32;
    let mut vulnerabilities = Vec::new();

    if let Some(matches) = report.matches {
        for m in matches {
            let severity_normalized = normalize_severity(&m.vulnerability.severity);
            match severity_normalized.as_str() {
                "critical" => critical += 1,
                "high" => high += 1,
                "medium" => medium += 1,
                "low" => low += 1,
                _ => unknown += 1,
            }

            let fixed_version = m
                .vulnerability
                .fix
                .and_then(|f| f.versions)
                .and_then(|v| v.into_iter().next());

            // Cap stored vulns to keep payload manageable; counts are still
            // accurate because we counted before truncating.
            if vulnerabilities.len() < 500 {
                vulnerabilities.push(Vuln {
                    cve: m.vulnerability.id,
                    severity: severity_normalized,
                    package: m.artifact.name,
                    installed_version: m.artifact.version,
                    fixed_version,
                    description: m.vulnerability.description,
                });
            }
        }
    }

    Ok(ImageScanResult {
        image: image.to_string(),
        scanner: "grype".to_string(),
        critical_count: critical,
        high_count: high,
        medium_count: medium,
        low_count: low,
        unknown_count: unknown,
        vulnerabilities,
        scanned_at: chrono::Utc::now().to_rfc3339(),
    })
}

fn normalize_severity(s: &str) -> String {
    match s.to_ascii_lowercase().as_str() {
        "critical" => "critical".to_string(),
        "high" => "high".to_string(),
        "medium" => "medium".to_string(),
        "low" => "low".to_string(),
        "negligible" => "low".to_string(),
        _ => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_grype_severities() {
        let json = r#"{
          "matches": [
            {"vulnerability":{"id":"CVE-1","severity":"Critical","description":"x","fix":{"versions":["1.2.3"]}},
             "artifact":{"name":"libfoo","version":"1.2.0"}},
            {"vulnerability":{"id":"CVE-2","severity":"High","fix":null},
             "artifact":{"name":"libbar","version":"2.0.0"}},
            {"vulnerability":{"id":"CVE-3","severity":"Negligible","fix":null},
             "artifact":{"name":"libbaz","version":"0.1"}}
          ]
        }"#;
        let result = parse_grype_output("nginx:latest", json).unwrap();
        assert_eq!(result.critical_count, 1);
        assert_eq!(result.high_count, 1);
        assert_eq!(result.low_count, 1); // Negligible folds into low
        assert_eq!(result.vulnerabilities.len(), 3);
        assert_eq!(result.vulnerabilities[0].fixed_version.as_deref(), Some("1.2.3"));
    }

    #[test]
    fn rejects_bad_image_refs() {
        // These would be caught at handler boundary; sanity-check the validator.
        let bad = ["", "  ", "img;rm -rf /", "img$(whoami)", "img|cat"];
        for b in bad {
            let allowed = b.chars().all(|c| {
                c.is_ascii_alphanumeric()
                    || c == ':' || c == '/' || c == '.' || c == '-' || c == '_' || c == '@'
            }) && !b.is_empty();
            assert!(!allowed, "should reject: {b}");
        }
        let good = ["nginx:latest", "ghcr.io/owner/repo:1.2.3", "image@sha256:abcd"];
        for g in good {
            let allowed = g.chars().all(|c| {
                c.is_ascii_alphanumeric()
                    || c == ':' || c == '/' || c == '.' || c == '-' || c == '_' || c == '@'
            }) && !g.is_empty();
            assert!(allowed, "should accept: {g}");
        }
    }
}
