use axum::{routing::{get, post}, Json, Router};
use axum::body::Body;
use axum::response::Response;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio_stream::StreamExt;
use crate::safe_cmd::{safe_command, safe_command_unsandboxed};
use crate::services::pkg::{self, Installer, PkgMgr};

use super::AppState;

#[derive(Serialize)]
struct PackageUpdate {
    name: String,
    current_version: String,
    new_version: String,
    repo: String,
    security: bool,
}

#[derive(Deserialize)]
struct ApplyRequest {
    packages: Option<Vec<String>>,
}

#[derive(Serialize)]
struct UpdateCount {
    count: usize,
    security: usize,
    reboot_required: bool,
}

#[derive(Serialize)]
struct RebootResult {
    success: bool,
    message: String,
}

/// Parse a single apt upgradable line into a PackageUpdate.
///
/// Format: `package/repo version_new arch [upgradable from: version_old]`
fn parse_upgradable_line(line: &str) -> Option<PackageUpdate> {
    if !line.contains("upgradable from:") {
        return None;
    }

    // Split "package/repo version_new arch [upgradable from: version_old]"
    let slash_pos = line.find('/')?;
    let name = line[..slash_pos].to_string();

    let after_slash = &line[slash_pos + 1..];
    let parts: Vec<&str> = after_slash.split_whitespace().collect();
    // parts: ["repo", "version_new", "arch", "[upgradable", "from:", "version_old]"]
    if parts.len() < 6 {
        return None;
    }

    let repo = parts[0].to_string();
    let new_version = parts[1].to_string();
    // old version is last element, strip trailing ']'
    let current_version = parts[parts.len() - 1].trim_end_matches(']').to_string();
    let security = repo.contains("security");

    Some(PackageUpdate {
        name,
        current_version,
        new_version,
        repo,
        security,
    })
}

/// `check-update`'s exit status is not a plain success/failure boolean: 0
/// means "ran fine, nothing to upgrade", 100 means "ran fine, updates
/// listed on stdout", and anything else (network down, a broken repo) is a
/// real error. Treating a non-zero exit as failure — the way every other
/// command in this file does — would make the "0 updates" exit look
/// identical to a hard failure that also happens to report 0, for the
/// wrong reason. Returns `None` only for that real-error case.
async fn rpm_check_update(bin: &str) -> Option<String> {
    match safe_command_unsandboxed(bin, &[])
        .args(["check-update"])
        .output_with_timeout(Duration::from_secs(60))
        .await
    {
        Ok(output) => match output.status.code() {
            Some(0) | Some(100) => Some(String::from_utf8_lossy(&output.stdout).into_owned()),
            _ => None,
        },
        Err(_) => None,
    }
}

/// Parse one `dnf`/`yum check-update` line into a PackageUpdate.
///
/// Format: `name.arch    version-release    repo` (whitespace-separated).
/// Section headers ("Obsoleting Packages"), the metadata-refresh banner,
/// and blank lines never have this 3-token shape and are skipped;
/// requiring the version token to contain a digit guards against a repo
/// name that happened to also split into exactly 3 words.
fn parse_rpm_update_line(line: &str) -> Option<PackageUpdate> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() != 3 {
        return None;
    }
    let (name_arch, version, repo) = (parts[0], parts[1], parts[2]);
    if !version.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    // A real package token is always `name.arch` — requiring the dot (rather
    // than falling back to the whole first token when absent) is what
    // rejects a 3-word, digit-containing line that ISN'T a package, such as
    // a hypothetical "Security: 3 patches" summary line.
    let (name, _arch) = name_arch.rsplit_once('.')?;
    Some(PackageUpdate {
        name: name.to_string(),
        current_version: String::new(),
        new_version: version.to_string(),
        repo: repo.to_string(),
        // dnf/yum don't classify security updates in `check-update`'s own
        // output the way apt's repo name does; a real classifier needs
        // `updateinfo list security` parsed and cross-referenced, which
        // this session could not verify against a live RHEL box — left
        // false rather than shipping an unverified parse.
        security: false,
    })
}

