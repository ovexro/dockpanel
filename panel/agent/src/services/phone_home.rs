use crate::safe_cmd::safe_command;
use sysinfo::System;
use std::time::Duration;

/// Configuration for phone-home mode (remote agent connecting to central API).
#[derive(Clone)]
pub struct PhoneHomeConfig {
    pub central_url: String,
    pub server_token: String,
    pub server_id: String,
}

impl PhoneHomeConfig {
    /// Read from environment variables. Returns None if not configured.
    pub fn from_env() -> Option<Self> {
        let url = std::env::var("DOCKPANEL_CENTRAL_URL").ok()?;
        let token = std::env::var("DOCKPANEL_SERVER_TOKEN").ok()?;
        let id = std::env::var("DOCKPANEL_SERVER_ID").ok()?;

        if url.is_empty() || token.is_empty() || id.is_empty() {
            return None;
        }

        Some(Self {
            central_url: url.trim_end_matches('/').to_string(),
            server_token: token,
            server_id: id,
        })
    }
}

/// Collect system info for checkin payload.
fn collect_system_info() -> serde_json::Value {
    let mut sys = System::new_all();
    sys.refresh_all();

    let disks = sysinfo::Disks::new_with_refreshed_list();
    let root_disk = disks
        .iter()
        .find(|d| d.mount_point() == std::path::Path::new("/"));
    let disk_total_gb = root_disk
        .map(|d| (d.total_space() as f64 / 1_073_741_824.0).round() as i64)
        .unwrap_or(0);
    let (disk_used_gb, disk_usage_pct) = root_disk
        .map(|d| {
            let total = d.total_space();
            let used = total - d.available_space();
            (
                (used as f64 / 1_073_741_824.0).round() as i64,
                if total > 0 { (used as f32 / total as f32) * 100.0 } else { 0.0 },
            )
        })
        .unwrap_or((0, 0.0));

    serde_json::json!({
        "server_id": "",  // filled by caller
        "os_info": System::long_os_version().unwrap_or_default(),
        "hostname": System::host_name().unwrap_or_default(),
        "cpu_cores": sys.cpus().len(),
        "ram_mb": (sys.total_memory() / 1_048_576) as i64,
        "disk_gb": disk_total_gb,
        "disk_used_gb": disk_used_gb,
        "disk_usage_pct": disk_usage_pct,
        "agent_version": env!("CARGO_PKG_VERSION"),
        // Live metrics
        "cpu_usage": sys.global_cpu_usage(),
        "mem_used_mb": (sys.used_memory() / 1_048_576) as i64,
        "uptime_secs": System::uptime(),
    })
}

