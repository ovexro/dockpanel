use std::path::Path;
use std::time::Instant;
use tokio::process::Command;

const WEBROOT: &str = "/var/www";
const DEPLOY_KEYS_DIR: &str = "/etc/dockpanel/deploy-keys";

pub struct DeployResult {
    pub success: bool,
    pub output: String,
    pub commit_hash: Option<String>,
    pub duration_ms: u64,
}

/// Build GIT_SSH_COMMAND for deploy key authentication.
fn ssh_command(key_path: &str) -> String {
    format!("ssh -i {key_path} -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null")
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

    let env_ssh = key_path.map(|k| ssh_command(k));

    if Path::new(&git_dir).exists() {
        // Git pull (fetch + reset to match remote)
        let mut cmd = Command::new("git");
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
        let reset = Command::new("git")
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

        let mut cmd = Command::new("git");
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
    let hash = Command::new("git")
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

/// Run a deploy script in the site directory.
pub async fn run_script(domain: &str, script: &str) -> Result<(bool, String), String> {
    if script.trim().is_empty() {
        return Ok((true, String::new()));
    }

    let site_dir = format!("{WEBROOT}/{domain}");

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        Command::new("bash")
            .args(["-c", script])
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
    let output = std::process::Command::new("ssh-keygen")
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
