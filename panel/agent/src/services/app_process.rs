//! Systemd service management for Node.js/Python app processes.
//!
//! Creates a per-site systemd service that runs the app, proxied by nginx.

use crate::safe_cmd::safe_command_sync;

use super::command_filter;

const SERVICE_PREFIX: &str = "dockpanel-app-";

/// Service unit name for a domain.
fn service_name(domain: &str) -> String {
    format!("{SERVICE_PREFIX}{}", domain.replace('.', "-"))
}

/// Create and start a systemd service for an app.
pub fn create_app_service(
    domain: &str,
    command: &str,
    port: u16,
    runtime: &str,
) -> Result<(), String> {
    // Validate the command before doing anything else
    command_filter::is_safe_exec_start(command, runtime)?;

    let svc = service_name(domain);
    let working_dir = format!("/var/www/{domain}/public");

    // Determine the ExecStart based on runtime
    let exec_start = match runtime {
        "node" => {
            // Check if it looks like a bare command (e.g., "server.js") vs full command
            if command.starts_with("node ")
                || command.starts_with("npm ")
                || command.starts_with("npx ")
                || command.starts_with("yarn ")
                || command.starts_with("pnpm ")
                || command.starts_with("/")
            {
                command.to_string()
            } else {
                format!("node {command}")
            }
        }
        "python" => {
            if command.starts_with("python")
                || command.starts_with("gunicorn")
                || command.starts_with("uvicorn")
                || command.starts_with("flask")
                || command.starts_with("django")
                || command.starts_with("/")
            {
                command.to_string()
            } else {
                format!("python3 {command}")
            }
        }
        _ => command.to_string(),
    };

    let unit = format!(
        r#"[Unit]
Description=DockPanel App: {domain}
After=network.target

[Service]
Type=simple
User=www-data
Group=www-data
WorkingDirectory={working_dir}
ExecStart={exec_start}
Restart=always
RestartSec=5
Environment=PORT={port}
Environment=NODE_ENV=production
Environment=HOST=0.0.0.0
EnvironmentFile=-/var/www/{domain}/.env

# Resource limits
MemoryMax=512M
CPUQuota=100%
LimitNOFILE=65536
TasksMax=512

# Security hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
PrivateDevices=true
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectControlGroups=true
RestrictSUIDSGID=true
RestrictNamespaces=true
RestrictRealtime=true
LockPersonality=true
SystemCallArchitectures=native
CapabilityBoundingSet=CAP_NET_BIND_SERVICE
SystemCallFilter=@system-service
SystemCallErrorNumber=EPERM
ProtectProc=invisible
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6
ReadWritePaths=/var/www/{domain}

# Logging
StandardOutput=journal
StandardError=journal
SyslogIdentifier={svc}

[Install]
WantedBy=multi-user.target
"#
    );

    let unit_path = format!("/etc/systemd/system/{svc}.service");
    std::fs::write(&unit_path, &unit)
        .map_err(|e| format!("Failed to write service unit: {e}"))?;

    // Create working directory if it doesn't exist
    std::fs::create_dir_all(&working_dir).ok();
    // Set ownership
    safe_command_sync("chown")
        .args(["-R", "www-data:www-data", &format!("/var/www/{domain}")])
        .output()
        .ok();

    // Reload systemd and enable+start the service
    safe_command_sync("systemctl")
        .args(["daemon-reload"])
        .output()
        .map_err(|e| format!("daemon-reload failed: {e}"))?;

    safe_command_sync("systemctl")
        .args(["enable", "--now", &svc])
        .output()
        .map_err(|e| format!("Failed to start service: {e}"))?;

    tracing::info!("App service created and started: {svc} (port={port}, runtime={runtime})");
    Ok(())
}

/// Stop and remove the systemd service for an app.
pub fn remove_app_service(domain: &str) -> Result<(), String> {
    let svc = service_name(domain);
    let unit_path = format!("/etc/systemd/system/{svc}.service");

    if !std::path::Path::new(&unit_path).exists() {
        return Ok(()); // No service to remove
    }

    // Stop and disable
    safe_command_sync("systemctl")
        .args(["stop", &svc])
        .output()
        .ok();
    safe_command_sync("systemctl")
        .args(["disable", &svc])
        .output()
        .ok();

    // Remove unit file
    std::fs::remove_file(&unit_path).ok();

    // Reload systemd
    safe_command_sync("systemctl")
        .args(["daemon-reload"])
        .output()
        .ok();

    tracing::info!("App service removed: {svc}");
    Ok(())
}

/// Restart the app service.
pub fn restart_app_service(domain: &str) -> Result<(), String> {
    let svc = service_name(domain);
    let output = safe_command_sync("systemctl")
        .args(["restart", &svc])
        .output()
        .map_err(|e| format!("Failed to restart {svc}: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Failed to restart {svc}: {stderr}"));
    }

    Ok(())
}

/// Get the status of an app service.
pub fn app_service_status(domain: &str) -> String {
    let svc = service_name(domain);
    let output = safe_command_sync("systemctl")
        .args(["is-active", &svc])
        .output();

    match output {
        Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        Err(_) => "unknown".to_string(),
    }
}
