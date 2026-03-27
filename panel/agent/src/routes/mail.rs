use axum::{
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use crate::safe_cmd::safe_command;
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

#[derive(Deserialize)]
struct RateLimitRequest {
    rate: String, // e.g., "100/hour", "500/day"
}

#[derive(Deserialize)]
struct MailboxBackupRequest {
    email: String,
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
        // Rspamd spam filter
        .route("/mail/rspamd/install", post(rspamd_install))
        .route("/mail/rspamd/status", get(rspamd_status))
        .route("/mail/rspamd/toggle", post(rspamd_toggle))
        // Webmail (Roundcube)
        .route("/mail/webmail/install", post(webmail_install))
        .route("/mail/webmail/status", get(webmail_status))
        .route("/mail/webmail/remove", post(webmail_remove))
        // SMTP Relay
        .route("/mail/relay/configure", post(relay_configure))
        .route("/mail/relay/status", get(relay_status))
        .route("/mail/relay/remove", post(relay_remove))
        // Logs & Storage
        .route("/mail/logs", get(mail_logs))
        .route("/mail/storage", get(storage_usage))
        // Rate Limiting
        .route("/mail/rate-limit/set", post(rate_limit_set))
        .route("/mail/rate-limit/status", get(rate_limit_status))
        .route("/mail/rate-limit/remove", post(rate_limit_remove))
        // Mailbox Backup/Restore
        .route("/mail/backup", post(mailbox_backup))
        .route("/mail/restore", post(mailbox_restore))
        .route("/mail/backups", get(mailbox_backups))
        .route("/mail/backups/delete", post(mailbox_backup_delete))
        // TLS Enforcement
        .route("/mail/tls/status", get(tls_status))
        .route("/mail/tls/enforce", post(tls_enforce))
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
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        safe_command("apt-get")
            .args(["-o", "Dpkg::Options::=--force-confnew", "install", "-y",
                   "postfix", "dovecot-imapd", "dovecot-pop3d", "dovecot-lmtpd", "opendkim", "opendkim-tools"])
            .env("DEBIAN_FRONTEND", "noninteractive")
            .output()
    ).await
        .map_err(|_| err(StatusCode::GATEWAY_TIMEOUT, "Mail package installation timed out (300s)"))?
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("apt install failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Package install failed: {}", stderr.chars().take(200).collect::<String>())));
    }

    // 2. Create vmail user (uid/gid 5000)
    let _ = safe_command("groupadd").args(["-g", "5000", "vmail"]).output().await;
    let _ = safe_command("useradd").args(["-g", "5000", "-u", "5000", "-d", VMAIL_DIR, "-s", "/usr/sbin/nologin", "-m", "vmail"]).output().await;
    tokio::fs::create_dir_all(VMAIL_DIR).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to create vmail dir: {e}")))?;
    let _ = safe_command("chown").args(["-R", "vmail:vmail", VMAIL_DIR]).output().await;

    // 3. Create config directories
    tokio::fs::create_dir_all(DKIM_KEYS_DIR).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to create DKIM dir: {e}")))?;
    tokio::fs::create_dir_all("/etc/dockpanel/mail").await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to create mail config dir: {e}")))?;

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

# SMTP smuggling prevention (CVE-2023-51764)
smtpd_forbid_bare_newline = yes

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
    write_file_atomic(POSTFIX_VIRTUAL_DOMAINS, "").await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write virtual_domains: {e}")))?;
    write_file_atomic(POSTFIX_VIRTUAL_MAILBOX, "").await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write virtual_mailbox_maps: {e}")))?;
    write_file_atomic(POSTFIX_VIRTUAL_ALIAS, "").await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write virtual_alias_maps: {e}")))?;
    write_file_atomic(DOVECOT_USERS, "").await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write dovecot users: {e}")))?;
    let _ = safe_command("postmap").arg(POSTFIX_VIRTUAL_MAILBOX).output().await;
    let _ = safe_command("postmap").arg(POSTFIX_VIRTUAL_ALIAS).output().await;

    // 8. Configure OpenDKIM
    let opendkim_conf = "Syslog yes\nUMask 007\nSocket local:/var/spool/postfix/opendkim/opendkim.sock\nPidFile /run/opendkim/opendkim.pid\nOversignHeaders From\nTrustAnchorFile /usr/share/dns/root.key\nKeyTable /etc/dockpanel/dkim/key.table\nSigningTable refile:/etc/dockpanel/dkim/signing.table\nExternalIgnoreList /etc/dockpanel/dkim/trusted.hosts\nInternalHosts /etc/dockpanel/dkim/trusted.hosts\n";
    write_file_atomic("/etc/opendkim.conf", opendkim_conf).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write opendkim.conf: {e}")))?;

    let trusted_hosts = "127.0.0.1\nlocalhost\n";
    write_file_atomic("/etc/dockpanel/dkim/trusted.hosts", trusted_hosts).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write trusted.hosts: {e}")))?;
    write_file_atomic("/etc/dockpanel/dkim/key.table", "").await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write key.table: {e}")))?;
    write_file_atomic("/etc/dockpanel/dkim/signing.table", "").await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write signing.table: {e}")))?;

    // Create opendkim socket directory in Postfix chroot
    tokio::fs::create_dir_all("/var/spool/postfix/opendkim").await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to create opendkim socket dir: {e}")))?;
    let _ = safe_command("chown").args(["opendkim:postfix", "/var/spool/postfix/opendkim"]).output().await;

    // 9. Enable and start services
    if let Ok(out) = safe_command("systemctl").args(["enable", "postfix", "dovecot", "opendkim"]).output().await {
        if !out.status.success() {
            tracing::warn!("Failed to enable mail services: {}", String::from_utf8_lossy(&out.stderr));
        }
    }
    for service in &["postfix", "dovecot", "opendkim"] {
        if let Ok(out) = safe_command("systemctl").args(["restart", service]).output().await {
            if !out.status.success() {
                tracing::warn!("Failed to restart {service}: {}", String::from_utf8_lossy(&out.stderr));
            }
        } else {
            tracing::warn!("Failed to execute systemctl restart {service}");
        }
    }

    tracing::info!("Mail server installation complete");

    Ok(ok("Mail server installed and configured"))
}

