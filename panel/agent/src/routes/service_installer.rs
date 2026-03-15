use axum::{
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use tokio::process::Command;

use super::AppState;

type ApiErr = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: &str) -> ApiErr {
    (status, Json(serde_json::json!({ "error": msg })))
}

fn ok(msg: &str) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ok": true, "message": msg }))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/services/install-status", get(install_status))
        .route("/services/install/php", post(install_php))
        .route("/services/install/certbot", post(install_certbot))
        .route("/services/install/ufw", post(install_ufw))
        .route("/services/install/fail2ban", post(install_fail2ban))
}

// ── Status check ────────────────────────────────────────────────────────

async fn install_status() -> Result<Json<serde_json::Value>, ApiErr> {
    let php_installed = is_installed("php-fpm").await || is_installed("php8.3-fpm").await || is_installed("php8.2-fpm").await || is_installed("php8.1-fpm").await;
    let php_running = is_active("php8.3-fpm").await || is_active("php8.2-fpm").await || is_active("php8.1-fpm").await;
    let certbot_installed = which("certbot").await;
    let ufw_installed = which("ufw").await;
    let ufw_active = is_ufw_active().await;
    let fail2ban_installed = is_installed("fail2ban").await;
    let fail2ban_running = is_active("fail2ban").await;

    // Detect installed PHP version
    let php_version = detect_php_version().await;

    Ok(Json(serde_json::json!({
        "php": { "installed": php_installed, "running": php_running, "version": php_version },
        "certbot": { "installed": certbot_installed },
        "ufw": { "installed": ufw_installed, "active": ufw_active },
        "fail2ban": { "installed": fail2ban_installed, "running": fail2ban_running },
    })))
}

// ── PHP installer ───────────────────────────────────────────────────────

async fn install_php() -> Result<Json<serde_json::Value>, ApiErr> {
    tracing::info!("Installing PHP...");

    // Detect best PHP version available
    let version = detect_available_php().await.unwrap_or_else(|| "8.3".to_string());

    let packages = format!(
        "php{v}-fpm php{v}-cli php{v}-mysql php{v}-pgsql php{v}-sqlite3 \
         php{v}-curl php{v}-gd php{v}-mbstring php{v}-xml php{v}-zip \
         php{v}-bcmath php{v}-intl php{v}-readline php{v}-opcache",
        v = version
    );

    let output = Command::new("sh")
        .args(["-c", &format!("DEBIAN_FRONTEND=noninteractive apt-get install -y {packages}")])
        .output()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("apt install failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!("PHP install failed: {}", stderr.chars().take(300).collect::<String>())));
    }

    // Enable and start PHP-FPM
    let _ = Command::new("systemctl").args(["enable", &format!("php{version}-fpm")]).output().await;
    let _ = Command::new("systemctl").args(["start", &format!("php{version}-fpm")]).output().await;

    tracing::info!("PHP {version} installed");
    Ok(ok(&format!("PHP {version} with FPM installed and started")))
}

// ── Certbot installer ───────────────────────────────────────────────────

async fn install_certbot() -> Result<Json<serde_json::Value>, ApiErr> {
    tracing::info!("Installing Certbot...");

    let output = Command::new("sh")
        .args(["-c", "DEBIAN_FRONTEND=noninteractive apt-get install -y certbot python3-certbot-nginx"])
        .output()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("apt install failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Certbot install failed: {}", stderr.chars().take(300).collect::<String>())));
    }

    // Set up auto-renewal timer
    let _ = Command::new("systemctl").args(["enable", "certbot.timer"]).output().await;
    let _ = Command::new("systemctl").args(["start", "certbot.timer"]).output().await;

    tracing::info!("Certbot installed with auto-renewal");
    Ok(ok("Certbot installed with nginx plugin and auto-renewal timer"))
}

// ── UFW installer ───────────────────────────────────────────────────────

async fn install_ufw() -> Result<Json<serde_json::Value>, ApiErr> {
    tracing::info!("Installing UFW...");

    let output = Command::new("sh")
        .args(["-c", "DEBIAN_FRONTEND=noninteractive apt-get install -y ufw"])
        .output()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("apt install failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!("UFW install failed: {}", stderr.chars().take(300).collect::<String>())));
    }

    // Configure default rules
    let _ = Command::new("ufw").args(["default", "deny", "incoming"]).output().await;
    let _ = Command::new("ufw").args(["default", "allow", "outgoing"]).output().await;

    // Open essential ports
    let _ = Command::new("ufw").args(["allow", "22/tcp"]).output().await;   // SSH
    let _ = Command::new("ufw").args(["allow", "80/tcp"]).output().await;   // HTTP
    let _ = Command::new("ufw").args(["allow", "443/tcp"]).output().await;  // HTTPS
    let _ = Command::new("ufw").args(["allow", "587/tcp"]).output().await;  // SMTP submission
    let _ = Command::new("ufw").args(["allow", "993/tcp"]).output().await;  // IMAPS

    // Enable (--force to skip interactive prompt)
    let _ = Command::new("ufw").args(["--force", "enable"]).output().await;

    tracing::info!("UFW installed and enabled with default rules");
    Ok(ok("UFW installed — SSH, HTTP, HTTPS, SMTP, IMAPS ports opened"))
}

// ── Fail2Ban installer ──────────────────────────────────────────────────

async fn install_fail2ban() -> Result<Json<serde_json::Value>, ApiErr> {
    tracing::info!("Installing Fail2Ban...");

    let output = Command::new("sh")
        .args(["-c", "DEBIAN_FRONTEND=noninteractive apt-get install -y fail2ban"])
        .output()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("apt install failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Fail2Ban install failed: {}", stderr.chars().take(300).collect::<String>())));
    }

    // Write default jail config
    let jail_config = r#"[DEFAULT]
bantime = 3600
findtime = 600
maxretry = 5

[sshd]
enabled = true

[nginx-http-auth]
enabled = true

[nginx-limit-req]
enabled = true

[postfix]
enabled = true

[dovecot]
enabled = true
"#;

    let _ = tokio::fs::write("/etc/fail2ban/jail.local", jail_config).await;

    let _ = Command::new("systemctl").args(["enable", "fail2ban"]).output().await;
    let _ = Command::new("systemctl").args(["restart", "fail2ban"]).output().await;

    tracing::info!("Fail2Ban installed with default jails");
    Ok(ok("Fail2Ban installed with SSH, Nginx, Postfix, Dovecot jails"))
}

// ── Helpers ─────────────────────────────────────────────────────────────

async fn is_installed(package: &str) -> bool {
    Command::new("dpkg")
        .args(["-l", package])
        .output()
        .await
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains("ii"))
        .unwrap_or(false)
}

async fn is_active(service: &str) -> bool {
    Command::new("systemctl")
        .args(["is-active", "--quiet", service])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

async fn which(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

async fn is_ufw_active() -> bool {
    Command::new("ufw")
        .arg("status")
        .output()
        .await
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("Status: active"))
        .unwrap_or(false)
}

async fn detect_php_version() -> Option<String> {
    let output = Command::new("php").args(["-r", "echo PHP_MAJOR_VERSION.'.'.PHP_MINOR_VERSION;"]).output().await.ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

async fn detect_available_php() -> Option<String> {
    for v in ["8.3", "8.2", "8.1"] {
        let output = Command::new("apt-cache")
            .args(["show", &format!("php{v}-fpm")])
            .output()
            .await;
        if output.map(|o| o.status.success()).unwrap_or(false) {
            return Some(v.to_string());
        }
    }
    None
}
