use axum::{
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use std::time::Duration;
use crate::safe_cmd::{safe_command, safe_command_unsandboxed};

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
        .route("/services/install/redis", post(install_redis))
        .route("/services/install/nodejs", post(install_nodejs))
        .route("/services/install/composer", post(install_composer))
        .route("/services/install/sshpass", post(install_sshpass))
        .route("/services/uninstall/php", post(uninstall_php))
        .route("/services/uninstall/certbot", post(uninstall_certbot))
        .route("/services/uninstall/ufw", post(uninstall_ufw))
        .route("/services/uninstall/fail2ban", post(uninstall_fail2ban))
        .route("/services/uninstall/powerdns", post(uninstall_powerdns))
        .route("/services/uninstall/redis", post(uninstall_redis))
        .route("/services/uninstall/nodejs", post(uninstall_nodejs))
        .route("/services/uninstall/composer", post(uninstall_composer))
        .route("/services/install/waf", post(install_waf))
        .route("/services/uninstall/waf", post(uninstall_waf))
        .route("/services/install/cloudflared", post(install_cloudflared))
        .route("/services/uninstall/cloudflared", post(uninstall_cloudflared))
        .route("/services/cloudflared/configure", post(configure_cloudflared))
        .route("/services/cloudflared/status", get(cloudflared_status))
}

// ── Status check ────────────────────────────────────────────────────────

async fn install_status() -> Result<Json<serde_json::Value>, ApiErr> {
    let pdns_installed = is_installed("pdns-server").await;
    let pdns_running = is_active("pdns").await;

    // Keep these version lists in lock-step with php.rs ALLOWED_VERSIONS — both
    // installer paths (setup.sh, php.rs, this file) install `php{ver}-fpm`
    // directly (never the `php-fpm` metapackage), so a missing version here
    // reports an installed+running PHP as absent on the Services page.
    let php_installed = is_installed("php-fpm").await || is_installed("php8.5-fpm").await || is_installed("php8.4-fpm").await || is_installed("php8.3-fpm").await || is_installed("php8.2-fpm").await || is_installed("php8.1-fpm").await;
    // Debian names the unit per version (`php8.3-fpm`); the RHEL family has a
    // single `php-fpm`. Checking only the versioned names reported an active
    // PHP as stopped on every RPM box (s265) — the same shape as the dpkg
    // problem one line above, in the service manager instead of the package
    // manager.
    let php_running = is_active("php8.5-fpm").await || is_active("php8.4-fpm").await || is_active("php8.3-fpm").await || is_active("php8.2-fpm").await || is_active("php8.1-fpm").await
        || is_active("php-fpm").await;
    let certbot_installed = which("certbot").await;
    let ufw_installed = which("ufw").await;
    let ufw_active = is_ufw_active().await;
    let fail2ban_installed = is_installed("fail2ban").await;
    let fail2ban_running = is_active("fail2ban").await;

    // Detect installed PHP version
    let php_version = detect_php_version().await;

    let redis_installed = which("redis-server").await || is_installed("redis-server").await;
    let redis_running = is_active("redis-server").await;

    let nodejs_installed = which("node").await;
    let composer_installed = which("composer").await;

    // Ubuntu 24.04 (Noble) renamed libmodsecurity3 → libmodsecurity3t64 in the
    // time_t-64 ABI transition (virtual-provides, no transitional shim). Check
    // both names so the install-state detect works on Noble and Debian alike.
    let waf_installed = std::path::Path::new("/etc/modsecurity/modsecurity.conf").exists()
        && (is_installed("libmodsecurity3").await
            || is_installed("libmodsecurity3t64").await);

    let cloudflared_installed = which("cloudflared").await;
    let cloudflared_running = is_active("cloudflared").await;
    // No unit to probe: sshpass is a binary the backup path execs, not a daemon.
    let sshpass_installed = which("sshpass").await;

    Ok(Json(serde_json::json!({
        "php": { "installed": php_installed, "running": php_running, "version": php_version },
        "certbot": { "installed": certbot_installed },
        "ufw": { "installed": ufw_installed, "active": ufw_active },
        "fail2ban": { "installed": fail2ban_installed, "running": fail2ban_running },
        "powerdns": { "installed": pdns_installed, "running": pdns_running },
        "redis": { "installed": redis_installed, "running": redis_running },
        "nodejs": { "installed": nodejs_installed },
        "composer": { "installed": composer_installed },
        "waf": { "installed": waf_installed },
        "cloudflared": { "installed": cloudflared_installed, "running": cloudflared_running },
        "sshpass": { "installed": sshpass_installed },
    })))
}

// ── PHP installer ───────────────────────────────────────────────────────

async fn install_php() -> Result<Json<serde_json::Value>, ApiErr> {
    use crate::services::pkg;

    if let Some(why) = pkg::no_installer_reason("Installing PHP").await {
        return Err(err(StatusCode::NOT_IMPLEMENTED, &why));
    }
    tracing::info!("Installing PHP...");

    // Version selection is the one part that genuinely differs by family.
    // apt picks a version out of the archive; dnf resolves `php-fpm` to the
    // NON-modular base package unless a module stream is enabled first — which
    // on Rocky 9 means PHP 8.0, older than every stream the box offers and
    // end-of-life since 2023, installed silently under a green status (s266).
    let streams = pkg::php_streams().await;
    let version = if streams.is_empty() {
        detect_available_php().await.unwrap_or_else(|| "8.3".to_string())
    } else {
        // Honour the distro's newest stream. `detect_available_php` probes
        // apt-cache and cannot answer here, so it is not consulted.
        let chosen = streams[0].clone();
        pkg::enable_php_stream(&chosen).await.map_err(|e| {
            err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Could not select PHP {chosen}: {e}"))
        })?;
        chosen
    };

    // Core first (must succeed), then only the extensions this box has a real
    // candidate for. Both families need that split: one uninstallable name
    // fails the ENTIRE transaction on apt (Lesson #73) and equally on dnf,
    // which answers a missing package with "Unable to find a match" and
    // installs nothing.
    let core = [format!("php{version}-fpm"), format!("php{version}-cli")];
    let core_refs: Vec<&str> = core.iter().map(String::as_str).collect();
    pkg::install(&core_refs).await.map_err(|e| {
        err(StatusCode::INTERNAL_SERVER_ERROR, &format!("PHP install failed: {e}"))
    })?;

    let exts: Vec<String> = [
        "mysql", "pgsql", "sqlite3", "curl", "gd", "mbstring",
        "xml", "zip", "bcmath", "intl", "readline", "opcache",
    ]
    .iter()
    .map(|e| format!("php{version}-{e}"))
    .collect();
    let ext_refs: Vec<&str> = exts.iter().map(String::as_str).collect();
    let skipped = pkg::install_available(&ext_refs).await.map_err(|e| {
        err(StatusCode::INTERNAL_SERVER_ERROR, &format!("PHP extension install failed: {e}"))
    })?;
    if !skipped.is_empty() {
        // Not a failure: newer PHP builds some of these in (8.5 absorbed
        // opcache) and the RHEL family provides curl/sqlite3/json from
        // php-common. Logged so an operator asking "where is my zip extension"
        // has an answer.
        tracing::info!("PHP extensions with no candidate here, skipped: {}", skipped.join(" "));
    }

    // The UNIT name is a separate translation from the PACKAGE name — on the
    // RHEL family the package set is unversioned and so is the unit, so
    // enabling `php8.3-fpm` there would enable nothing at all.
    let unit = pkg::service_name(&format!("php{version}-fpm")).await;
    let _ = tokio::time::timeout(Duration::from_secs(120), safe_command("systemctl").args(["enable", &unit]).output()).await;
    let _ = tokio::time::timeout(Duration::from_secs(120), safe_command("systemctl").args(["start", &unit]).output()).await;

    tracing::info!("PHP {version} installed (unit {unit})");
    Ok(ok(&format!("PHP {version} with FPM installed and started")))
}

// ── Certbot installer ───────────────────────────────────────────────────

