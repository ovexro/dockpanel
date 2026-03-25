use axum::{routing::get, Json, Router};
use serde::Serialize;
use crate::safe_cmd::safe_command;

use super::AppState;

#[derive(Serialize)]
struct ServiceStatus {
    name: String,
    status: String,
}

/// GET /services/health — check status of critical system services.
async fn check_services() -> Json<Vec<ServiceStatus>> {
    let services = [
        "nginx",
        "mysql",
        "mariadb",
        "postgresql",
        "php-fpm",
        "redis-server",
        "docker",
    ];

    let mut results = Vec::new();
    for name in services {
        let status = check_systemd_service(name).await;
        results.push(status);
    }

    // Also check for PHP-FPM version-specific services (php8.x-fpm)
    if let Ok(output) = safe_command("sh")
        .args(["-c", "systemctl list-units --type=service --no-pager --plain 2>/dev/null | grep 'php.*fpm' | awk '{print $1}'"])
        .output()
        .await
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            let svc = line.trim().strip_suffix(".service").unwrap_or(line.trim());
            if !svc.is_empty() && svc != "php-fpm" {
                results.push(check_systemd_service(svc).await);
            }
        }
    }

    Json(results)
}

async fn check_systemd_service(name: &str) -> ServiceStatus {
    // First check if the unit is enabled/loaded
    let enabled = safe_command("systemctl")
        .args(["is-enabled", name])
        .output()
        .await;

    let is_managed = match &enabled {
        Ok(o) => {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            matches!(s.as_str(), "enabled" | "static")
        }
        Err(_) => false,
    };

    // If not enabled, check if it even exists as a unit
    if !is_managed {
        let is_disabled = match &enabled {
            Ok(o) => {
                let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                matches!(s.as_str(), "disabled" | "masked" | "indirect")
            }
            Err(_) => false,
        };
        if !is_disabled {
            return ServiceStatus {
                name: name.to_string(),
                status: "not_installed".to_string(),
            };
        }
        // Disabled services are intentionally off — don't alert
        return ServiceStatus {
            name: name.to_string(),
            status: "disabled".to_string(),
        };
    }

    // Now check active status for enabled/static services
    let output = safe_command("systemctl")
        .args(["is-active", name])
        .output()
        .await;

    let status = match output {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout).trim().to_string();
            match stdout.as_str() {
                "active" => "running",
                "inactive" | "dead" => "stopped",
                "failed" => "failed",
                _ => "unknown",
            }
        }
        Err(_) => "unknown",
    };

    ServiceStatus {
        name: name.to_string(),
        status: status.to_string(),
    }
}

pub fn router() -> Router<AppState> {
    Router::new().route("/services/health", get(check_services))
}
