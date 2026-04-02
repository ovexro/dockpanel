use axum::{routing::{get, post}, Json, Router};
use axum::body::Body;
use axum::response::Response;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_stream::StreamExt;
use std::process::Stdio;
use crate::safe_cmd::safe_command;

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
struct UpdateCount {
    count: usize,
    security: usize,
    reboot_required: bool,
}

#[derive(Serialize)]
struct RebootResult {
    success: bool,
    message: String,
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
        safe_command("apt-get")
            .args(["update", "-qq"])
            .env("DEBIAN_FRONTEND", "noninteractive")
            .output(),
    )
    .await;

    // Get upgradable list
    let output = tokio::time::timeout(
        Duration::from_secs(60),
        safe_command("apt")
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

/// POST /system/updates/apply — apply package updates with streaming NDJSON output.
///
/// Returns newline-delimited JSON: each line is `{"type":"line","line":"..."}` for output,
/// and the final line is `{"type":"done","success":bool,"reboot_required":bool}`.
async fn apply_updates(Json(body): Json<ApplyRequest>) -> Response {
    let has_packages = body
        .packages
        .as_ref()
        .is_some_and(|p| !p.is_empty());

    // Validate package names up-front
    if has_packages {
        for pkg in body.packages.as_ref().unwrap() {
            if pkg.is_empty()
                || !pkg
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '+' || c == ':')
            {
                let error_line = serde_json::json!({"type":"line","line":format!("Invalid package name: {pkg}")});
                let done_line = serde_json::json!({"type":"done","success":false,"reboot_required":false});
                let body_str = format!("{}\n{}\n", error_line, done_line);
                return Response::builder()
                    .header("content-type", "application/x-ndjson")
                    .body(Body::from(body_str))
                    .unwrap();
            }
        }
    }

    let (tx, rx) = tokio::sync::mpsc::channel::<String>(128);

    tokio::spawn(async move {
        let mut cmd = safe_command("apt-get");
        cmd.env("DEBIAN_FRONTEND", "noninteractive");

        if has_packages {
            let packages = body.packages.unwrap();
            cmd.arg("install").arg("-y");
            for pkg in &packages {
                cmd.arg(pkg);
            }
        } else {
            cmd.args(["upgrade", "-y"]);
        }

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(format!("{}\n", serde_json::json!({"type":"line","line":format!("Failed to start apt: {e}")}))).await;
                let _ = tx.send(format!("{}\n", serde_json::json!({"type":"done","success":false,"reboot_required":false}))).await;
                return;
            }
        };

        let stderr = child.stderr.take().unwrap();
        let stdout = child.stdout.take().unwrap();

        // Read stderr in a separate task and send lines through the same channel
        let tx_err = tx.clone();
        let stderr_task = tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.is_empty() { continue; }
                let msg = serde_json::json!({"type":"line","line":line});
                if tx_err.send(format!("{msg}\n")).await.is_err() { break; }
            }
        });

        // Read stdout line-by-line and stream immediately
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if line.is_empty() { continue; }
            let msg = serde_json::json!({"type":"line","line":line});
            if tx.send(format!("{msg}\n")).await.is_err() { break; }
        }

        // Wait for stderr reader to finish
        let _ = stderr_task.await;

        // Wait for process to exit (with timeout)
        let success = match tokio::time::timeout(Duration::from_secs(10), child.wait()).await {
            Ok(Ok(status)) => status.success(),
            _ => false,
        };

        let reboot_required = tokio::fs::metadata("/var/run/reboot-required").await.is_ok();

        let done = serde_json::json!({
            "type": "done",
            "success": success,
            "reboot_required": reboot_required,
        });
        let _ = tx.send(format!("{done}\n")).await;
    });

    let stream = tokio_stream::wrappers::ReceiverStream::new(rx)
        .map(|line| Ok::<_, std::convert::Infallible>(line));

    Response::builder()
        .header("content-type", "application/x-ndjson")
        .body(Body::from_stream(stream))
        .unwrap()
}

/// GET /system/updates/count — quick count of available updates (no apt update).
async fn update_count() -> Json<UpdateCount> {
    let output = safe_command("apt")
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

    let reboot_required =
        tokio::fs::metadata("/var/run/reboot-required").await.is_ok();

    Json(UpdateCount { count, security, reboot_required })
}

/// POST /system/reboot — schedule a system reboot in 1 minute.
async fn system_reboot() -> Json<RebootResult> {
    let result = safe_command("shutdown")
        .args(["-r", "+1", "DockPanel initiated reboot"])
        .output()
        .await;

    match result {
        Ok(output) if output.status.success() => Json(RebootResult {
            success: true,
            message: "System will reboot in 1 minute".to_string(),
        }),
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            Json(RebootResult {
                success: false,
                message: format!("Reboot command failed: {stderr}"),
            })
        }
        Err(e) => Json(RebootResult {
            success: false,
            message: format!("Failed to execute shutdown: {e}"),
        }),
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/system/updates", get(list_updates))
        .route("/system/updates/apply", post(apply_updates))
        .route("/system/updates/count", get(update_count))
        .route("/system/reboot", post(system_reboot))
}