async fn install_certbot() -> Result<Json<serde_json::Value>, ApiErr> {
    use crate::services::pkg;

    if let Some(why) = pkg::no_installer_reason("Installing Certbot").await {
        return Err(err(StatusCode::NOT_IMPLEMENTED, &why));
    }
    tracing::info!("Installing Certbot...");

    // On the RHEL family EPEL packages certbot and its nginx plugin directly.
    // The snap-then-pip ladder below is a Debian/Ubuntu strategy for getting a
    // 4.x certbot with ARI support; snap is not the idiomatic route here, and
    // the whole ladder was unreachable anyway because the guard refused before
    // any of it ran — this refusal was over-broad, since its only apt call is
    // `python3-venv` inside the *fallback*.
    if pkg::manager().await == pkg::PkgMgr::Rpm {
        pkg::install(&["certbot", "python3-certbot-nginx"]).await.map_err(|e| {
            err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Certbot install failed: {e}"))
        })?;
        // EPEL ships a renewal timer with the package.
        let _ = tokio::time::timeout(Duration::from_secs(30),
            safe_command("systemctl").args(["enable", "--now", "certbot-renew.timer"]).output()).await;
        tracing::info!("Certbot installed from EPEL with the nginx plugin");
        return Ok(ok("Certbot installed with the nginx plugin and automatic renewal"));
    }

    // Remove old apt-based certbot first (if present) to avoid conflicts
    let _ = tokio::time::timeout(
        Duration::from_secs(120),
        safe_command_unsandboxed("sh", &[])
            .args(["-c", "systemctl stop certbot.timer 2>/dev/null; systemctl disable certbot.timer 2>/dev/null; DEBIAN_FRONTEND=noninteractive apt-get purge -y certbot python3-certbot-nginx 2>/dev/null; true"])
            .output()
    ).await;

    // Strategy: snap (gets certbot 4.x with ARI support for 45-day certs)
    // Fallback: pip (works when snap is unavailable)
    let snap_ok = tokio::time::timeout(
        Duration::from_secs(300),
        safe_command_unsandboxed("sh", &[])
            .args(["-c", "snap install --classic certbot && ln -sf /snap/bin/certbot /usr/bin/certbot"])
            .output()
    ).await
        .ok()
        .and_then(|r| r.ok())
        .map(|o| o.status.success())
        .unwrap_or(false);

    // Install nginx plugin separately (non-fatal if it fails — certbot still works)
    if snap_ok {
        let _ = tokio::time::timeout(
            Duration::from_secs(120),
            safe_command_unsandboxed("sh", &[])
                .args(["-c", "snap set certbot trust-plugin-with-root=ok && snap install certbot-nginx"])
                .output()
        ).await;
    }

    if snap_ok {
        tracing::info!("Certbot installed via snap (4.x with ARI support)");
        // snap auto-renewal runs via snap.certbot.renew.timer
        let _ = tokio::time::timeout(Duration::from_secs(30),
            safe_command("systemctl").args(["enable", "snap.certbot.renew.timer"]).output()).await;
        let _ = tokio::time::timeout(Duration::from_secs(30),
            safe_command("systemctl").args(["start", "snap.certbot.renew.timer"]).output()).await;
        return Ok(ok("Certbot 4.x installed via snap with nginx plugin and auto-renewal (ARI-ready for 45-day certs)"));
    }

    tracing::warn!("Snap certbot failed, falling back to pip...");

    // Fallback: pip install (gets latest certbot from PyPI)
    let pip_ok = tokio::time::timeout(
        Duration::from_secs(300),
        safe_command_unsandboxed("sh", &[])
            .args(["-c", "apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y python3-venv && \
                python3 -m venv /opt/certbot && \
                /opt/certbot/bin/pip install --upgrade pip && \
                /opt/certbot/bin/pip install certbot certbot-nginx && \
                ln -sf /opt/certbot/bin/certbot /usr/bin/certbot"])
            .output()
    ).await
        .ok()
        .and_then(|r| r.ok())
        .map(|o| o.status.success())
        .unwrap_or(false);

    if pip_ok {
        tracing::info!("Certbot installed via pip");
        // Create systemd timer for auto-renewal
        let timer_unit = "[Unit]\nDescription=Certbot renewal timer\n\n[Timer]\nOnCalendar=*-*-* 00,12:00:00\nRandomizedDelaySec=3600\nPersistent=true\n\n[Install]\nWantedBy=timers.target\n";
        let service_unit = "[Unit]\nDescription=Certbot renewal\n\n[Service]\nType=oneshot\nExecStart=/usr/bin/certbot renew --quiet --deploy-hook \"systemctl reload nginx\"\n";
        std::fs::write("/etc/systemd/system/certbot.timer", timer_unit).ok();
        std::fs::write("/etc/systemd/system/certbot.service", service_unit).ok();
        let _ = tokio::time::timeout(Duration::from_secs(30), safe_command("systemctl").args(["daemon-reload"]).output()).await;
        let _ = tokio::time::timeout(Duration::from_secs(30), safe_command("systemctl").args(["enable", "certbot.timer"]).output()).await;
        let _ = tokio::time::timeout(Duration::from_secs(30), safe_command("systemctl").args(["start", "certbot.timer"]).output()).await;
        return Ok(ok("Certbot installed via pip with nginx plugin and auto-renewal timer"));
    }

    Err(err(StatusCode::INTERNAL_SERVER_ERROR, "Certbot install failed: both snap and pip methods failed"))
}

// ── UFW installer ───────────────────────────────────────────────────────

/// Install UFW — on Debian/Ubuntu only, **deliberately**.
///
/// This is the one installer s266 did NOT give a `dnf` path, and the reason is
/// the s265 outage: the RHEL family boots with **firewalld** enforcing, and
/// installing UFW beside it produces two rule sets where the one nobody
/// configured is the one the kernel consults. That is how a box ended up with
/// 80/443 "open" in ufw, unreachable in practice, and no certificate — because
/// Let's Encrypt could not fetch the ACME challenge either.
///
/// UFW *is* installable from EPEL, so "the package exists" is true and beside
/// the point. Ports are opened through [`crate::services::firewall`], which
/// speaks to whichever filter is actually running, so refusing costs nothing.
async fn install_ufw() -> Result<Json<serde_json::Value>, ApiErr> {
    use crate::services::pkg;

    if let Some(why) = pkg::no_installer_reason("Installing UFW").await {
        return Err(err(StatusCode::NOT_IMPLEMENTED, &why));
    }
    if let Some(why) = pkg::ufw_refusal_reason().await {
        return Err(err(StatusCode::NOT_IMPLEMENTED, &why));
    }
    tracing::info!("Installing UFW...");

    pkg::install(&["ufw"]).await.map_err(|e| {
        err(StatusCode::INTERNAL_SERVER_ERROR, &format!("UFW install failed: {e}"))
    })?;

    // Configure default rules
    let _ = tokio::time::timeout(Duration::from_secs(120), safe_command("ufw").args(["default", "deny", "incoming"]).output()).await;
    let _ = tokio::time::timeout(Duration::from_secs(120), safe_command("ufw").args(["default", "allow", "outgoing"]).output()).await;

    // Open essential ports. Routed through the firewall service rather than
    // `ufw` directly so the result is a fact we can report instead of assume —
    // `open_mail_ports()` used to discard every result and log success anyway.
    let failed = crate::services::firewall::allow_tcp_ports(
        &["22", "80", "443", "587", "993"],
    ).await;

    // Enable (--force to skip interactive prompt)
    let _ = tokio::time::timeout(Duration::from_secs(120), safe_command("ufw").args(["--force", "enable"]).output()).await;

    if !failed.is_empty() {
        tracing::warn!("UFW installed but these ports could not be opened: {}", failed.join(" "));
        return Ok(ok(&format!(
            "UFW installed, but these ports could not be opened: {}",
            failed.join(", ")
        )));
    }

    tracing::info!("UFW installed and enabled with default rules");
    Ok(ok("UFW installed — SSH, HTTP, HTTPS, SMTP, IMAPS ports opened"))
}

// ── Fail2Ban installer ──────────────────────────────────────────────────

async fn install_fail2ban() -> Result<Json<serde_json::Value>, ApiErr> {
    use crate::services::pkg;

    if let Some(why) = pkg::no_installer_reason("Installing Fail2Ban").await {
        return Err(err(StatusCode::NOT_IMPLEMENTED, &why));
    }
    tracing::info!("Installing Fail2Ban...");

    pkg::install(&["fail2ban"]).await.map_err(|e| {
        err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Fail2Ban install failed: {e}"))
    })?;

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

    let _ = tokio::time::timeout(Duration::from_secs(120), safe_command("systemctl").args(["enable", "fail2ban"]).output()).await;
    let _ = tokio::time::timeout(Duration::from_secs(120), safe_command("systemctl").args(["restart", "fail2ban"]).output()).await;

    tracing::info!("Fail2Ban installed with default jails");
    Ok(ok("Fail2Ban installed with SSH, Nginx, Postfix, Dovecot jails"))
}

