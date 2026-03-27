use axum::{
    extract::Path,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use crate::safe_cmd::safe_command;

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
    safe_command("dpkg")
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
    safe_command("systemctl")
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
    let ppa_check = safe_command("bash")
        .args(["-c", "apt-cache policy php8.4-fpm 2>/dev/null | grep -q ondrej || true"])
        .output()
        .await;

    if ppa_check.is_err() {
        // Try adding PPA
        tracing::info!("Adding ondrej/php PPA...");
        let ppa_result = safe_command("bash")
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
        safe_command("bash")
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
    let _ = safe_command("systemctl")
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

// ──────────────────────────────────────────────────────────────
// PHP Extensions Manager
// ──────────────────────────────────────────────────────────────

type PhpApiErr = (StatusCode, Json<serde_json::Value>);

fn php_api_err(status: StatusCode, msg: &str) -> PhpApiErr {
    (status, Json(serde_json::json!({ "error": msg })))
}

/// GET /php/extensions/{version} — List installed PHP extensions.
async fn list_extensions(Path(version): Path<String>) -> Result<Json<serde_json::Value>, PhpApiErr> {
    if !ALLOWED_VERSIONS.contains(&version.as_str()) {
        return Err(php_api_err(StatusCode::BAD_REQUEST, &format!("Invalid PHP version. Allowed: {}", ALLOWED_VERSIONS.join(", "))));
    }

    // List all installed extensions
    let output = safe_command("php")
        .args([&format!("-d"), "error_reporting=0", "-m"])
        .env("PATH", "/usr/bin:/usr/sbin:/bin")
        .output().await
        .map_err(|e| php_api_err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let extensions: Vec<String> = stdout.lines()
        .filter(|l| !l.is_empty() && !l.starts_with('['))
        .map(|l| l.trim().to_lowercase())
        .collect();

    // List available (installable) extensions
    let avail_output = safe_command("apt-cache")
        .args(["search", &format!("php{version}-")])
        .output().await;

    let available: Vec<String> = avail_output.ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).lines()
            .filter_map(|l| {
                let pkg = l.split_whitespace().next()?;
                let ext = pkg.strip_prefix(&format!("php{version}-"))?;
                if ["common", "cli", "fpm", "dev", "dbg"].contains(&ext) { return None; }
                Some(ext.to_string())
            })
            .collect())
        .unwrap_or_default();

    Ok(Json(serde_json::json!({ "installed": extensions, "available": available, "version": version })))
}

/// POST /php/extensions/install — Install a PHP extension.
async fn install_extension(Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, PhpApiErr> {
    let version = body.get("version").and_then(|v| v.as_str()).unwrap_or("8.3");
    let extension = body.get("extension").and_then(|v| v.as_str()).unwrap_or("");

    if !ALLOWED_VERSIONS.contains(&version) {
        return Err(php_api_err(StatusCode::BAD_REQUEST, &format!("Invalid PHP version. Allowed: {}", ALLOWED_VERSIONS.join(", "))));
    }

    if extension.is_empty() || !extension.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(php_api_err(StatusCode::BAD_REQUEST, "Invalid extension name"));
    }

    let package = format!("php{version}-{extension}");
    tracing::info!("Installing PHP extension: {package}");

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        safe_command("apt-get")
            .args(["install", "-y", &package])
            .env("DEBIAN_FRONTEND", "noninteractive")
            .output()
    ).await
        .map_err(|_| php_api_err(StatusCode::GATEWAY_TIMEOUT, "Install timed out"))?
        .map_err(|e| php_api_err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let msg = &stderr[..200.min(stderr.len())];
        return Err(php_api_err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Install failed: {msg}")));
    }

    // Restart PHP-FPM
    let _ = safe_command("systemctl")
        .args(["restart", &format!("php{version}-fpm")])
        .output().await;

    tracing::info!("PHP extension installed: {package}");
    Ok(Json(serde_json::json!({ "ok": true, "package": package })))
}

/// POST /php/uninstall — Remove a PHP version and all its extensions.
async fn uninstall_version(
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

    // Check if installed
    if !is_installed(version).await {
        return Ok(Json(InstallResponse {
            success: true,
            message: format!("PHP {version} is not installed"),
            version: version.to_string(),
        }));
    }

    // 1. Stop and disable FPM service
    let _ = safe_command("systemctl")
        .args(["stop", &format!("php{version}-fpm")])
        .output()
        .await;
    let _ = safe_command("systemctl")
        .args(["disable", &format!("php{version}-fpm")])
        .output()
        .await;

    // 2. Purge all php{version}-* packages
    tracing::info!("Uninstalling PHP {version}...");
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        safe_command("bash")
            .args(["-c", &format!(
                "DEBIAN_FRONTEND=noninteractive apt-get purge -y php{version}-* 2>&1"
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
                    message: format!("Purge command failed: {e}"),
                    version: version.to_string(),
                }),
            ));
        }
        Err(_) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(InstallResponse {
                    success: false,
                    message: "Uninstall timed out (5 min limit)".into(),
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
                message: format!("apt purge failed:\n{stdout}\n{stderr}"),
                version: version.to_string(),
            }),
        ));
    }

    // 3. Autoremove
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        safe_command("bash")
            .args(["-c", "DEBIAN_FRONTEND=noninteractive apt-get autoremove -y 2>&1"])
            .output(),
    )
    .await;

    tracing::info!("PHP {version} uninstalled");

    Ok(Json(InstallResponse {
        success: true,
        message: format!("PHP {version} has been uninstalled"),
        version: version.to_string(),
    }))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/php/versions", get(list_versions))
        .route("/php/install", post(install_version))
        .route("/php/uninstall", post(uninstall_version))
        // PHP Extensions
        .route("/php/extensions/{version}", get(list_extensions))
        .route("/php/extensions/install", post(install_extension))
}
