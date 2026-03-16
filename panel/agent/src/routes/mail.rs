use axum::{
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use tokio::process::Command;
use std::path::Path;

use super::AppState;

type ApiErr = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: &str) -> ApiErr {
    (status, Json(serde_json::json!({ "error": msg })))
}

fn ok(msg: &str) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ok": true, "message": msg }))
}

// ── Request types ───────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct DkimRequest {
    pub domain: String,
    pub selector: String,
}

#[derive(Deserialize)]
pub struct DomainRequest {
    pub domain: String,
}

#[derive(Deserialize)]
pub struct SyncRequest {
    pub domains: Vec<SyncDomain>,
    pub accounts: Vec<SyncAccount>,
    pub aliases: Vec<SyncAlias>,
}

#[derive(Deserialize)]
pub struct SyncDomain {
    pub domain: String,
    pub enabled: bool,
    pub catch_all: Option<String>,
}

#[derive(Deserialize)]
pub struct SyncAccount {
    pub email: String,
    pub password_hash: String,
    pub quota_mb: i32,
    pub enabled: bool,
    pub forward_to: Option<String>,
}

#[derive(Deserialize)]
pub struct SyncAlias {
    pub source: String,
    pub destination: String,
}

#[derive(Deserialize)]
pub struct QueueDeleteRequest {
    pub id: String,
}

const VMAIL_DIR: &str = "/var/vmail";
const POSTFIX_VIRTUAL_DOMAINS: &str = "/etc/postfix/virtual_domains";
const POSTFIX_VIRTUAL_MAILBOX: &str = "/etc/postfix/virtual_mailbox_maps";
const POSTFIX_VIRTUAL_ALIAS: &str = "/etc/postfix/virtual_alias_maps";
const DOVECOT_USERS: &str = "/etc/dovecot/users";
const DKIM_KEYS_DIR: &str = "/etc/dockpanel/dkim";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/mail/status", get(mail_status))
        .route("/mail/install", post(mail_install))
        .route("/mail/dkim/generate", post(dkim_generate))
        .route("/mail/domains/configure", post(domain_configure))
        .route("/mail/domains/remove", post(domain_remove))
        .route("/mail/sync", post(sync_config))
        .route("/mail/queue", get(queue_list))
        .route("/mail/queue/flush", post(queue_flush))
        .route("/mail/queue/delete", post(queue_delete))
}

// ── Mail server status + installation ────────────────────────────────────

async fn mail_status() -> Result<Json<serde_json::Value>, ApiErr> {
    let postfix = is_service_active("postfix").await;
    let dovecot = is_service_active("dovecot").await;
    let opendkim = is_service_active("opendkim").await;
    let postfix_installed = is_installed("postfix").await;
    let dovecot_installed = is_installed("dovecot-imapd").await;
    let opendkim_installed = is_installed("opendkim").await;
    let vmail_exists = Path::new(VMAIL_DIR).exists();

    let installed = postfix_installed && dovecot_installed;
    let running = postfix && dovecot;

    Ok(Json(serde_json::json!({
        "installed": installed,
        "running": running,
        "postfix": { "installed": postfix_installed, "running": postfix },
        "dovecot": { "installed": dovecot_installed, "running": dovecot },
        "opendkim": { "installed": opendkim_installed, "running": opendkim },
        "vmail_user": vmail_exists,
    })))
}

async fn mail_install() -> Result<Json<serde_json::Value>, ApiErr> {
    tracing::info!("Starting mail server installation...");

    // 1. Install packages
    let output = Command::new("apt-get")
        .args(["-o", "Dpkg::Options::=--force-confnew", "install", "-y",
               "postfix", "dovecot-imapd", "dovecot-pop3d", "dovecot-lmtpd", "opendkim", "opendkim-tools"])
        .env("DEBIAN_FRONTEND", "noninteractive")
        .output()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("apt install failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Package install failed: {}", stderr.chars().take(200).collect::<String>())));
    }

    // 2. Create vmail user (uid/gid 5000)
    let _ = Command::new("groupadd").args(["-g", "5000", "vmail"]).output().await;
    let _ = Command::new("useradd").args(["-g", "5000", "-u", "5000", "-d", VMAIL_DIR, "-s", "/usr/sbin/nologin", "-m", "vmail"]).output().await;
    tokio::fs::create_dir_all(VMAIL_DIR).await.ok();
    let _ = Command::new("chown").args(["-R", "vmail:vmail", VMAIL_DIR]).output().await;

    // 3. Create config directories
    tokio::fs::create_dir_all(DKIM_KEYS_DIR).await.ok();
    tokio::fs::create_dir_all("/etc/dockpanel/mail").await.ok();

    // 4. Write Postfix main.cf additions for virtual mailbox hosting
    let postfix_config = r#"
