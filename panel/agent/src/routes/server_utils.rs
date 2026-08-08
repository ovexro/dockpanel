use axum::{
    extract::Path,
    http::StatusCode,
    routing::{get, post, delete},
    Json, Router,
};
use serde::Deserialize;
use crate::safe_cmd::safe_command_unsandboxed;

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
    /// Base64-encoded file content. Accepts `content` (frontend name) or
    /// `content_base64` (legacy agent name) for backwards compatibility.
    #[serde(alias = "content_base64")]
    pub content: String,
    /// Optional filename. When present, it is joined onto `path` so the
    /// caller can send the directory in `path` and the basename separately.
    #[serde(default)]
    pub filename: Option<String>,
}

async fn file_upload(
    Path(domain): Path<String>,
    Json(body): Json<UploadRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    use base64::Engine as _;
    use crate::services::files as file_svc;

    // Validate domain
    if !super::is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }

    // Decode base64 content first (fail fast before any FS ops)
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&body.content)
        .map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid base64 content"))?;

    // Enforce 50MB size limit for all uploads
    if bytes.len() > 50 * 1024 * 1024 {
        return Err(err(StatusCode::PAYLOAD_TOO_LARGE, "File too large (max 50MB)"));
    }

    // Resolve the target relative path. When a filename is provided we treat
    // `path` as the containing directory and join the basename onto it.
    let target_rel = match &body.filename {
        Some(name) if !name.is_empty() => {
            if name.contains('/') || name.contains('\\') || name.contains("..") || name.contains('\0') {
                return Err(err(StatusCode::BAD_REQUEST, "Invalid filename"));
            }
            let dir = body.path.trim_matches('/');
            if dir.is_empty() { name.clone() } else { format!("{dir}/{name}") }
        }
        _ => body.path.clone(),
    };

    // Confine strictly to the site root via the shared, symlink-safe resolver.
    // (The former `domain == "_server"` magic-domain branch — a root-write primitive
    // that could target /etc/nginx, /etc/dockpanel, /home, /opt via a hand-rolled,
    // weaker traversal check — was removed in s247: it had no live caller anywhere in
    // the panel/CLI/scripts and was dead attack surface.)
    let full_path = file_svc::resolve_safe_path(&domain, &target_rel)
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;

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

/// The file these three handlers manage. sshd reads root's keys from here and
/// nowhere else, so — unlike `known_hosts` in `services/remote_backup.rs` — it
/// cannot be relocated out of the sandbox's way.
const AUTHORIZED_KEYS: &str = "/root/.ssh/authorized_keys";

