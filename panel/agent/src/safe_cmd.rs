//! Safe command execution helpers.
//!
//! Every child process spawned by the agent MUST use these helpers instead of
//! raw `Command::new()`.  They call `.env_clear()` and set a minimal, safe
//! environment so that inherited variables like `LD_PRELOAD`, `LD_LIBRARY_PATH`,
//! or a tampered `PATH` cannot be used to hijack child processes.

/// Minimal safe PATH containing only system directories.
const SAFE_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// Create an async `tokio::process::Command` with a sanitized environment.
///
/// The child process starts with an **empty** environment and only receives:
/// - `PATH`  – system directories only
/// - `HOME`  – `/root`
/// - `LANG`  – `C.UTF-8`
/// - `LC_ALL` – `C.UTF-8`
///
/// Callers that need additional env vars (e.g. `PGPASSWORD`) should add them
/// via `.env("KEY", "value")` **after** calling this function.
pub fn safe_command(binary: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(binary);
    cmd.env_clear();
    cmd.env("PATH", SAFE_PATH);
    cmd.env("HOME", "/root");
    cmd.env("LANG", "C.UTF-8");
    cmd.env("LC_ALL", "C.UTF-8");
    cmd
}

/// Create a synchronous `std::process::Command` with a sanitized environment.
///
/// Same safety guarantees as [`safe_command`] but for blocking contexts
/// (e.g. `app_process.rs` which writes systemd units synchronously).
pub fn safe_command_sync(binary: &str) -> std::process::Command {
    let mut cmd = std::process::Command::new(binary);
    cmd.env_clear();
    cmd.env("PATH", SAFE_PATH);
    cmd.env("HOME", "/root");
    cmd.env("LANG", "C.UTF-8");
    cmd.env("LC_ALL", "C.UTF-8");
    cmd
}