# DockPanel mail configuration
virtual_mailbox_domains = /etc/postfix/virtual_domains
virtual_mailbox_maps = hash:/etc/postfix/virtual_mailbox_maps
virtual_alias_maps = hash:/etc/postfix/virtual_alias_maps
virtual_mailbox_base = /var/vmail
virtual_uid_maps = static:5000
virtual_gid_maps = static:5000
virtual_transport = lmtp:unix:private/dovecot-lmtp

# SMTP authentication via Dovecot
smtpd_sasl_type = dovecot
smtpd_sasl_path = private/auth
smtpd_sasl_auth_enable = yes
smtpd_recipient_restrictions = permit_sasl_authenticated, permit_mynetworks, reject_unauth_destination

# TLS
smtpd_tls_security_level = may
smtpd_tls_auth_only = yes

# OpenDKIM milter
milter_protocol = 6
milter_default_action = accept
smtpd_milters = unix:opendkim/opendkim.sock
non_smtpd_milters = unix:opendkim/opendkim.sock
"#;

    // Append to main.cf if not already configured
    let main_cf = tokio::fs::read_to_string("/etc/postfix/main.cf").await.unwrap_or_default();
    if !main_cf.contains("DockPanel mail configuration") {
        let new_content = format!("{main_cf}\n{postfix_config}");
        write_file_atomic("/etc/postfix/main.cf", &new_content).await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write main.cf: {e}")))?;
    }

    // 5. Enable submission port (587) in master.cf
    let master_cf = tokio::fs::read_to_string("/etc/postfix/master.cf").await.unwrap_or_default();
    if !master_cf.contains("submission inet") || master_cf.contains("#submission inet") {
        let submission_config = "\nsubmission inet n - y - - smtpd\n  -o syslog_name=postfix/submission\n  -o smtpd_tls_security_level=encrypt\n  -o smtpd_sasl_auth_enable=yes\n  -o smtpd_recipient_restrictions=permit_sasl_authenticated,reject\n";
        let new_master = format!("{master_cf}\n{submission_config}");
        write_file_atomic("/etc/postfix/master.cf", &new_master).await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write master.cf: {e}")))?;
    }

    // 6. Write Dovecot configuration for virtual users
    let dovecot_config = r#"# DockPanel Dovecot configuration
protocols = imap pop3 lmtp

mail_location = maildir:/var/vmail/%d/%n
mail_uid = 5000
mail_gid = 5000
first_valid_uid = 5000

# Authentication
passdb {
  driver = passwd-file
  args = /etc/dovecot/users
}

userdb {
  driver = passwd-file
  args = /etc/dovecot/users
  default_fields = uid=5000 gid=5000 home=/var/vmail/%d/%n
}

# LMTP for Postfix delivery
service lmtp {
  unix_listener /var/spool/postfix/private/dovecot-lmtp {
    mode = 0600
    user = postfix
    group = postfix
  }
}

# SASL auth for Postfix
service auth {
  unix_listener /var/spool/postfix/private/auth {
    mode = 0660
    user = postfix
    group = postfix
  }
}

