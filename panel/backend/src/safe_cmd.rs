//! Safe command execution helpers.
//!
//! Every child process spawned by the backend MUST use these helpers instead of
//! raw `Command::new()`.  They call `.env_clear()` and set a minimal, safe
//! environment so that inherited variables like `LD_PRELOAD`, `LD_LIBRARY_PATH`,
//! or a tampered `PATH` cannot be used to hijack child processes.

/// Minimal safe PATH containing only system directories.
const SAFE_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// Create an async `tokio::process::Command` with a sanitized environment.
pub fn safe_command(binary: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(binary);
    cmd.env_clear();
    cmd.env("PATH", SAFE_PATH);
    cmd.env("HOME", "/root");
    cmd.env("LANG", "C.UTF-8");
    cmd.env("LC_ALL", "C.UTF-8");
    cmd
}
