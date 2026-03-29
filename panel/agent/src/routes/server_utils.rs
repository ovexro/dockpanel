use axum::{
    extract::Path,
    http::StatusCode,
    routing::{get, post, delete},
    Json, Router,
};
use serde::Deserialize;
use crate::safe_cmd::safe_command;

use super::AppState;
use base64::Engine as _;

type ApiErr = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: &str) -> ApiErr {
    (status, Json(serde_json::json!({ "error": msg })))
}

fn ok(msg: &str) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ok": true, "message": msg }))
}

pub fn router() -> Router<AppState> {
    Router::new()
        // File upload (binary)
        .route("/files/{domain}/upload", post(file_upload))
        // SSH keys
        .route("/ssh-keys", get(list_ssh_keys).post(add_ssh_key))
        .route("/ssh-keys/{fingerprint}", delete(remove_ssh_key))
        // Auto-updates
        .route("/auto-updates/status", get(auto_updates_status))
        .route("/auto-updates/enable", post(enable_auto_updates))
        .route("/auto-updates/disable", post(disable_auto_updates))
        // IP whitelist for panel
        .route("/panel-whitelist", get(get_whitelist).post(set_whitelist))
}

// ── File Upload ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct UploadRequest {
    pub path: String,
    pub content_base64: String,
}

async fn file_upload(
    Path(domain): Path<String>,
    Json(body): Json<UploadRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    use base64::Engine as _;
    use crate::services::files as file_svc;

    // Validate domain
    if domain != "_server" && !super::is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }

    // Decode base64 content first (fail fast before any FS ops)
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&body.content_base64)
        .map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid base64 content"))?;

    // Enforce 50MB size limit for all uploads
    if bytes.len() > 50 * 1024 * 1024 {
        return Err(err(StatusCode::PAYLOAD_TOO_LARGE, "File too large (max 50MB)"));
    }

    let full_path = if domain == "_server" {
        // Server-level upload: validate no traversal manually
        let p = format!("/{}", body.path.trim_start_matches('/'));
        if p.contains("..") || p.contains('\0') {
            return Err(err(StatusCode::BAD_REQUEST, "Path traversal not allowed"));
        }
        // Only allow uploads to specific directories
        const ALLOWED_SERVER_PATHS: &[&str] = &[
            "/var/www/",
            "/etc/nginx/",
            "/etc/dockpanel/",
            "/var/backups/dockpanel/",
            "/home/",
            "/opt/",
        ];
        let allowed = ALLOWED_SERVER_PATHS.iter().any(|prefix| p.starts_with(prefix));
        if !allowed {
            return Err(err(StatusCode::BAD_REQUEST, "Upload path not in allowed directories"));
        }
        // Canonicalize parent to resolve symlinks, then re-check prefix
        let path_buf = std::path::PathBuf::from(&p);
        if let Some(parent) = path_buf.parent() {
            if parent.exists() {
                if let Ok(canon) = parent.canonicalize() {
                    let canon_str = canon.to_string_lossy();
                    let still_allowed = ALLOWED_SERVER_PATHS.iter().any(|prefix| {
                        canon_str.starts_with(prefix.trim_end_matches('/'))
                    });
                    if !still_allowed {
                        return Err(err(StatusCode::BAD_REQUEST, "Resolved path not in allowed directories"));
                    }
                }
            }
        }
        path_buf
    } else {
        // Site upload: use resolve_safe_path to prevent TOCTOU race
        file_svc::resolve_safe_path(&domain, &body.path)
            .map_err(|e| err(StatusCode::BAD_REQUEST, &e))?
    };

    // Create parent directory (safe — path already validated)
    if let Some(parent) = full_path.parent() {
        tokio::fs::create_dir_all(parent).await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to create directory: {e}")))?;
    }

    tokio::fs::write(&full_path, &bytes).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write file: {e}")))?;

    let path_str = full_path.to_string_lossy().to_string();
    tracing::info!("File uploaded: {} ({} bytes)", path_str, bytes.len());
    Ok(Json(serde_json::json!({ "ok": true, "path": path_str, "size": bytes.len() })))
}