# SSL
ssl = required
"#;

    write_file_atomic("/etc/dovecot/conf.d/99-dockpanel.conf", dovecot_config).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write dovecot config: {e}")))?;

    // 7. Create empty map files
    write_file_atomic(POSTFIX_VIRTUAL_DOMAINS, "").await.ok();
    write_file_atomic(POSTFIX_VIRTUAL_MAILBOX, "").await.ok();
    write_file_atomic(POSTFIX_VIRTUAL_ALIAS, "").await.ok();
    write_file_atomic(DOVECOT_USERS, "").await.ok();
    let _ = Command::new("postmap").arg(POSTFIX_VIRTUAL_MAILBOX).output().await;
    let _ = Command::new("postmap").arg(POSTFIX_VIRTUAL_ALIAS).output().await;

    // 8. Configure OpenDKIM
    let opendkim_conf = "Syslog yes\nUMask 007\nSocket local:/var/spool/postfix/opendkim/opendkim.sock\nPidFile /run/opendkim/opendkim.pid\nOversignHeaders From\nTrustAnchorFile /usr/share/dns/root.key\nKeyTable /etc/dockpanel/dkim/key.table\nSigningTable refile:/etc/dockpanel/dkim/signing.table\nExternalIgnoreList /etc/dockpanel/dkim/trusted.hosts\nInternalHosts /etc/dockpanel/dkim/trusted.hosts\n";
    write_file_atomic("/etc/opendkim.conf", opendkim_conf).await.ok();

    let trusted_hosts = "127.0.0.1\nlocalhost\n";
    write_file_atomic("/etc/dockpanel/dkim/trusted.hosts", trusted_hosts).await.ok();
    write_file_atomic("/etc/dockpanel/dkim/key.table", "").await.ok();
    write_file_atomic("/etc/dockpanel/dkim/signing.table", "").await.ok();

    // Create opendkim socket directory in Postfix chroot
    tokio::fs::create_dir_all("/var/spool/postfix/opendkim").await.ok();
    let _ = Command::new("chown").args(["opendkim:postfix", "/var/spool/postfix/opendkim"]).output().await;

    // 9. Enable and start services
    let _ = Command::new("systemctl").args(["enable", "postfix", "dovecot", "opendkim"]).output().await;
    let _ = Command::new("systemctl").args(["restart", "postfix"]).output().await;
    let _ = Command::new("systemctl").args(["restart", "dovecot"]).output().await;
    let _ = Command::new("systemctl").args(["restart", "opendkim"]).output().await;

    tracing::info!("Mail server installation complete");

    Ok(ok("Mail server installed and configured"))
}

async fn is_service_active(name: &str) -> bool {
    Command::new("systemctl")
        .args(["is-active", "--quiet", name])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

async fn is_installed(package: &str) -> bool {
    Command::new("dpkg")
        .args(["-l", package])
        .output()
        .await
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains("ii"))
        .unwrap_or(false)
}

// ── DKIM key generation ─────────────────────────────────────────────────

async fn dkim_generate(
    Json(body): Json<DkimRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    let domain = body.domain.trim();
    let selector = body.selector.trim();

    if domain.is_empty() || selector.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Domain and selector required"));
    }

    // Create DKIM directory
    let key_dir = format!("{DKIM_KEYS_DIR}/{domain}");
    tokio::fs::create_dir_all(&key_dir).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to create DKIM dir: {e}")))?;

    let private_path = format!("{key_dir}/{selector}.private");
    let public_path = format!("{key_dir}/{selector}.public");

    // Generate RSA key pair
    let output = Command::new("openssl")
        .args(["genrsa", "-out", &private_path, "2048"])
        .output()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("openssl genrsa failed: {e}")))?;

    if !output.status.success() {
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, "Failed to generate DKIM private key"));
    }

    // Extract public key
    let output = Command::new("openssl")
        .args(["rsa", "-in", &private_path, "-pubout", "-out", &public_path])
        .output()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("openssl rsa failed: {e}")))?;

    if !output.status.success() {
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, "Failed to extract DKIM public key"));
    }

    // Read keys
    let private_key = tokio::fs::read_to_string(&private_path).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to read private key: {e}")))?;
    let public_key = tokio::fs::read_to_string(&public_path).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to read public key: {e}")))?;

    // Set permissions
    let _ = Command::new("chmod").args(["600", &private_path]).output().await;
    let _ = Command::new("chown").args(["opendkim:opendkim", &private_path]).output().await;

    tracing::info!("DKIM keys generated for {domain} (selector: {selector})");

    Ok(Json(serde_json::json!({
        "private_key": private_key,
        "public_key": public_key,
        "selector": selector,
    })))
}

// ── Domain configuration ────────────────────────────────────────────────

async fn domain_configure(
    Json(body): Json<DomainRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    let domain = body.domain.trim();

    // Create vmail directory for domain
    let maildir = format!("{VMAIL_DIR}/{domain}");
    tokio::fs::create_dir_all(&maildir).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to create maildir: {e}")))?;

    // Set ownership to vmail user
    let _ = Command::new("chown").args(["-R", "vmail:vmail", &maildir]).output().await;

    tracing::info!("Mail domain configured: {domain}");
    Ok(ok(&format!("Domain {domain} configured")))
}