async fn is_service_active(name: &str) -> bool {
    safe_command("systemctl")
        .args(["is-active", "--quiet", name])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

async fn is_installed(package: &str) -> bool {
    safe_command("dpkg")
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

    if domain.contains('/') || domain.contains('\\') || domain.contains("..")
        || !domain.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    if selector.contains('/') || selector.contains('\\') || selector.contains("..")
        || !selector.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid selector format"));
    }

    // Create DKIM directory
    let key_dir = format!("{DKIM_KEYS_DIR}/{domain}");
    tokio::fs::create_dir_all(&key_dir).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to create DKIM dir: {e}")))?;

    let private_path = format!("{key_dir}/{selector}.private");
    let public_path = format!("{key_dir}/{selector}.public");

    // Generate RSA key pair
    let output = safe_command("openssl")
        .args(["genrsa", "-out", &private_path, "2048"])
        .output()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("openssl genrsa failed: {e}")))?;

    if !output.status.success() {
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, "Failed to generate DKIM private key"));
    }

    // Extract public key
    let output = safe_command("openssl")
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
    let _ = safe_command("chmod").args(["600", &private_path]).output().await;
    let _ = safe_command("chown").args(["opendkim:opendkim", &private_path]).output().await;

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

    if domain.contains('/') || domain.contains('\\') || domain.contains("..")
        || !domain.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }

    // Create vmail directory for domain
    let maildir = format!("{VMAIL_DIR}/{domain}");
    tokio::fs::create_dir_all(&maildir).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to create maildir: {e}")))?;

    // Set ownership to vmail user
    let _ = safe_command("chown").args(["-R", "vmail:vmail", &maildir]).output().await;

    tracing::info!("Mail domain configured: {domain}");
    Ok(ok(&format!("Domain {domain} configured")))
}

