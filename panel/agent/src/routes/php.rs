use axum::{
    extract::Path,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use crate::safe_cmd::{safe_command, safe_command_unsandboxed};

use super::AppState;

/// Allowed PHP versions (distro repos, deb.sury.org, or ondrej/php PPA).
const ALLOWED_VERSIONS: &[&str] = &["8.1", "8.2", "8.3", "8.4", "8.5"];

/// PHP extensions to install with each version — each is installed only if
/// this apt source actually has a candidate for it. Newer PHP releases build
/// some of these in (8.5 absorbed opcache), so a fixed all-or-nothing list
/// would fail the entire transaction over one obsolete package name.
const COMMON_EXTENSIONS: &[&str] = &[
    "mysql", "pgsql", "sqlite3", "curl", "gd", "mbstring",
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

/// Check if a PHP-FPM version is installed.
///
/// On RPM boxes there is one `php-fpm` package rather than one per version, so
/// `services::pkg::is_installed` collapses every `php{v}-fpm` onto it — a
/// `true` there means "some PHP-FPM is installed", which is the wrong question
/// for a page that lists five versions.
///
/// Asking it anyway reported **every** offered version as installed on a RHEL
/// box. So on that family the real installed version is read from the package
/// database and compared; on apt the per-version packages answer directly.
async fn is_installed(version: &str) -> bool {
    if let Some(actual) = crate::services::pkg::installed_php_version().await {
        return actual == version;
    }
    crate::services::pkg::is_installed(&format!("php{version}-fpm")).await
}

/// Check if a PHP-FPM socket file exists for THIS version.
///
/// Delegates to [`crate::services::pkg::resolve_php_fpm_socket`], the shared
/// resolver that also backs `nginx.rs::put_site()` — so this route and site
/// creation can never again disagree about which family's socket is real.
async fn socket_exists(version: &str) -> bool {
    crate::services::pkg::resolve_php_fpm_socket(version).await.is_some()
}

/// Check if PHP-FPM service is active.
async fn is_fpm_running(version: &str) -> bool {
    let unit = crate::services::pkg::service_name(&format!("php{version}-fpm")).await;
    safe_command("systemctl")
        .args(["is-active", "--quiet", &unit])
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
        let real_socket = if installed {
            crate::services::pkg::resolve_php_fpm_socket(v).await
        } else {
            None
        };
        let fpm_running = installed && (is_fpm_running(v).await || real_socket.is_some());

        versions.push(PhpVersion {
            version: v.to_string(),
            installed,
            fpm_running,
            // Falls back to the Debian-shaped guess when nothing real was found yet
            // (not installed, or FPM hasn't opened its socket) — same as before.
            socket: real_socket.unwrap_or_else(|| format!("/run/php/php{v}-fpm.sock")),
        });
    }

    Json(PhpListResponse { versions })
}

/// Install a PHP version on the RHEL family by selecting its module stream.
///
/// The version is chosen BEFORE the install, because `dnf install php-fpm` with
/// no stream enabled resolves to the non-modular base package — PHP 8.0 on
/// Rocky 9, older than every stream the box offers and long end-of-life, with
/// nothing in the UI to say so.
async fn install_version_rpm(
    version: &str,
) -> Result<Json<InstallResponse>, (StatusCode, Json<InstallResponse>)> {
    use crate::services::pkg;

    let fail = |msg: String| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(InstallResponse { success: false, message: msg, version: version.to_string() }),
        )
    };

    tracing::info!("Installing PHP {version} from its module stream");
    pkg::enable_php_stream(version)
        .await
        .map_err(|e| fail(format!("Could not select PHP {version}: {e}")))?;

    let core = [
        format!("php{version}-fpm"),
        format!("php{version}-cli"),
    ];
    let core_refs: Vec<&str> = core.iter().map(String::as_str).collect();
    pkg::install(&core_refs)
        .await
        .map_err(|e| fail(format!("PHP {version} install failed: {e}")))?;

    let exts: Vec<String> = COMMON_EXTENSIONS
        .iter()
        .map(|e| format!("php{version}-{e}"))
        .collect();
    let ext_refs: Vec<&str> = exts.iter().map(String::as_str).collect();
    let skipped = pkg::install_available(&ext_refs)
        .await
        .map_err(|e| fail(format!("PHP {version} extension install failed: {e}")))?;
    if !skipped.is_empty() {
        tracing::info!("PHP {version}: no candidate for {}", skipped.join(" "));
    }

    let unit = pkg::service_name(&format!("php{version}-fpm")).await;
    let _ = safe_command("systemctl").args(["enable", &unit]).output().await;
    let _ = safe_command("systemctl").args(["restart", &unit]).output().await;

    Ok(Json(settle(version, "was installed").await))
}

