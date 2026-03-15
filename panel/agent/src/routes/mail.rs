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
        .route("/mail/dkim/generate", post(dkim_generate))
        .route("/mail/domains/configure", post(domain_configure))
        .route("/mail/domains/remove", post(domain_remove))
        .route("/mail/sync", post(sync_config))
        .route("/mail/queue", get(queue_list))
        .route("/mail/queue/flush", post(queue_flush))
        .route("/mail/queue/delete", post(queue_delete))
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