// ── PowerDNS installer ──────────────────────────────────────────────────

/// Addresses pdns should bind, or `None` to leave PowerDNS on its `0.0.0.0`
/// default.
///
/// Ubuntu runs systemd-resolved, whose stub listeners own `127.0.0.53:53` and
/// `127.0.0.54:53`. A wildcard bind therefore fails with EADDRINUSE and pdns
/// crash-loops, so when port 53 is already spoken for we pin pdns to the real
/// interface addresses and leave the stub resolver alone. Distros without a
/// stub listener (Debian 12 ships none) keep the wildcard bind, which also
/// keeps pdns listening on addresses added after install.
async fn pdns_local_addresses() -> Option<String> {
    // apt's postinst starts pdns with the stock config, so pdns may be holding
    // :53 itself by now. Ignore its own sockets — only a *foreign* listener
    // means the wildcard bind is unavailable.
    let listeners = safe_command("ss").args(["-tulnpH", "sport", "=", ":53"]).output().await
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    let busy = listeners
        .lines()
        .any(|l| !l.trim().is_empty() && !l.contains("pdns"));
    if !busy {
        return None;
    }

    let out = safe_command("ip").args(["-o", "addr", "show", "scope", "global"]).output().await.ok()?;
    let mut addrs: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split_whitespace().nth(3))
        .filter_map(|cidr| cidr.split('/').next())
        .map(str::to_string)
        .collect();
    if addrs.is_empty() {
        return None;
    }
    // Loopback stays reachable for local tooling; 127.0.0.1 never collides with
    // the stub listeners, which sit on .53/.54.
    addrs.push("127.0.0.1".to_string());
    Some(addrs.join(","))
}

/// The panel's PostgreSQL password, parsed out of the API's env file.
///
/// `setup.sh` writes `DATABASE_URL=postgresql://dockpanel:<pw>@127.0.0.1:<port>/dockpanel`
/// to `/etc/dockpanel/api.env` (mode 600, and the agent runs as root). Env vars
/// are checked first so a non-standard deployment can still override.
async fn panel_db_password() -> Option<String> {
    fn password_from_url(url: &str) -> Option<String> {
        let creds = url.split("://").nth(1)?.split('@').next()?;
        let pw = creds.split(':').nth(1)?;
        (!pw.is_empty()).then(|| pw.to_string())
    }

    if let Ok(pw) = std::env::var("PANEL_DB_PASSWORD") {
        if !pw.is_empty() {
            return Some(pw);
        }
    }
    if let Some(pw) = std::env::var("DATABASE_URL").ok().as_deref().and_then(password_from_url) {
        return Some(pw);
    }

    let env_file = tokio::fs::read_to_string("/etc/dockpanel/api.env").await.ok()?;
    env_file
        .lines()
        .find_map(|l| l.trim().strip_prefix("DATABASE_URL="))
        .map(|v| v.trim_matches(['"', '\''].as_ref()))
        .and_then(password_from_url)
}

/// Whether pdns settled into `active`.
///
/// A failing pdns is restarted by systemd, so a single probe can catch it
/// mid-`activating` and read as healthy. Poll until it is unambiguously up or
/// the budget runs out.
async fn pdns_is_active() -> bool {
    for _ in 0..10 {
        let state = safe_command("systemctl").args(["is-active", "pdns"]).output().await
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        if state == "active" {
            return true;
        }
        if state == "failed" || state == "inactive" {
            return false;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    false
}

/// The most useful line from pdns's journal, for the operator-facing error.
async fn pdns_failure_detail() -> String {
    let journal = safe_command("journalctl")
        .args(["-u", "pdns", "--no-pager", "-n", "40", "-o", "cat"])
        .output().await
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();

    journal
        .lines()
        .rev()
        .find(|l| l.contains("Fatal error") || l.contains("Unable to bind") || l.contains("error"))
        .map(|l| l.trim().chars().take(300).collect::<String>())
        .unwrap_or_else(|| "check `journalctl -u pdns` for details".to_string())
}

/// Write `/etc/powerdns/pdns.conf` through the unsandboxed escape hatch.
///
/// `/etc/powerdns` is in the agent unit's `ReadWritePaths`, and systemd creates
/// it at unit start — but `apt-get purge pdns-server` (see the uninstaller)
/// deletes the directory, which detaches that bind mount for the remaining life
/// of the agent process. A direct `tokio::fs::write` then fails EROFS on every
/// reinstall until the agent restarts. Staging through `/var/lib/dockpanel` (a
/// live ReadWritePaths entry) and moving the file into place with `install`
/// sidesteps the detached mount entirely — and keeps the embedded API key off
/// both the process argv and a world-readable config.
async fn write_pdns_conf(contents: &str) -> Result<(), ApiErr> {
    const STAGED: &str = "/var/lib/dockpanel/pdns.conf.staged";

    // The staged copy carries the generated API key (and, on the pgsql path, the
    // database password), so create it 0600 rather than letting the umask decide
    // — and remove it on every exit path, not just the happy one.
    let staged_result = stage_and_install_pdns_conf(STAGED, contents).await;
    let _ = tokio::fs::remove_file(STAGED).await;
    staged_result
}

async fn stage_and_install_pdns_conf(staged: &str, contents: &str) -> Result<(), ApiErr> {
    // `mode` comes from tokio's own OpenOptions on unix — no std trait import needed.
    let mut f = tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(staged)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to stage pdns.conf: {e}")))?;
    {
        use tokio::io::AsyncWriteExt;
        f.write_all(contents.as_bytes()).await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to stage pdns.conf: {e}")))?;
        f.flush().await.ok();
    }

    let placed = tokio::time::timeout(
        Duration::from_secs(60),
        safe_command_unsandboxed("sh", &[])
            .args(["-c", &format!(
                "mkdir -p /etc/powerdns && install -o root -g pdns -m 640 {staged} /etc/powerdns/pdns.conf"
            )])
            .output(),
    ).await
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Writing pdns.conf timed out"))?
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write pdns.conf: {e}")))?;

    if !placed.status.success() {
        let stderr = String::from_utf8_lossy(&placed.stderr);
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!(
            "Failed to write pdns.conf: {}", stderr.trim().chars().take(300).collect::<String>()
        )));
    }
    Ok(())
}

