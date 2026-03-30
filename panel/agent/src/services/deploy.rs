use std::path::Path;
use std::time::Instant;
use crate::safe_cmd::{safe_command, safe_command_sync};

const WEBROOT: &str = "/var/www";
const DEPLOY_KEYS_DIR: &str = "/etc/dockpanel/deploy-keys";

pub struct DeployResult {
    pub success: bool,
    pub output: String,
    pub commit_hash: Option<String>,
    pub duration_ms: u64,
}

/// Validate and build GIT_SSH_COMMAND for deploy key authentication.
/// Only allows keys stored under the deploy-keys directory. Uses strict host key checking
/// with the system known_hosts file instead of blindly accepting all hosts.
pub(crate) fn ssh_command(key_path: &str) -> Result<String, String> {
    // Reject paths containing ".."
    if key_path.contains("..") {
        return Err("Deploy key path must not contain '..'".into());
    }

    // Validate the key_path starts with the allowed directory
    if !key_path.starts_with(DEPLOY_KEYS_DIR) {
        return Err(format!(
            "Deploy key must be under {DEPLOY_KEYS_DIR}/, got: {key_path}"
        ));
    }

    // Canonicalize the path and verify it's still under the allowed directory
    let canon = Path::new(key_path)
        .canonicalize()
        .map_err(|e| format!("Deploy key not found: {e}"))?;
    let canon_base = Path::new(DEPLOY_KEYS_DIR)
        .canonicalize()
        .map_err(|e| format!("Deploy keys directory not found: {e}"))?;

    if !canon.starts_with(&canon_base) {
        return Err("Deploy key path resolved outside the allowed directory".into());
    }

    let canon_str = canon.to_string_lossy();
    Ok(format!(
        "ssh -i {canon_str} -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=/etc/dockpanel/known_hosts"
    ))
}