async fn domain_remove(
    Json(body): Json<DomainRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    let domain = body.domain.trim();

    if domain.contains('/') || domain.contains('\\') || domain.contains("..")
        || !domain.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }

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
    // Validate account and alias fields for injection attacks
    for acc in &body.accounts {
        // Strict email character set: only safe chars for Postfix maps
        if !acc.email.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | '-' | '+')) {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid characters in email address"));
        }
        if !acc.email.contains('@') || acc.email.matches('@').count() != 1 {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid email format"));
        }
        // Dovecot users file uses ':' as field separator — reject in password hash
        if acc.password_hash.contains(':') || acc.password_hash.contains('\n')
            || acc.password_hash.contains('\r') || acc.password_hash.contains('\0')
            || acc.password_hash.contains('\t') {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid characters in password hash"));
        }
        // Validate forward_to if present
        if let Some(fwd) = &acc.forward_to {
            if !fwd.is_empty() && !fwd.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | '-' | '+')) {
                return Err(err(StatusCode::BAD_REQUEST, "Invalid characters in forward_to address"));
            }
        }
    }
    for alias in &body.aliases {
        if !alias.source.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | '-' | '+'))
            || !alias.destination.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | '-' | '+' | ',')) {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid characters in alias data"));
        }
    }
    // Validate catch-all entries
    for domain in &body.domains {
        if let Some(catch_all) = &domain.catch_all {
            if !catch_all.is_empty() && !catch_all.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | '-' | '+' | '/')) {
                return Err(err(StatusCode::BAD_REQUEST, "Invalid characters in catch-all address"));
            }
        }
    }

    // Ensure directories exist
    tokio::fs::create_dir_all(VMAIL_DIR).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to create vmail dir: {e}")))?;
    tokio::fs::create_dir_all("/etc/postfix").await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to create postfix dir: {e}")))?;
    tokio::fs::create_dir_all("/etc/dovecot").await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to create dovecot dir: {e}")))?;

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
    let _ = safe_command("postmap").arg(POSTFIX_VIRTUAL_MAILBOX).output().await;
    let _ = safe_command("postmap").arg(POSTFIX_VIRTUAL_ALIAS).output().await;

    // 6. Reload Postfix and Dovecot
    for service in &["postfix", "dovecot"] {
        if let Ok(out) = safe_command("systemctl").args(["reload", service]).output().await {
            if !out.status.success() {
                tracing::warn!("Failed to reload {service}: {}", String::from_utf8_lossy(&out.stderr));
            }
        } else {
            tracing::warn!("Failed to execute systemctl reload {service}");
        }
    }

    // 7. Create maildir directories for each account
    for acc in &body.accounts {
        if !acc.enabled { continue; }
        let parts: Vec<&str> = acc.email.splitn(2, '@').collect();
        if parts.len() == 2 {
            let maildir = format!("{VMAIL_DIR}/{}/{}", parts[1], parts[0]);
            if let Err(e) = tokio::fs::create_dir_all(&maildir).await {
                tracing::warn!("Failed to create maildir {maildir}: {e}");
            }
            let _ = safe_command("chown").args(["-R", "vmail:vmail", &maildir]).output().await;
        }
    }

    tracing::info!("Mail config synced: {} domains, {} accounts, {} aliases",
        body.domains.len(), body.accounts.len(), body.aliases.len());

    Ok(ok("Mail configuration synced"))
}

// ── Mail queue management ───────────────────────────────────────────────

async fn queue_list() -> Result<Json<serde_json::Value>, ApiErr> {
    let output = safe_command("mailq")
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
    let output = safe_command("postqueue")
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
    let id = body.id.trim();
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_hexdigit()) || id.len() > 20 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid queue ID format"));
    }

    let output = safe_command("postsuper")
        .args(["-d", id])
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

// ── Rspamd spam filter ───────────────────────────────────────────────────

/// POST /mail/rspamd/install — Install and configure Rspamd.
async fn rspamd_install() -> Result<Json<serde_json::Value>, ApiErr> {
    tracing::info!("Installing Rspamd spam filter...");

    // Install rspamd
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        safe_command("apt-get")
            .args(["-o", "Dpkg::Options::=--force-confnew", "install", "-y", "rspamd", "redis-server"])
            .env("DEBIAN_FRONTEND", "noninteractive")
            .output()
    ).await
        .map_err(|_| err(StatusCode::GATEWAY_TIMEOUT, "Rspamd installation timed out (300s)"))?
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Install failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Rspamd install failed: {}", &stderr[..200.min(stderr.len())])));
    }

    // Configure Rspamd milter for Postfix
    let main_cf = tokio::fs::read_to_string("/etc/postfix/main.cf").await.unwrap_or_default();
    if !main_cf.contains("rspamd") {
        // Add Rspamd milter (alongside OpenDKIM)
        let new_cf = main_cf.replace(
            "smtpd_milters = unix:opendkim/opendkim.sock",
            "smtpd_milters = unix:opendkim/opendkim.sock, inet:localhost:11332"
        ).replace(
            "non_smtpd_milters = unix:opendkim/opendkim.sock",
            "non_smtpd_milters = unix:opendkim/opendkim.sock, inet:localhost:11332"
        );
        write_file_atomic("/etc/postfix/main.cf", &new_cf).await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Config write failed: {e}")))?;
    }

    // Enable and start
    if let Ok(out) = safe_command("systemctl").args(["enable", "rspamd", "redis-server"]).output().await {
        if !out.status.success() {
            tracing::warn!("Failed to enable rspamd/redis: {}", String::from_utf8_lossy(&out.stderr));
        }
    }
    for service in &["redis-server", "rspamd"] {
        if let Ok(out) = safe_command("systemctl").args(["restart", service]).output().await {
            if !out.status.success() {
                tracing::warn!("Failed to restart {service}: {}", String::from_utf8_lossy(&out.stderr));
            }
        } else {
            tracing::warn!("Failed to execute systemctl restart {service}");
        }
    }
    if let Ok(out) = safe_command("systemctl").args(["reload", "postfix"]).output().await {
        if !out.status.success() {
            tracing::warn!("Failed to reload postfix: {}", String::from_utf8_lossy(&out.stderr));
        }
    }

    tracing::info!("Rspamd installed and configured");
    Ok(ok("Rspamd spam filter installed"))
}