async fn install_powerdns(body: axum::body::Bytes) -> Result<Json<serde_json::Value>, ApiErr> {
    use crate::services::pkg;

    if let Some(why) = pkg::no_installer_reason("Installing PowerDNS").await {
        return Err(err(StatusCode::NOT_IMPLEMENTED, &why));
    }
    // Optional JSON body {"backend": "sqlite" | "pgsql"}. Absent/empty/invalid → pgsql
    // (back-compat: older panels POST no body). SQLite removes the dependency on the
    // panel's containerized PostgreSQL that broke installs in issue #63.
    let use_sqlite = serde_json::from_slice::<serde_json::Value>(&body).ok()
        .and_then(|v| v.get("backend").and_then(|b| b.as_str()).map(str::to_string))
        .as_deref() == Some("sqlite");
    let backend_name = if use_sqlite { "sqlite" } else { "pgsql" };
    tracing::info!("Installing PowerDNS (backend={backend_name})...");

    // 1. Install packages (backend-specific). Both backend package names differ
    // on the RHEL family — Debian names them after the library version
    // (`pdns-backend-sqlite3`), the RHEL family after the database
    // (`pdns-backend-sqlite`) — and neither was translated before s266, so this
    // installer would have failed on its very first argument.
    let pkgs: &[&str] = if use_sqlite {
        &["pdns-server", "pdns-backend-sqlite3", "sqlite3"]
    } else {
        &["pdns-server", "pdns-backend-pgsql"]
    };
    pkg::install(pkgs).await.map_err(|e| {
        err(StatusCode::INTERNAL_SERVER_ERROR, &format!("PowerDNS install failed: {e}"))
    })?;

    // 2. Generate the HTTP-API key (shared by both backends)
    let api_key: String = {
        use rand::Rng;
        let mut rng = rand::rng();
        (0..32).map(|_| rng.sample(rand::distr::Alphanumeric) as char).collect()
    };

    // 3. Backend-specific storage setup
    if use_sqlite {
        // /var/lib/powerdns is NOT in the agent's ReadWritePaths, so create the DB
        // and load the schema via the unsandboxed escape hatch (same mechanism the
        // package transaction uses) — otherwise ProtectSystem=strict EROFS's the write.
        //
        // The schema's PATH is distro-specific and the old code hardcoded Debian's:
        // `pdns-backend-sqlite3` ships it under its own doc directory, while the RHEL
        // family puts it in `/usr/share/doc/pdns/` (verified s266). Both branches here
        // were `[ -f ]`-guarded, so a miss did not fail — it **skipped**, leaving an
        // empty database, a PowerDNS that starts fine and a DNS server that answers
        // nothing, under an installer reporting success. Searching the candidates and
        // then FAILING when none matched is the difference between a bug that reports
        // itself and one that waits to be discovered by a user.
        let setup = "set -e; mkdir -p /var/lib/powerdns; \
            if [ ! -s /var/lib/powerdns/pdns.sqlite3 ]; then \
              SCHEMA=''; \
              for c in /usr/share/doc/pdns-backend-sqlite3/schema.sqlite3.sql \
                       /usr/share/doc/pdns/schema.sqlite3.sql \
                       /usr/share/pdns-backend-sqlite3/schema.sqlite3.sql \
                       /usr/share/doc/pdns-backend-sqlite/schema.sqlite3.sql; do \
                if [ -f \"$c\" ]; then SCHEMA=\"$c\"; break; fi; \
                if [ -f \"$c.gz\" ]; then SCHEMA=\"$c.gz\"; break; fi; \
              done; \
              if [ -z \"$SCHEMA\" ]; then \
                echo 'PowerDNS SQLite schema not found in any known location' >&2; exit 1; \
              fi; \
              case \"$SCHEMA\" in \
                *.gz) zcat \"$SCHEMA\" | sqlite3 /var/lib/powerdns/pdns.sqlite3 ;; \
                *)    sqlite3 /var/lib/powerdns/pdns.sqlite3 < \"$SCHEMA\" ;; \
              esac; \
              if ! sqlite3 /var/lib/powerdns/pdns.sqlite3 \
                   'SELECT 1 FROM domains LIMIT 1;' >/dev/null 2>&1; then \
                echo 'PowerDNS SQLite schema loaded but the domains table is missing' >&2; exit 1; \
              fi; \
            fi; \
            chown -R pdns:pdns /var/lib/powerdns";
        let sql_out = tokio::time::timeout(
            Duration::from_secs(120),
            safe_command_unsandboxed("sh", &[]).args(["-c", setup]).output()
        ).await
            .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "PowerDNS SQLite schema load timed out"))?
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("SQLite setup failed: {e}")))?;
        if !sql_out.status.success() {
            let stderr = String::from_utf8_lossy(&sql_out.stderr);
            return Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!("PowerDNS SQLite setup failed: {}", stderr.chars().take(300).collect::<String>())));
        }
    } else {
        // Create the pdns database in the panel's PostgreSQL container + load schema.
        let db_exists = tokio::time::timeout(
            Duration::from_secs(120),
            safe_command("docker")
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
                safe_command("docker")
                    .args(["exec", "dockpanel-postgres", "psql", "-U", "dockpanel", "-c", "CREATE DATABASE pdns;"])
                    .output()
            ).await;

            // Load PowerDNS schema
            let schema_path = "/usr/share/doc/pdns-backend-pgsql/schema.pgsql.sql";
            if tokio::fs::metadata(schema_path).await.is_ok() {
                // Use shell pipe to feed schema to psql
                let _ = tokio::time::timeout(
                    Duration::from_secs(120),
                    safe_command_unsandboxed("sh", &[])
                        .args(["-c", &format!("cat {} | docker exec -i dockpanel-postgres psql -U dockpanel -d pdns", schema_path)])
                        .output()
                ).await;
            }
        }
    }

    // 4. Build the backend-specific config
    let mut pdns_conf = if use_sqlite {
        format!(r#"# DockPanel PowerDNS configuration (SQLite backend)
launch=gsqlite3
gsqlite3-database=/var/lib/powerdns/pdns.sqlite3

# HTTP API
api=yes
api-key={api_key}
webserver=yes
webserver-address=127.0.0.1
webserver-port=8081
webserver-allow-from=127.0.0.1

# SOA defaults
default-soa-content=ns1.@ hostmaster.@ 0 10800 3600 604800 3600
"#)
    } else {
        // Use the same DB password as the panel's postgres connection (never
        // hardcode). The agent unit carries no EnvironmentFile — it is started
        // with `Environment=RUST_LOG=info` and nothing else — so PANEL_DB_PASSWORD
        // and DATABASE_URL are never set here and the old env-var lookup always
        // fell through to a freshly generated random password. That password
        // matches no role, so pdns failed authentication on every start and the
        // pgsql backend could not have worked on any install. Read the real
        // credential from the API's env file instead.
        let pdns_db_password = panel_db_password().await.ok_or_else(|| err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not read the panel database password from /etc/dockpanel/api.env — \
             install PowerDNS with the SQLite backend instead, or repair DATABASE_URL",
        ))?;

        format!(r#"# DockPanel PowerDNS configuration
launch=gpgsql
gpgsql-host=127.0.0.1
gpgsql-port=5450
gpgsql-dbname=pdns
gpgsql-user=dockpanel
gpgsql-password={pdns_db_password}

# HTTP API
api=yes
api-key={api_key}
webserver=yes
webserver-address=127.0.0.1
webserver-port=8081
webserver-allow-from=127.0.0.1

# SOA defaults
default-soa-content=ns1.@ hostmaster.@ 0 10800 3600 604800 3600
"#)
    };

    // 5. Pin the listening addresses when something already owns port 53
    if let Some(addrs) = pdns_local_addresses().await {
        tracing::info!("Port 53 already in use — binding PowerDNS to {addrs}");
        pdns_conf.push_str(&format!(
            "\n# Port 53 was already claimed at install time (systemd-resolved stub\n\
             # listener on 127.0.0.53/127.0.0.54 on Ubuntu), so bind explicitly.\n\
             local-address={addrs}\n"
        ));
    }

    // 6. Write PowerDNS config
    write_pdns_conf(&pdns_conf).await?;

    // 7. Enable and start. `enable` stays best-effort, but a pdns that never
    //    comes up must not be reported as a successful install — that silence
    //    is what let the port-53 conflict ship unnoticed.
    let _ = tokio::time::timeout(Duration::from_secs(120), safe_command("systemctl").args(["reset-failed", "pdns"]).output()).await;
    let _ = tokio::time::timeout(Duration::from_secs(120), safe_command("systemctl").args(["enable", "pdns"]).output()).await;
    let _ = tokio::time::timeout(Duration::from_secs(120), safe_command("systemctl").args(["restart", "pdns"]).output()).await;

    if !pdns_is_active().await {
        let detail = pdns_failure_detail().await;
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!(
            "PowerDNS was installed and configured ({backend_name} backend) but the service failed to start: {detail}"
        )));
    }

    tracing::info!("PowerDNS installed with API key (backend={backend_name})");

    Ok(Json(serde_json::json!({
        "ok": true,
        "message": format!("PowerDNS installed and configured ({backend_name} backend)"),
        "api_url": "http://127.0.0.1:8081",
        "api_key": api_key,
        "backend": backend_name,
    })))
}

// ── Redis installer ────────────────────────────────────────────────

