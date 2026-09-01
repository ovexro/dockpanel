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

/// Top-level entries of a site directory that the panel cannot prove it wrote.
///
/// Creating a site populates the web root — a filler page in whichever
/// directory `services::nginx::document_root_for` picks — so a first deploy
/// never meets an empty destination, and git refuses to clone into one that is
/// not empty. Clearing the way is only defensible for files the panel wrote
/// itself, which is why this asks about CONTENT rather than trusting a name:
/// `index.html` is the most ordinary filename there is. A non-empty answer
/// means refuse, and the names are returned so the refusal can say which.
fn foreign_entries(site_dir: &str) -> Vec<String> {
    let entries = match std::fs::read_dir(site_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut foreign: Vec<String> = entries
        .flatten()
        .filter(|entry| !panel_owned(&entry.path()))
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    foreign.sort();
    foreign
}

/// Whether `path` is something the panel put there and may clear away.
///
/// A directory qualifies only when everything below it does. An empty one
/// qualifies because it holds nothing of the operator's; an unreadable one does
/// not, because absence of proof is not proof.
fn panel_owned(path: &Path) -> bool {
    if !path.is_dir() {
        return crate::services::nginx::is_placeholder_page(path);
    }
    match std::fs::read_dir(path) {
        Ok(entries) => entries.flatten().all(|entry| panel_owned(&entry.path())),
        Err(_) => false,
    }
}

/// Move every top-level entry of `from` into `to`, returning what was moved.
///
/// Renaming the clone onto the site directory cannot work: that directory
/// exists and is not empty, which is the entire reason this path exists, and
/// `rename` refuses a non-empty destination. So the move goes entry by entry —
/// and an entry the clone does NOT supply is left alone, which is what keeps a
/// document root present when a repository ships no `public/`.
fn merge_into(from: &Path, to: &Path) -> Result<Vec<String>, String> {
    let entries = std::fs::read_dir(from)
        .map_err(|e| format!("Failed to read the staged clone: {e}"))?;
    let mut moved = Vec::new();

    for entry in entries.flatten() {
        let name = entry.file_name();
        let target = to.join(&name);

        if target.is_symlink() || target.is_file() {
            std::fs::remove_file(&target)
                .map_err(|e| format!("Failed to replace {}: {e}", target.display()))?;
        } else if target.is_dir() {
            std::fs::remove_dir_all(&target)
                .map_err(|e| format!("Failed to replace {}: {e}", target.display()))?;
        }

        std::fs::rename(entry.path(), &target)
            .map_err(|e| format!("Failed to move {} into place: {e}", name.to_string_lossy()))?;
        moved.push(name.to_string_lossy().to_string());
    }

    Ok(moved)
}

/// Arguments prefixed to every `git` invocation the agent makes against a
/// deployed tree.
///
/// Ownership is the real control: `.git` belongs to root and the web user
/// cannot write it (`hand_tree_to_web_user`). These are the belt to that pair of
/// braces, and they earn their place on installs upgrading from v2.64.0 or
/// v2.64.1, whose repositories were handed to www-data and may already carry a
/// planted hook or `core.fsmonitor` written while they were writable. Command
/// scope overrides a repository's own config, so a payload placed before this
/// version was installed is inert from the first deploy after it.
///
/// `core.hooksPath` may name a directory that does not exist; git treats that as
/// "no hooks" rather than an error.
const GIT_NO_EXEC: [&str; 4] = [
    "-c",
    "core.hooksPath=/var/empty/dockpanel-no-hooks",
    "-c",
    "core.fsmonitor=",
];

/// Whether `path` is owned by uid 0.
#[cfg(unix)]
fn owned_by_root(path: &str) -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).map(|m| m.uid() == 0).unwrap_or(false)
}

#[cfg(not(unix))]
fn owned_by_root(_path: &str) -> bool {
    true
}

/// Refuse to run git as root out of a repository root does not own.
///
/// `git` reads a repository's `config` and `hooks/` as executable instruction,
/// so the only repository root may safely operate on is root's own. A foreign
/// one is discarded rather than repaired: repairing means chowning content the
/// web user wrote back to root and then executing it, which is the original
/// defect with an extra step. The caller clones again — one extra clone on a
/// site upgrading from v2.64.x, and no adoption at all of a repository planted
/// by a compromised application.
fn discard_repo_we_do_not_own(site_dir: &str, output_buf: &mut String) {
    let git_dir = format!("{site_dir}/.git");
    if !Path::new(&git_dir).exists() || owned_by_root(&git_dir) {
        return;
    }
    output_buf.push_str(
        "==> This site's repository is not owned by root, so running git against it as root \
         is unsafe. Discarding it and cloning the branch again.\n",
    );
    let _ = std::fs::remove_dir_all(&git_dir);
}