/// GET /mail/rspamd/status — Check Rspamd status.
async fn rspamd_status() -> Json<serde_json::Value> {
    let installed = is_installed("rspamd").await;
    let running = is_service_active("rspamd").await;
    let redis = is_service_active("redis-server").await;
    Json(serde_json::json!({ "installed": installed, "running": running, "redis": redis }))
}

/// POST /mail/rspamd/toggle — Enable/disable Rspamd.
async fn rspamd_toggle(Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiErr> {
    let enable = body.get("enable").and_then(|v| v.as_bool()).unwrap_or(true);
    if enable {
        for (action, service) in &[("start", "rspamd"), ("enable", "rspamd")] {
            if let Ok(out) = safe_command("systemctl").args([*action, service]).output().await {
                if !out.status.success() {
                    tracing::warn!("Failed to {action} {service}: {}", String::from_utf8_lossy(&out.stderr));
                }
            } else {
                tracing::warn!("Failed to execute systemctl {action} {service}");
            }
        }
    } else {
        for (action, service) in &[("stop", "rspamd"), ("disable", "rspamd")] {
            if let Ok(out) = safe_command("systemctl").args([*action, service]).output().await {
                if !out.status.success() {
                    tracing::warn!("Failed to {action} {service}: {}", String::from_utf8_lossy(&out.stderr));
                }
            } else {
                tracing::warn!("Failed to execute systemctl {action} {service}");
            }
        }
    }
    Ok(ok(if enable { "Rspamd enabled" } else { "Rspamd disabled" }))
}

// ── Webmail (Roundcube) ─────────────────────────────────────────────────

/// POST /mail/webmail/install — Deploy Roundcube webmail via Docker.
async fn webmail_install(Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiErr> {
    let domain = body.get("domain").and_then(|v| v.as_str()).unwrap_or("localhost");
    let port = body.get("port").and_then(|v| v.as_u64()).unwrap_or(8888) as u16;

    tracing::info!("Installing Roundcube webmail on port {port}...");

    // Run Roundcube as Docker container
    let output = safe_command("docker")
        .args([
            "run", "-d",
            "--name", "dockpanel-roundcube",
            "--restart", "unless-stopped",
            "-p", &format!("127.0.0.1:{port}:80"),
            "-e", &format!("ROUNDCUBEMAIL_DEFAULT_HOST=ssl://{domain}"),
            "-e", "ROUNDCUBEMAIL_DEFAULT_PORT=993",
            "-e", &format!("ROUNDCUBEMAIL_SMTP_SERVER=tls://{domain}"),
            "-e", "ROUNDCUBEMAIL_SMTP_PORT=587",
            "-e", "ROUNDCUBEMAIL_UPLOAD_MAX_FILESIZE=25M",
            "-l", "dockpanel.managed=true",
            "-l", "dockpanel.app.template=roundcube",
            "-l", "dockpanel.app.name=roundcube",
            "roundcube/roundcubemail:latest",
        ])
        .output().await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Docker failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("already in use") {
            let _ = safe_command("docker").args(["rm", "-f", "dockpanel-roundcube"]).output().await;
            return Err(err(StatusCode::CONFLICT, "Roundcube container already exists. Remove it first or restart."));
        }
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Roundcube deploy failed: {}", &stderr[..200.min(stderr.len())])));
    }

    tracing::info!("Roundcube webmail deployed on port {port}");
    Ok(Json(serde_json::json!({ "ok": true, "port": port })))
}

/// GET /mail/webmail/status — Check if Roundcube is running.
async fn webmail_status() -> Json<serde_json::Value> {
    let output = safe_command("docker")
        .args(["inspect", "--format", "{{.State.Running}}", "dockpanel-roundcube"])
        .output().await;
    let running = output.map(|o| String::from_utf8_lossy(&o.stdout).trim() == "true").unwrap_or(false);

    // Get port
    let port_output = safe_command("docker")
        .args(["inspect", "--format", "{{range .NetworkSettings.Ports}}{{range .}}{{.HostPort}}{{end}}{{end}}", "dockpanel-roundcube"])
        .output().await;
    let port = port_output.ok().and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u16>().ok()).unwrap_or(0);

    Json(serde_json::json!({ "installed": running || port > 0, "running": running, "port": port }))
}

/// POST /mail/webmail/remove — Remove Roundcube container.
async fn webmail_remove() -> Result<Json<serde_json::Value>, ApiErr> {
    let _ = safe_command("docker").args(["rm", "-f", "dockpanel-roundcube"]).output().await;
    Ok(ok("Roundcube removed"))
}

// ── SMTP Relay ──────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct RelayConfig {
    host: String,
    port: u16,
    username: String,
    password: String,
}

