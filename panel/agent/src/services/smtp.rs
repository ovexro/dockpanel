use crate::safe_cmd::{safe_command, safe_command_sync, safe_command_sync_unsandboxed};

const MSMTP_CONFIG: &str = "/etc/msmtprc";

/// Named so the route can answer it as a precondition rather than a fault.
///
/// Test Email spawned msmtp and reported a failed spawn as a 5xx, which the
/// panel replaces with an incident id — so an operator whose host simply has no
/// msmtp concluded their credentials or their relay were wrong and went to debug
/// the mail provider. Two things make it ordinary rather than exotic: the binary
/// is installed only by `ensure_msmtp` below, which runs on `configure`, so a
/// member that was offline when SMTP was saved never got it; and that installer
/// is **apt-get only**, so on the RHEL family it is never installed at all. The
/// panel's own precondition check reads a panel-global setting, not per-host
/// state, so it passes while this host has nothing.
pub const MSMTP_MISSING: &str =
    "This server has no msmtp binary, so it cannot send mail yet — your SMTP \
     settings are not the problem. Save the SMTP settings again while this server \
     is online to install and configure it. On RHEL-family systems the panel \
     cannot install msmtp automatically; install it with your package manager.";

/// Install msmtp if not already present.
fn ensure_msmtp() -> Result<(), String> {
    let status = safe_command_sync("which")
        .arg("msmtp")
        .output()
        .map_err(|e| format!("Failed to check msmtp: {e}"))?;

    if !status.status.success() {
        tracing::info!("Installing msmtp...");
        let install = safe_command_sync_unsandboxed("apt-get", &[])
            .args(["install", "-y", "msmtp", "msmtp-mta"])
            .output()
            .map_err(|e| format!("Failed to install msmtp: {e}"))?;

        if !install.status.success() {
            return Err(format!(
                "msmtp installation failed: {}",
                String::from_utf8_lossy(&install.stderr)
            ));
        }
    }
    Ok(())
}

/// Check that a config value does not contain newlines or control characters
/// that could allow SMTP header injection or config file manipulation.
fn is_safe_config_value(s: &str) -> bool {
    !s.contains('\n') && !s.contains('\r') && !s.contains('\0')
}