async fn install_redis() -> Result<Json<serde_json::Value>, ApiErr> {
    use crate::services::pkg;

    if let Some(why) = pkg::no_installer_reason("Installing Redis").await {
        return Err(err(StatusCode::NOT_IMPLEMENTED, &why));
    }
    tracing::info!("Installing Redis...");

    pkg::refresh_index().await;
    pkg::install(&["redis-server"]).await.map_err(|e| {
        err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Redis install failed: {e}"))
    })?;

    // `redis-server` is the package AND the unit on Debian; on the RHEL family
    // both are `redis`, and they are translated by separate maps because a
    // package name and a unit name are not the same kind of string.
    let unit = pkg::service_name("redis-server").await;
    let _ = tokio::time::timeout(Duration::from_secs(120), safe_command("systemctl").args(["enable", &unit]).output()).await;
    let _ = tokio::time::timeout(Duration::from_secs(120), safe_command("systemctl").args(["start", &unit]).output()).await;

    // Verify Redis is responding
    let verify = tokio::time::timeout(
        Duration::from_secs(10),
        safe_command("redis-cli").arg("ping").output()
    ).await;
    let verified = verify
        .ok()
        .and_then(|r| r.ok())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_uppercase() == "PONG")
        .unwrap_or(false);

    tracing::info!("Redis installed, ping verified: {verified}");
    Ok(ok("Redis installed and started"))
}

// ── Node.js installer ──────────────────────────────────────────────

async fn install_nodejs() -> Result<Json<serde_json::Value>, ApiErr> {
    use crate::services::pkg;

    if let Some(why) = pkg::no_installer_reason("Installing Node.js").await {
        return Err(err(StatusCode::NOT_IMPLEMENTED, &why));
    }
    tracing::info!("Installing Node.js...");

    // NodeSource publishes a per-family setup script. `setup.sh` has branched to
    // the rpm one since s264; the agent still fetched the **deb** script on every
    // distro, which s265 recorded as a half-migration deliberately left alone
    // until there was a dnf path to migrate onto. This is that path.
    pkg::add_repo(pkg::Repo::NodeSource).await.map_err(|e| {
        err(StatusCode::INTERNAL_SERVER_ERROR, &format!("NodeSource repository setup failed: {e}"))
    })?;
    pkg::install(&["nodejs"]).await.map_err(|e| {
        err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Node.js install failed: {e}"))
    })?;

    // Verify
    let ver = tokio::time::timeout(
        Duration::from_secs(10),
        safe_command("node").arg("--version").output()
    ).await
        .ok()
        .and_then(|r| r.ok())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    tracing::info!("Node.js {ver} installed");
    Ok(ok(&format!("Node.js {ver} with npm installed")))
}

// ── Composer installer ─────────────────────────────────────────────

async fn install_composer() -> Result<Json<serde_json::Value>, ApiErr> {
    tracing::info!("Installing Composer...");

    let output = tokio::time::timeout(
        Duration::from_secs(300),
        safe_command_unsandboxed("sh", &[])
            .args(["-c", "curl -sS https://getcomposer.org/installer | php -- --install-dir=/usr/local/bin --filename=composer"])
            .output()
    ).await
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Composer install timed out after 300s"))?
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Composer install failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Composer install failed: {}", stderr.chars().take(300).collect::<String>())));
    }

    // Verify
    let ver = tokio::time::timeout(
        Duration::from_secs(10),
        safe_command("composer").arg("--version").output()
    ).await
        .ok()
        .and_then(|r| r.ok())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    tracing::info!("Composer installed: {ver}");
    Ok(ok("Composer installed globally at /usr/local/bin/composer"))
}

// ── PHP uninstaller ─────────────────────────────────────────────────────

async fn uninstall_php() -> Result<Json<serde_json::Value>, ApiErr> {
    use crate::services::pkg;

    if let Some(why) = pkg::no_installer_reason("Removing PHP").await {
        return Err(err(StatusCode::NOT_IMPLEMENTED, &why));
    }
    tracing::info!("Uninstalling PHP...");

    let version = detect_php_version().await.unwrap_or_else(|| "8.3".to_string());

    // Stop and disable PHP-FPM under whichever unit name this family uses.
    let unit = pkg::service_name(&format!("php{version}-fpm")).await;
    let _ = tokio::time::timeout(Duration::from_secs(120), safe_command("systemctl").args(["stop", &unit]).output()).await;
    let _ = tokio::time::timeout(Duration::from_secs(120), safe_command("systemctl").args(["disable", &unit]).output()).await;

    // Purge every PHP package. The glob is family-specific and deliberately
    // NOT routed through `pkg::remove` — that translates package NAMES, and a
    // glob is not a name; passing `php8.3-*` through the map would produce
    // something that matches nothing. Debian's packages are versioned, the
    // RHEL family's are not, so the two patterns differ in shape as well as
    // spelling.
    let glob = if pkg::manager().await == pkg::PkgMgr::Rpm {
        "php-*".to_string()
    } else {
        format!("php{version}-*")
    };
    let cmd = if pkg::manager().await == pkg::PkgMgr::Rpm {
        format!("dnf remove -y '{glob}'")
    } else {
        format!("DEBIAN_FRONTEND=noninteractive apt-get purge -y {glob} && apt-get autoremove -y")
    };

    let output = tokio::time::timeout(
        Duration::from_secs(300),
        safe_command_unsandboxed("sh", &[]).args(["-c", &cmd]).output()
    ).await
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "PHP uninstall timed out after 300s"))?
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("PHP uninstall failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!("PHP uninstall failed: {}", stderr.chars().take(300).collect::<String>())));
    }

    tracing::info!("PHP {version} uninstalled");
    Ok(ok(&format!("PHP {version} purged and removed")))
}

// ── Certbot uninstaller ─────────────────────────────────────────────────

async fn uninstall_certbot() -> Result<Json<serde_json::Value>, ApiErr> {
    use crate::services::pkg;

    if let Some(why) = pkg::no_installer_reason("Removing Certbot").await {
        return Err(err(StatusCode::NOT_IMPLEMENTED, &why));
    }
    tracing::info!("Uninstalling Certbot...");

    // Stop every renewal timer certbot might have arrived with — the apt/pip
    // one, the snap one, and EPEL's `certbot-renew.timer`. Which one exists
    // depends on how it was installed, so all are tried and none is required.
    for t in ["certbot.timer", "snap.certbot.renew.timer", "certbot-renew.timer"] {
        let _ = tokio::time::timeout(Duration::from_secs(30), safe_command("systemctl").args(["stop", t]).output()).await;
        let _ = tokio::time::timeout(Duration::from_secs(30), safe_command("systemctl").args(["disable", t]).output()).await;
    }

    // Remove snap certbot (if installed)
    let _ = tokio::time::timeout(
        Duration::from_secs(120),
        safe_command_unsandboxed("sh", &[])
            .args(["-c", "snap remove certbot 2>/dev/null; snap remove certbot-nginx 2>/dev/null; true"])
            .output()
    ).await;

    // Remove pip certbot (if installed)
    if std::path::Path::new("/opt/certbot").exists() {
        let _ = std::fs::remove_dir_all("/opt/certbot");
        let _ = std::fs::remove_file("/etc/systemd/system/certbot.timer");
        let _ = std::fs::remove_file("/etc/systemd/system/certbot.service");
        let _ = tokio::time::timeout(Duration::from_secs(30), safe_command("systemctl").args(["daemon-reload"]).output()).await;
    }

    // Remove the packaged certbot (if installed). Best-effort on purpose: the
    // snap and pip paths above may already have removed it, and a box that
    // never had the packaged one must not fail the uninstall.
    let _ = pkg::remove(&["certbot", "python3-certbot-nginx"]).await;

    // Clean up symlink
    let _ = std::fs::remove_file("/usr/bin/certbot");

    tracing::info!("Certbot uninstalled");
    Ok(ok("Certbot and auto-renewal timer removed"))
}

// ── UFW uninstaller ─────────────────────────────────────────────────────

/// Remove UFW.
///
/// Unlike [`install_ufw`], this is allowed on every family — and on the RHEL
/// family it is actively useful, because it is the cleanup for boxes that a
/// pre-v2.38.0 `setup.sh` left with ufw installed beside firewalld. Refusing to
/// remove a package we should never have installed would strand exactly the
/// operators worst affected.
async fn uninstall_ufw() -> Result<Json<serde_json::Value>, ApiErr> {
    use crate::services::pkg;

    if let Some(why) = pkg::no_installer_reason("Removing UFW").await {
        return Err(err(StatusCode::NOT_IMPLEMENTED, &why));
    }
    tracing::info!("Uninstalling UFW...");

    // Disable UFW first
    let _ = tokio::time::timeout(Duration::from_secs(120), safe_command("ufw").arg("disable").output()).await;

    pkg::remove(&["ufw"]).await.map_err(|e| {
        err(StatusCode::INTERNAL_SERVER_ERROR, &format!("UFW uninstall failed: {e}"))
    })?;

    tracing::info!("UFW uninstalled");
    Ok(ok("UFW disabled and removed"))
}