/// POST /mail/relay/configure — Set up SMTP relay (smarthost).
async fn relay_configure(Json(body): Json<RelayConfig>) -> Result<Json<serde_json::Value>, ApiErr> {
    if body.host.is_empty() { return Err(err(StatusCode::BAD_REQUEST, "Relay host required")); }

    if body.host.contains('\n') || body.host.contains('\r') || body.host.contains('\0')
        || body.username.contains('\n') || body.username.contains('\0')
        || body.password.contains('\n') || body.password.contains('\0') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid characters in relay config"));
    }

    // Write SASL password file
    let sasl_content = format!("[{}]:{} {}:{}\n", body.host, body.port, body.username, body.password);
    write_file_atomic("/etc/postfix/sasl_passwd", &sasl_content).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write sasl_passwd: {e}")))?;

    // Set permissions
    let _ = safe_command("chmod").args(["600", "/etc/postfix/sasl_passwd"]).output().await;
    let _ = safe_command("postmap").arg("/etc/postfix/sasl_passwd").output().await;

    // Update Postfix main.cf
    let main_cf = tokio::fs::read_to_string("/etc/postfix/main.cf").await.unwrap_or_default();

    // Remove existing relay config lines
    let cleaned: String = main_cf.lines()
        .filter(|l| !l.starts_with("relayhost") && !l.starts_with("smtp_sasl_") && !l.starts_with("smtp_tls_") && !l.contains("# DockPanel relay"))
        .collect::<Vec<_>>().join("\n");

    let relay_config = format!(
        "\n# DockPanel relay configuration\nrelayhost = [{}]:{}\nsmtp_sasl_auth_enable = yes\nsmtp_sasl_password_maps = hash:/etc/postfix/sasl_passwd\nsmtp_sasl_security_options = noanonymous\nsmtp_tls_security_level = encrypt\nsmtp_tls_CAfile = /etc/ssl/certs/ca-certificates.crt\n",
        body.host, body.port
    );

    write_file_atomic("/etc/postfix/main.cf", &format!("{cleaned}{relay_config}")).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Config write failed: {e}")))?;

    let _ = safe_command("systemctl").args(["reload", "postfix"]).output().await;

    tracing::info!("SMTP relay configured: [{}]:{}", body.host, body.port);
    Ok(ok("SMTP relay configured"))
}

/// GET /mail/relay/status — Check current relay configuration.
async fn relay_status() -> Json<serde_json::Value> {
    let main_cf = tokio::fs::read_to_string("/etc/postfix/main.cf").await.unwrap_or_default();
    let relayhost = main_cf.lines()
        .find(|l| l.starts_with("relayhost"))
        .map(|l| l.split('=').nth(1).unwrap_or("").trim().to_string());

    Json(serde_json::json!({
        "configured": relayhost.is_some() && !relayhost.as_ref().unwrap().is_empty(),
        "relayhost": relayhost.unwrap_or_default(),
    }))
}

/// POST /mail/relay/remove — Remove SMTP relay configuration.
async fn relay_remove() -> Result<Json<serde_json::Value>, ApiErr> {
    let main_cf = tokio::fs::read_to_string("/etc/postfix/main.cf").await.unwrap_or_default();
    let cleaned: String = main_cf.lines()
        .filter(|l| !l.starts_with("relayhost") && !l.starts_with("smtp_sasl_") && !l.starts_with("smtp_tls_") && !l.contains("# DockPanel relay"))
        .collect::<Vec<_>>().join("\n");

    write_file_atomic("/etc/postfix/main.cf", &cleaned).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Config write failed: {e}")))?;

    let _ = tokio::fs::remove_file("/etc/postfix/sasl_passwd").await;
    let _ = tokio::fs::remove_file("/etc/postfix/sasl_passwd.db").await;
    let _ = safe_command("systemctl").args(["reload", "postfix"]).output().await;

    Ok(ok("SMTP relay removed"))
}

// ── Mail Logs ───────────────────────────────────────────────────────────

