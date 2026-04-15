// Per-image SBOM generator using Anchore's syft.
//
// Produces an SPDX 2.3 JSON document describing every package present in a
// Docker image. Different from the per-image vulnerability scanner (grype):
// SBOMs describe *what is installed*, vuln scans describe *what is vulnerable*.
// Operators want both — the SBOM is the supply-chain artifact, the vuln scan
// is the risk assessment.

use crate::safe_cmd::safe_command;
use std::time::Duration;

// Same hardened-sandbox-compatible install location used by image_scanner.rs.
const SYFT_DIR: &str = "/var/lib/dockpanel/scanners";
const SYFT_BIN: &str = "/var/lib/dockpanel/scanners/syft";

/// True if the syft binary is present at the managed path.
pub async fn is_installed() -> bool {
    tokio::fs::metadata(SYFT_BIN).await.is_ok()
}

/// Install syft via Anchore's official installer script into the DockPanel
/// data directory so the hardened agent sandbox can read/write it.
pub async fn install_syft() -> Result<(), String> {
    if is_installed().await {
        return Ok(());
    }

    tokio::fs::create_dir_all(SYFT_DIR)
        .await
        .map_err(|e| format!("create {SYFT_DIR}: {e}"))?;

    let cmd = format!(
        "curl -sSfL https://raw.githubusercontent.com/anchore/syft/main/install.sh \
         | sh -s -- -b {SYFT_DIR}"
    );

    let output = tokio::time::timeout(
        Duration::from_secs(180),
        safe_command("sh").args(["-c", &cmd]).output(),
    )
    .await
    .map_err(|_| "syft install timed out after 180s".to_string())?
    .map_err(|e| format!("syft install failed to execute: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "syft install failed: {}",
            stderr.chars().take(300).collect::<String>()
        ));
    }

    Ok(())
}

/// Remove syft binary.
pub async fn uninstall_syft() -> Result<(), String> {
    let _ = tokio::fs::remove_file(SYFT_BIN).await;
    Ok(())
}

/// Generate an SPDX 2.3 JSON SBOM for a Docker image. Returns the raw JSON
/// string so the backend can persist and serve it without re-serializing.
/// Times out at 180s so a hung image pull cannot wedge the agent's request
/// handler.
pub async fn generate_sbom(image: &str) -> Result<String, String> {
    if image.is_empty() || image.len() > 512 {
        return Err("Invalid image reference".to_string());
    }
    if !image.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || c == ':' || c == '/' || c == '.' || c == '-' || c == '_' || c == '@'
    }) {
        return Err("Image reference contains disallowed characters".to_string());
    }

    if !is_installed().await {
        return Err("Scanner not installed. Install syft first.".to_string());
    }

    let output = tokio::time::timeout(
        Duration::from_secs(180),
        safe_command(SYFT_BIN)
            .args([image, "-o", "spdx-json", "-q"])
            .output(),
    )
    .await
    .map_err(|_| "SBOM generation timed out after 180s".to_string())?
    .map_err(|e| format!("syft invocation failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "syft failed: {}",
            stderr.chars().take(400).collect::<String>()
        ));
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|e| format!("syft produced non-UTF8 output: {e}"))?;

    // Defensive: confirm syft actually emitted SPDX (top-level field present).
    // Avoids storing a corrupt/empty document that would fail later validation.
    if !stdout.contains("\"spdxVersion\"") {
        return Err("syft output missing SPDX header — unexpected format".to_string());
    }

    Ok(stdout)
}

#[cfg(test)]
mod tests {
    #[test]
    fn rejects_bad_image_refs() {
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