// ── Fail2Ban uninstaller ────────────────────────────────────────────────

async fn uninstall_fail2ban() -> Result<Json<serde_json::Value>, ApiErr> {
    use crate::services::pkg;

    if let Some(why) = pkg::no_installer_reason("Removing Fail2Ban").await {
        return Err(err(StatusCode::NOT_IMPLEMENTED, &why));
    }
    tracing::info!("Uninstalling Fail2Ban...");

    // Stop and disable fail2ban
    let _ = tokio::time::timeout(Duration::from_secs(120), safe_command("systemctl").args(["stop", "fail2ban"]).output()).await;
    let _ = tokio::time::timeout(Duration::from_secs(120), safe_command("systemctl").args(["disable", "fail2ban"]).output()).await;

    pkg::remove(&["fail2ban"]).await.map_err(|e| {
        err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Fail2Ban uninstall failed: {e}"))
    })?;

    // Remove custom jail config
    let _ = tokio::fs::remove_file("/etc/fail2ban/jail.local").await;

    tracing::info!("Fail2Ban uninstalled");
    Ok(ok("Fail2Ban stopped and purged with jail config removed"))
}

// ── PowerDNS uninstaller ────────────────────────────────────────────────

async fn uninstall_powerdns() -> Result<Json<serde_json::Value>, ApiErr> {
    use crate::services::pkg;

    if let Some(why) = pkg::no_installer_reason("Removing PowerDNS").await {
        return Err(err(StatusCode::NOT_IMPLEMENTED, &why));
    }
    tracing::info!("Uninstalling PowerDNS...");

    // Stop and disable pdns. The unit is literally `pdns` on BOTH families —
    // Debian's PACKAGE is `pdns-server` while its UNIT is already `pdns` — so
    // this deliberately does NOT go through `service_name`, which would hand
    // back `pdns-server` on Debian and stop a unit that has never existed
    // there. It is the sharpest example of why the package map and the unit
    // map have to be separate: for this service they disagree on Debian and
    // agree on RHEL, which no single translation can express.
    let _ = tokio::time::timeout(Duration::from_secs(120), safe_command("systemctl").args(["stop", "pdns"]).output()).await;
    let _ = tokio::time::timeout(Duration::from_secs(120), safe_command("systemctl").args(["disable", "pdns"]).output()).await;

    pkg::remove(&["pdns-server", "pdns-backend-pgsql", "pdns-backend-sqlite3"])
        .await
        .map_err(|e| {
            err(StatusCode::INTERNAL_SERVER_ERROR, &format!("PowerDNS uninstall failed: {e}"))
        })?;

    // Remove config file (but keep the pdns database — user may want DNS records)
    let _ = tokio::fs::remove_file("/etc/powerdns/pdns.conf").await;

    tracing::info!("PowerDNS uninstalled (database preserved)");
    Ok(ok("PowerDNS purged and config removed (database preserved)"))
}

// ── Redis uninstaller ───────────────────────────────────────────────────

async fn uninstall_redis() -> Result<Json<serde_json::Value>, ApiErr> {
    use crate::services::pkg;

    if let Some(why) = pkg::no_installer_reason("Removing Redis").await {
        return Err(err(StatusCode::NOT_IMPLEMENTED, &why));
    }
    tracing::info!("Uninstalling Redis...");

    let unit = pkg::service_name("redis-server").await;
    let _ = tokio::time::timeout(Duration::from_secs(120), safe_command("systemctl").args(["stop", &unit]).output()).await;
    let _ = tokio::time::timeout(Duration::from_secs(120), safe_command("systemctl").args(["disable", &unit]).output()).await;

    pkg::remove(&["redis-server"]).await.map_err(|e| {
        err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Redis uninstall failed: {e}"))
    })?;

    tracing::info!("Redis uninstalled");
    Ok(ok("Redis stopped and purged"))
}

// ── Node.js uninstaller ─────────────────────────────────────────────────

async fn uninstall_nodejs() -> Result<Json<serde_json::Value>, ApiErr> {
    use crate::services::pkg;

    if let Some(why) = pkg::no_installer_reason("Removing Node.js").await {
        return Err(err(StatusCode::NOT_IMPLEMENTED, &why));
    }
    tracing::info!("Uninstalling Node.js...");

    pkg::remove(&["nodejs"]).await.map_err(|e| {
        err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Node.js uninstall failed: {e}"))
    })?;

    // Remove the NodeSource repo on whichever family added it. This used to
    // delete the apt sources file only, so an RPM box would have kept a repo
    // definition for a package it no longer has.
    pkg::remove_repo(pkg::Repo::NodeSource).await;

    tracing::info!("Node.js uninstalled");
    Ok(ok("Node.js purged and nodesource repo removed"))
}

// ── Composer uninstaller ────────────────────────────────────────────────

async fn uninstall_composer() -> Result<Json<serde_json::Value>, ApiErr> {
    tracing::info!("Uninstalling Composer...");

    // Composer is just a binary — remove it. Wrapped in systemd-run because
    // /usr/local/bin is under /usr, which the agent's ProtectSystem=strict
    // sandbox makes read-only.
    let output = tokio::time::timeout(
        Duration::from_secs(120),
        safe_command_unsandboxed("rm", &[]).args(["-f", "/usr/local/bin/composer"]).output()
    ).await
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Composer uninstall timed out"))?
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Composer uninstall failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Composer uninstall failed: {}", stderr.chars().take(300).collect::<String>())));
    }

    tracing::info!("Composer uninstalled");
    Ok(ok("Composer binary removed from /usr/local/bin"))
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Package presence, on dpkg and rpm boxes alike — see `services::pkg`.
/// This used to shell out to `dpkg` directly, which answered `false` for
/// every package on the entire RHEL family (s265).
async fn is_installed(package: &str) -> bool {
    crate::services::pkg::is_installed(package).await
}

async fn is_active(service: &str) -> bool {
    tokio::time::timeout(
        Duration::from_secs(120),
        safe_command("systemctl").args(["is-active", "--quiet", service]).output()
    ).await
        .ok()
        .and_then(|r| r.ok())
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Install `sshpass`, which password-authenticated SFTP backup destinations
/// shell out to.
///
/// Servers added through `install-agent.sh` get it at install time. A box that
/// was already running when that landed never did, and `update.sh` upgrades
/// binaries without installing packages — so before this route the only way to
/// fix such a server was to tell its operator to run a package manager by hand,
/// which is exactly what issue #93 was left open until we stopped doing.
async fn install_sshpass() -> Result<Json<serde_json::Value>, ApiErr> {
    use crate::services::pkg;

    if let Some(why) = pkg::no_installer_reason("Installing sshpass").await {
        return Err(err(StatusCode::NOT_IMPLEMENTED, &why));
    }
    tracing::info!("Installing sshpass...");

    // An already-running server is precisely the box whose package index is
    // months stale — that is the population this route exists for.
    pkg::refresh_index().await;
    pkg::install(&["sshpass"]).await.map_err(|e| {
        err(StatusCode::INTERNAL_SERVER_ERROR, &format!("sshpass install failed: {e}"))
    })?;

    // Report the thing the caller actually depends on: whether the binary the
    // backup path shells out to is now on PATH. A package manager reporting
    // success is a different claim, and `remote_backup` fails on ENOENT of the
    // binary, not on the package's absence.
    if !which("sshpass").await {
        return Err(err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "The sshpass package reported a successful install but no sshpass binary is on \
             PATH. Password-authenticated SFTP destinations will still fail from this server.",
        ));
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "sshpass installed. Password-authenticated SFTP backup destinations can \
                    now run from this server."
    })))
}

async fn which(cmd: &str) -> bool {
    tokio::time::timeout(
        Duration::from_secs(120),
        safe_command("which").arg(cmd).output()
    ).await
        .ok()
        .and_then(|r| r.ok())
        .map(|o| o.status.success())
        .unwrap_or(false)
}

async fn is_ufw_active() -> bool {
    tokio::time::timeout(
        Duration::from_secs(120),
        safe_command("ufw").arg("status").output()
    ).await
        .ok()
        .and_then(|r| r.ok())
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("Status: active"))
        .unwrap_or(false)
}

