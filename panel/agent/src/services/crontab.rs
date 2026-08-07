//! Root's crontab, read and written as a whole file.
//!
//! `crontab` has no partial-update verb: every caller reads the entire file,
//! edits lines, and writes the entire file back. That makes the READ the
//! dangerous half — an empty read is indistinguishable from "no entries", and
//! writing that back deletes every job on the box, belonging to every tenant and
//! to the operator.
//!
//! `routes/crons.rs` learned this and grew a fail-closed reader. Its twin in
//! `services/wordpress.rs::set_auto_update` did not, and went on collapsing a
//! failed `crontab -l` into `""` with `.unwrap_or_default()` — reached from
//! `POST /api/sites/{id}/wordpress/auto-update`, an ordinary authenticated
//! route naming ONE site, with a box-wide blast radius. A fix applied to one
//! instance of a pattern owes a grep for the pattern; this module is that grep's
//! answer, so there is now exactly one reader and one writer to get right.
//!
//! `crontab-fail-closed-pin-e2e` arms enumerate every `crontab` invocation in
//! this crate and require each writer to route through here.

use crate::safe_cmd::safe_command;
use std::time::Duration;

/// Read root's crontab, failing CLOSED.
///
/// `Ok("")` means root genuinely has no crontab. Anything that merely *looks*
/// like an empty crontab — a timeout, a spawn failure, a non-zero exit that is
/// not the "no crontab for ..." message — is an `Err`, because every caller
/// feeds this straight back into [`write_crontab`] as the complete new content.
/// Collapsing the two used to mean a slow `crontab -l` destroyed every operator
/// and system entry on the box.
///
/// The `-u root` is explicit rather than implied by the agent's own uid: this
/// function names the file it is about to let a caller overwrite.
pub async fn read_crontab() -> Result<String, String> {
    let output = tokio::time::timeout(
        Duration::from_secs(15),
        safe_command("crontab").args(["-l", "-u", "root"]).output(),
    )
    .await;

    match output {
        Ok(Ok(o)) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).to_string()),
        Ok(Ok(o)) => {
            let stderr = String::from_utf8_lossy(&o.stderr).to_lowercase();
            // The one benign non-zero exit: a user with no crontab at all. Every
            // other non-zero exit is refused, because the caller's next act is a
            // full overwrite.
            if stderr.contains("no crontab for") {
                Ok(String::new())
            } else {
                Err(format!(
                    "Refusing to rewrite the crontab: `crontab -l` exited {} ({}). \
                     Treating that as an empty crontab would delete every existing entry.",
                    o.status.code().unwrap_or(-1),
                    stderr.trim()
                ))
            }
        }
        Ok(Err(e)) => Err(format!(
            "Refusing to rewrite the crontab: `crontab -l` failed to run: {e}"
        )),
        Err(_) => Err("Refusing to rewrite the crontab: `crontab -l` timed out after 15s".to_string()),
    }
}

/// Replace root's crontab with `content`.
pub async fn write_crontab(content: &str) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;

    let mut child = safe_command("crontab")
        .args(["-u", "root", "-"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn crontab: {e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(content.as_bytes())
            .await
            .map_err(|e| format!("Failed to write crontab: {e}"))?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|e| format!("Failed to write crontab newline: {e}"))?;
    }

    match tokio::time::timeout(Duration::from_secs(10), child.wait()).await {
        Ok(Ok(status)) => {
            if status.success() {
                Ok(())
            } else {
                Err("crontab command failed".into())
            }
        }
        Ok(Err(e)) => Err(format!("crontab failed: {e}")),
        Err(_) => {
            let _ = child.kill().await;
            Err("crontab timed out after 10s".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The distinction the whole module exists for, pinned on the classifier
    /// rather than on a live `crontab` binary: only the "no crontab for" text
    /// may become an empty string. Everything else must be an Err, because the
    /// caller's next act overwrites the file.
    fn classify(success: bool, stderr: &str, code: i32) -> Result<String, String> {
        if success {
            return Ok(String::new());
        }
        let s = stderr.to_lowercase();
        if s.contains("no crontab for") {
            Ok(String::new())
        } else {
            Err(format!("exited {code} ({})", s.trim()))
        }
    }

    #[test]
    fn absent_crontab_is_empty_not_an_error() {
        assert!(classify(false, "no crontab for root", 1).is_ok());
    }

    #[test]
    fn every_other_failure_refuses() {
        assert!(classify(false, "must be privileged to use -u", 1).is_err());
        assert!(classify(false, "", 127).is_err());
        assert!(classify(false, "Resource temporarily unavailable", 1).is_err());
    }

    #[tokio::test]
    async fn reader_never_yields_empty_on_a_missing_binary() {
        // Names a binary that does not exist, so the spawn fails. The old
        // `.unwrap_or_default()` shape returned Ok("") here, and the caller
        // wrote that back as root's complete crontab.
        let out = tokio::time::timeout(
            Duration::from_secs(5),
            safe_command("dockpanel-no-such-crontab-binary")
                .arg("-l")
                .output(),
        )
        .await;
        let collapsed = matches!(&out, Ok(Ok(o)) if o.status.success());
        assert!(
            !collapsed,
            "a missing crontab binary must not present as a successful empty read"
        );
    }
}
