use crate::services::cpu_sampler::CpuSampler;
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};
use std::time::Duration;

/// Configuration for phone-home mode (remote agent connecting to central API).
#[derive(Clone)]
pub struct PhoneHomeConfig {
    pub central_url: String,
    pub server_token: String,
    pub server_id: String,
    /// SHA-256 hex fingerprint of the agent's TLS cert. Sent in every checkin
    /// so the central panel can pin it (Trust On First Use). Populated by
    /// `main.rs` after loading the cert; `None` keeps backward compatibility
    /// with older panels that don't know about pinning.
    pub cert_fingerprint: Option<String>,
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
            cert_fingerprint: None,
        })
    }
}

/// Collect system info for checkin payload.
///
/// `cpu` supplies the CPU figure. This function used to build a `System::new_all()`
/// and read `global_cpu_usage()` off it, which reports the delta between the
/// constructor's refresh and `refresh_all()`'s — a window consisting almost
/// entirely of this function's own walk of `/proc`. Measured on an idle 12-core
/// box whose true usage was 4.5%, that pattern returned anywhere from 4.5% to
/// 22.1% across consecutive runs; on a single-core box the self-inflicted share
/// is twelve times larger. The value lands in `servers.cpu_usage`, which the
/// Servers page displays and the alert engine compares against CPU thresholds,
/// so a fabricated number there fires (or withholds) real alerts.
fn collect_system_info(cpu: &CpuSampler) -> serde_json::Value {
    // Only what the payload reads: the CPU list (for its length) and memory.
    // Refreshing processes here is what corrupted the CPU figure above, and
    // nothing in the payload needs them.
    let mut sys = System::new_with_specifics(
        RefreshKind::nothing()
            .with_cpu(CpuRefreshKind::nothing())
            .with_memory(MemoryRefreshKind::everything()),
    );
    sys.refresh_memory();

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
        "cpu_usage": cpu.usage(),
        "mem_used_mb": (sys.used_memory() / 1_048_576) as i64,
        "uptime_secs": System::uptime(),
        // Replay prevention: server rejects requests >120s old
        "timestamp": chrono::Utc::now().timestamp(),
    })
}