/// Hand a deployed tree to the web server without handing over the repository.
///
/// PHP-FPM runs as www-data and must be able to write the application's own
/// directory — that requirement is real, and it is why v2.64.0 added a
/// recursive chown. The mistake was its reach: a recursive chown includes
/// `.git`, which gives the application `config` and `hooks/`, and git runs both
/// as root on the next deploy.
///
/// So the tree is split. Everything the application needs is the application's;
/// `.git` is root's and unreadable to anyone else. The site directory itself
/// stays root-owned, group www-data, setgid so files the application creates
/// keep the group, and **sticky** so the application cannot unlink `.git` and
/// leave a repository of its own in its place.
///
/// The repository is never handed over even momentarily: chowning the whole tree
/// and taking `.git` back would leave a window in which the application can
/// write a file that root then owns and executes.
async fn hand_tree_to_web_user(dir: &str) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.file_name() == std::ffi::OsStr::new(".git") {
                continue;
            }
            let _ = safe_command("chown")
                .args(["-R", "www-data:www-data", &entry.path().to_string_lossy()])
                .output()
                .await;
        }
    }

    let _ = safe_command("chown").args(["root:www-data", dir]).output().await;
    let _ = safe_command("chmod").args(["3775", dir]).output().await;

    let git_dir = format!("{dir}/.git");
    if Path::new(&git_dir).exists() {
        let _ = safe_command("chown")
            .args(["-R", "root:root", &git_dir])
            .output()
            .await;
        let _ = safe_command("chmod")
            .args(["-R", "go-rwx", &git_dir])
            .output()
            .await;
    }
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

    // SSRF: this process is the one that actually dials `repo_url` (the panel's
    // own check at write time runs on a different host, at a different — often
    // much earlier — moment). See ssrf_guard's module docs.
    crate::services::ssrf_guard::validate_repo_url_not_internal(repo_url).await?;

    let env_ssh = match key_path {
        Some(k) => Some(ssh_command(k)?),
        None => None,
    };

    // Git's `dubious ownership` guard exists so root cannot be tricked into
    // running hooks out of a tree that another user controls. v2.64.1 lifted it,
    // per path and per invocation, so that deploys would keep working after
    // v2.64.0 handed the whole checkout — `.git` included — to www-data. The two
    // together made every git-deployed site a www-data-to-root escalation: the
    // site writes one line into `.git/config` and the next deploy executes it as
    // root. The guard stays switched on now, and the repository is kept root's
    // own instead — see `hand_tree_to_web_user`.
    discard_repo_we_do_not_own(&site_dir, &mut output_buf);

    if Path::new(&git_dir).exists() {
        // Git pull (fetch + reset to match remote)
        let mut cmd = safe_command("git");
        cmd.args(GIT_NO_EXEC)
            .args(["-C", &site_dir, "fetch", "origin", branch])
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
            .args(GIT_NO_EXEC)
            .args(["-C", &site_dir, "reset", "--hard", &format!("origin/{branch}")])
            .output()
            .await
            .map_err(|e| format!("git reset failed: {e}"))?;

        output_buf.push_str(&String::from_utf8_lossy(&reset.stdout));
        output_buf.push_str(&String::from_utf8_lossy(&reset.stderr));

        // The fetch above is checked and this was not, so a reset that failed —
        // most reachably when the configured branch changes and the old checkout
        // still holds local commits — reported a successful deploy over a
        // working tree still serving the previous release.
        if !reset.status.success() {
            return Ok(DeployResult {
                success: false,
                output: output_buf,
                commit_hash: None,
                duration_ms: start.elapsed().as_millis() as u64,
            });
        }
    } else {
        // Fresh clone.
        //
        // The destination is not reliably empty. Creating a site writes a filler
        // page into the document root, so git — which refuses any destination
        // that exists and is not empty — used to fail the FIRST deploy of every
        // site the panel itself created, and with it every step the panel runs
        // only on a successful deploy.
        //
        // Cloning to one side and moving the result in keeps two properties that
        // "delete the directory, then clone" does not: a repository shipping no
        // `public/` still leaves a document root behind, and anything the panel
        // did not write is refused rather than destroyed.
        std::fs::create_dir_all(&site_dir)
            .map_err(|e| format!("Failed to create site dir: {e}"))?;

        let foreign = foreign_entries(&site_dir);
        if !foreign.is_empty() {
            output_buf.push_str(&format!(
                "==> Refusing to deploy into {site_dir}: it already holds files the panel did \
                 not write ({}).\n    Move them aside, or turn on atomic deploys, which build \
                 each release in its own directory.\n",
                foreign.join(", ")
            ));
            return Ok(DeployResult {
                success: false,
                output: output_buf,
                commit_hash: None,
                duration_ms: start.elapsed().as_millis() as u64,
            });
        }

        // Stage beside the site, never inside it: nginx serves the site
        // directory and must never be able to see a half-finished checkout.
        let staging = format!("{WEBROOT}/.{domain}.deploy-staging");
        let _ = std::fs::remove_dir_all(&staging);

        let mut cmd = safe_command("git");
        cmd.args(GIT_NO_EXEC)
            .args(["clone", "--branch", branch, "--single-branch", "--depth", "50", repo_url, &staging])
            .env("GIT_TERMINAL_PROMPT", "0");
        if let Some(ref ssh) = env_ssh {
            cmd.env("GIT_SSH_COMMAND", ssh);
        }

        let clone = match tokio::time::timeout(
            std::time::Duration::from_secs(300),
            cmd.output(),
        )
        .await
        {
            Ok(Ok(c)) => c,
            Ok(Err(e)) => {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(format!("git clone failed: {e}"));
            }
            Err(_) => {
                let _ = std::fs::remove_dir_all(&staging);
                return Err("git clone timed out".to_string());
            }
        };

        output_buf.push_str(&String::from_utf8_lossy(&clone.stdout));
        output_buf.push_str(&String::from_utf8_lossy(&clone.stderr));

        if !clone.status.success() {
            let _ = std::fs::remove_dir_all(&staging);
            return Ok(DeployResult {
                success: false,
                output: output_buf,
                commit_hash: None,
                duration_ms: start.elapsed().as_millis() as u64,
            });
        }

        let moved = match merge_into(Path::new(&staging), Path::new(&site_dir)) {
            Ok(m) => m,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(e);
            }
        };
        let _ = std::fs::remove_dir_all(&staging);
        output_buf.push_str(&format!("==> Placed {} entries into {site_dir}\n", moved.len()));

        // Static and PHP sites are served from `public/`. When the repository
        // supplies none, the filler directory survives the merge and remains
        // exactly what a visitor gets — report that instead of a clean deploy
        // over a page nobody asked for.
        let public_dir = Path::new(&site_dir).join("public");
        if !moved.iter().any(|name| name == "public")
            && public_dir.is_dir()
            && panel_owned(&public_dir)
        {
            output_buf.push_str(
                "==> NOTE: this repository has no public/ directory, and static and PHP sites \
                 are served from one, so visitors still get the placeholder page. Move the \
                 site's files under public/, or use a runtime that proxies to a process.\n",
            );
        }
    }

    // PHP-FPM runs as www-data and must be able to write the application's own
    // directory; root runs git and must not be steerable by what the application
    // writes. Both, without handing over `.git`.
    hand_tree_to_web_user(&site_dir).await;

    // Get current commit hash
    let hash = safe_command("git")
        .args(GIT_NO_EXEC)
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

    // SSRF: this process is the one that actually dials `repo_url` (the panel's
    // own check at write time runs on a different host, at a different — often
    // much earlier — moment). See ssrf_guard's module docs.
    crate::services::ssrf_guard::validate_repo_url_not_internal(repo_url).await?;

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
    cmd.args(GIT_NO_EXEC)
        .args(["clone", "--branch", branch, "--single-branch", "--depth", "1", repo_url, &release_dir])
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
        .args(GIT_NO_EXEC)
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

    // Ensure correct ownership. Same split as the non-atomic path: this release
    // has always been handed to www-data wholesale, which is why `list_releases`
    // could never read its commit hash back without an ownership exception — and
    // why an exception was there to be lifted at all.
    hand_tree_to_web_user(&release_dir).await;

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

            // Get commit hash from release. This answered None on every atomic
            // release for as long as the feature existed, because the release
            // had been handed to www-data; it now reads back because `.git`
            // stays root's. A release still owned by www-data — one written by
            // a version before this — is left answering None rather than
            // adopted, for the reason `discard_repo_we_do_not_own` gives.
            let git_dir = format!("{}/{}", e.path().display(), ".git");
            let commit = if Path::new(&git_dir).exists() {
                crate::safe_cmd::safe_command_sync("git")
                    .args(GIT_NO_EXEC)
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

#[cfg(test)]
mod first_deploy_tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("dockpanel-first-deploy-tests").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent");
        }
        std::fs::write(path, body).expect("write");
    }

    /// The filler page must be recognised through the SAME helper that writes
    /// it, and an operator's own file must not be.
    #[test]
    fn only_the_panels_own_page_is_adoptable() {
        let root = scratch("ownership");

        let ours = root.join("public/index.html");
        write(&ours, &crate::services::nginx::placeholder_page("example.test"));
        assert!(panel_owned(&ours), "the page the panel writes must read as the panel's");
        assert!(panel_owned(&root.join("public")), "a directory of only filler is adoptable");

        let theirs = root.join("public/about.html");
        write(&theirs, "<h1>my real page</h1>");
        assert!(!panel_owned(&theirs), "an operator's file is never the panel's");
        assert!(
            !panel_owned(&root.join("public")),
            "one foreign file makes the whole directory foreign"
        );
    }

    /// A site directory as `put_site` leaves it is fully adoptable, in BOTH
    /// document-root layouts; anything else is named rather than deleted.
    #[test]
    fn a_freshly_created_site_is_adoptable_and_a_real_one_is_not() {
        for layout in ["public/index.html", "index.html"] {
            let root = scratch(&format!("fresh-{}", layout.replace('/', "-")));
            write(&root.join(layout), &crate::services::nginx::placeholder_page("example.test"));
            assert!(
                foreign_entries(root.to_str().unwrap()).is_empty(),
                "a site the panel just created must be clonable into ({layout})"
            );
        }

        let root = scratch("occupied");
        write(&root.join("public/index.html"), &crate::services::nginx::placeholder_page("example.test"));
        write(&root.join("wp-config.php"), "<?php // a real site");
        let foreign = foreign_entries(root.to_str().unwrap());
        assert_eq!(foreign, vec!["wp-config.php".to_string()],
            "a real site must be refused, and named");
    }

    /// The merge replaces what the clone supplies and LEAVES what it does not —
    /// the property that keeps a document root present when a repository has no
    /// `public/`.
    #[test]
    fn merge_replaces_what_it_supplies_and_keeps_what_it_does_not() {
        let base = scratch("merge");
        let site = base.join("site");
        let staged = base.join("staged");

        write(&site.join("public/index.html"), &crate::services::nginx::placeholder_page("example.test"));
        write(&staged.join("index.php"), "<?php echo 'app';");
        write(&staged.join(".git/HEAD"), "ref: refs/heads/main\n");

        let moved = merge_into(&staged, &site).expect("merge");
        assert!(moved.contains(&"index.php".to_string()));
        assert!(moved.contains(&".git".to_string()), "the repository itself must move too");
        assert!(site.join("index.php").is_file(), "the repository's files land in the site");
        assert!(
            site.join("public/index.html").is_file(),
            "an untouched document root must survive, or nginx serves nothing"
        );
    }

    /// The premise, EXECUTED rather than asserted: real git really does refuse
    /// the destination `put_site` leaves behind. If this ever stops being true
    /// the whole staging dance is unnecessary, and this test says so.
    #[test]
    fn git_refuses_the_destination_a_new_site_leaves_behind() {
        let base = scratch("git-premise");
        let origin = base.join("origin");
        std::fs::create_dir_all(&origin).unwrap();

        let git = |args: &[&str], cwd: &Path| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(cwd)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .output()
                .expect("git must be installed for this test to mean anything")
        };
        git(&["init", "-q", "-b", "main", "."], &origin);
        write(&origin.join("index.php"), "<?php");
        git(&["add", "-A"], &origin);
        git(&["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "x"], &origin);

        for layout in ["public/index.html", "index.html"] {
            let dest = base.join(format!("dest-{}", layout.replace('/', "-")));
            write(&dest.join(layout), &crate::services::nginx::placeholder_page("example.test"));

            let out = git(
                &["clone", "-q", origin.to_str().unwrap(), dest.to_str().unwrap()],
                &base,
            );
            assert!(
                !out.status.success(),
                "git cloned into a populated web root ({layout}) — the staging path is now dead code"
            );
            let stderr = String::from_utf8_lossy(&out.stderr).to_lowercase();
            assert!(
                stderr.contains("not an empty directory") || stderr.contains("already exists"),
                "unexpected git failure for {layout}: {stderr}"
            );
        }

        // ...and an empty destination is fine, so it really is the emptiness.
        let empty = base.join("dest-empty");
        std::fs::create_dir_all(&empty).unwrap();
        let out = git(&["clone", "-q", origin.to_str().unwrap(), empty.to_str().unwrap()], &base);
        assert!(out.status.success(), "an empty destination must still clone");
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    use std::path::PathBuf;

    /// Build a repository in the state v2.64.0/v2.64.1 left every git-deployed
    /// site in — owned by another uid — and return its path plus a payload
    /// planted in its config the way a compromised application would.
    ///
    /// The payload is `core.fsmonitor`, not a hook file: it needs no executable
    /// bit and no `hooks/` directory, which is what makes the escalation cheap.
    fn a_repo_owned_by_someone_else(name: &str) -> Option<(PathBuf, PathBuf)> {
        if !nix_is_root() {
            eprintln!("skipped: needs root to chown");
            return None;
        }
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .output()
                .expect("git must be installed")
        };
        git(&["init", "-q", "-b", "main", "."]);
        std::fs::write(dir.join("f.txt"), "x").unwrap();
        git(&["add", "-A"]);
        git(&["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "x"]);

        let proof = dir.join("EXECUTED-AS-ROOT");
        let payload = format!(
            "[core]\n\tfsmonitor = \"/bin/sh -c 'id -u > {}'\"\n",
            proof.display()
        );
        let mut cfg = std::fs::read_to_string(dir.join(".git/config")).unwrap();
        cfg.push_str(&payload);
        std::fs::write(dir.join(".git/config"), cfg).unwrap();

        let chown = std::process::Command::new("chown")
            .args(["-R", "nobody", dir.to_str().unwrap()])
            .status()
            .expect("chown");
        if !chown.success() {
            eprintln!("skipped: chown unavailable");
            let _ = std::fs::remove_dir_all(&dir);
            return None;
        }
        Some((dir, proof))
    }

    /// EXECUTED, not asserted. Two halves, and the second is the control that
    /// proves the first can fail: git still refuses a repository owned by
    /// another user, and the command-scope `safe.directory` exception v2.64.1
    /// shipped really does lift that refusal and really does then run the
    /// application's own payload as root.
    ///
    /// If this control ever stops reproducing, the arm below it is measuring
    /// nothing and must be re-derived rather than trusted.
    #[test]
    fn the_exception_v2_64_1_shipped_really_did_run_the_sites_code_as_root() {
        let Some((dir, proof)) = a_repo_owned_by_someone_else("dockpanel-escalation-control")
        else {
            return;
        };
        let path = dir.to_str().unwrap().to_string();

        let bare = std::process::Command::new("git")
            .args(["-C", &path, "reset", "--hard", "HEAD"])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .unwrap();
        assert!(
            !bare.status.success(),
            "git accepted a repository owned by another user — the ownership guard this fix \
             relies on is gone, and `.git` being root's is no longer sufficient on its own"
        );
        assert!(!proof.exists(), "the guard refused the command and it ran anyway");

        // The exact shape v2.64.1 shipped at deploy.rs:177.
        let exploited = std::process::Command::new("git")
            .args([
                "-c",
                &format!("safe.directory={path}"),
                "-C",
                &path,
                "reset",
                "--hard",
                "HEAD",
            ])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .unwrap();
        assert!(
            exploited.status.success(),
            "the v2.64.1 shape no longer runs at all, so this control proves nothing: {}",
            String::from_utf8_lossy(&exploited.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(&proof).unwrap_or_default().trim(),
            "0",
            "the v2.64.1 shape did not execute the planted payload as root, so the arm below \
             is not measuring the defect it claims to"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `GIT_NO_EXEC` neutralises a planted payload **on its own**, with the
    /// ownership guard deliberately taken out of the picture.
    ///
    /// The repository here is root's, so git raises no ownership objection and
    /// the only thing standing between the payload and uid 0 is the suppression
    /// pair. Testing this against a foreign-owned repository would pass whether
    /// or not `GIT_NO_EXEC` does anything, because the guard would refuse the
    /// command first — green for the wrong reason, which is the failure mode
    /// this project keeps finding in its own arms.
    #[test]
    fn the_suppression_pair_neutralises_a_planted_payload_by_itself() {
        let dir = std::env::temp_dir().join("dockpanel-escalation-closed");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .output()
                .expect("git must be installed")
        };
        git(&["init", "-q", "-b", "main", "."]);
        std::fs::write(dir.join("f.txt"), "x").unwrap();
        git(&["add", "-A"]);
        git(&["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "x"]);

        let proof = dir.join("EXECUTED");
        let mut cfg = std::fs::read_to_string(dir.join(".git/config")).unwrap();
        cfg.push_str(&format!(
            "[core]\n\tfsmonitor = \"/bin/sh -c 'id -u > {}'\"\n",
            proof.display()
        ));
        std::fs::write(dir.join(".git/config"), cfg).unwrap();
        let path = dir.to_str().unwrap().to_string();

        // Control: without the pair, this repository's own config runs.
        let _ = std::process::Command::new("git")
            .args(["-C", &path, "reset", "--hard", "HEAD"])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .unwrap();
        assert!(
            proof.exists(),
            "the planted payload did not fire even without the suppression pair, so this arm \
             cannot tell whether the pair does anything"
        );
        std::fs::remove_file(&proof).unwrap();

        // Shipped shape.
        let shipped = std::process::Command::new("git")
            .args(GIT_NO_EXEC)
            .args(["-C", &path, "reset", "--hard", "HEAD"])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .unwrap();
        assert!(
            shipped.status.success(),
            "the shipped invocation failed outright, so it proves nothing about the payload: {}",
            String::from_utf8_lossy(&shipped.stderr)
        );
        assert!(
            !proof.exists(),
            "the suppression pair did not stop the repository's own config executing as root"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A repository root does not own is discarded, not adopted.
    #[test]
    fn a_foreign_repository_is_discarded_rather_than_repaired() {
        let Some((dir, _proof)) = a_repo_owned_by_someone_else("dockpanel-escalation-discard")
        else {
            return;
        };
        let path = dir.to_str().unwrap().to_string();
        assert!(dir.join(".git").exists(), "fixture did not build a repository");
        assert!(
            !owned_by_root(&format!("{path}/.git")),
            "fixture failed to hand the repository to another uid"
        );

        let mut out = String::new();
        discard_repo_we_do_not_own(&path, &mut out);

        assert!(
            !dir.join(".git").exists(),
            "a repository owned by another user was kept; the next deploy would run git \
             against a tree that user controls"
        );
        assert!(
            out.contains("not owned by root"),
            "the discard was silent — the operator has no way to know a re-clone happened"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The split itself: after handing a tree over, `.git` is still root's and
    /// is unreadable to anyone else, and the site directory carries the setgid
    /// and sticky bits that stop the application replacing the repository.
    #[test]
    fn handing_the_tree_over_does_not_hand_over_the_repository() {
        if !nix_is_root() {
            eprintln!("skipped: needs root to chown");
            return;
        }
        let dir = std::env::temp_dir().join("dockpanel-tree-split");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join("public")).unwrap();
        std::fs::write(dir.join(".git/config"), "[core]\n").unwrap();
        std::fs::write(dir.join("public/index.php"), "<?php\n").unwrap();

        let path = dir.to_str().unwrap().to_string();
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(hand_tree_to_web_user(&path));

        assert!(
            owned_by_root(&format!("{path}/.git")),
            "the repository was handed to the web user — this is the v2.64.0 defect"
        );
        assert!(
            owned_by_root(&format!("{path}/.git/config")),
            "the repository's config was handed to the web user, which is all the \
             escalation ever needed"
        );

        use std::os::unix::fs::PermissionsExt;
        let git_mode = std::fs::metadata(dir.join(".git")).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            git_mode & 0o077,
            0,
            "the repository is readable or writable beyond root (mode {git_mode:o})"
        );

        let site_mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(
            site_mode & 0o1000,
            0o1000,
            "the site directory is not sticky, so the application can unlink .git and leave \
             a repository of its own in its place (mode {:o})",
            site_mode & 0o7777
        );
        assert_eq!(
            site_mode & 0o2000,
            0o2000,
            "the site directory is not setgid, so files the application creates will not \
             carry the web group (mode {:o})",
            site_mode & 0o7777
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn nix_is_root() -> bool {
        std::process::Command::new("id").arg("-u").output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0").unwrap_or(false)
    }
}
