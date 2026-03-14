use axum::{routing::{get, post}, Json, Router};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::process::Command;

use super::AppState;

#[derive(Serialize)]
struct PackageUpdate {
    name: String,
    current_version: String,
    new_version: String,
    repo: String,
    security: bool,
}

#[derive(Deserialize)]
struct ApplyRequest {
    packages: Option<Vec<String>>,
}

#[derive(Serialize)]
struct ApplyResult {
    success: bool,
    updated: usize,
    output: String,
}

#[derive(Serialize)]
struct UpdateCount {
    count: usize,
    security: usize,
}

/// Parse a single apt upgradable line into a PackageUpdate.
///
/// Format: `package/repo version_new arch [upgradable from: version_old]`
fn parse_upgradable_line(line: &str) -> Option<PackageUpdate> {
    if !line.contains("upgradable from:") {
        return None;
    }

    // Split "package/repo version_new arch [upgradable from: version_old]"
    let slash_pos = line.find('/')?;
    let name = line[..slash_pos].to_string();

    let after_slash = &line[slash_pos + 1..];
    let parts: Vec<&str> = after_slash.split_whitespace().collect();
    // parts: ["repo", "version_new", "arch", "[upgradable", "from:", "version_old]"]
    if parts.len() < 6 {
        return None;
    }

    let repo = parts[0].to_string();
    let new_version = parts[1].to_string();
    // old version is last element, strip trailing ']'
    let current_version = parts[parts.len() - 1].trim_end_matches(']').to_string();
    let security = repo.contains("security");

    Some(PackageUpdate {
        name,
        current_version,
        new_version,
        repo,
        security,
    })
}

/// GET /system/updates — list available package updates.
async fn list_updates() -> Json<Vec<PackageUpdate>> {
    // Run apt update first (suppress output, 60s timeout)
    let _ = tokio::time::timeout(
        Duration::from_secs(60),
        Command::new("apt-get")
            .args(["update", "-qq"])
            .env("DEBIAN_FRONTEND", "noninteractive")
            .output(),
    )
    .await;

    // Get upgradable list
    let output = tokio::time::timeout(
        Duration::from_secs(60),
        Command::new("apt")
            .args(["list", "--upgradable"])
            .stderr(std::process::Stdio::null())
            .output(),
    )
    .await;

    let mut packages = Vec::new();

    if let Ok(Ok(output)) = output {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if let Some(pkg) = parse_upgradable_line(line) {
                packages.push(pkg);
            }
        }
    }

    // Sort: security first, then alphabetically
    packages.sort_by(|a, b| {
        b.security
            .cmp(&a.security)
            .then_with(|| a.name.cmp(&b.name))
    });

    Json(packages)
}

/// POST /system/updates/apply — apply package updates.
async fn apply_updates(Json(body): Json<ApplyRequest>) -> Json<ApplyResult> {
    let has_packages = body
        .packages
        .as_ref()
        .is_some_and(|p| !p.is_empty());

    let result = if has_packages {
        let packages = body.packages.unwrap();
        // Validate package names — only allow alphanumeric, dash, dot, plus, colon
        for pkg in &packages {
            if pkg.is_empty()
                || !pkg
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '+' || c == ':')
            {
                return Json(ApplyResult {
                    success: false,
                    updated: 0,
                    output: format!("Invalid package name: {pkg}"),
                });
            }
        }

        let mut args = vec!["install".to_string(), "-y".to_string()];
        args.extend(packages);

        tokio::time::timeout(
            Duration::from_secs(300),
            Command::new("apt-get")
                .args(&args)
                .env("DEBIAN_FRONTEND", "noninteractive")
                .output(),
        )
        .await
    } else {
        tokio::time::timeout(
            Duration::from_secs(300),
            Command::new("apt-get")
                .args(["upgrade", "-y"])
                .env("DEBIAN_FRONTEND", "noninteractive")
                .output(),
        )
        .await
    };

    match result {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let combined = if stderr.is_empty() {
                stdout
            } else {
                format!("{stdout}\n{stderr}")
            };

            // Count updated packages from apt output
            let updated = combined
                .lines()
                .filter(|l| l.starts_with("Unpacking ") || l.starts_with("Setting up "))
                .filter(|l| l.starts_with("Setting up "))
                .count();

            Json(ApplyResult {
                success: output.status.success(),
                updated,
                output: combined,
            })
        }
        Ok(Err(e)) => Json(ApplyResult {
            success: false,
            updated: 0,
            output: format!("Failed to execute apt: {e}"),
        }),
        Err(_) => Json(ApplyResult {
            success: false,
            updated: 0,
            output: "Command timed out after 300 seconds".to_string(),
        }),
    }
}

/// GET /system/updates/count — quick count of available updates (no apt update).
async fn update_count() -> Json<UpdateCount> {
    let output = Command::new("apt")
        .args(["list", "--upgradable"])
        .stderr(std::process::Stdio::null())
        .output()
        .await;

    let (count, security) = match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut total = 0usize;
            let mut sec = 0usize;
            for line in stdout.lines() {
                if line.contains("upgradable from:") {
                    total += 1;
                    if line.contains("security") {
                        sec += 1;
                    }
                }
            }
            (total, sec)
        }
        Err(_) => (0, 0),
    };

    Json(UpdateCount { count, security })
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/system/updates", get(list_updates))
        .route("/system/updates/apply", post(apply_updates))
        .route("/system/updates/count", get(update_count))
}