/// GET /mail/logs — Parse mail.log for recent activity and stats.
async fn mail_logs() -> Result<Json<serde_json::Value>, ApiErr> {
    // Read last portion of mail.log (tail -5000 to avoid reading huge files)
    let output = safe_command("tail")
        .args(["-n", "5000", "/var/log/mail.log"])
        .output().await;
    let content = output.ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();

    let mut sent = 0u32;
    let mut received = 0u32;
    let mut bounced = 0u32;
    let mut rejected = 0u32;
    let mut recent: Vec<serde_json::Value> = Vec::new();

    for line in content.lines().rev() {
        if line.contains("status=sent") { sent += 1; }
        if line.contains("status=bounced") { bounced += 1; }
        if line.contains("NOQUEUE: reject") || line.contains("status=rejected") { rejected += 1; }
        if line.contains("delivered to maildir") || line.contains("lmtp(") { received += 1; }

        // Collect recent entries (last 50 interesting lines)
        if recent.len() < 50 && (line.contains("status=") || line.contains("NOQUEUE") || line.contains("delivered")) {
            let time = if line.len() >= 15 { &line[..15] } else { "" };
            let is_error = line.contains("bounced") || line.contains("reject") || line.contains("error");
            recent.push(serde_json::json!({
                "time": time,
                "message": if line.len() > 16 { &line[16..line.len().min(200)] } else { line },
                "level": if is_error { "error" } else { "info" },
            }));
        }
    }

    Ok(Json(serde_json::json!({
        "stats": { "sent": sent, "received": received, "bounced": bounced, "rejected": rejected },
        "recent": recent,
    })))
}

// ── Storage Usage ───────────────────────────────────────────────────────

/// GET /mail/storage — Get storage usage for all mailboxes.
async fn storage_usage() -> Result<Json<serde_json::Value>, ApiErr> {
    let mut usage = Vec::new();

    // Scan /var/vmail for domain/user directories
    let mut domains = match tokio::fs::read_dir("/var/vmail").await {
        Ok(d) => d,
        Err(_) => return Ok(Json(serde_json::json!({ "accounts": [] }))),
    };

    while let Ok(Some(domain_entry)) = domains.next_entry().await {
        if !domain_entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) { continue; }
        let domain = domain_entry.file_name().to_string_lossy().to_string();

        let mut users = match tokio::fs::read_dir(domain_entry.path()).await {
            Ok(u) => u,
            Err(_) => continue,
        };

        while let Ok(Some(user_entry)) = users.next_entry().await {
            if !user_entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) { continue; }
            let user = user_entry.file_name().to_string_lossy().to_string();

            // Get directory size using du
            let output = safe_command("du")
                .args(["-sb", &user_entry.path().to_string_lossy()])
                .output().await;

            let bytes: u64 = output.ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().next().unwrap_or("0").parse().unwrap_or(0))
                .unwrap_or(0);

            usage.push(serde_json::json!({
                "email": format!("{user}@{domain}"),
                "bytes": bytes,
                "mb": (bytes as f64 / 1024.0 / 1024.0 * 10.0).round() / 10.0,
            }));
        }
    }

    Ok(Json(serde_json::json!({ "accounts": usage })))
}

// ── Rate Limiting ───────────────────────────────────────────────────────

/// POST /mail/rate-limit/set — Set global outbound rate limit.
async fn rate_limit_set(Json(body): Json<RateLimitRequest>) -> Result<Json<serde_json::Value>, ApiErr> {
    if body.rate.is_empty() { return Err(err(StatusCode::BAD_REQUEST, "Rate required")); }

    // Parse rate: "100/hour" → smtp_destination_rate_delay = 36s (3600/100)
    // "500/day" → smtp_destination_rate_delay = 172s (86400/500)
    let parts: Vec<&str> = body.rate.split('/').collect();
    if parts.len() != 2 { return Err(err(StatusCode::BAD_REQUEST, "Rate format: N/hour or N/day")); }
    let count: u32 = parts[0].parse().map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid count"))?;
    if count == 0 { return Err(err(StatusCode::BAD_REQUEST, "Count must be > 0")); }
    let period_secs: u32 = match parts[1] {
        "hour" => 3600,
        "day" => 86400,
        "minute" => 60,
        _ => return Err(err(StatusCode::BAD_REQUEST, "Period must be minute, hour, or day")),
    };
    let delay = period_secs / count;

    // Update Postfix config
    let main_cf = tokio::fs::read_to_string("/etc/postfix/main.cf").await.unwrap_or_default();
    let cleaned: String = main_cf.lines()
        .filter(|l| !l.starts_with("smtp_destination_rate_delay") && !l.starts_with("smtp_extra_recipient_limit") && !l.contains("# DockPanel rate limit"))
        .collect::<Vec<_>>().join("\n");

    let rate_config = format!("\n# DockPanel rate limit: {}\nsmtp_destination_rate_delay = {}s\nsmtp_extra_recipient_limit = {}\n", body.rate, delay, count.min(50));

    write_file_atomic("/etc/postfix/main.cf", &format!("{cleaned}{rate_config}")).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Config write failed: {e}")))?;

    let _ = safe_command("systemctl").args(["reload", "postfix"]).output().await;

    tracing::info!("Mail rate limit set: {} (delay: {}s)", body.rate, delay);
    Ok(ok(&format!("Rate limit set: {}", body.rate)))
}