/// Run the phone-home loop: periodically POST system info to central API.
pub async fn run(config: PhoneHomeConfig, cpu: CpuSampler) {
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
        let mut info = collect_system_info(&cpu);
        info["server_id"] = serde_json::json!(config.server_id);
        if let Some(fp) = &config.cert_fingerprint {
            info["cert_fingerprint"] = serde_json::json!(fp);
        }

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

/// Ask the panel every 6 hours whether this agent should move, and if so hand
/// the work to the SAME updater the panel-driven fleet path uses.
///
/// This loop used to download and swap the binary itself. It checksummed the
/// download (fail-closed, to its credit) but then: never verified the new binary
/// afterwards, wrote a `.bak` that nothing in the tree ever read back, staged
/// through `/tmp` so the rename was cross-device wherever `/tmp` is a tmpfs, and
/// restarted itself from inside its own cgroup. `scripts/agent-self-update.sh`
/// already does every one of those correctly, is checksum-verified against the
/// release's own `checksums.txt`, health-verifies the result, rolls back when it
/// does not come up, and is pinned by tests. So the timer goes through it, and
/// nothing in this file touches a binary any more (s233).
async fn auto_update_loop(config: PhoneHomeConfig) {
    let client = reqwest::Client::new();
    let version_url = format!("{}/api/agent/version", config.central_url);
    let current_version = env!("CARGO_PKG_VERSION");

    // Wait an hour before the first check, so a box that has just been updated
    // is not immediately asked to move again.
    tokio::time::sleep(Duration::from_secs(3600)).await;

    loop {
        match fetch_target(&client, &version_url, &config.server_token).await {
            Ok(Some(target)) if target.trim_start_matches('v') != current_version => {
                // A failed update leaves this agent's version unchanged, so the
                // comparison above stays true for ever. Without this check, a
                // target that cannot come up on this box would make the timer
                // repeat the whole destructive sequence — download, swap,
                // restart, failed health poll, roll back, restart again —
                // indefinitely and unattended. Worse, the rollback restarts this
                // process, so the loop would resume at its INITIAL delay rather
                // than the 6-hour one, repeating roughly hourly on every box in
                // the fleet. An operator-driven fleet run is not blocked by this;
                // only the unattended path refuses.
                if let Some(why) =
                    crate::routes::panel_update::target_already_failed_destructively(&target)
                {
                    tracing::warn!(
                        "Agent auto-update: NOT retrying {target} — an earlier attempt already \
                         replaced this binary and had to roll back ({why}). Staying on \
                         {current_version}. Fix the release, or start a fleet update from the \
                         panel to override this."
                    );
                } else {
                    tracing::info!(
                        "Panel target is {target}, running {current_version} — starting self-update"
                    );
                    if let Err(e) =
                        crate::routes::panel_update::start_agent_self_update(&target).await
                    {
                        tracing::warn!("Agent self-update did not start: {e}");
                    }
                }
            }
            Ok(Some(_)) => {
                tracing::info!("Agent auto-update: already at the panel's target (v{current_version})");
            }
            Ok(None) => {
                tracing::info!("Agent auto-update: not enabled by the panel");
            }
            Err(e) => {
                tracing::warn!("Agent auto-update check failed: {e}");
            }
        }

        // Jitter. Unlike the panel-driven fleet path — which the orchestrator
        // serialises — every agent runs this timer independently, so a fleet
        // restarted together would otherwise check in together and then all pull
        // from GitHub at the same moment.
        let jitter = rand::random::<u64>() % 1800;
        tokio::time::sleep(Duration::from_secs(6 * 3600 + jitter)).await;
    }
}

/// Ask the panel what version this agent should be running.
///
/// `Ok(None)` means "nothing to do": the panel returns a null target when the
/// operator has not enabled agent auto-update, or when the update channel is on
/// hold. That is how the OFF switch reaches a box — an agent is never trusted to
/// decide for itself that it should update.
///
/// **The status check before `.json()` is the entire point of this function.**
/// It used to be absent, and an error body is still valid JSON: a 401
/// deserialised cleanly, the missing version field fell back to the running
/// version, the two compared equal, and the loop reported "up to date" at debug
/// level — below the unit's own log level. A permanently dead auth path was
/// therefore indistinguishable in the journal from a healthy fleet, for four
/// releases (s233).
async fn fetch_target(
    client: &reqwest::Client,
    version_url: &str,
    server_token: &str,
) -> Result<Option<String>, String> {
    let resp = client
        .get(version_url)
        .header("Authorization", format!("Bearer {server_token}"))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let status = resp.status();
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    Ok(body
        .get("target_version")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty()))
}


#[cfg(test)]
mod tests {
    /// The periodic update check must never swap a binary itself again.
    ///
    /// It used to: it downloaded the asset, hashed it, wrote a backup nothing
    /// ever read, and renamed over its own running executable — with no health
    /// check afterwards and no rollback. `scripts/agent-self-update.sh` does all
    /// of that correctly and is pinned by its own tests, so the only thing this
    /// file may do is ASK the panel and hand a version string to the shared
    /// launcher.
    ///
    /// The forbidden fragments are assembled rather than written out, because
    /// this test greps its own file (the source-pin prose trap).
    #[test]
    fn the_update_check_delegates_the_swap_and_never_performs_one() {
        let src = include_str!("phone_home.rs");
        let code: String = src
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        for bad in [
            concat!("std::fs::", "rename"),
            concat!("current_", "exe()"),
            concat!("sha2::", "Sha256"),
            concat!("std::process::", "exit"),
        ] {
            assert!(
                !code.contains(bad),
                "the update check must not do its own binary swap, found: {bad}"
            );
        }

        // And it must go through the one shared, tested launcher.
        assert!(
            code.contains("start_agent_self_update("),
            "the check must hand the work to the shared launcher"
        );
        // The status of the version response has to be inspected before its body
        // is trusted: an error body is valid JSON, and reading it as a version
        // answer is what made a dead auth path look like a healthy fleet.
        assert!(
            code.contains("if !status.is_success()"),
            "a non-2xx must never be parsed as a version answer"
        );
        // The request must carry the agent's credential.
        assert!(
            code.contains("format!(\"Bearer {server_token}\")"),
            "the version query must authenticate"
        );
    }
}