// ── SSH Key Management ──────────────────────────────────────────────────

async fn list_ssh_keys() -> Result<Json<serde_json::Value>, ApiErr> {
    let path = "/root/.ssh/authorized_keys";
    let content = tokio::fs::read_to_string(path).await.unwrap_or_default();

    let keys: Vec<serde_json::Value> = content
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim().starts_with('#'))
        .map(|line| {
            let parts: Vec<&str> = line.splitn(3, ' ').collect();
            let key_type = parts.first().unwrap_or(&"").to_string();
            let key_data = parts.get(1).unwrap_or(&"").to_string();
            let comment = parts.get(2).unwrap_or(&"").to_string();

            // Generate fingerprint
            let fingerprint = if !key_data.is_empty() {
                use sha2::{Sha256, Digest};
                let decoded = base64::engine::general_purpose::STANDARD.decode(&key_data).unwrap_or_default();
                let hash = Sha256::digest(&decoded);
                format!("SHA256:{}", base64::engine::general_purpose::STANDARD.encode(&hash).trim_end_matches('='))
            } else {
                String::new()
            };

            serde_json::json!({
                "type": key_type,
                "fingerprint": fingerprint,
                "comment": comment,
                "key": format!("{} {}...", key_type, &key_data[..key_data.len().min(20)]),
            })
        })
        .collect();

    Ok(Json(serde_json::json!({ "keys": keys })))
}

#[derive(Deserialize)]
pub struct AddKeyRequest {
    pub key: String,
}

async fn add_ssh_key(
    Json(body): Json<AddKeyRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    let key = body.key.trim();
    // Reject embedded newlines (prevents multi-key injection)
    if key.contains('\n') || key.contains('\r') || key.contains('\0') {
        return Err(err(StatusCode::BAD_REQUEST, "SSH key must be a single line"));
    }
    // Validate key format: must start with a known key type prefix
    if !key.starts_with("ssh-") && !key.starts_with("ecdsa-") && !key.starts_with("sk-") {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid SSH key format"));
    }
    // Validate structure: should have at least 2 space-separated parts (type + base64)
    let parts: Vec<&str> = key.split_whitespace().collect();
    if parts.len() < 2 || parts[1].len() < 16 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid SSH key: missing key data"));
    }
    // Reject keys with authorized_keys options prefix (e.g. command=, from=, restrict)
    if key.contains("command=") || key.contains("from=") || key.contains("restrict")
        || key.contains("no-pty") || key.contains("permitopen") {
        return Err(err(StatusCode::BAD_REQUEST, "SSH key options not allowed"));
    }

    let path = "/root/.ssh/authorized_keys";
    tokio::fs::create_dir_all("/root/.ssh").await.ok();

    let mut content = tokio::fs::read_to_string(path).await.unwrap_or_default();
    if content.contains(key) {
        return Err(err(StatusCode::CONFLICT, "Key already exists"));
    }

    if !content.ends_with('\n') && !content.is_empty() {
        content.push('\n');
    }
    content.push_str(key);
    content.push('\n');

    tokio::fs::write(path, &content).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write: {e}")))?;
    let _ = safe_command("chmod").args(["600", path]).output().await;

    tracing::info!("SSH key added");
    Ok(ok("SSH key added"))
}

async fn remove_ssh_key(
    Path(fingerprint): Path<String>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    let path = "/root/.ssh/authorized_keys";
    let content = tokio::fs::read_to_string(path).await.unwrap_or_default();

    let new_content: String = content
        .lines()
        .filter(|line| {
            if line.trim().is_empty() || line.starts_with('#') {
                return true;
            }
            let parts: Vec<&str> = line.splitn(3, ' ').collect();
            let key_data = parts.get(1).unwrap_or(&"");
            if key_data.is_empty() { return true; }

            use sha2::{Sha256, Digest};
            let decoded = base64::engine::general_purpose::STANDARD.decode(key_data).unwrap_or_default();
            let hash = Sha256::digest(&decoded);
            let fp = format!("SHA256:{}", base64::engine::general_purpose::STANDARD.encode(&hash).trim_end_matches('='));
            fp != fingerprint
        })
        .collect::<Vec<_>>()
        .join("\n");

    tokio::fs::write(path, format!("{new_content}\n")).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write: {e}")))?;

    tracing::info!("SSH key removed: {fingerprint}");
    Ok(ok("SSH key removed"))
}