/// Read root's `authorized_keys` from OUTSIDE the agent's sandbox.
///
/// ⚠ THIS FEATURE HAS NEVER WORKED, ON ANY INSTALL. The agent unit sets
/// `ProtectHome=yes`, which binds an inaccessible read-only directory over
/// `/root` inside the agent's mount namespace, so an in-process `read_to_string`
/// answers ENOENT for a file that exists. `ProtectHome=yes` landed 2026-03-13
/// and these handlers landed 2026-03-15 — born broken, the next commit but one.
///
/// It failed in the worst possible direction: the read discarded its error into
/// a default, so a failure became an empty string and the endpoint answered
/// **200 with an empty list**. (Spelled in prose rather than in code on purpose:
/// `agent-security-signals-pin-e2e.sh` §E greps this file for that call shape,
/// and a pin that matches the comment describing it cannot fail.) Measured
/// on the demo box at v2.87.0: three live ed25519 keys on disk, `GET /ssh-keys`
/// → `{"keys":[]}`. The panel's only view of "who can SSH in as root" has shown
/// nothing, always, on every install — a false all-clear on the most
/// consequential access list a server has. Add and Remove returned 500.
///
/// ⚠ `ReadWritePaths=/root/.ssh` and `BindPaths=/root/.ssh` do NOT override
/// `ProtectHome=yes` — both were measured in a transient unit and both still
/// hide the file. Either fix loosens `ProtectHome` itself, which is the exact
/// direction `agent_unit.rs`'s `compiled_unit_is_the_hardened_one` exists to
/// prevent, or it does what this does: run the handful of file operations
/// unsandboxed, the precedent `services/wordpress.rs` already sets.
///
/// `Ok(None)` means the file genuinely does not exist, which is a legitimate
/// empty list. An unreadable file is an `Err` — the whole point of the fix.
async fn read_authorized_keys() -> Result<Option<String>, String> {
    // `test -e` first so "absent" and "unreadable" are DIFFERENT answers rather
    // than being told apart by matching a locale-dependent error string.
    let exists = safe_command_unsandboxed("test", &[])
        .args(["-e", AUTHORIZED_KEYS])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !exists {
        return Ok(None);
    }
    let out = safe_command_unsandboxed("cat", &[])
        .arg(AUTHORIZED_KEYS)
        .output()
        .await
        .map_err(|e| format!("could not read {AUTHORIZED_KEYS}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "could not read {AUTHORIZED_KEYS}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(Some(String::from_utf8_lossy(&out.stdout).into_owned()))
}

/// Replace root's `authorized_keys` atomically, from outside the sandbox.
///
/// Staged inside `/var/lib/dockpanel` (which IS in the unit's `ReadWritePaths`)
/// and then moved into place by `install`, which creates `/root/.ssh`, sets the
/// mode and copies in one call that runs outside the namespace.
async fn write_authorized_keys(content: &str) -> Result<(), String> {
    let staging = format!("/var/lib/dockpanel/.authorized_keys.{}", uuid::Uuid::new_v4());
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&staging)
            .map_err(|e| format!("could not stage the new key file: {e}"))?;
        f.write_all(content.as_bytes())
            .map_err(|e| format!("could not stage the new key file: {e}"))?;
    }
    let out = safe_command_unsandboxed("install", &[])
        .args(["-D", "-m", "600", "-o", "root", "-g", "root", &staging, AUTHORIZED_KEYS])
        .output()
        .await;
    let _ = std::fs::remove_file(&staging);
    let out = out.map_err(|e| format!("could not install {AUTHORIZED_KEYS}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "could not install {AUTHORIZED_KEYS}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

async fn list_ssh_keys() -> Result<Json<serde_json::Value>, ApiErr> {
    // An empty list must mean "root has no keys", never "I could not look".
    let content = read_authorized_keys()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?
        .unwrap_or_default();

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

    // A read that FAILED must not be mistaken for an empty key file — that is
    // how an append would silently become a replace.
    let mut content = read_authorized_keys()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?
        .unwrap_or_default();
    if content.contains(key) {
        return Err(err(StatusCode::CONFLICT, "Key already exists"));
    }

    if !content.ends_with('\n') && !content.is_empty() {
        content.push('\n');
    }
    content.push_str(key);
    content.push('\n');

    write_authorized_keys(&content)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    tracing::info!("SSH key added");
    Ok(ok("SSH key added"))
}

async fn remove_ssh_key(
    Path(fingerprint): Path<String>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    // ⚠ THE ORDER OF THESE TWO FIXES MATTERS, and getting only one of them
    // would have been worse than shipping neither. This read was also
    // `unwrap_or_default()`: on a failed read `content` is empty, every line is
    // filtered out of nothing, and the write below stores `"\n"` — i.e. it
    // DELETES EVERY ROOT SSH KEY ON THE BOX. Today that is inert only because
    // the write fails too, under the same sandbox. Repairing the sandbox alone
    // would have converted a dead feature into a destructive one on the first
    // transient read error.
    let content = read_authorized_keys()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "root has no authorized_keys file"))?;

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

    write_authorized_keys(&format!("{new_content}\n"))
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    tracing::info!("SSH key removed: {fingerprint}");
    Ok(ok("SSH key removed"))
}

// ── Auto-Updates ────────────────────────────────────────────────────────

async fn auto_updates_status() -> Result<Json<serde_json::Value>, ApiErr> {
    // `services::pkg` maps this onto `dnf-automatic` on the RHEL family.
    let installed = crate::services::pkg::is_installed("unattended-upgrades").await;

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
    // Install unattended-upgrades if not present. Unsandboxed: apt-get install
    // writes to /var/lib/dpkg + /usr.
    let _ = safe_command_unsandboxed("sh", &[])
        .args(["-c", "apt-get install -y unattended-upgrades"])
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