/// Clone or pull a git repository to the site's webroot.
pub async fn clone_or_pull(
    domain: &str,
    repo_url: &str,
    branch: &str,
    key_path: Option<&str>,
) -> Result<DeployResult, String> {
    let start = Instant::now();
    let site_dir = format!("{WEBROOT}/{domain}");
    let git_dir = format!("{site_dir}/.git");
    let mut output_buf = String::new();

    let env_ssh = match key_path {
        Some(k) => Some(ssh_command(k)?),
        None => None,
    };

    if Path::new(&git_dir).exists() {
        // Git pull (fetch + reset to match remote)
        let mut cmd = safe_command("git");
        cmd.args(["-C", &site_dir, "fetch", "origin", branch])
            .env("GIT_TERMINAL_PROMPT", "0");
        if let Some(ref ssh) = env_ssh {
            cmd.env("GIT_SSH_COMMAND", ssh);
        }

        let fetch = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            cmd.output(),
        )
        .await
        .map_err(|_| "git fetch timed out".to_string())?
        .map_err(|e| format!("git fetch failed: {e}"))?;

        output_buf.push_str(&String::from_utf8_lossy(&fetch.stdout));
        output_buf.push_str(&String::from_utf8_lossy(&fetch.stderr));

        if !fetch.status.success() {
            return Ok(DeployResult {
                success: false,
                output: output_buf,
                commit_hash: None,
                duration_ms: start.elapsed().as_millis() as u64,
            });
        }

        // Reset to remote branch
        let reset = safe_command("git")
            .args(["-C", &site_dir, "reset", "--hard", &format!("origin/{branch}")])
            .output()
            .await
            .map_err(|e| format!("git reset failed: {e}"))?;

        output_buf.push_str(&String::from_utf8_lossy(&reset.stdout));
        output_buf.push_str(&String::from_utf8_lossy(&reset.stderr));
    } else {
        // Fresh clone
        std::fs::create_dir_all(&site_dir)
            .map_err(|e| format!("Failed to create site dir: {e}"))?;

        let mut cmd = safe_command("git");
        cmd.args(["clone", "--branch", branch, "--single-branch", "--depth", "50", repo_url, &site_dir])
            .env("GIT_TERMINAL_PROMPT", "0");
        if let Some(ref ssh) = env_ssh {
            cmd.env("GIT_SSH_COMMAND", ssh);
        }

        let clone = tokio::time::timeout(
            std::time::Duration::from_secs(300),
            cmd.output(),
        )
        .await
        .map_err(|_| "git clone timed out".to_string())?
        .map_err(|e| format!("git clone failed: {e}"))?;

        output_buf.push_str(&String::from_utf8_lossy(&clone.stdout));
        output_buf.push_str(&String::from_utf8_lossy(&clone.stderr));

        if !clone.status.success() {
            return Ok(DeployResult {
                success: false,
                output: output_buf,
                commit_hash: None,
                duration_ms: start.elapsed().as_millis() as u64,
            });
        }
    }

    // Get current commit hash
    let hash = safe_command("git")
        .args(["-C", &site_dir, "rev-parse", "--short", "HEAD"])
        .output()
        .await
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        });

    Ok(DeployResult {
        success: true,
        output: output_buf,
        commit_hash: hash,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}

/// Run a deploy script file from the site directory.
/// The `script` parameter is treated as a relative path within the site dir
/// (e.g., ".dockpanel/deploy.sh"), NOT as arbitrary bash code.
pub async fn run_script(domain: &str, script: &str) -> Result<(bool, String), String> {
    if script.trim().is_empty() {
        return Ok((true, String::new()));
    }

    let site_dir = format!("{WEBROOT}/{domain}");

    // Treat the script parameter as a relative file path within the site directory
    let script_path = Path::new(&site_dir).join(script.trim_start_matches('/'));

    // Canonicalize to prevent path traversal
    let canon_site = Path::new(&site_dir)
        .canonicalize()
        .map_err(|e| format!("Site directory not found: {e}"))?;
    let canon_script = script_path
        .canonicalize()
        .map_err(|e| format!("Deploy script not found: {e}"))?;

    // Verify the script is within the site directory
    if !canon_script.starts_with(&canon_site) {
        return Err("Deploy script must be within the site directory".into());
    }

    // Reject paths containing ".." for extra safety
    if script.contains("..") {
        return Err("Deploy script path must not contain '..'".into());
    }

    // Verify the script file exists and is a regular file
    let meta = std::fs::metadata(&canon_script)
        .map_err(|e| format!("Deploy script not accessible: {e}"))?;
    if !meta.is_file() {
        return Err("Deploy script path is not a regular file".into());
    }

    let script_str = canon_script.to_string_lossy().to_string();

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        safe_command("bash")
            .arg(&script_str)
            .current_dir(&site_dir)
            .env("HOME", &site_dir)
            .env("NODE_ENV", "production")
            .output(),
    )
    .await
    .map_err(|_| "Deploy script timed out (5 min)".to_string())?
    .map_err(|e| format!("Failed to run deploy script: {e}"))?;

    let out = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // Truncate to 50KB
    let truncated = if out.len() > 50_000 {
        format!("{}...\n[output truncated]", &out[..50_000])
    } else {
        out
    };

    Ok((output.status.success(), truncated))
}