async fn detect_php_version() -> Option<String> {
    let output = tokio::time::timeout(
        Duration::from_secs(120),
        safe_command("php").args(["-r", "echo PHP_MAJOR_VERSION.'.'.PHP_MINOR_VERSION;"]).output()
    ).await.ok()?.ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

// ── WAF (ModSecurity3 + OWASP CRS) installer ───────────────────────

async fn install_waf() -> Result<Json<serde_json::Value>, ApiErr> {
    use crate::services::pkg;

    if let Some(why) = pkg::no_installer_reason("Installing the WAF").await {
        return Err(err(StatusCode::NOT_IMPLEMENTED, &why));
    }
    tracing::info!("Installing WAF (ModSecurity3 + OWASP CRS)...");

    // 1. Install the library and the nginx connector. Debian splits them and
    // needs both names; the RHEL family ships one `nginx-mod-modsecurity` that
    // pulls the library in, so both Debian spellings collapse onto it — which
    // is why passing both here is correct on either family rather than a
    // duplicate. (Only one of the two was translated before s266.)
    pkg::refresh_index().await;
    pkg::install(&["libmodsecurity3", "libnginx-mod-http-modsecurity"])
        .await
        .map_err(|e| {
            err(StatusCode::INTERNAL_SERVER_ERROR,
                &format!("WAF install failed (libmodsecurity3 or the nginx module is not available): {e}"))
        })?;

    // 2. Create directory structure
    let dirs = ["/etc/modsecurity", "/etc/modsecurity/sites", "/var/log/modsecurity"];
    for dir in dirs {
        std::fs::create_dir_all(dir).ok();
    }

    // 3. Download OWASP CRS v4
    let crs_dir = "/etc/modsecurity/crs";
    if !std::path::Path::new(&format!("{crs_dir}/crs-setup.conf")).exists() {
        let dl = tokio::time::timeout(
            Duration::from_secs(120),
            safe_command_unsandboxed("sh", &[])
                .args(["-c", &format!(
                    "cd /tmp && \
                     curl -sL https://github.com/coreruleset/coreruleset/archive/refs/tags/v4.25.0.tar.gz -o crs.tar.gz && \
                     tar xzf crs.tar.gz && \
                     rm -rf {crs_dir} && \
                     mv coreruleset-4.25.0 {crs_dir} && \
                     cp {crs_dir}/crs-setup.conf.example {crs_dir}/crs-setup.conf && \
                     rm -f crs.tar.gz"
                )])
                .output()
        ).await;

        match dl {
            Ok(Ok(o)) if o.status.success() => {
                tracing::info!("OWASP CRS v4.25.0 downloaded");
            }
            _ => {
                tracing::warn!("OWASP CRS download failed — WAF will work without rules");
            }
        }
    }

    // 4. Write base ModSecurity config
    let modsec_conf = r#"# ModSecurity base config (managed by DockPanel)
SecRuleEngine DetectionOnly
SecRequestBodyAccess On
SecRequestBodyLimit 13107200
SecRequestBodyNoFilesLimit 131072
SecResponseBodyAccess Off
SecTmpDir /tmp/
SecDataDir /tmp/
SecAuditEngine RelevantOnly
SecAuditLogRelevantStatus "^(?:5|4(?!04))"
SecAuditLogParts ABIJDEFHZ
SecAuditLogType Serial
SecAuditLog /var/log/modsecurity/modsec_audit.log
SecArgumentSeparator &
SecCookieFormat 0
SecUnicodeMapFile unicode.mapping 20127
SecStatusEngine Off
"#;

    std::fs::write("/etc/modsecurity/modsecurity.conf", modsec_conf)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Write modsec config: {e}")))?;

    // 5. Write unicode mapping (required by ModSecurity)
    //
    // `modsecurity.conf` above declares `SecUnicodeMapFile unicode.mapping
    // 20127`, and ModSecurity refuses to load a mapping file it cannot parse —
    // so an ABSENT or EMPTY file here does not fail the install, it fails
    // `nginx -t` later, the first time a site actually turns the WAF on. That
    // is what issue #92 hit on Debian 13, where `libmodsecurity3t64` ships no
    // `unicode.mapping` anywhere on the filesystem (verified: `find / -name
    // unicode.mapping` on a clean debian:13 after installing the package
    // returns nothing) and the old code's copy source
    // `/usr/share/modsecurity-crs/unicode.mapping` therefore never existed —
    // leaving the `write("")` fallback to create a zero-byte file that is
    // guaranteed to break the very nginx config this installer is for.
    //
    // The file rides the BINARY instead of depending on distro packaging, the
    // same delivery discipline as the agent's own systemd unit. Vendored from
    // upstream ModSecurity at `panel/agent/assets/unicode.mapping`.
    const UNICODE_MAPPING: &str = include_str!("../../assets/unicode.mapping");
    const MAPPING_PATH: &str = "/etc/modsecurity/unicode.mapping";

    // A system copy is preferred when the distro does ship one, but only if it
    // is real: an empty or truncated file is worse than no file, because it
    // looks installed.
    let usable_system_copy = std::fs::metadata(MAPPING_PATH)
        .map(|m| m.len() > 1024)
        .unwrap_or(false);

    if !usable_system_copy {
        let from_distro = ["/usr/share/modsecurity/unicode.mapping",
                           "/usr/share/modsecurity-crs/unicode.mapping",
                           "/etc/nginx/modsecurity/unicode.mapping"]
            .iter()
            .find(|p| std::fs::metadata(p).map(|m| m.len() > 1024).unwrap_or(false))
            .and_then(|p| std::fs::read_to_string(p).ok());

        let contents = from_distro.as_deref().unwrap_or(UNICODE_MAPPING);
        std::fs::write(MAPPING_PATH, contents).map_err(|e| {
            err(StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Write unicode.mapping: {e}"))
        })?;
    }

    // Assert the thing the later nginx reload depends on, rather than assuming
    // the write did what it said (#45: an installer that discards its results
    // hides every other bug in itself).
    match std::fs::read_to_string(MAPPING_PATH) {
        Ok(c) if c.contains("20127") => {}
        Ok(_) => {
            return Err(err(StatusCode::INTERNAL_SERVER_ERROR,
                "unicode.mapping is present but does not define code page 20127, \
                 which modsecurity.conf requires — the WAF would fail nginx -t \
                 the first time a site enabled it"));
        }
        Err(e) => {
            return Err(err(StatusCode::INTERNAL_SERVER_ERROR,
                &format!("unicode.mapping unreadable after install: {e}")));
        }
    }

    // 6. Verify nginx can load the module
    let test = tokio::time::timeout(
        Duration::from_secs(10),
        safe_command("nginx").args(["-t"]).output()
    ).await;

    let nginx_ok = test.ok()
        .and_then(|r| r.ok())
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !nginx_ok {
        tracing::warn!("nginx -t failed after WAF install — module may need manual load_module directive");
    }

    tracing::info!("WAF installed (ModSecurity3 + OWASP CRS)");
    Ok(ok("WAF installed: ModSecurity3 with OWASP Core Rule Set v4.25.0"))
}

async fn uninstall_waf() -> Result<Json<serde_json::Value>, ApiErr> {
    use crate::services::pkg;

    if let Some(why) = pkg::no_installer_reason("Removing the WAF").await {
        return Err(err(StatusCode::NOT_IMPLEMENTED, &why));
    }
    tracing::info!("Uninstalling WAF...");

    // Remove WAF directives from all nginx configs
    let sites_dir = crate::services::nginx::sites_dir();
    if let Ok(entries) = std::fs::read_dir(sites_dir) {
        for entry in entries.flatten() {
            if let Ok(content) = std::fs::read_to_string(entry.path()) {
                if content.contains("modsecurity") {
                    let cleaned: String = content.lines()
                        .filter(|l| !l.contains("modsecurity"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    let _ = std::fs::write(entry.path(), cleaned);
                }
            }
        }
    }

    // Reload nginx to remove WAF from active configs
    let _ = tokio::time::timeout(
        Duration::from_secs(10),
        safe_command("nginx").args(["-s", "reload"]).output()
    ).await;

    // Purge packages. All three Debian spellings collapse onto the single RHEL
    // package, so `remove` is handed the same list `install` was given.
    pkg::remove(&[
        "libnginx-mod-http-modsecurity",
        "libmodsecurity3",
        "libmodsecurity3t64",
    ])
    .await
    .map_err(|e| {
        err(StatusCode::INTERNAL_SERVER_ERROR, &format!("WAF uninstall failed: {e}"))
    })?;

    // Clean up config files (preserve logs)
    let _ = std::fs::remove_dir_all("/etc/modsecurity");

    tracing::info!("WAF uninstalled");
    Ok(ok("WAF (ModSecurity3) uninstalled"))
}

// ── Cloudflare Tunnel (cloudflared) ─────────────────────────────────

async fn install_cloudflared() -> Result<Json<serde_json::Value>, ApiErr> {
    use crate::services::pkg;

    if let Some(why) = pkg::no_installer_reason("Installing Cloudflare Tunnel").await {
        return Err(err(StatusCode::NOT_IMPLEMENTED, &why));
    }
    tracing::info!("Installing cloudflared...");

    // The codename lookup that used to live here — and hard-errored with
    // "Cannot detect OS codename" on every RHEL release, none of which carries
    // VERSION_CODENAME — now sits inside `pkg::add_repo`, on the apt branch
    // only. Cloudflare publishes a codename-keyed apt repo and a single flat
    // rpm repo, so on the RPM side the codename is not defaulted or guessed:
    // it is not part of the question. `add_repo` also removes a half-written
    // repo file on failure, which is what stops a partial install from breaking
    // every later transaction on the operator's box (Lesson #73C).
    pkg::add_repo(pkg::Repo::Cloudflared).await.map_err(|e| {
        err(StatusCode::INTERNAL_SERVER_ERROR, &format!("cloudflared repository setup failed: {e}"))
    })?;
    pkg::install(&["cloudflared"]).await.map_err(|e| {
        err(StatusCode::INTERNAL_SERVER_ERROR, &format!("cloudflared install failed: {e}"))
    })?;

    std::fs::create_dir_all("/etc/cloudflared").ok();

    let verify = tokio::time::timeout(
        Duration::from_secs(5),
        safe_command("cloudflared").args(["version"]).output()
    ).await;
    let version = verify.ok()
        .and_then(|r| r.ok())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    tracing::info!("cloudflared installed: {version}");
    Ok(ok(&format!("cloudflared installed: {version}")))
}

async fn uninstall_cloudflared() -> Result<Json<serde_json::Value>, ApiErr> {
    use crate::services::pkg;

    if let Some(why) = pkg::no_installer_reason("Removing Cloudflare Tunnel").await {
        return Err(err(StatusCode::NOT_IMPLEMENTED, &why));
    }
    tracing::info!("Uninstalling cloudflared...");

    let _ = tokio::time::timeout(Duration::from_secs(30), safe_command("systemctl").args(["stop", "cloudflared"]).output()).await;
    let _ = tokio::time::timeout(Duration::from_secs(30), safe_command("systemctl").args(["disable", "cloudflared"]).output()).await;

    pkg::remove(&["cloudflared"]).await.map_err(|e| {
        err(StatusCode::INTERNAL_SERVER_ERROR, &format!("cloudflared uninstall failed: {e}"))
    })?;

    // Drop the repo definition on whichever family added it, so an operator who
    // removes the tunnel is not left carrying Cloudflare's repo forever.
    pkg::remove_repo(pkg::Repo::Cloudflared).await;

    let _ = std::fs::remove_dir_all("/etc/cloudflared");
    tracing::info!("cloudflared uninstalled");
    Ok(ok("cloudflared uninstalled"))
}

/// POST /services/cloudflared/configure — Configure tunnel with token and ingress rules.
async fn configure_cloudflared(
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    let token = body.get("token").and_then(|v| v.as_str()).unwrap_or("");
    if token.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Missing tunnel token"));
    }

    // Validate token format: must be base64-like, no newlines/specifiers/shell chars
    if token.len() < 50 || token.len() > 4096
        || token.contains('\n') || token.contains('\r') || token.contains('\0')
        || token.contains('%') || token.contains('\'') || token.contains('"')
        || token.contains(';') || token.contains('|') || token.contains('&')
        || !token.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '=')
    {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid tunnel token format"));
    }

    std::fs::create_dir_all("/etc/cloudflared").ok();

    // Write the tunnel token to a root-only (0600) EnvironmentFile so it is NOT
    // world-readable in the unit file and NOT exposed on the process command line
    // (cloudflared reads TUNNEL_TOKEN from the environment for `tunnel run`).
    let token_env_path = "/etc/cloudflared/token.env";
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(token_env_path)
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Write token file: {e}")))?;
        f.write_all(format!("TUNNEL_TOKEN={token}\n").as_bytes())
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Write token file: {e}")))?;
    }
    // Harden perms even if the file already existed with a looser mode.
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(token_env_path, std::fs::Permissions::from_mode(0o600));
    }

    // Write systemd service. The token is supplied via the 0600 EnvironmentFile
    // (TUNNEL_TOKEN), never on the ExecStart command line.
    let service = format!(
        "[Unit]\n\
         Description=Cloudflare Tunnel\n\
         After=network-online.target\n\
         Wants=network-online.target\n\n\
         [Service]\n\
         Type=simple\n\
         EnvironmentFile={token_env_path}\n\
         ExecStart=/usr/bin/cloudflared tunnel run\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         LimitNOFILE=65536\n\n\
         [Install]\n\
         WantedBy=multi-user.target\n"
    );

    std::fs::write("/etc/systemd/system/cloudflared.service", &service)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Write service: {e}")))?;

    // Reload systemd and start
    let _ = tokio::time::timeout(Duration::from_secs(10), safe_command("systemctl").args(["daemon-reload"]).output()).await;
    let _ = tokio::time::timeout(Duration::from_secs(10), safe_command("systemctl").args(["enable", "cloudflared"]).output()).await;
    let _ = tokio::time::timeout(Duration::from_secs(15), safe_command("systemctl").args(["restart", "cloudflared"]).output()).await;

    // Check if running
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let running = is_active("cloudflared").await;

    tracing::info!("Cloudflare Tunnel configured, running: {running}");
    Ok(Json(serde_json::json!({
        "ok": true,
        "running": running,
    })))
}