/// `rpm -qa` once, as `name -> version-release`, so the RPM listing can show
/// the CURRENTLY installed version the way apt's "upgradable from:" bracket
/// already does — cheaper than one `rpm -q` per package.
async fn rpm_installed_versions() -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    if let Ok(output) = safe_command("rpm")
        .args(["-qa", "--qf", "%{NAME} %{VERSION}-%{RELEASE}\n"])
        .output()
        .await
    {
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if let Some((name, ver)) = line.split_once(' ') {
                map.insert(name.to_string(), ver.to_string());
            }
        }
    }
    map
}

/// Whether the host wants a reboot after applying updates.
///
/// Debian family: `update-notifier-common` (installed by default on server
/// images) drops `/var/run/reboot-required` the moment a reboot-needing
/// package lands. The RHEL family has no equivalent file; `needs-restarting
/// -r` (from `dnf-utils`/`yum-utils`) is the standard proxy — exit 1 means a
/// reboot is recommended, exit 0 means it is not. Neither check is
/// guaranteed present (a minimal RHEL install may lack `dnf-utils`), so
/// absence degrades to "no reboot needed" rather than an error — the same
/// soft-fail shape the file-existence check already had on Debian.
async fn reboot_required() -> bool {
    if tokio::fs::metadata("/var/run/reboot-required").await.is_ok() {
        return true;
    }
    if matches!(pkg::manager().await, PkgMgr::Rpm)
        && let Ok(output) = safe_command("needs-restarting").arg("-r").output().await
    {
        return output.status.code() == Some(1);
    }
    false
}

/// GET /system/updates — list available package updates.
async fn list_updates() -> Json<Vec<PackageUpdate>> {
    if matches!(pkg::manager().await, PkgMgr::Rpm) {
        let bin = match pkg::installer().await {
            Some(Installer::Dnf) => "dnf",
            Some(Installer::Yum) => "yum",
            _ => return Json(Vec::new()),
        };
        let mut packages: Vec<PackageUpdate> = match rpm_check_update(bin).await {
            Some(stdout) => stdout.lines().filter_map(parse_rpm_update_line).collect(),
            None => Vec::new(),
        };
        if !packages.is_empty() {
            let installed = rpm_installed_versions().await;
            for p in &mut packages {
                if let Some(v) = installed.get(&p.name) {
                    p.current_version = v.clone();
                }
            }
        }
        packages.sort_by(|a, b| a.name.cmp(&b.name));
        return Json(packages);
    }

    // Run apt update first (suppress output, 60s timeout). Wrapped in
    // systemd-run because `apt-get update` writes to /var/lib/apt/lists,
    // which the agent's ProtectSystem=strict sandbox blocks.
    let _ = tokio::time::timeout(
        Duration::from_secs(60),
        safe_command_unsandboxed("apt-get", &[])
            .args(["update", "-qq"])
            .output(),
    )
    .await;

    // Get upgradable list
    let output = tokio::time::timeout(
        Duration::from_secs(60),
        safe_command("apt")
            .args(["list", "--upgradable"])
            .stderr(std::process::Stdio::null())
            .output(),
    )
    .await;

    let mut packages = Vec::new();

    if let Ok(Ok(output)) = output {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if let Some(pkg) = parse_upgradable_line(line) {
                packages.push(pkg);
            }
        }
    }

    // Sort: security first, then alphabetically
    packages.sort_by(|a, b| {
        b.security
            .cmp(&a.security)
            .then_with(|| a.name.cmp(&b.name))
    });

    Json(packages)
}