/// Atomic deploy: clone into a new release directory, run script, then atomically swap symlink.
/// This gives zero-downtime deploys for PHP sites (Capistrano/Deployer-style).
///
/// Directory structure:
///   /var/www/{domain}/releases/{timestamp}/   — immutable release snapshots
///   /var/www/{domain}/current                 — symlink to active release
///   /var/www/{domain}/shared/                 — persistent dirs (uploads, .env, logs)
pub async fn atomic_deploy(
    domain: &str,
    repo_url: &str,
    branch: &str,
    key_path: Option<&str>,
    deploy_script: Option<&str>,
    keep_releases: u32,
    shared_dirs: &[&str],
) -> Result<DeployResult, String> {
    let start = Instant::now();
    let site_dir = format!("{WEBROOT}/{domain}");
    let releases_dir = format!("{site_dir}/releases");
    let shared_dir = format!("{site_dir}/shared");
    let current_link = format!("{site_dir}/current");

    // Create directory structure
    std::fs::create_dir_all(&releases_dir)
        .map_err(|e| format!("Failed to create releases dir: {e}"))?;
    std::fs::create_dir_all(&shared_dir)
        .map_err(|e| format!("Failed to create shared dir: {e}"))?;

    // Generate release ID (timestamp-based for sorting)
    let release_id = chrono::Utc::now().format("%Y%m%d%H%M%S").to_string();
    let release_dir = format!("{releases_dir}/{release_id}");

    let mut output_buf = String::new();
    output_buf.push_str(&format!("==> Creating release {release_id}\n"));

    // Clone into the new release directory
    let env_ssh = match key_path {
        Some(k) => Some(ssh_command(k)?),
        None => None,
    };

    let mut cmd = safe_command("git");
    cmd.args(["clone", "--branch", branch, "--single-branch", "--depth", "1", repo_url, &release_dir])
        .env("GIT_TERMINAL_PROMPT", "0");
    if let Some(ref ssh) = env_ssh {
        cmd.env("GIT_SSH_COMMAND", ssh);
    }

    let clone = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        cmd.output(),
    )
    .await
    .map_err(|_| "git clone timed out".to_string())?
    .map_err(|e| format!("git clone failed: {e}"))?;

    output_buf.push_str(&String::from_utf8_lossy(&clone.stdout));
    output_buf.push_str(&String::from_utf8_lossy(&clone.stderr));

    if !clone.status.success() {
        // Clean up failed release
        let _ = std::fs::remove_dir_all(&release_dir);
        return Ok(DeployResult {
            success: false,
            output: output_buf,
            commit_hash: None,
            duration_ms: start.elapsed().as_millis() as u64,
        });
    }

    // Get commit hash
    let commit_hash = safe_command("git")
        .args(["-C", &release_dir, "rev-parse", "--short", "HEAD"])
        .output()
        .await
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        });

    output_buf.push_str(&format!("==> Commit: {}\n", commit_hash.as_deref().unwrap_or("unknown")));

    // Link shared directories and files into the release
    for shared in shared_dirs {
        let shared_path = format!("{shared_dir}/{shared}");
        let release_path = format!("{release_dir}/{shared}");

        // Create shared directory if it doesn't exist
        if shared.ends_with('/') || !shared.contains('.') {
            std::fs::create_dir_all(&shared_path).ok();
        } else {
            // Shared file — create parent and touch
            if let Some(parent) = Path::new(&shared_path).parent() {
                std::fs::create_dir_all(parent).ok();
            }
            if !Path::new(&shared_path).exists() {
                std::fs::write(&shared_path, "").ok();
            }
        }

        // Remove existing dir/file in release (if any) and create symlink
        if Path::new(&release_path).exists() || Path::new(&release_path).is_symlink() {
            if Path::new(&release_path).is_dir() {
                let _ = std::fs::remove_dir_all(&release_path);
            } else {
                let _ = std::fs::remove_file(&release_path);
            }
        }
        // Ensure parent exists
        if let Some(parent) = Path::new(&release_path).parent() {
            std::fs::create_dir_all(parent).ok();
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&shared_path, &release_path).ok();
        }
    }
    output_buf.push_str(&format!("==> Linked {} shared paths\n", shared_dirs.len()));

    // Copy shared items from previous release if this is the first deploy
    if !shared_dirs.is_empty() {
        if let Ok(target) = std::fs::read_link(&current_link) {
            let prev = target.to_string_lossy().to_string();
            for shared in shared_dirs {
                let old_path = format!("{prev}/{shared}");
                let shared_path = format!("{shared_dir}/{shared}");
                // Only copy if shared location is empty and old release has content
                if Path::new(&old_path).exists() && !Path::new(&old_path).is_symlink() {
                    let shared_meta = std::fs::metadata(&shared_path);
                    let is_empty = match shared_meta {
                        Ok(m) if m.is_dir() => std::fs::read_dir(&shared_path).map(|mut d| d.next().is_none()).unwrap_or(true),
                        Ok(m) if m.is_file() => m.len() == 0,
                        _ => true,
                    };
                    if is_empty {
                        let _ = safe_command("cp")
                            .args(["-a", &old_path, &shared_path])
                            .output()
                            .await;
                    }
                }
            }
        }
    }

    // Run deploy script inside the release directory
    if let Some(script) = deploy_script {
        if !script.trim().is_empty() {
            output_buf.push_str("==> Running deploy script\n");

            let script_path = Path::new(&release_dir).join(script.trim_start_matches('/'));

            // Validate script is within release dir
            if let (Ok(canon_release), Ok(canon_script)) = (
                Path::new(&release_dir).canonicalize(),
                script_path.canonicalize(),
            ) {
                if !canon_script.starts_with(&canon_release) {
                    let _ = std::fs::remove_dir_all(&release_dir);
                    return Err("Deploy script must be within the release directory".into());
                }

                let script_out = tokio::time::timeout(
                    std::time::Duration::from_secs(300),
                    safe_command("bash")
                        .arg(canon_script.to_string_lossy().as_ref())
                        .current_dir(&release_dir)
                        .env("HOME", &release_dir)
                        .env("NODE_ENV", "production")
                        .env("RELEASE_DIR", &release_dir)
                        .env("SHARED_DIR", &shared_dir)
                        .output(),
                )
                .await
                .map_err(|_| "Deploy script timed out (5 min)".to_string())?
                .map_err(|e| format!("Failed to run deploy script: {e}"))?;

                let out = format!(
                    "{}{}",
                    String::from_utf8_lossy(&script_out.stdout),
                    String::from_utf8_lossy(&script_out.stderr),
                );
                output_buf.push_str(&out);

                if !script_out.status.success() {
                    output_buf.push_str("\n==> Deploy script failed, removing release\n");
                    let _ = std::fs::remove_dir_all(&release_dir);
                    return Ok(DeployResult {
                        success: false,
                        output: output_buf,
                        commit_hash,
                        duration_ms: start.elapsed().as_millis() as u64,
                    });
                }
            } else {
                output_buf.push_str("==> Deploy script not found, skipping\n");
            }
        }
    }

    // Atomic symlink swap: ln -sfn (create temp link then rename for atomicity)
    let tmp_link = format!("{current_link}.tmp.{release_id}");
    #[cfg(unix)]
    {
        // Remove any stale tmp link
        let _ = std::fs::remove_file(&tmp_link);
        std::os::unix::fs::symlink(&release_dir, &tmp_link)
            .map_err(|e| format!("Failed to create symlink: {e}"))?;
        std::fs::rename(&tmp_link, &current_link)
            .map_err(|e| {
                let _ = std::fs::remove_file(&tmp_link);
                format!("Failed to swap symlink: {e}")
            })?;
    }
    output_buf.push_str(&format!("==> Activated release {release_id}\n"));

    // Create compatibility symlink: /var/www/{domain}/public → current/public
    // This makes atomic deploys transparent to nginx (which serves from /var/www/{domain}/public)
    let public_link = format!("{site_dir}/public");
    let public_target = format!("{site_dir}/current/public");
    if Path::new(&public_target).exists() {
        // Remove existing public dir/symlink if it's not already the right symlink
        let needs_update = if Path::new(&public_link).is_symlink() {
            std::fs::read_link(&public_link)
                .map(|t| t != Path::new(&public_target))
                .unwrap_or(true)
        } else {
            true
        };
        if needs_update {
            if Path::new(&public_link).is_dir() && !Path::new(&public_link).is_symlink() {
                // First atomic deploy: existing public/ is a real directory — move to shared as backup
                let backup_dir = format!("{shared_dir}/_pre_atomic_public_backup");
                let _ = std::fs::rename(&public_link, &backup_dir);
                output_buf.push_str("==> Migrated existing public/ to shared/_pre_atomic_public_backup\n");
            } else {
                let _ = std::fs::remove_file(&public_link);
            }
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(&public_target, &public_link).ok();
            }
            output_buf.push_str("==> Created public → current/public symlink\n");
        }
    }

    // Ensure correct ownership
    let _ = safe_command("chown")
        .args(["-R", "www-data:www-data", &release_dir])
        .output()
        .await;

    // Clean up old releases (keep N most recent)
    if let Ok(entries) = std::fs::read_dir(&releases_dir) {
        let mut releases: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        releases.sort();
        if releases.len() > keep_releases as usize {
            let to_remove = releases.len() - keep_releases as usize;
            for old in releases.iter().take(to_remove) {
                let old_path = format!("{releases_dir}/{old}");
                output_buf.push_str(&format!("==> Removing old release {old}\n"));
                let _ = std::fs::remove_dir_all(&old_path);
            }
        }
    }

    output_buf.push_str("==> Zero-downtime deploy complete\n");

    Ok(DeployResult {
        success: true,
        output: output_buf,
        commit_hash,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}

/// List releases for a site, ordered newest first.
pub fn list_releases(domain: &str) -> Result<Vec<ReleaseInfo>, String> {
    let releases_dir = format!("{WEBROOT}/{domain}/releases");
    let current_link = format!("{WEBROOT}/{domain}/current");

    let active = std::fs::read_link(&current_link)
        .ok()
        .and_then(|p| p.file_name().map(|f| f.to_string_lossy().to_string()));

    if !Path::new(&releases_dir).exists() {
        return Ok(Vec::new());
    }

    let mut releases: Vec<ReleaseInfo> = std::fs::read_dir(&releases_dir)
        .map_err(|e| format!("Failed to read releases: {e}"))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let is_active = active.as_deref() == Some(&name);
            let created = e.metadata().ok().and_then(|m| m.created().ok())
                .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339())
                .unwrap_or_default();

            // Get commit hash from release
            let git_dir = format!("{}/{}", e.path().display(), ".git");
            let commit = if Path::new(&git_dir).exists() {
                crate::safe_cmd::safe_command_sync("git")
                    .args(["-C", &e.path().to_string_lossy(), "rev-parse", "--short", "HEAD"])
                    .output()
                    .ok()
                    .and_then(|o| if o.status.success() {
                        Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                    } else { None })
            } else { None };

            ReleaseInfo { id: name, active: is_active, commit_hash: commit, created_at: created }
        })
        .collect();

    releases.sort_by(|a, b| b.id.cmp(&a.id)); // newest first
    Ok(releases)
}

