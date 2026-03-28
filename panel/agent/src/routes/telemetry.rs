use axum::{routing::get, Json, Router};
use serde::Serialize;

use super::AppState;
use crate::safe_cmd::safe_command;

#[derive(Serialize)]
pub struct SystemInfo {
    pub os: String,
    pub os_version: String,
    pub kernel: String,
    pub arch: String,
    pub hostname: String,
    pub total_memory_mb: u64,
    pub total_disk_mb: u64,
    pub cpu_count: u32,
    pub uptime_seconds: u64,
    pub dockpanel_agent_version: String,
    pub services: Vec<ServiceStatus>,
}

#[derive(Serialize)]
pub struct ServiceStatus {
    pub name: String,
    pub active: bool,
}

/// GET /telemetry/system-info — Collect system information for telemetry reports.
async fn system_info() -> Json<SystemInfo> {
    let timeout = std::time::Duration::from_secs(5);

    // OS info from /etc/os-release
    let os_release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let os = os_release
        .lines()
        .find(|l| l.starts_with("NAME="))
        .map(|l| l.trim_start_matches("NAME=").trim_matches('"').to_string())
        .unwrap_or_else(|| "Linux".to_string());
    let os_version = os_release
        .lines()
        .find(|l| l.starts_with("VERSION_ID="))
        .map(|l| {
            l.trim_start_matches("VERSION_ID=")
                .trim_matches('"')
                .to_string()
        })
        .unwrap_or_default();

    // Kernel
    let kernel = tokio::time::timeout(timeout, safe_command("uname").arg("-r").output())
        .await
        .ok()
        .and_then(|r| r.ok())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    // Architecture
    let arch = tokio::time::timeout(timeout, safe_command("uname").arg("-m").output())
        .await
        .ok()
        .and_then(|r| r.ok())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    // Hostname (sanitized — no FQDN, just the short hostname)
    let hostname = tokio::time::timeout(timeout, safe_command("hostname").arg("-s").output())
        .await
        .ok()
        .and_then(|r| r.ok())
        .map(|o| {
            let h = String::from_utf8_lossy(&o.stdout).trim().to_string();
            // Return only first 32 chars, no dots (privacy)
            h.split('.').next().unwrap_or("unknown").chars().take(32).collect()
        })
        .unwrap_or_else(|| "unknown".to_string());

    // Memory
    let total_memory_mb = tokio::time::timeout(
        timeout,
        safe_command("free").arg("-m").output(),
    )
    .await
    .ok()
    .and_then(|r| r.ok())
    .and_then(|o| {
        String::from_utf8_lossy(&o.stdout)
            .lines()
            .nth(1)?
            .split_whitespace()
            .nth(1)?
            .parse::<u64>()
            .ok()
    })
    .unwrap_or(0);

    // Disk (root partition)
    let total_disk_mb = tokio::time::timeout(
        timeout,
        safe_command("df")
            .args(["--output=size", "-BM", "/"])
            .output(),
    )
    .await
    .ok()
    .and_then(|r| r.ok())
    .and_then(|o| {
        String::from_utf8_lossy(&o.stdout)
            .lines()
            .nth(1)?
            .trim()
            .trim_end_matches('M')
            .parse::<u64>()
            .ok()
    })
    .unwrap_or(0);

    // CPU count
    let cpu_count = tokio::time::timeout(timeout, safe_command("nproc").output())
        .await
        .ok()
        .and_then(|r| r.ok())
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse::<u32>()
                .ok()
        })
        .unwrap_or(1);

    // Uptime
    let uptime_seconds = std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next()?.parse::<f64>().ok())
        .map(|f| f as u64)
        .unwrap_or(0);

    // Service status for key DockPanel services
    let service_names = [
        "dockpanel-agent",
        "dockpanel-api",
        "nginx",
        "postgresql",
        "docker",
        "fail2ban",
        "ufw",
        "postfix",
        "dovecot",
        "redis-server",
    ];

    let mut services = Vec::new();
    for name in &service_names {
        let active = tokio::time::timeout(
            timeout,
            safe_command("systemctl")
                .args(["is-active", "--quiet", name])
                .status(),
        )
        .await
        .ok()
        .and_then(|r| r.ok())
        .is_some_and(|s| s.success());

        services.push(ServiceStatus {
            name: name.to_string(),
            active,
        });
    }

    Json(SystemInfo {
        os,
        os_version,
        kernel,
        arch,
        hostname,
        total_memory_mb,
        total_disk_mb,
        cpu_count,
        uptime_seconds,
        dockpanel_agent_version: env!("CARGO_PKG_VERSION").to_string(),
        services,
    })
}

pub fn router() -> Router<AppState> {
    Router::new().route("/telemetry/system-info", get(system_info))
}