/// POST /system/updates/apply — apply package updates with streaming NDJSON output.
///
/// Returns newline-delimited JSON: each line is `{"type":"line","line":"..."}` for output,
/// and the final line is `{"type":"done","success":bool,"reboot_required":bool}`.
async fn apply_updates(Json(body): Json<ApplyRequest>) -> Response {
    let has_packages = body
        .packages
        .as_ref()
        .is_some_and(|p| !p.is_empty());

    // Validate package names up-front
    if has_packages {
        for pkg in body.packages.as_ref().unwrap() {
            if pkg.is_empty()
                || !pkg
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '+' || c == ':')
            {
                let error_line = serde_json::json!({"type":"line","line":format!("Invalid package name: {pkg}")});
                let done_line = serde_json::json!({"type":"done","success":false,"reboot_required":false});
                let body_str = format!("{}\n{}\n", error_line, done_line);
                return Response::builder()
                    .header("content-type", "application/x-ndjson")
                    .body(Body::from(body_str))
                    .unwrap();
            }
        }
    }

    let (tx, rx) = tokio::sync::mpsc::channel::<String>(128);

    tokio::spawn(async move {
        // Wrapped in systemd-run: apt-get/dnf/yum install/upgrade write to
        // /var/cache/{apt,dnf}, /var/lib/{dpkg,rpm}, and /usr, all blocked
        // by the agent's ProtectSystem=strict sandbox.
        let installer_bin = match pkg::installer().await {
            Some(Installer::AptGet) => "apt-get",
            Some(Installer::Dnf) => "dnf",
            Some(Installer::Yum) => "yum",
            None => {
                let line = serde_json::json!({"type":"line","line":"No supported package manager found"});
                let done = serde_json::json!({"type":"done","success":false,"reboot_required":false});
                let _ = tx.send(format!("{line}\n")).await;
                let _ = tx.send(format!("{done}\n")).await;
                return;
            }
        };
        let is_apt = installer_bin == "apt-get";
        let mut cmd = safe_command_unsandboxed(installer_bin, &[]);

        if has_packages {
            let packages = body.packages.unwrap();
            // apt: `install` picks up the candidate (newest) version of an
            // already-installed package, so it doubles as an upgrade.
            // dnf/yum's `install` would happily install a MISTYPED name
            // that isn't currently present at all; `upgrade <name>` only
            // touches something already installed, matching what "apply
            // this update" is actually supposed to do.
            cmd.arg(if is_apt { "install" } else { "upgrade" }).arg("-y");
            for pkg_name in &packages {
                cmd.arg(pkg_name);
            }
        } else {
            cmd.args(["upgrade", "-y"]);
        }

        // stdout and stderr both arrive on one channel: the escape hatch no
        // longer passes descriptors (see safe_cmd::UnsandboxedCommand), so the
        // inner command's output is tailed out of its capture files.
        let mut run = match cmd.spawn_streaming() {
            Ok(r) => r,
            Err(e) => {
                let _ = tx.send(format!("{}\n", serde_json::json!({"type":"line","line":format!("Failed to start apt: {e}")}))).await;
                let _ = tx.send(format!("{}\n", serde_json::json!({"type":"done","success":false,"reboot_required":false}))).await;
                return;
            }
        };

        while let Some(line) = run.lines.recv().await {
            if line.is_empty() { continue; }
            let msg = serde_json::json!({"type":"line","line":line});
            if tx.send(format!("{msg}\n")).await.is_err() { break; }
        }

        // The channel closes only after the inner command has exited and its
        // output has been drained, so this resolves immediately in practice.
        let success = match tokio::time::timeout(Duration::from_secs(10), run.status()).await {
            Ok(Ok(status)) => status.success(),
            _ => false,
        };

        let reboot_needed = reboot_required().await;

        let done = serde_json::json!({
            "type": "done",
            "success": success,
            "reboot_required": reboot_needed,
        });
        let _ = tx.send(format!("{done}\n")).await;
    });

    let stream = tokio_stream::wrappers::ReceiverStream::new(rx)
        .map(|line| Ok::<_, std::convert::Infallible>(line));

    Response::builder()
        .header("content-type", "application/x-ndjson")
        .body(Body::from_stream(stream))
        .unwrap()
}