async fn domain_remove(
    Json(body): Json<DomainRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    let domain = body.domain.trim();

    // Remove DKIM keys
    let key_dir = format!("{DKIM_KEYS_DIR}/{domain}");
    let _ = tokio::fs::remove_dir_all(&key_dir).await;

    // Note: we don't delete the maildir — that's destructive.
    // The sync_config will remove the domain from Postfix/Dovecot maps.

    tracing::info!("Mail domain removed: {domain}");
    Ok(ok(&format!("Domain {domain} removed")))
}

// ── Full sync (rebuild all Postfix/Dovecot config) ──────────────────────

async fn sync_config(
    Json(body): Json<SyncRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    // Ensure directories exist
    tokio::fs::create_dir_all(VMAIL_DIR).await.ok();
    tokio::fs::create_dir_all("/etc/postfix").await.ok();
    tokio::fs::create_dir_all("/etc/dovecot").await.ok();

    // 1. Write virtual_domains (one domain per line)
    let domains_content: String = body.domains.iter()
        .filter(|d| d.enabled)
        .map(|d| d.domain.clone())
        .collect::<Vec<_>>()
        .join("\n");
    write_file_atomic(POSTFIX_VIRTUAL_DOMAINS, &domains_content).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write virtual_domains: {e}")))?;

    // 2. Write virtual_mailbox_maps (email → maildir path)
    let mut mailbox_lines = Vec::new();
    for acc in &body.accounts {
        if !acc.enabled { continue; }
        let parts: Vec<&str> = acc.email.splitn(2, '@').collect();
        if parts.len() == 2 {
            mailbox_lines.push(format!("{}\t{}/{}/", acc.email, parts[1], parts[0]));
        }
    }
    // Add catch-all entries
    for domain in &body.domains {
        if let Some(catch_all) = &domain.catch_all {
            if !catch_all.is_empty() && domain.enabled {
                mailbox_lines.push(format!("@{}\t{}", domain.domain, catch_all));
            }
        }
    }
    write_file_atomic(POSTFIX_VIRTUAL_MAILBOX, &mailbox_lines.join("\n")).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write virtual_mailbox_maps: {e}")))?;

    // 3. Write virtual_alias_maps
    let mut alias_lines: Vec<String> = body.aliases.iter()
        .map(|a| format!("{}\t{}", a.source, a.destination))
        .collect();
    // Add forwarding from accounts
    for acc in &body.accounts {
        if let Some(fwd) = &acc.forward_to {
            if !fwd.is_empty() && acc.enabled {
                alias_lines.push(format!("{}\t{}", acc.email, fwd));
            }
        }
    }
    write_file_atomic(POSTFIX_VIRTUAL_ALIAS, &alias_lines.join("\n")).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write virtual_alias_maps: {e}")))?;

    // 4. Write Dovecot users file (email:{password_hash}::::/var/vmail/domain/user::quota=XM)
    let dovecot_lines: Vec<String> = body.accounts.iter()
        .filter(|a| a.enabled)
        .map(|a| {
            let parts: Vec<&str> = a.email.splitn(2, '@').collect();
            let maildir = if parts.len() == 2 {
                format!("{VMAIL_DIR}/{}/{}", parts[1], parts[0])
            } else {
                format!("{VMAIL_DIR}/{}", a.email)
            };
            format!("{}:{}::::{}::userdb_quota_rule=*:storage={}M", a.email, a.password_hash, maildir, a.quota_mb)
        })
        .collect();
    write_file_atomic(DOVECOT_USERS, &dovecot_lines.join("\n")).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write dovecot users: {e}")))?;

    // 5. Run postmap to rebuild hash tables
    let _ = Command::new("postmap").arg(POSTFIX_VIRTUAL_MAILBOX).output().await;
    let _ = Command::new("postmap").arg(POSTFIX_VIRTUAL_ALIAS).output().await;

    // 6. Reload Postfix and Dovecot
    let _ = Command::new("systemctl").args(["reload", "postfix"]).output().await;
    let _ = Command::new("systemctl").args(["reload", "dovecot"]).output().await;

    // 7. Create maildir directories for each account
    for acc in &body.accounts {
        if !acc.enabled { continue; }
        let parts: Vec<&str> = acc.email.splitn(2, '@').collect();
        if parts.len() == 2 {
            let maildir = format!("{VMAIL_DIR}/{}/{}", parts[1], parts[0]);
            tokio::fs::create_dir_all(&maildir).await.ok();
            let _ = Command::new("chown").args(["-R", "vmail:vmail", &maildir]).output().await;
        }
    }

    tracing::info!("Mail config synced: {} domains, {} accounts, {} aliases",
        body.domains.len(), body.accounts.len(), body.aliases.len());

    Ok(ok("Mail configuration synced"))
}

