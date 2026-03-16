use axum::{
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use std::time::Duration;
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
        .route("/services/install/powerdns", post(install_powerdns))
}

// ── Status check ────────────────────────────────────────────────────────

async fn install_status() -> Result<Json<serde_json::Value>, ApiErr> {
    let pdns_installed = is_installed("pdns-server").await;
    let pdns_running = is_active("pdns").await;

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
        "powerdns": { "installed": pdns_installed, "running": pdns_running },
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

    let output = tokio::time::timeout(
        Duration::from_secs(300),
        Command::new("sh")
            .args(["-c", &format!("DEBIAN_FRONTEND=noninteractive apt-get -o Dpkg::Options::=--force-confnew install -y {packages}")])
            .output()
    ).await
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "PHP install timed out after 300s"))?
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("apt install failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!("PHP install failed: {}", stderr.chars().take(300).collect::<String>())));
    }

    // Enable and start PHP-FPM
    let _ = tokio::time::timeout(Duration::from_secs(120), Command::new("systemctl").args(["enable", &format!("php{version}-fpm")]).output()).await;
    let _ = tokio::time::timeout(Duration::from_secs(120), Command::new("systemctl").args(["start", &format!("php{version}-fpm")]).output()).await;

    tracing::info!("PHP {version} installed");
    Ok(ok(&format!("PHP {version} with FPM installed and started")))
}

// ── Certbot installer ───────────────────────────────────────────────────

async fn install_certbot() -> Result<Json<serde_json::Value>, ApiErr> {
    tracing::info!("Installing Certbot...");

    let output = tokio::time::timeout(
        Duration::from_secs(300),
        Command::new("sh")
            .args(["-c", "DEBIAN_FRONTEND=noninteractive apt-get -o Dpkg::Options::=--force-confnew install -y certbot python3-certbot-nginx"])
            .output()
    ).await
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Certbot install timed out after 300s"))?
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("apt install failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Certbot install failed: {}", stderr.chars().take(300).collect::<String>())));
    }

    // Set up auto-renewal timer
    let _ = tokio::time::timeout(Duration::from_secs(120), Command::new("systemctl").args(["enable", "certbot.timer"]).output()).await;
    let _ = tokio::time::timeout(Duration::from_secs(120), Command::new("systemctl").args(["start", "certbot.timer"]).output()).await;

    tracing::info!("Certbot installed with auto-renewal");
    Ok(ok("Certbot installed with nginx plugin and auto-renewal timer"))
}

// ── UFW installer ───────────────────────────────────────────────────────

async fn install_ufw() -> Result<Json<serde_json::Value>, ApiErr> {
    tracing::info!("Installing UFW...");

    let output = tokio::time::timeout(
        Duration::from_secs(300),
        Command::new("sh")
            .args(["-c", "DEBIAN_FRONTEND=noninteractive apt-get -o Dpkg::Options::=--force-confnew install -y ufw"])
            .output()
    ).await
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "UFW install timed out after 300s"))?
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("apt install failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!("UFW install failed: {}", stderr.chars().take(300).collect::<String>())));
    }

    // Configure default rules
    let _ = tokio::time::timeout(Duration::from_secs(120), Command::new("ufw").args(["default", "deny", "incoming"]).output()).await;
    let _ = tokio::time::timeout(Duration::from_secs(120), Command::new("ufw").args(["default", "allow", "outgoing"]).output()).await;

    // Open essential ports
    let _ = tokio::time::timeout(Duration::from_secs(120), Command::new("ufw").args(["allow", "22/tcp"]).output()).await;   // SSH
    let _ = tokio::time::timeout(Duration::from_secs(120), Command::new("ufw").args(["allow", "80/tcp"]).output()).await;   // HTTP
    let _ = tokio::time::timeout(Duration::from_secs(120), Command::new("ufw").args(["allow", "443/tcp"]).output()).await;  // HTTPS
    let _ = tokio::time::timeout(Duration::from_secs(120), Command::new("ufw").args(["allow", "587/tcp"]).output()).await;  // SMTP submission
    let _ = tokio::time::timeout(Duration::from_secs(120), Command::new("ufw").args(["allow", "993/tcp"]).output()).await;  // IMAPS

    // Enable (--force to skip interactive prompt)
    let _ = tokio::time::timeout(Duration::from_secs(120), Command::new("ufw").args(["--force", "enable"]).output()).await;

    tracing::info!("UFW installed and enabled with default rules");
    Ok(ok("UFW installed — SSH, HTTP, HTTPS, SMTP, IMAPS ports opened"))
}

// ── Fail2Ban installer ──────────────────────────────────────────────────

async fn install_fail2ban() -> Result<Json<serde_json::Value>, ApiErr> {
    tracing::info!("Installing Fail2Ban...");

    let output = tokio::time::timeout(
        Duration::from_secs(300),
        Command::new("sh")
            .args(["-c", "DEBIAN_FRONTEND=noninteractive apt-get -o Dpkg::Options::=--force-confnew install -y fail2ban"])
            .output()
    ).await
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Fail2Ban install timed out after 300s"))?
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

    let _ = tokio::time::timeout(Duration::from_secs(120), Command::new("systemctl").args(["enable", "fail2ban"]).output()).await;
    let _ = tokio::time::timeout(Duration::from_secs(120), Command::new("systemctl").args(["restart", "fail2ban"]).output()).await;

    tracing::info!("Fail2Ban installed with default jails");
    Ok(ok("Fail2Ban installed with SSH, Nginx, Postfix, Dovecot jails"))
}