/// Configure msmtp system-wide so PHP mail() and sendmail work.
pub fn configure(
    host: &str,
    port: u16,
    username: &str,
    password: &str,
    from: &str,
    from_name: &str,
    encryption: &str,
) -> Result<(), String> {
    // Validate all string inputs against newline/control character injection
    for (name, value) in [
        ("host", host),
        ("username", username),
        ("password", password),
        ("from", from),
        ("from_name", from_name),
    ] {
        if !is_safe_config_value(value) {
            return Err(format!("Invalid {name}: contains newline or control characters"));
        }
    }

    ensure_msmtp()?;

    let tls = match encryption {
        "none" => "off",
        _ => "on", // tls, starttls
    };
    let tls_starttls = match encryption {
        "starttls" => "on",
        "none" => "off",
        _ => "off", // "tls" means implicit TLS, not STARTTLS
    };

    let config = format!(
        r#"# DockPanel SMTP configuration — managed automatically
defaults
auth           on
tls            {tls}
tls_starttls   {tls_starttls}
tls_trust_file /etc/ssl/certs/ca-certificates.crt
logfile        /var/log/msmtp.log

account        default
host           {host}
port           {port}
from           {from}
user           {username}
password       {password}
"#
    );

    // Create the file 0600 from the outset. `fs::write` creates under the umask —
    // 0644 on every distro the installer supports — and line 6 of what is being
    // written is the SMTP password, so create-then-chmod published the password to
    // every local account for the length of the write. The old
    // `set_permissions(...).ok()` also discarded its own failure, in which case
    // the file simply stayed 0644 with a password in it and nothing said so.
    #[cfg(unix)]
    let written = {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(MSMTP_CONFIG)
            .and_then(|mut f| f.write_all(config.as_bytes()))
    };
    #[cfg(not(unix))]
    let written = std::fs::write(MSMTP_CONFIG, &config);

    written.map_err(|e| format!("Failed to write {MSMTP_CONFIG}: {e}"))?;

    // `.mode()` applies only when the file is CREATED, so a config already on disk
    // keeps whatever mode it has — including a 0644 left by a pre-v2.123.0 agent,
    // which is the case an upgrade has to repair. Asserted explicitly, and this
    // time the failure is returned rather than swallowed: there is no sensible way
    // to carry on having failed to protect a stored password.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(MSMTP_CONFIG, std::fs::Permissions::from_mode(0o600)).map_err(
            |e| format!("Failed to restrict {MSMTP_CONFIG}, which holds the SMTP password: {e}"),
        )?;
    }

    // Also configure PHP to use msmtp as sendmail
    let php_ini_content = format!("sendmail_path = /usr/bin/msmtp -t\n");
    let msmtp_ini_dir = "/etc/php/mods-available";
    if std::path::Path::new(msmtp_ini_dir).exists() {
        let ini_path = format!("{msmtp_ini_dir}/msmtp.ini");
        std::fs::write(&ini_path, &php_ini_content).ok();
        // Enable for all PHP versions
        if let Ok(entries) = std::fs::read_dir("/etc/php") {
        for entry in entries {
            if let Ok(e) = entry {
                let ver = e.file_name();
                let ver_str = ver.to_string_lossy();
                if ver_str.starts_with('8') || ver_str.starts_with('7') {
                    let fpm_conf = format!("/etc/php/{ver_str}/fpm/conf.d/99-msmtp.ini");
                    let cli_conf = format!("/etc/php/{ver_str}/cli/conf.d/99-msmtp.ini");
                    std::fs::write(&fpm_conf, &php_ini_content).ok();
                    std::fs::write(&cli_conf, &php_ini_content).ok();
                }
            }
        }
        }
    }

    // Create the relay log named by `logfile` above. msmtp writes it as whoever
    // sent the mail: root for the panel's own Test Email, `www-data` for anything
    // PHP relays through `sendmail_path` (every FPM pool this agent writes runs as
    // www-data). 0666 was the shortest way to let both — and it also made the log
    // world-READABLE, which is envelope metadata (who mailed whom, when), and
    // world-WRITABLE, so any local account could forge entries or fill the disk.
    //
    // 0660 owned by root:www-data covers both writers and nobody else. The chown
    // is attempted FIRST and the mode only follows if it succeeded: on a host with
    // no www-data group, tightening the mode regardless would take the log away
    // from msmtp-as-www-data and break relaying to fix a disclosure. Repairing a
    // loud problem must not manufacture a silent one, so that case keeps the old
    // mode and says why.
    std::fs::write("/var/log/msmtp.log", "").ok();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let grouped = safe_command_sync("chown")
            .args(["root:www-data", "/var/log/msmtp.log"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if grouped {
            std::fs::set_permissions("/var/log/msmtp.log", std::fs::Permissions::from_mode(0o660))
                .ok();
        } else {
            tracing::warn!(
                "SMTP: no www-data group to own /var/log/msmtp.log, so it stays \
                 world-writable — PHP relays keep logging, but envelope metadata in \
                 it is readable by any local account on this host."
            );
        }
    }

    tracing::info!("SMTP configured: {host}:{port} from={from}");
    Ok(())
}

/// Send a test email via msmtp.
pub async fn send_test(to: &str, from: &str, from_name: &str) -> Result<String, String> {
    // Reject CRLF injection in email headers
    for (label, val) in [("to", to), ("from", from), ("from_name", from_name)] {
        if val.contains('\r') || val.contains('\n') || val.contains('\0') {
            return Err(format!("{label} must not contain newlines or null bytes"));
        }
    }
    let subject = "DockPanel SMTP Test";
    let body = format!(
        "From: {from_name} <{from}>\r\n\
         To: {to}\r\n\
         Subject: {subject}\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         This is a test email from DockPanel.\r\n\
         If you received this, your SMTP configuration is working correctly.\r\n"
    );

    let mut child = safe_command("msmtp")
        .args(["--read-envelope-from", to])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                MSMTP_MISSING.to_string()
            } else {
                format!("Failed to spawn msmtp: {e}")
            }
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin.write_all(body.as_bytes()).await.ok();
        drop(stdin);
    }

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| format!("msmtp failed: {e}"))?;

    if output.status.success() {
        Ok(format!("Test email sent to {to}"))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("msmtp failed: {stderr}"))
    }
}