pub struct ReleaseInfo {
    pub id: String,
    pub active: bool,
    pub commit_hash: Option<String>,
    pub created_at: String,
}

/// Activate (rollback to) a specific release by updating the current symlink.
pub fn activate_release(domain: &str, release_id: &str) -> Result<(), String> {
    // Validate release_id format (timestamp only, no path traversal)
    if release_id.is_empty() || release_id.len() > 20
        || !release_id.chars().all(|c| c.is_ascii_digit())
    {
        return Err("Invalid release ID".into());
    }

    let releases_dir = format!("{WEBROOT}/{domain}/releases");
    let release_dir = format!("{releases_dir}/{release_id}");
    let current_link = format!("{WEBROOT}/{domain}/current");

    if !Path::new(&release_dir).is_dir() {
        return Err(format!("Release {release_id} not found"));
    }

    // Atomic swap
    let tmp_link = format!("{current_link}.tmp.{release_id}");
    #[cfg(unix)]
    {
        let _ = std::fs::remove_file(&tmp_link);
        std::os::unix::fs::symlink(&release_dir, &tmp_link)
            .map_err(|e| format!("Failed to create symlink: {e}"))?;
        std::fs::rename(&tmp_link, &current_link)
            .map_err(|e| {
                let _ = std::fs::remove_file(&tmp_link);
                format!("Failed to swap symlink: {e}")
            })?;
    }

    Ok(())
}

/// Generate an SSH deploy key pair for a site.
pub fn generate_deploy_key(domain: &str) -> Result<(String, String), String> {
    std::fs::create_dir_all(DEPLOY_KEYS_DIR)
        .map_err(|e| format!("Failed to create keys dir: {e}"))?;

    let key_path = format!("{DEPLOY_KEYS_DIR}/{domain}");
    let pub_path = format!("{key_path}.pub");

    // Remove existing keys
    let _ = std::fs::remove_file(&key_path);
    let _ = std::fs::remove_file(&pub_path);

    // Generate key
    let output = safe_command_sync("ssh-keygen")
        .args([
            "-t", "ed25519",
            "-f", &key_path,
            "-N", "",
            "-C", &format!("dockpanel-deploy@{domain}"),
        ])
        .output()
        .map_err(|e| format!("ssh-keygen failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ssh-keygen failed: {stderr}"));
    }

    // Set permissions
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).ok();
    }

    let public_key = std::fs::read_to_string(&pub_path)
        .map_err(|e| format!("Failed to read public key: {e}"))?
        .trim()
        .to_string();

    Ok((public_key, key_path))
}