// ── Auto-Updates ────────────────────────────────────────────────────────

async fn auto_updates_status() -> Result<Json<serde_json::Value>, ApiErr> {
    let installed = safe_command("dpkg")
        .args(["-l", "unattended-upgrades"])
        .output()
        .await
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains("ii"))
        .unwrap_or(false);

    let enabled = if installed {
        tokio::fs::read_to_string("/etc/apt/apt.conf.d/20auto-upgrades")
            .await
            .map(|c| c.contains("\"1\""))
            .unwrap_or(false)
    } else {
        false
    };

    Ok(Json(serde_json::json!({ "installed": installed, "enabled": enabled })))
}

async fn enable_auto_updates() -> Result<Json<serde_json::Value>, ApiErr> {
    // Install unattended-upgrades if not present
    let _ = safe_command("sh")
        .args(["-c", "DEBIAN_FRONTEND=noninteractive apt-get install -y unattended-upgrades"])
        .output()
        .await;

    let config = "APT::Periodic::Update-Package-Lists \"1\";\nAPT::Periodic::Unattended-Upgrade \"1\";\n";
    tokio::fs::write("/etc/apt/apt.conf.d/20auto-upgrades", config).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write config: {e}")))?;

    tracing::info!("Auto-updates enabled");
    Ok(ok("Automatic security updates enabled"))
}

async fn disable_auto_updates() -> Result<Json<serde_json::Value>, ApiErr> {
    let config = "APT::Periodic::Update-Package-Lists \"0\";\nAPT::Periodic::Unattended-Upgrade \"0\";\n";
    tokio::fs::write("/etc/apt/apt.conf.d/20auto-upgrades", config).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write config: {e}")))?;

    tracing::info!("Auto-updates disabled");
    Ok(ok("Automatic security updates disabled"))
}

// ── Panel IP Whitelist ──────────────────────────────────────────────────

async fn get_whitelist() -> Result<Json<serde_json::Value>, ApiErr> {
    let path = "/etc/dockpanel/panel-whitelist.conf";
    let content = tokio::fs::read_to_string(path).await.unwrap_or_default();
    let ips: Vec<String> = content
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| l.trim().to_string())
        .collect();

    Ok(Json(serde_json::json!({ "ips": ips, "enabled": !ips.is_empty() })))
}

#[derive(Deserialize)]
pub struct WhitelistRequest {
    pub ips: Vec<String>,
}

async fn set_whitelist(
    Json(body): Json<WhitelistRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    let path = "/etc/dockpanel/panel-whitelist.conf";

    // Validate IPs
    for ip in &body.ips {
        let trimmed = ip.trim();
        if !trimmed.is_empty() && !trimmed.contains('.') && !trimmed.contains(':') {
            return Err(err(StatusCode::BAD_REQUEST, &format!("Invalid IP: {trimmed}")));
        }
    }

    let content: String = body.ips.iter()
        .filter(|ip| !ip.trim().is_empty())
        .map(|ip| ip.trim().to_string())
        .collect::<Vec<_>>()
        .join("\n");

    tokio::fs::write(path, format!("{content}\n")).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write: {e}")))?;

    // Update nginx config to include allow/deny directives
    // This would be picked up by the panel's nginx config
    tracing::info!("Panel whitelist updated: {} IPs", body.ips.len());
    Ok(ok(&format!("Whitelist updated with {} IPs", body.ips.iter().filter(|ip| !ip.trim().is_empty()).count())))
}