/// GET /mail/rate-limit/status — Get current rate limit.
async fn rate_limit_status() -> Json<serde_json::Value> {
    let main_cf = tokio::fs::read_to_string("/etc/postfix/main.cf").await.unwrap_or_default();
    let rate_line = main_cf.lines().find(|l| l.contains("# DockPanel rate limit:"));
    let configured = rate_line.is_some();
    let rate = rate_line.and_then(|l| l.split(':').nth(1)).unwrap_or("").trim().to_string();
    Json(serde_json::json!({ "configured": configured, "rate": rate }))
}

/// POST /mail/rate-limit/remove — Remove rate limit.
async fn rate_limit_remove() -> Result<Json<serde_json::Value>, ApiErr> {
    let main_cf = tokio::fs::read_to_string("/etc/postfix/main.cf").await.unwrap_or_default();
    let cleaned: String = main_cf.lines()
        .filter(|l| !l.starts_with("smtp_destination_rate_delay") && !l.starts_with("smtp_extra_recipient_limit") && !l.contains("# DockPanel rate limit"))
        .collect::<Vec<_>>().join("\n");
    write_file_atomic("/etc/postfix/main.cf", &cleaned).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Config write failed: {e}")))?;
    let _ = safe_command("systemctl").args(["reload", "postfix"]).output().await;
    Ok(ok("Rate limit removed"))
}

// ── Mailbox Backup/Restore ──────────────────────────────────────────────

/// POST /mail/backup — Create a backup of a mailbox (tar.gz of maildir).
async fn mailbox_backup(Json(body): Json<MailboxBackupRequest>) -> Result<Json<serde_json::Value>, ApiErr> {
    let email = body.email.trim();
    if email.is_empty() || !email.contains('@') { return Err(err(StatusCode::BAD_REQUEST, "Invalid email")); }

    let parts: Vec<&str> = email.splitn(2, '@').collect();
    let (user, domain) = (parts[0], parts[1]);

    if user.contains('/') || user.contains('\\') || user.contains("..")
        || domain.contains('/') || domain.contains('\\') || domain.contains("..") {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid email format"));
    }

    let maildir = format!("/var/vmail/{domain}/{user}");

    if !Path::new(&maildir).exists() {
        return Err(err(StatusCode::NOT_FOUND, "Mailbox directory not found"));
    }

    let backup_dir = "/var/lib/dockpanel/mail-backups";
    tokio::fs::create_dir_all(backup_dir).await.ok();

    let timestamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let backup_file = format!("{backup_dir}/{user}_{domain}_{timestamp}.tar.gz");

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        safe_command("tar").args(["czf", &backup_file, "-C", &format!("/var/vmail/{domain}"), user]).output()
    ).await
        .map_err(|_| err(StatusCode::GATEWAY_TIMEOUT, "Backup timed out"))?
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Backup failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Backup failed: {stderr}")));
    }

    // Get file size
    let size = tokio::fs::metadata(&backup_file).await.map(|m| m.len()).unwrap_or(0);

    tracing::info!("Mailbox backed up: {email} -> {backup_file} ({size} bytes)");
    Ok(Json(serde_json::json!({ "ok": true, "file": backup_file, "size": size })))
}

/// POST /mail/restore — Restore a mailbox from backup.
async fn mailbox_restore(Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiErr> {
    let email = body.get("email").and_then(|v| v.as_str()).unwrap_or("");
    let backup_file = body.get("file").and_then(|v| v.as_str()).unwrap_or("");

    if email.is_empty() || !email.contains('@') { return Err(err(StatusCode::BAD_REQUEST, "Invalid email")); }
    if backup_file.is_empty() || !backup_file.starts_with("/var/lib/dockpanel/mail-backups/") {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid backup file path"));
    }
    if backup_file.contains("..") { return Err(err(StatusCode::BAD_REQUEST, "Path traversal not allowed")); }
    if !Path::new(backup_file).exists() { return Err(err(StatusCode::NOT_FOUND, "Backup file not found")); }

    let parts: Vec<&str> = email.splitn(2, '@').collect();
    let (user, domain) = (parts[0], parts[1]);
    let maildir = format!("/var/vmail/{domain}");

    // Restore
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        safe_command("tar").args(["xzf", backup_file, "-C", &maildir]).output()
    ).await
        .map_err(|_| err(StatusCode::GATEWAY_TIMEOUT, "Restore timed out"))?
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Restore failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Restore failed: {stderr}")));
    }

    // Fix permissions
    let _ = safe_command("chown").args(["-R", "vmail:vmail", &format!("{maildir}/{user}")]).output().await;

    tracing::info!("Mailbox restored: {email} from {backup_file}");
    Ok(ok(&format!("Mailbox {email} restored")))
}