/// GET /services/cloudflared/status — Get tunnel connection status.
async fn cloudflared_status() -> Result<Json<serde_json::Value>, ApiErr> {
    let installed = which("cloudflared").await;
    let running = is_active("cloudflared").await;

    let version = if installed {
        tokio::time::timeout(
            Duration::from_secs(5),
            safe_command("cloudflared").args(["version"]).output()
        ).await.ok()
            .and_then(|r| r.ok())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    } else {
        None
    };

    let has_service = std::path::Path::new("/etc/systemd/system/cloudflared.service").exists();

    Ok(Json(serde_json::json!({
        "installed": installed,
        "running": running,
        "version": version,
        "configured": has_service,
    })))
}

async fn detect_available_php() -> Option<String> {
    // The distro's own default first: `apt-cache depends php-fpm` names the
    // real package ("Depends: php8.5-fpm" on Ubuntu 26.04, php8.3-fpm on
    // 24.04, php8.4-fpm on Debian 13). The old fixed probe list topped out
    // at 8.3, so on any distro shipping a newer PHP it returned None and the
    // caller fell back to a php8.3 install that no repo could satisfy.
    if let Ok(Ok(o)) = tokio::time::timeout(
        Duration::from_secs(120),
        safe_command("apt-cache").args(["depends", "php-fpm"]).output()
    ).await {
        if o.status.success() {
            let out = String::from_utf8_lossy(&o.stdout);
            for line in out.lines() {
                if let Some(rest) = line.trim().strip_prefix("Depends: php") {
                    if let Some(v) = rest.strip_suffix("-fpm") {
                        if !v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == '.') {
                            return Some(v.to_string());
                        }
                    }
                }
            }
        }
    }
    // Fallback probe, newest first (covers boxes with a 3rd-party PHP repo
    // already configured but no php-fpm metapackage).
    for v in ["8.5", "8.4", "8.3", "8.2", "8.1"] {
        let output = tokio::time::timeout(
            Duration::from_secs(120),
            safe_command("apt-cache").args(["show", &format!("php{v}-fpm")]).output()
        ).await;
        if output.ok().and_then(|r| r.ok()).map(|o| o.status.success()).unwrap_or(false) {
            return Some(v.to_string());
        }
    }
    None
}