/// GET /system/updates/count — quick count of available updates (no apt update).
async fn update_count() -> Json<UpdateCount> {
    let (count, security) = if matches!(pkg::manager().await, PkgMgr::Rpm) {
        let bin = match pkg::installer().await {
            Some(Installer::Dnf) => Some("dnf"),
            Some(Installer::Yum) => Some("yum"),
            _ => None,
        };
        match bin {
            Some(b) => match rpm_check_update(b).await {
                Some(stdout) => (
                    stdout.lines().filter(|l| parse_rpm_update_line(l).is_some()).count(),
                    0, // security classification not implemented for RPM this session
                ),
                None => (0, 0),
            },
            None => (0, 0),
        }
    } else {
        let output = safe_command("apt")
            .args(["list", "--upgradable"])
            .stderr(std::process::Stdio::null())
            .output()
            .await;

        match output {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let mut total = 0usize;
                let mut sec = 0usize;
                for line in stdout.lines() {
                    if line.contains("upgradable from:") {
                        total += 1;
                        if line.contains("security") {
                            sec += 1;
                        }
                    }
                }
                (total, sec)
            }
            Err(_) => (0, 0),
        }
    };

    let reboot_required = reboot_required().await;

    Json(UpdateCount { count, security, reboot_required })
}

/// POST /system/reboot — schedule a system reboot in 1 minute.
async fn system_reboot() -> Json<RebootResult> {
    let result = safe_command("shutdown")
        .args(["-r", "+1", "DockPanel initiated reboot"])
        .output()
        .await;

    match result {
        Ok(output) if output.status.success() => Json(RebootResult {
            success: true,
            message: "System will reboot in 1 minute".to_string(),
        }),
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            Json(RebootResult {
                success: false,
                message: format!("Reboot command failed: {stderr}"),
            })
        }
        Err(e) => Json(RebootResult {
            success: false,
            message: format!("Failed to execute shutdown: {e}"),
        }),
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/system/updates", get(list_updates))
        .route("/system/updates/apply", post(apply_updates))
        .route("/system/updates/count", get(update_count))
        .route("/system/reboot", post(system_reboot))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real `dnf check-update` line, Rocky/RHEL 9 shape.
    #[test]
    fn parses_a_real_dnf_check_update_line() {
        let pkg = parse_rpm_update_line("bash.x86_64    5.1.8-4.el9_2    baseos")
            .expect("a genuine 3-token update line must parse");
        assert_eq!(pkg.name, "bash");
        assert_eq!(pkg.new_version, "5.1.8-4.el9_2");
        assert_eq!(pkg.repo, "baseos");
    }

    /// A real `yum check-update` line, epoch-qualified — same 3-token shape.
    #[test]
    fn parses_a_yum_line_with_an_epoch_qualified_version() {
        let pkg = parse_rpm_update_line("kernel.x86_64    3:5.14.0-284.11.1.el9_2    baseos")
            .expect("an epoch-qualified version is still a 3-token line");
        assert_eq!(pkg.name, "kernel");
        assert_eq!(pkg.new_version, "3:5.14.0-284.11.1.el9_2");
    }

    /// The exact class of line this parser exists to NOT misread as a
    /// package: `check-update`'s own section headers and refresh banner.
    #[test]
    fn skips_section_headers_and_the_metadata_banner() {
        for noise in [
            "",
            "Last metadata expiration check: 0:12:34 ago on Mon 04 Sep 2026.",
            "Obsoleting Packages",
            "Security: 3 patches",
        ] {
            assert!(
                parse_rpm_update_line(noise).is_none(),
                "must not parse as a package: {noise:?}"
            );
        }
    }

    /// The digit-in-version guard is what tells a real update line apart
    /// from a 3-word line that ISN'T one — this is what would misfire
    /// without it. `Some(0) | Some(100)` alone can't catch this; it is a
    /// per-line parsing decision, not an exit-code one.
    #[test]
    fn a_three_word_line_with_no_digit_in_the_middle_token_is_not_a_package() {
        assert!(parse_rpm_update_line("word.one word two").is_none());
    }

    /// `name.arch` splits on the LAST dot, so a package name that itself
    /// contains a dot (real example: `python3.11-libs`) keeps its name
    /// intact and only the trailing arch token is stripped.
    #[test]
    fn strips_only_the_trailing_arch_not_a_dotted_package_name() {
        let pkg = parse_rpm_update_line("python3.11-libs.x86_64    3.11.7-1.el9    appstream")
            .expect("dotted package name must still parse");
        assert_eq!(pkg.name, "python3.11-libs");
    }
}