/// Run the phone-home loop: periodically POST system info to central API.
pub async fn run(config: PhoneHomeConfig) {
    tracing::info!(
        "Phone-home enabled: server_id={}, central={}",
        config.server_id,
        config.central_url
    );

    // Initial delay to let the agent fully start
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Spawn command poller alongside checkin loop
    let cmd_config = config.clone();
    tokio::spawn(async move {
        command_poll_loop(cmd_config).await;
    });

    // Spawn auto-update checker (every 6 hours)
    let update_config = config.clone();
    tokio::spawn(async move {
        auto_update_loop(update_config).await;
    });

    let client = reqwest::Client::new();
    let checkin_url = format!("{}/api/agent/checkin", config.central_url);

    loop {
        let mut info = collect_system_info();
        info["server_id"] = serde_json::json!(config.server_id);

        match client
            .post(&checkin_url)
            .header("Authorization", format!("Bearer {}", config.server_token))
            .json(&info)
            .timeout(Duration::from_secs(15))
            .send()
            .await
        {
            Ok(resp) => {
                if resp.status().is_success() {
                    tracing::debug!("Phone-home checkin OK");
                } else {
                    tracing::warn!(
                        "Phone-home checkin failed: HTTP {}",
                        resp.status()
                    );
                }
            }
            Err(e) => {
                tracing::warn!("Phone-home checkin error: {e}");
            }
        }

        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

#[derive(serde::Deserialize)]
struct RemoteCommand {
    id: String,
    action: String,
    payload: serde_json::Value,
}

/// Poll central API for pending commands and execute them locally via the agent HTTP server.
async fn command_poll_loop(config: PhoneHomeConfig) {
    let client = reqwest::Client::new();
    let poll_url = format!("{}/api/agent/commands", config.central_url);
    let result_url = format!("{}/api/agent/commands/result", config.central_url);
    let agent_url = "http://127.0.0.1:9090"; // Agent's own HTTP listener

    // Wait for local agent HTTP to be ready
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Read local agent token for forwarding requests
    let agent_token = std::env::var("AGENT_TOKEN").unwrap_or_default();

    loop {
        match client
            .get(&poll_url)
            .header("Authorization", format!("Bearer {}", config.server_token))
            .timeout(Duration::from_secs(10))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                if let Ok(commands) = resp.json::<Vec<RemoteCommand>>().await {
                    for cmd in commands {
                        let result = execute_command(
                            &client, agent_url, &agent_token, &cmd.action, &cmd.payload,
                        )
                        .await;

                        let (status, result_body) = match result {
                            Ok(body) => ("completed", Some(body)),
                            Err(e) => {
                                tracing::error!("Command {} failed: {e}", cmd.action);
                                ("failed", Some(serde_json::json!({ "error": e })))
                            }
                        };

                        // Report result back to central
                        let _ = client
                            .post(&result_url)
                            .header("Authorization", format!("Bearer {}", config.server_token))
                            .json(&serde_json::json!({
                                "command_id": cmd.id,
                                "status": status,
                                "result": result_body,
                            }))
                            .timeout(Duration::from_secs(10))
                            .send()
                            .await;
                    }
                }
            }
            Ok(resp) => {
                tracing::debug!("Command poll: HTTP {}", resp.status());
            }
            Err(e) => {
                tracing::debug!("Command poll error: {e}");
            }
        }

        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

/// Execute a command by forwarding it to the local agent HTTP API.
/// Maps action names to agent API endpoints.
async fn execute_command(
    client: &reqwest::Client,
    agent_url: &str,
    agent_token: &str,
    action: &str,
    payload: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    // Strict allowlist of permitted actions
    const ALLOWED_COMMANDS: &[&str] = &[
        "site.create",
        "site.delete",
        "ssl.provision",
        "nginx.reload",
        "health",
        "restart_agent",
        "check_health",
        "update_agent",
        "sync_config",
        "run_security_scan",
        "run_backup",
    ];

    if !ALLOWED_COMMANDS.contains(&action) {
        return Err(format!("Action not allowed: {action}"));
    }

    // Map action names to HTTP method + path
    let (method, path): (&str, String) = match action {
        // Site operations
        "site.create" => ("POST", "/sites".to_string()),
        "site.delete" => ("DELETE", format!("/sites/{}", payload["domain"].as_str().unwrap_or(""))),
        // SSL
        "ssl.provision" => ("POST", "/ssl/provision".to_string()),
        // Nginx
        "nginx.reload" => ("POST", "/nginx/reload".to_string()),
        // System
        "health" | "check_health" => ("GET", "/health".to_string()),
        "restart_agent" => ("POST", "/system/restart".to_string()),
        "update_agent" => ("POST", "/system/update".to_string()),
        "sync_config" => ("POST", "/system/sync-config".to_string()),
        "run_security_scan" => ("POST", "/security/scan".to_string()),
        "run_backup" => ("POST", "/backups/run".to_string()),
        _ => return Err(format!("Unknown action: {action}")),
    };

    let url = format!("{agent_url}{path}");
    let builder = match method {
        "GET" => client.get(&url),
        "POST" => client.post(&url).json(payload),
        "PUT" => client.put(&url).json(payload),
        "DELETE" => client.delete(&url),
        _ => return Err(format!("Unsupported method: {method}")),
    };

    let resp = builder
        .header("Authorization", format!("Bearer {agent_token}"))
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::json!({}));

    if status.is_success() {
        Ok(body)
    } else {
        let error = body["error"].as_str().unwrap_or("Unknown error");
        Err(format!("HTTP {status}: {error}"))
    }
}

/// Check for agent updates every 6 hours. Download new binary, verify checksum, replace, restart.
async fn auto_update_loop(config: PhoneHomeConfig) {
    let client = reqwest::Client::new();
    let version_url = format!("{}/api/agent/version", config.central_url);
    let current_version = env!("CARGO_PKG_VERSION");

    // Wait 1 hour before first check
    tokio::time::sleep(Duration::from_secs(3600)).await;

    loop {
        match check_and_update(&client, &version_url, current_version).await {
            Ok(true) => {
                tracing::info!("Agent updated, restarting via systemd...");
                // Restart self via systemd
                let _ = safe_command("systemctl")
                    .args(["restart", "dockpanel-agent"])
                    .status()
                    .await;
                // If systemctl not available, exit and let the service manager restart us
                tokio::time::sleep(Duration::from_secs(5)).await;
                std::process::exit(0);
            }
            Ok(false) => {
                tracing::debug!("Agent is up to date (v{current_version})");
            }
            Err(e) => {
                tracing::warn!("Auto-update check failed: {e}");
            }
        }

        tokio::time::sleep(Duration::from_secs(6 * 3600)).await; // 6 hours
    }
}

async fn check_and_update(
    client: &reqwest::Client,
    version_url: &str,
    current_version: &str,
) -> Result<bool, String> {
    let resp = client
        .get(version_url)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;

    let latest = body["version"].as_str().unwrap_or(current_version);
    if latest == current_version {
        return Ok(false);
    }

    let download_url = body["download_url"]
        .as_str()
        .ok_or("No download URL provided")?;
    let expected_checksum = body["checksum"].as_str();

    tracing::info!("New agent version available: {current_version} -> {latest}");

    // Download new binary to temp file
    let tmp_path = "/tmp/dockpanel-agent-new";
    let resp = client
        .get(download_url)
        .timeout(Duration::from_secs(300))
        .send()
        .await
        .map_err(|e| format!("Download failed: {e}"))?;

    let bytes = resp.bytes().await.map_err(|e| format!("Read failed: {e}"))?;

    // Verify checksum if provided
    if let Some(expected) = expected_checksum {
        use sha2::Digest;
        let mut hasher = sha2::Sha256::new();
        hasher.update(&bytes);
        let actual = hex::encode(hasher.finalize());
        if actual != expected {
            return Err(format!("Checksum mismatch: expected {expected}, got {actual}"));
        }
        tracing::info!("Checksum verified");
    }

    // Write to temp file
    tokio::fs::write(tmp_path, &bytes)
        .await
        .map_err(|e| format!("Write failed: {e}"))?;

    // Make executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(tmp_path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("chmod failed: {e}"))?;
    }

    // Backup current binary
    let current_path = std::env::current_exe().map_err(|e| format!("Can't find self: {e}"))?;
    let backup_path = format!("{}.bak", current_path.display());
    let _ = std::fs::copy(&current_path, &backup_path);

    // Replace current binary
    std::fs::rename(tmp_path, &current_path)
        .map_err(|e| format!("Replace failed: {e}"))?;

    tracing::info!("Agent binary replaced: {current_version} -> {latest}");
    Ok(true)
}