/// GET /mail/backups — List available mailbox backups.
async fn mailbox_backups() -> Result<Json<serde_json::Value>, ApiErr> {
    let backup_dir = "/var/lib/dockpanel/mail-backups";
    let mut backups = Vec::new();

    let mut entries = match tokio::fs::read_dir(backup_dir).await {
        Ok(e) => e,
        Err(_) => return Ok(Json(serde_json::json!({ "backups": [] }))),
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".tar.gz") { continue; }
        let size = entry.metadata().await.map(|m| m.len()).unwrap_or(0);
        let path = entry.path().to_string_lossy().to_string();

        // Parse email from filename: user_domain_timestamp.tar.gz
        let parts: Vec<&str> = name.trim_end_matches(".tar.gz").rsplitn(2, '_').collect();
        let email_hint = if parts.len() >= 2 { parts[1].replacen('_', "@", 1) } else { name.clone() };

        backups.push(serde_json::json!({ "file": path, "name": name, "email": email_hint, "size": size }));
    }

    // Sort by name (timestamp in filename = chronological)
    backups.sort_by(|a, b| b["name"].as_str().unwrap_or("").cmp(a["name"].as_str().unwrap_or("")));

    Ok(Json(serde_json::json!({ "backups": backups })))
}

/// POST /mail/backups/delete — Delete a backup file.
async fn mailbox_backup_delete(Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiErr> {
    let file = body.get("file").and_then(|v| v.as_str()).unwrap_or("");
    if file.is_empty() || !file.starts_with("/var/lib/dockpanel/mail-backups/") || file.contains("..") {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid backup file"));
    }
    tokio::fs::remove_file(file).await.map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Delete failed: {e}")))?;
    Ok(ok("Backup deleted"))
}

// ── TLS Enforcement ─────────────────────────────────────────────────────

/// GET /mail/tls/status — Check TLS configuration in Postfix and Dovecot.
async fn tls_status() -> Json<serde_json::Value> {
    let main_cf = tokio::fs::read_to_string("/etc/postfix/main.cf").await.unwrap_or_default();

    let smtpd_tls = main_cf.lines().find(|l| l.starts_with("smtpd_tls_security_level"))
        .and_then(|l| l.split('=').nth(1)).unwrap_or("").trim().to_string();
    let smtp_tls = main_cf.lines().find(|l| l.starts_with("smtp_tls_security_level"))
        .and_then(|l| l.split('=').nth(1)).unwrap_or("").trim().to_string();

    // Check Dovecot SSL
    let dovecot_conf = tokio::fs::read_to_string("/etc/dovecot/conf.d/99-dockpanel.conf").await.unwrap_or_default();
    let dovecot_ssl = dovecot_conf.lines().find(|l| l.starts_with("ssl"))
        .and_then(|l| l.split('=').nth(1)).unwrap_or("").trim().to_string();

    Json(serde_json::json!({
        "inbound_tls": smtpd_tls,
        "outbound_tls": smtp_tls,
        "dovecot_ssl": dovecot_ssl,
        "inbound_enforced": smtpd_tls == "encrypt",
        "outbound_enforced": smtp_tls == "encrypt",
    }))
}

/// POST /mail/tls/enforce — Set TLS enforcement level.
async fn tls_enforce(Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiErr> {
    let inbound = body.get("inbound").and_then(|v| v.as_str()).unwrap_or("may");
    let outbound = body.get("outbound").and_then(|v| v.as_str()).unwrap_or("may");

    if !["may", "encrypt", "none"].contains(&inbound) || !["may", "encrypt", "none"].contains(&outbound) {
        return Err(err(StatusCode::BAD_REQUEST, "Level must be 'may', 'encrypt', or 'none'"));
    }

    let main_cf = tokio::fs::read_to_string("/etc/postfix/main.cf").await.unwrap_or_default();
    let cleaned: String = main_cf.lines()
        .filter(|l| !l.starts_with("smtpd_tls_security_level") && !l.starts_with("smtp_tls_security_level"))
        .collect::<Vec<_>>().join("\n");

    let tls_config = format!("\nsmtpd_tls_security_level = {inbound}\nsmtp_tls_security_level = {outbound}\n");

    write_file_atomic("/etc/postfix/main.cf", &format!("{cleaned}{tls_config}")).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Config write failed: {e}")))?;

    let _ = safe_command("systemctl").args(["reload", "postfix"]).output().await;

    tracing::info!("TLS enforcement: inbound={inbound}, outbound={outbound}");
    Ok(ok(&format!("TLS: inbound={inbound}, outbound={outbound}")))
}

// ── Helper ──────────────────────────────────────────────────────────────

async fn write_file_atomic(path: &str, content: &str) -> Result<(), String> {
    let tmp_path = format!("{path}.tmp");
    tokio::fs::write(&tmp_path, content).await.map_err(|e| e.to_string())?;
    tokio::fs::rename(&tmp_path, path).await.map_err(|e| e.to_string())?;
    Ok(())
}