/// Enable and start this version's FPM unit, under whichever name the family uses.
async fn start_fpm(version: &str) {
    let unit = crate::services::pkg::service_name(&format!("php{version}-fpm")).await;
    let _ = safe_command("systemctl")
        .args(["enable", "--now", &unit])
        .output()
        .await;
}

/// Report on what an install actually achieved, judged by the thing that matters.
///
/// Every caller of this route wants a PHP site to work afterwards, and the only
/// fact that decides that is whether the FPM socket exists — packages installed
/// and units enabled are means, not the end. So the verdict waits briefly for
/// the socket and then says what it found. The window is short because
/// `systemctl enable --now` has already returned by this point; it covers FPM
/// finishing its own startup, not the install.
///
/// Answering `success: false` here is deliberate even though the packages may be
/// perfectly installed: a caller that is told "installed" and then cannot create
/// a PHP site has been told the wrong thing, and that exact sequence is what
/// this route was reported for.
async fn settle(version: &str, what_happened: &str) -> InstallResponse {
    for _ in 0..25 {
        if socket_exists(version).await {
            return InstallResponse {
                success: true,
                message: format!("PHP {version} {what_happened} and PHP-FPM is running"),
                version: version.to_string(),
            };
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }

    let unit = crate::services::pkg::service_name(&format!("php{version}-fpm")).await;
    InstallResponse {
        success: false,
        message: format!(
            "PHP {version} {what_happened}, but PHP-FPM did not open its socket, so PHP sites \
             on this version will not work yet. Check `systemctl status {unit}` and \
             `journalctl -u {unit}` on the server."
        ),
        version: version.to_string(),
    }
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

    // Already installed — but that is not the question anyone is asking.
    //
    // `is_installed` reads the PACKAGE database, while everything downstream of
    // an install cares about the SOCKET: `nginx.rs` refuses to write a PHP vhost
    // when `/run/php/php{v}-fpm.sock` is missing, and that guard is what sends
    // people here in the first place. A box whose php8.3-fpm unit is installed
    // and stopped satisfied this branch, so the install reported success and the
    // very next action — the switch that prompted it — failed again with the
    // same message. Start the unit and let the socket answer.
    if is_installed(version).await {
        start_fpm(version).await;
        return Ok(Json(settle(version, "was already installed").await));
    }

    // The RHEL family selects a PHP version by enabling a module stream, not by
    // installing a versioned package, so everything below this point — the
    // apt-cache probe, deb.sury.org, ppa:ondrej/php — is Debian machinery.
    // This whole file had NO family guard before s266, so on a RHEL box every
    // handler here failed with a raw "Failed to find executable apt-get": the
    // same defect s265 fixed in the service installers and did not reach here.
    let streams = crate::services::pkg::php_streams().await;
    if !streams.is_empty() {
        if !streams.iter().any(|s| s == version) {
            return Err((
                StatusCode::NOT_IMPLEMENTED,
                Json(InstallResponse {
                    success: false,
                    message: format!(
                        "PHP {version} is not offered by this system's package repositories. \
                         Available versions here: {}. (Other versions would need the third-party \
                         remi repository, which DockPanel does not configure.)",
                        streams.join(", ")
                    ),
                    version: version.to_string(),
                }),
            ));
        }
        return install_version_rpm(version).await;
    }

    // Ensure php{version}-fpm is available from some apt source. On Debian 13
    // and Ubuntu 24.04 the requested version may already be in the default
    // repo. If not, configure a 3rd-party repo: deb.sury.org for Debian,
    // ppa:ondrej/php for Ubuntu. (Pre-v2.8.16 used Ondrej PPA for both,
    // which doesn't work on Debian — packages aren't built for trixie.)
    let pkg_available = safe_command("bash")
        .args(["-c", &format!("apt-cache show php{version}-fpm 2>/dev/null | grep -q '^Package:'")])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !pkg_available {
        let osr = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
        let parse = |key: &str| -> String {
            osr.lines()
                .find_map(|l| l.strip_prefix(&format!("{key}=")))
                .map(|v| v.trim_matches('"').to_string())
                .unwrap_or_default()
        };
        let os_id = parse("ID");
        let codename = parse("VERSION_CODENAME");

        // Each repo script: (1) pre-checks that the 3rd-party repo actually
        // publishes for this release — brand-new distro releases lag behind
        // (ppa:ondrej/php had no Ubuntu 26.04 builds at that release's
        // launch); (2) verifies the requested version exists after adding.
        // Failing either way exits non-zero, and the failure branch below
        // REMOVES the added source — a dead source left behind breaks every
        // later `apt-get update` on the box with a 404.
        let repo_cmd = if os_id == "debian" && !codename.is_empty() {
            tracing::info!("Adding deb.sury.org repo for Debian ({codename})...");
            format!(
                "curl -sfI --max-time 15 https://packages.sury.org/php/dists/{codename}/Release > /dev/null || \
                     {{ echo 'deb.sury.org publishes no PHP packages for Debian {codename} yet' >&2; exit 42; }}; \
                 apt-get update -qq && apt-get install -y -qq apt-transport-https lsb-release ca-certificates curl gnupg && \
                 curl -sSLo /usr/share/keyrings/deb.sury.org-php.gpg https://packages.sury.org/php/apt.gpg && \
                 echo 'deb [signed-by=/usr/share/keyrings/deb.sury.org-php.gpg] https://packages.sury.org/php/ {codename} main' > /etc/apt/sources.list.d/sury-php.list && \
                 apt-get update -qq && \
                 {{ apt-cache policy php{version}-fpm 2>/dev/null | grep -q 'Candidate: [0-9]' || \
                     {{ echo 'php{version}-fpm is still unavailable after adding deb.sury.org' >&2; exit 43; }}; }}"
            )
        } else if os_id == "ubuntu" && !codename.is_empty() {
            tracing::info!("Adding ppa:ondrej/php for Ubuntu ({codename})...");
            format!(
                "curl -sfI --max-time 15 https://ppa.launchpadcontent.net/ondrej/php/ubuntu/dists/{codename}/Release > /dev/null || \
                     {{ echo 'ppa:ondrej/php publishes no packages for Ubuntu {codename} yet' >&2; exit 42; }}; \
                 apt-get update -qq && apt-get install -y -qq software-properties-common && \
                 add-apt-repository -y ppa:ondrej/php && apt-get update -qq && \
                 {{ apt-cache policy php{version}-fpm 2>/dev/null | grep -q 'Candidate: [0-9]' || \
                     {{ echo 'php{version}-fpm is still unavailable after adding ppa:ondrej/php' >&2; exit 43; }}; }}"
            )
        } else {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(InstallResponse {
                    success: false,
                    message: format!("Unsupported distro for PHP install: ID='{os_id}'. Install PHP {version} manually."),
                    version: version.to_string(),
                }),
            ));
        };

        let repo_result = safe_command_unsandboxed("bash", &[])
            .args(["-c", &repo_cmd])
            .output()
            .await;

        match repo_result {
            Ok(o) if !o.status.success() => {
                // Roll the source back so a failed attempt can't poison apt.
                let _ = safe_command_unsandboxed("bash", &[])
                    .args(["-c",
                        "rm -f /etc/apt/sources.list.d/sury-php.list \
                               /etc/apt/sources.list.d/ondrej-ubuntu-php-*.sources \
                               /etc/apt/sources.list.d/ondrej-ubuntu-php-*.list; \
                         apt-get update -qq || true"])
                    .output()
                    .await;
                let err = String::from_utf8_lossy(&o.stderr);
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(InstallResponse {
                        success: false,
                        message: format!("Failed to configure PHP repo: {err}"),
                        version: version.to_string(),
                    }),
                ));
            }
            Err(e) => {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(InstallResponse {
                        success: false,
                        message: format!("Failed to configure PHP repo: {e}"),
                        version: version.to_string(),
                    }),
                ));
            }
            Ok(_) => {}
        }
    }

    // Core first (must succeed), then every extension this apt source has a
    // real candidate for. `apt-cache show` is not enough — a package can
    // keep a stanza while its Candidate is "(none)" (php-opcache on Ubuntu
    // 26.04, where OPcache became built-in) and one dead name fails the
    // whole apt transaction.
    let ext_names = COMMON_EXTENSIONS.join(" ");
    let install_cmd = format!(
        "set -e; export DEBIAN_FRONTEND=noninteractive; \
         apt-get install -y -qq php{version}-fpm php{version}-cli php{version}-common; \
         avail=''; \
         for e in {ext_names}; do \
             p=\"php{version}-$e\"; \
             c=$(apt-cache policy \"$p\" 2>/dev/null | sed -n 's/^  Candidate: //p'); \
             if [ -n \"$c\" ] && [ \"$c\" != '(none)' ]; then avail=\"$avail $p\"; fi; \
         done; \
         if [ -n \"$avail\" ]; then apt-get install -y -qq $avail; fi"
    );

    tracing::info!("Installing PHP {version} (core + available extensions)");

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        safe_command_unsandboxed("bash", &[])
            .args(["-c", &install_cmd])
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
    start_fpm(version).await;

    tracing::info!("PHP {version} installed and started");

    Ok(Json(
        settle(version, "was installed with FPM and all available extensions").await,
    ))
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

    // List available (installable) extensions. The query differs per family and
    // so does the prefix to strip: Debian's packages are `php8.3-gd`, the RHEL
    // family's are `php-gd`. Asking apt-cache on a dnf box does not error
    // loudly — `.ok()` swallows it and the operator simply sees an empty list
    // of extensions they could install, with nothing saying why.
    let rpm = crate::services::pkg::manager().await == crate::services::pkg::PkgMgr::Rpm;
    let (prefix, avail_output) = if rpm {
        (
            "php-".to_string(),
            safe_command("dnf")
                .args(["-q", "list", "--available", "php-*"])
                .output()
                .await,
        )
    } else {
        (
            format!("php{version}-"),
            safe_command("apt-cache")
                .args(["search", &format!("php{version}-")])
                .output()
                .await,
        )
    };

    let mut available: Vec<String> = avail_output.ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).lines()
            .filter_map(|l| {
                let pkg = l.split_whitespace().next()?;
                // dnf prints `php-gd.x86_64`; apt-cache prints a bare name.
                let pkg = pkg.split('.').next().unwrap_or(pkg);
                let ext = pkg.strip_prefix(prefix.as_str())?;
                if ext.is_empty() || ["common", "cli", "fpm", "dev", "dbg", "devel"].contains(&ext) {
                    return None;
                }
                Some(ext.to_string())
            })
            .collect())
        .unwrap_or_default();
    available.sort();
    available.dedup();

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

    crate::services::pkg::install(&[&package])
        .await
        .map_err(|e| php_api_err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Install failed: {e}")))?;

    // Restart PHP-FPM under whichever unit name this family uses.
    let unit = crate::services::pkg::service_name(&format!("php{version}-fpm")).await;
    let _ = safe_command("systemctl").args(["restart", &unit]).output().await;

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
    let unit = crate::services::pkg::service_name(&format!("php{version}-fpm")).await;
    let _ = safe_command("systemctl").args(["stop", &unit]).output().await;
    let _ = safe_command("systemctl").args(["disable", &unit]).output().await;

    // 2. Purge all PHP packages for this version. The glob is family-specific
    // and stays out of `pkg::remove`, which translates package NAMES — a glob
    // is not a name, and putting one through the map yields a pattern matching
    // nothing. Debian's packages are versioned; the RHEL family's are not, so
    // removing "this version" there means removing PHP.
    tracing::info!("Uninstalling PHP {version}...");
    let purge_cmd = if crate::services::pkg::manager().await == crate::services::pkg::PkgMgr::Rpm {
        "dnf remove -y 'php-*' 2>&1".to_string()
    } else {
        format!("apt-get purge -y php{version}-* 2>&1")
    };
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        safe_command_unsandboxed("bash", &[])
            .args(["-c", &purge_cmd])
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

    // 3. Autoremove. apt leaves orphans behind after a purge; dnf's `remove`
    // already takes unused dependencies with it, so there is nothing to call.
    if crate::services::pkg::manager().await != crate::services::pkg::PkgMgr::Rpm {
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(300),
            safe_command_unsandboxed("bash", &[])
                .args(["-c", "apt-get autoremove -y 2>&1"])
                .output(),
        )
        .await;
    }

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