// ── Mail queue management ───────────────────────────────────────────────

async fn queue_list() -> Result<Json<serde_json::Value>, ApiErr> {
    let output = Command::new("mailq")
        .output()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("mailq failed: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    if stdout.contains("Mail queue is empty") || stdout.trim().is_empty() {
        return Ok(Json(serde_json::json!({ "queue": [], "count": 0 })));
    }

    // Parse mailq output
    let mut items = Vec::new();
    let mut current_id = String::new();
    let mut current_sender = String::new();
    let mut current_size = String::new();
    let mut current_time = String::new();
    let mut current_recipients = Vec::new();
    let mut current_status = String::new();

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('-') || trimmed.is_empty() || trimmed.starts_with("-- ") {
            if !current_id.is_empty() {
                items.push(serde_json::json!({
                    "id": current_id,
                    "sender": current_sender,
                    "size": current_size,
                    "arrival_time": current_time,
                    "recipients": current_recipients.join(", "),
                    "status": current_status,
                }));
                current_id.clear();
                current_recipients.clear();
                current_status.clear();
            }
            continue;
        }

        // Queue ID line: "A1B2C3D4E5*    1234 Mon Mar 15 10:00:00  sender@example.com"
        if trimmed.len() > 10 && trimmed.chars().next().map(|c| c.is_alphanumeric()).unwrap_or(false) {
            let parts: Vec<&str> = trimmed.splitn(2, char::is_whitespace).collect();
            if parts.len() >= 2 {
                let id_part = parts[0].trim_end_matches('*').trim_end_matches('!');
                current_id = id_part.to_string();
                current_status = if parts[0].contains('*') { "active".to_string() } else if parts[0].contains('!') { "hold".to_string() } else { "deferred".to_string() };

                // Parse size, time, sender from remaining
                let rest = parts[1].trim();
                let fields: Vec<&str> = rest.splitn(2, char::is_whitespace).collect();
                if fields.len() >= 2 {
                    current_size = fields[0].to_string();
                    // Find sender (last word)
                    let words: Vec<&str> = rest.split_whitespace().collect();
                    if let Some(sender) = words.last() {
                        current_sender = sender.to_string();
                    }
                    current_time = words[1..words.len().saturating_sub(1)].join(" ");
                }
            }
        } else if trimmed.contains('@') && !trimmed.contains(' ') {
            // Recipient line
            current_recipients.push(trimmed.to_string());
        }
    }

    // Don't forget the last entry
    if !current_id.is_empty() {
        items.push(serde_json::json!({
            "id": current_id,
            "sender": current_sender,
            "size": current_size,
            "arrival_time": current_time,
            "recipients": current_recipients.join(", "),
            "status": current_status,
        }));
    }

    Ok(Json(serde_json::json!({ "queue": items, "count": items.len() })))
}

async fn queue_flush() -> Result<Json<serde_json::Value>, ApiErr> {
    let output = Command::new("postqueue")
        .arg("-f")
        .output()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("postqueue -f failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Flush failed: {stderr}")));
    }

    tracing::info!("Mail queue flushed");
    Ok(ok("Queue flushed"))
}

async fn queue_delete(
    Json(body): Json<QueueDeleteRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    let output = Command::new("postsuper")
        .args(["-d", &body.id])
        .output()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("postsuper -d failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Delete failed: {stderr}")));
    }

    tracing::info!("Queued message {} deleted", body.id);
    Ok(ok("Message deleted from queue"))
}

// ── Helper ──────────────────────────────────────────────────────────────

async fn write_file_atomic(path: &str, content: &str) -> Result<(), String> {
    let tmp_path = format!("{path}.tmp");
    tokio::fs::write(&tmp_path, content).await.map_err(|e| e.to_string())?;
    tokio::fs::rename(&tmp_path, path).await.map_err(|e| e.to_string())?;
    Ok(())
}
