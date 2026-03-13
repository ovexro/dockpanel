use axum::{
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use super::AppState;

/// Allowed PHP versions (ondrej/php PPA).
const ALLOWED_VERSIONS: &[&str] = &["8.1", "8.2", "8.3", "8.4"];

/// Common PHP extensions to install with each version.
const COMMON_EXTENSIONS: &[&str] = &[
    "cli", "common", "mysql", "pgsql", "sqlite3", "curl", "gd", "mbstring",
    "xml", "zip", "bcmath", "intl", "readline", "opcache", "redis", "imagick",
];

#[derive(Serialize)]
struct PhpVersion {
    version: String,
    installed: bool,
    fpm_running: bool,
    socket: String,
}

#[derive(Serialize)]
struct PhpListResponse {
    versions: Vec<PhpVersion>,
}

#[derive(Deserialize)]
struct InstallRequest {
    version: String,
}

#[derive(Serialize)]
struct InstallResponse {
    success: bool,
    message: String,
    version: String,
}

/// Check if a PHP-FPM version is installed via dpkg.
async fn is_installed(version: &str) -> bool {
    Command::new("dpkg")
        .args(["-s", &format!("php{version}-fpm")])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Check if a PHP-FPM socket file exists.
fn socket_exists(version: &str) -> bool {
    std::path::Path::new(&format!("/run/php/php{version}-fpm.sock")).exists()
}

/// Check if PHP-FPM service is active.
async fn is_fpm_running(version: &str) -> bool {
    Command::new("systemctl")
        .args(["is-active", "--quiet", &format!("php{version}-fpm")])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// GET /php/versions — List all PHP versions with install/running status.
async fn list_versions() -> Json<PhpListResponse> {
    let mut versions = Vec::new();

    for &v in ALLOWED_VERSIONS {
        let installed = is_installed(v).await;
        let fpm_running = if installed {
            is_fpm_running(v).await || socket_exists(v)
        } else {
            false
        };

        versions.push(PhpVersion {
            version: v.to_string(),
            installed,
            fpm_running,
            socket: format!("/run/php/php{v}-fpm.sock"),
        });
    }

    Json(PhpListResponse { versions })
}

/// POST /php/install — Install a PHP version with common extensions.
async fn install_version(
    Json(body): Json<InstallRequest>,
) -> Result<Json<InstallResponse>, (StatusCode, Json<InstallResponse>)> {
    let version = body.version.trim();

    if !ALLOWED_VERSIONS.contains(&version) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(InstallResponse {
                success: false,
                message: format!("Invalid version. Allowed: {}", ALLOWED_VERSIONS.join(", ")),
                version: version.to_string(),
            }),
        ));
    }

    // Check if already installed
    if is_installed(version).await {
        return Ok(Json(InstallResponse {
            success: true,
            message: format!("PHP {version} is already installed"),
            version: version.to_string(),
        }));
    }

    // Ensure ondrej/php PPA is added
    let ppa_check = Command::new("bash")
        .args(["-c", "apt-cache policy php8.4-fpm 2>/dev/null | grep -q ondrej || true"])
        .output()
        .await;

    if ppa_check.is_err() {
        // Try adding PPA
        tracing::info!("Adding ondrej/php PPA...");
        let ppa_result = Command::new("bash")
            .args(["-c", "apt-get update -qq && apt-get install -y -qq software-properties-common && add-apt-repository -y ppa:ondrej/php && apt-get update -qq"])
            .output()
            .await;

        if let Err(e) = ppa_result {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(InstallResponse {
                    success: false,
                    message: format!("Failed to add PHP PPA: {e}"),
                    version: version.to_string(),
                }),
            ));
        }
    }

    // Build package list: php{version}-fpm + extensions
    let mut packages = vec![format!("php{version}-fpm")];
    for ext in COMMON_EXTENSIONS {
        packages.push(format!("php{version}-{ext}"));
    }
    let pkg_str = packages.join(" ");

    tracing::info!("Installing PHP {version}: {pkg_str}");

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        Command::new("bash")
            .args(["-c", &format!(
                "DEBIAN_FRONTEND=noninteractive apt-get install -y -qq {pkg_str} 2>&1"
            )])
            .output(),
    )
    .await;

    let output = match output {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(InstallResponse {
                    success: false,
                    message: format!("Install command failed: {e}"),
                    version: version.to_string(),
                }),
            ));
        }
        Err(_) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(InstallResponse {
                    success: false,
                    message: "Installation timed out (5 min limit)".into(),
                    version: version.to_string(),
                }),
            ));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(InstallResponse {
                success: false,
                message: format!("apt install failed:\n{stdout}\n{stderr}"),
                version: version.to_string(),
            }),
        ));
    }

    // Enable and start FPM service
    let _ = Command::new("systemctl")
        .args(["enable", "--now", &format!("php{version}-fpm")])
        .output()
        .await;

    tracing::info!("PHP {version} installed and started");

    Ok(Json(InstallResponse {
        success: true,
        message: format!("PHP {version} installed with {} extensions", COMMON_EXTENSIONS.len()),
        version: version.to_string(),
    }))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/php/versions", get(list_versions))
        .route("/php/install", post(install_version))
}
