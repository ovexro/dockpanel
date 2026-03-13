use std::process::Command;
use tokio::process::Command as AsyncCommand;

const MSMTP_CONFIG: &str = "/etc/msmtprc";

/// Install msmtp if not already present.
fn ensure_msmtp() -> Result<(), String> {
    let status = Command::new("which")
        .arg("msmtp")
        .output()
        .map_err(|e| format!("Failed to check msmtp: {e}"))?;

    if !status.status.success() {
        tracing::info!("Installing msmtp...");
        let install = Command::new("apt-get")
            .args(["install", "-y", "msmtp", "msmtp-mta"])
            .env("DEBIAN_FRONTEND", "noninteractive")
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

    std::fs::write(MSMTP_CONFIG, &config)
        .map_err(|e| format!("Failed to write {MSMTP_CONFIG}: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(MSMTP_CONFIG, std::fs::Permissions::from_mode(0o600)).ok();
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

    // Create log file
    std::fs::write("/var/log/msmtp.log", "").ok();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions("/var/log/msmtp.log", std::fs::Permissions::from_mode(0o666)).ok();
    }

    tracing::info!("SMTP configured: {host}:{port} from={from}");
    Ok(())
}

/// Send a test email via msmtp.
pub async fn send_test(to: &str, from: &str, from_name: &str) -> Result<String, String> {
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

    let mut child = AsyncCommand::new("msmtp")
        .args(["--read-envelope-from", to])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn msmtp: {e}"))?;

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