// ── PowerDNS installer ──────────────────────────────────────────────────

async fn install_powerdns() -> Result<Json<serde_json::Value>, ApiErr> {
    tracing::info!("Installing PowerDNS...");

    // 1. Install packages
    let output = tokio::time::timeout(
        Duration::from_secs(300),
        Command::new("sh")
            .args(["-c", "DEBIAN_FRONTEND=noninteractive apt-get -o Dpkg::Options::=--force-confnew install -y pdns-server pdns-backend-pgsql"])
            .output()
    ).await
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "PowerDNS install timed out after 300s"))?
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("apt install failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!("PowerDNS install failed: {}", stderr.chars().take(300).collect::<String>())));
    }

    // 2. Create a PostgreSQL database for PowerDNS using the existing panel DB container
    let db_exists = tokio::time::timeout(
        Duration::from_secs(120),
        Command::new("docker")
            .args(["exec", "dockpanel-postgres", "psql", "-U", "dockpanel", "-lqt"])
            .output()
    ).await
        .ok()
        .and_then(|r| r.ok())
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("pdns"))
        .unwrap_or(false);

    if !db_exists {
        let _ = tokio::time::timeout(
            Duration::from_secs(120),
            Command::new("docker")
                .args(["exec", "dockpanel-postgres", "psql", "-U", "dockpanel", "-c", "CREATE DATABASE pdns;"])
                .output()
        ).await;

        // Load PowerDNS schema
        let schema_path = "/usr/share/doc/pdns-backend-pgsql/schema.pgsql.sql";
        if tokio::fs::metadata(schema_path).await.is_ok() {
            // Use shell pipe to feed schema to psql
            let _ = tokio::time::timeout(
                Duration::from_secs(120),
                Command::new("sh")
                    .args(["-c", &format!("cat {} | docker exec -i dockpanel-postgres psql -U dockpanel -d pdns", schema_path)])
                    .output()
            ).await;
        }
    }

    // 3. Generate API key
    let api_key: String = {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        (0..32).map(|_| rng.sample(rand::distributions::Alphanumeric) as char).collect()
    };

    // 4. Write PowerDNS config
    let pdns_conf = format!(r#"# DockPanel PowerDNS configuration
launch=gpgsql
gpgsql-host=127.0.0.1
gpgsql-port=5450
gpgsql-dbname=pdns
gpgsql-user=dockpanel
gpgsql-password=dockpanel2026

# HTTP API
api=yes
api-key={api_key}
webserver=yes
webserver-address=127.0.0.1
webserver-port=8081
webserver-allow-from=127.0.0.1

# SOA defaults
default-soa-content=ns1.@ hostmaster.@ 0 10800 3600 604800 3600
"#);

    tokio::fs::write("/etc/powerdns/pdns.conf", &pdns_conf).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write pdns.conf: {e}")))?;

    // 5. Enable and start
    let _ = tokio::time::timeout(Duration::from_secs(120), Command::new("systemctl").args(["enable", "pdns"]).output()).await;
    let _ = tokio::time::timeout(Duration::from_secs(120), Command::new("systemctl").args(["restart", "pdns"]).output()).await;

    tracing::info!("PowerDNS installed with API key");

    Ok(Json(serde_json::json!({
        "ok": true,
        "message": "PowerDNS installed and configured",
        "api_url": "http://127.0.0.1:8081",
        "api_key": api_key,
    })))
}

// ── Helpers ─────────────────────────────────────────────────────────────

async fn is_installed(package: &str) -> bool {
    tokio::time::timeout(
        Duration::from_secs(120),
        Command::new("dpkg").args(["-l", package]).output()
    ).await
        .ok()
        .and_then(|r| r.ok())
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains("ii"))
        .unwrap_or(false)
}

async fn is_active(service: &str) -> bool {
    tokio::time::timeout(
        Duration::from_secs(120),
        Command::new("systemctl").args(["is-active", "--quiet", service]).output()
    ).await
        .ok()
        .and_then(|r| r.ok())
        .map(|o| o.status.success())
        .unwrap_or(false)
}

async fn which(cmd: &str) -> bool {
    tokio::time::timeout(
        Duration::from_secs(120),
        Command::new("which").arg(cmd).output()
    ).await
        .ok()
        .and_then(|r| r.ok())
        .map(|o| o.status.success())
        .unwrap_or(false)
}

async fn is_ufw_active() -> bool {
    tokio::time::timeout(
        Duration::from_secs(120),
        Command::new("ufw").arg("status").output()
    ).await
        .ok()
        .and_then(|r| r.ok())
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("Status: active"))
        .unwrap_or(false)
}

async fn detect_php_version() -> Option<String> {
    let output = tokio::time::timeout(
        Duration::from_secs(120),
        Command::new("php").args(["-r", "echo PHP_MAJOR_VERSION.'.'.PHP_MINOR_VERSION;"]).output()
    ).await.ok()?.ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

async fn detect_available_php() -> Option<String> {
    for v in ["8.3", "8.2", "8.1"] {
        let output = tokio::time::timeout(
            Duration::from_secs(120),
            Command::new("apt-cache").args(["show", &format!("php{v}-fpm")]).output()
        ).await;
        if output.ok().and_then(|r| r.ok()).map(|o| o.status.success()).unwrap_or(false) {
            return Some(v.to_string());
        }
    }
    None
}
