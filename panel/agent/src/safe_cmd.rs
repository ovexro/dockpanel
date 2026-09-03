//! Safe command execution helpers.
//!
//! Every child process spawned by the agent MUST use these helpers instead of
//! raw `Command::new()`.  They call `.env_clear()` and set a minimal, safe
//! environment so that inherited variables like `LD_PRELOAD`, `LD_LIBRARY_PATH`,
//! or a tampered `PATH` cannot be used to hijack child processes.

/// Minimal safe PATH containing only system directories.
const SAFE_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// Config directory handed to every docker CLI invocation.
///
/// The agent unit runs `ProtectHome=yes`, so the `HOME=/root` these helpers set
/// is an empty read-only mount inside the sandbox. The docker CLI creates
/// `$HOME/.docker` on first use, and `docker build` does not tolerate failing to
/// — it aborts with `mkdir /root/.docker: read-only file system`. That took down
/// every Dockerfile-based git deploy on every hardened install (s261): the clone
/// succeeded, the build never ran once. Point the CLI at a directory that IS in
/// the unit's `ReadWritePaths` instead of widening the sandbox. Set here rather
/// than at a call site because `env_clear()` means a unit-level `Environment=`
/// would never reach the child, and because ~77 docker invocations share these
/// helpers — one of them silently missing it is exactly the drift that hid this.
pub const DOCKER_CONFIG_DIR: &str = "/var/lib/dockpanel/docker";

/// Create an async `tokio::process::Command` with a sanitized environment.
///
/// The child process starts with an **empty** environment and only receives:
/// - `PATH`  – system directories only
/// - `HOME`  – `/root`
/// - `LANG`  – `C.UTF-8`
/// - `LC_ALL` – `C.UTF-8`
/// - `DOCKER_CONFIG` – a writable dir, since `HOME` is not one under the sandbox
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
    cmd.env("DOCKER_CONFIG", DOCKER_CONFIG_DIR);
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
    cmd.env("DOCKER_CONFIG", DOCKER_CONFIG_DIR);
    cmd
}

/// Directory the transient unit's captured output is staged in.
///
/// Must live under a path PID1 itself may write. `/tmp` is NOT usable: on the
/// RHEL family PID1 runs as `init_t` and writing a `tmp_t` file is denied
/// (measured on Rocky 9.8). `/var/lib/dockpanel` is `var_lib_t`, which is
/// allowed, and is already in the agent unit's `ReadWritePaths` so the agent
/// can read the result back.
const CAPTURE_DIR: &str = "/var/lib/dockpanel/run";

/// The filesystem path capture logic actually writes to.
///
/// Identical to [`CAPTURE_DIR`] in production. Under `cargo test` it
/// resolves instead to a scratch directory under the process's own temp
/// dir: `cargo test` runs unprivileged (concretely: GitHub Actions' runner
/// user), which cannot `create_dir_all` anything under `/var/lib` — every
/// `DockerEnvFile` test failed with `PermissionDenied` in CI from the
/// moment those tests started actually calling `.new()` (v2.201.0) because
/// nothing distinguished a test run from a production one. `CAPTURE_DIR`
/// itself stays the literal production path so the RHEL SELinux pin below
/// keeps measuring what actually ships.
///
/// Branches on the `cfg!(test)` macro rather than a `#[cfg(test)]`
/// attribute deliberately: an attribute would put the literal token
/// `#[cfg(test)]` ahead of the real `#[cfg(test)] mod tests` block this
/// file already ends with, and every pin suite that reads this file treats
/// the FIRST such token as "everything after this is test code, stop
/// measuring" — see reference_dockpanel_ops_p7's mid-file cfg(test) gate.
fn capture_dir() -> std::path::PathBuf {
    if cfg!(test) {
        std::env::temp_dir().join("dockpanel-test-capture")
    } else {
        std::path::PathBuf::from(CAPTURE_DIR)
    }
}

/// Counter making capture filenames unique within a process.
static CAPTURE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A command that runs outside the agent's `ProtectSystem=strict` sandbox in a
/// transient systemd unit.
///
/// PID1 spawns the unit in its own mount namespace, so the inner binary sees
/// the full filesystem read-write — necessary for commands like
/// `apt-get`/`dnf install` that must write to `/var/cache/apt`,
/// `/var/lib/dpkg`, `/var/lib/rpm` and `/usr`, none of which are in the agent
/// unit's `ReadWritePaths`.
///
/// # Why this is not just a `Command` with `--pipe`
///
/// It used to be, and **that made every package operation fail on the entire
/// RHEL family** (s266 found the wall; s267 root-caused it). `systemd-run
/// --pipe` passes the caller's stdin/stdout/stderr **as file descriptors over
/// D-Bus**. On RHEL the system bus is `dbus-broker` (`system_dbusd_t`), and
/// SELinux checks the *receiver's* access to a passed object via its
/// `file_receive` hook. Receiving a writable pipe labelled
/// `unconfined_service_t` — the label every systemd service's pipes carry — is
/// denied, so the broker drops the connection and `systemd-run` reports:
///
///     Failed to start transient service unit: Connection reset by peer
///
/// The rule is `dontaudit`ed, so **nothing is logged**; the denial is only
/// visible after `semodule -DB`, and `ausearch` did not surface it even then
/// (grep `/var/log/audit/audit.log` directly). The same command works from a
/// *shell* because a shell's pipes are labelled `unconfined_t`, which policy
/// permits — that is the whole of the shell-vs-service discriminator.
///
/// So we never pass a descriptor. `-p StandardOutput=file:…` has PID1 open the
/// capture files itself, and `--wait` still propagates the inner command's exit
/// status. This path is used on **every** distro, not just RHEL: a second code
/// path exercised only on a family we do not develop on is exactly how the
/// original defect survived (#93d).
///
/// Env vars passed via `extra_env` reach the inner binary as `--setenv=K=V`.
/// The defaults (PATH, HOME, LANG, LC_ALL, DEBIAN_FRONTEND, DOCKER_CONFIG) are
/// always set so it does not inherit PID1's wider environment.
pub struct UnsandboxedCommand {
    argv: Vec<std::ffi::OsString>,
}

impl UnsandboxedCommand {
    fn new(binary: &str, extra_env: &[(&str, &str)]) -> Self {
        Self {
            argv: unsandboxed_prefix(binary, extra_env),
        }
    }

    /// Append one argument to the **inner** binary.
    pub fn arg<S: AsRef<std::ffi::OsStr>>(&mut self, a: S) -> &mut Self {
        self.argv.push(a.as_ref().to_os_string());
        self
    }

    /// Append several arguments to the **inner** binary.
    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        for a in args {
            self.argv.push(a.as_ref().to_os_string());
        }
        self
    }

    /// Run to completion, returning the inner command's status and output.
    pub async fn output(&mut self) -> std::io::Result<std::process::Output> {
        let cap = Capture::new()?;
        let mut cmd = tokio::process::Command::new("systemd-run");
        cmd.env_clear();
        cmd.env("PATH", SAFE_PATH);
        cmd.args(cap.properties());
        cmd.args(&self.argv);
        // Callers building the inner argv have no way to reach this
        // `Command` themselves, so `kill_on_drop` must be set HERE — the one
        // place that can. Without it, a caller-side `tokio::time::timeout`
        // around this future does not kill `systemd-run` on expiry: the
        // future is dropped, but the child (and whatever it started via
        // `--wait`) keeps running orphaned. Same class as every
        // caller-missing-`.kill_on_drop(true)` bug already fixed on the
        // `safe_command`-built path (s446-s457).
        cmd.kill_on_drop(true);
        let raw = cmd.output().await?;
        Ok(cap.finish(raw))
    }

    /// Spawn and stream the inner command's output line by line as it is
    /// produced, instead of buffering it all.
    ///
    /// The system-updates endpoint streams `apt-get` progress to the browser
    /// over NDJSON, which `--pipe` used to provide for free. Output now lands
    /// in the capture files, so we tail them. stdout and stderr are merged into
    /// one ordered-per-stream channel, which is what that endpoint already did
    /// with its two reader tasks.
    pub fn spawn_streaming(&mut self) -> std::io::Result<StreamingRun> {
        let cap = Capture::new()?;
        let mut cmd = tokio::process::Command::new("systemd-run");
        cmd.env_clear();
        cmd.env("PATH", SAFE_PATH);
        cmd.args(cap.properties());
        cmd.args(&self.argv);
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::piped());
        cmd.kill_on_drop(true);
        let mut child = cmd.spawn()?;

        let (tx, lines) = tokio::sync::mpsc::channel::<String>(128);
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut tails = vec![
            tokio::spawn(tail_lines(cap.out.clone(), tx.clone(), done.clone())),
            tokio::spawn(tail_lines(cap.err.clone(), tx.clone(), done.clone())),
        ];
        // systemd-run's OWN stderr must be drained, not merely piped: under
        // --quiet it is silent unless the transient unit cannot be started at
        // all, and that message is then the only explanation the user would
        // ever get. Leaving it unread would also let a full pipe buffer block
        // the process we are waiting on.
        if let Some(own_err) = child.stderr.take() {
            let tx_own = tx.clone();
            tails.push(tokio::spawn(async move {
                use tokio::io::AsyncBufReadExt;
                let mut l = tokio::io::BufReader::new(own_err).lines();
                while let Ok(Some(line)) = l.next_line().await {
                    if tx_own.send(line).await.is_err() {
                        break;
                    }
                }
            }));
        }
        drop(tx);

        let (status_tx, status) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let result = child.wait().await;
            // Set the flag BEFORE joining, so each tailer performs one final
            // read after the process is gone and cannot lose a trailing line.
            done.store(true, std::sync::atomic::Ordering::Relaxed);
            for t in tails {
                let _ = t.await;
            }
            cap.cleanup();
            let _ = status_tx.send(result);
        });

        Ok(StreamingRun { lines, status })
    }
}

/// A running [`UnsandboxedCommand`] whose output is being streamed.
///
/// Drain `lines` to completion first — it closes once the inner command has
/// exited and its output has been fully read — then await [`Self::status`].
pub struct StreamingRun {
    pub lines: tokio::sync::mpsc::Receiver<String>,
    status: tokio::sync::oneshot::Receiver<std::io::Result<std::process::ExitStatus>>,
}

impl StreamingRun {
    /// The inner command's exit status, once it has finished.
    pub async fn status(self) -> std::io::Result<std::process::ExitStatus> {
        match self.status.await {
            Ok(r) => r,
            Err(_) => Err(std::io::Error::other("status channel closed")),
        }
    }
}

/// Follow a capture file, emitting complete lines as they appear.
///
/// `done` is checked *before* each read so that the read which observes it set
/// is guaranteed to happen after the writer exited — otherwise the last line of
/// an install would be dropped whenever it arrived inside the poll interval.
async fn tail_lines(
    path: std::path::PathBuf,
    tx: tokio::sync::mpsc::Sender<String>,
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let mut pos: u64 = 0;
    let mut carry = String::new();
    loop {
        let finished = done.load(std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut f) = tokio::fs::File::open(&path).await {
            if f.seek(std::io::SeekFrom::Start(pos)).await.is_ok() {
                let mut buf = Vec::new();
                if let Ok(n) = f.read_to_end(&mut buf).await {
                    if n > 0 {
                        pos += n as u64;
                        carry.push_str(&String::from_utf8_lossy(&buf));
                        while let Some(i) = carry.find('\n') {
                            let line: String = carry.drain(..=i).collect();
                            if tx
                                .send(line.trim_end_matches(['\n', '\r']).to_string())
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                }
            }
        }
        if finished {
            if !carry.is_empty() {
                let _ = tx.send(carry).await;
            }
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

/// Synchronous sibling of [`UnsandboxedCommand`] for blocking contexts
/// (e.g. `services/smtp.rs::ensure_msmtp` which installs msmtp via apt).
pub struct UnsandboxedCommandSync {
    argv: Vec<std::ffi::OsString>,
}

impl UnsandboxedCommandSync {
    fn new(binary: &str, extra_env: &[(&str, &str)]) -> Self {
        Self {
            argv: unsandboxed_prefix(binary, extra_env),
        }
    }

    /// Append one argument to the **inner** binary.
    pub fn arg<S: AsRef<std::ffi::OsStr>>(&mut self, a: S) -> &mut Self {
        self.argv.push(a.as_ref().to_os_string());
        self
    }

    /// Append several arguments to the **inner** binary.
    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        for a in args {
            self.argv.push(a.as_ref().to_os_string());
        }
        self
    }

    /// Run to completion, returning the inner command's status and output.
    pub fn output(&mut self) -> std::io::Result<std::process::Output> {
        let cap = Capture::new()?;
        let mut cmd = std::process::Command::new("systemd-run");
        cmd.env_clear();
        cmd.env("PATH", SAFE_PATH);
        cmd.args(cap.properties());
        cmd.args(&self.argv);
        let raw = cmd.output()?;
        Ok(cap.finish(raw))
    }
}

/// The `systemd-run` argv shared by both flavours, up to and including the
/// inner binary. Deliberately carries **no** `--pipe`: see [`UnsandboxedCommand`].
fn unsandboxed_prefix(binary: &str, extra_env: &[(&str, &str)]) -> Vec<std::ffi::OsString> {
    let mut argv: Vec<std::ffi::OsString> = Vec::new();
    let mut push = |s: String| argv.push(std::ffi::OsString::from(s));
    push("--quiet".into());
    push("--wait".into());
    push("--collect".into());
    push(format!("--setenv=PATH={SAFE_PATH}"));
    push("--setenv=HOME=/root".into());
    push("--setenv=LANG=C.UTF-8".into());
    push("--setenv=LC_ALL=C.UTF-8".into());
    push("--setenv=DEBIAN_FRONTEND=noninteractive".into());
    push(format!("--setenv=DOCKER_CONFIG={DOCKER_CONFIG_DIR}"));
    for (k, v) in extra_env {
        push(format!("--setenv={k}={v}"));
    }
    push("--".into());
    push(binary.to_string());
    argv
}

/// A pair of files PID1 writes the transient unit's stdout/stderr into.
///
/// Created 0600 *before* the run — package output can carry credentials, and
/// systemd's `file:` scheme would otherwise create them 0644.
struct Capture {
    out: std::path::PathBuf,
    err: std::path::PathBuf,
}

impl Capture {
    fn new() -> std::io::Result<Self> {
        std::fs::create_dir_all(capture_dir())?;
        let _ = std::fs::set_permissions(
            capture_dir(),
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        );
        Self::sweep_stale();
        let n = CAPTURE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let base = format!("{}-{}", std::process::id(), n);
        let out = capture_dir().join(format!("{base}.out"));
        let err = capture_dir().join(format!("{base}.err"));
        for p in [&out, &err] {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .mode(0o600)
                .open(p)?;
        }
        Ok(Self { out, err })
    }

    /// A run abandoned mid-flight (the callers wrap these in
    /// `tokio::time::timeout`) leaves its pair behind. Drop anything older than
    /// an hour so a box that times out repeatedly does not accumulate them.
    fn sweep_stale() {
        let Ok(entries) = std::fs::read_dir(capture_dir()) else {
            return;
        };
        let cutoff = std::time::Duration::from_secs(3600);
        for e in entries.flatten() {
            let stale = e
                .metadata()
                .and_then(|m| m.modified())
                .map(|t| t.elapsed().map(|d| d > cutoff).unwrap_or(false))
                .unwrap_or(false);
            if stale {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }

    fn properties(&self) -> Vec<String> {
        vec![
            "-p".into(),
            format!("StandardOutput=file:{}", self.out.display()),
            "-p".into(),
            format!("StandardError=file:{}", self.err.display()),
        ]
    }

    /// Swap `systemd-run`'s own (empty) output for what the inner binary wrote.
    ///
    /// `systemd-run`'s own stderr is appended rather than discarded: when the
    /// transient unit cannot be started at all, that string is the only
    /// diagnostic there is, and the capture files stay empty.
    /// Remove the pair without reading it — the streaming path has already
    /// delivered every line to its caller.
    fn cleanup(self) {
        let _ = std::fs::remove_file(&self.out);
        let _ = std::fs::remove_file(&self.err);
    }

    fn finish(self, raw: std::process::Output) -> std::process::Output {
        let stdout = std::fs::read(&self.out).unwrap_or_default();
        let mut stderr = std::fs::read(&self.err).unwrap_or_default();
        stderr.extend_from_slice(&raw.stderr);
        let _ = std::fs::remove_file(&self.out);
        let _ = std::fs::remove_file(&self.err);
        std::process::Output {
            status: raw.status,
            stdout,
            stderr,
        }
    }
}

/// A temporary `docker run`/`docker exec` `--env-file` holding secrets (DB
/// passwords) that must reach a container WITHOUT appearing in the `docker`
/// CLI's own argv.
///
/// `/proc/<pid>/cmdline` is world-readable on every install this agent
/// targets (no `hidepid=` mount restriction assumed or required elsewhere in
/// this codebase), so a `-e MYSQL_PWD=secret`/`-e PGPASSWORD=secret` argument
/// is visible to any local process for the lifetime of the `docker` child —
/// including, concretely, a site-terminal session running as `www-data`. A
/// `--env-file <path>` argument leaks only the path; the secret lives in a
/// 0600 file only this agent's uid can read, and is gone the instant this
/// guard drops.
///
/// Written immediately before use under [`CAPTURE_DIR`] (already proven
/// writable + `dontaudit`-clean on every hardened distro this agent
/// supports) and removed on drop — keep the guard bound in scope for as long
/// as the `docker` child that reads it may still be running.
pub struct DockerEnvFile {
    path: std::path::PathBuf,
}

impl DockerEnvFile {
    /// Write `vars` as `KEY=VALUE` lines to a fresh 0600 temp file.
    pub fn new(vars: &[(&str, &str)]) -> std::io::Result<Self> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::create_dir_all(capture_dir())?;
        let _ = std::fs::set_permissions(
            capture_dir(),
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        );
        let n = CAPTURE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = capture_dir().join(format!("env-{}-{n}", std::process::id()));
        let mut body = String::new();
        for (k, v) in vars {
            // `docker --env-file` reads one `KEY=VALUE` per line — an embedded
            // newline in a value would silently start a second (bogus)
            // variable rather than being passed through, so refuse it
            // outright instead of writing a file docker will misparse.
            if v.contains('\n') {
                return Err(std::io::Error::other(format!(
                    "env value for {k} must not contain a newline"
                )));
            }
            body.push_str(k);
            body.push('=');
            body.push_str(v);
            body.push('\n');
        }
        std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)?
            .write_all(body.as_bytes())?;
        Ok(Self { path })
    }

    /// The path to hand to `docker run --env-file <path>` / `docker exec --env-file <path>`.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for DockerEnvFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Run a binary outside the agent's sandbox. See [`UnsandboxedCommand`].
///
/// Use sparingly: every call escapes the sandbox, so reserve this for commands
/// that genuinely cannot run sandboxed (apt/dnf/dpkg/rpm). Read-only commands
/// like `apt list --upgradable` work fine under the sandbox and should keep
/// using [`safe_command`].
pub fn safe_command_unsandboxed(binary: &str, extra_env: &[(&str, &str)]) -> UnsandboxedCommand {
    UnsandboxedCommand::new(binary, extra_env)
}

/// Synchronous sibling of [`safe_command_unsandboxed`].
pub fn safe_command_sync_unsandboxed(
    binary: &str,
    extra_env: &[(&str, &str)],
) -> UnsandboxedCommandSync {
    UnsandboxedCommandSync::new(binary, extra_env)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv_of(c: &UnsandboxedCommand) -> Vec<String> {
        c.argv
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect()
    }

    /// THE WALL. `--pipe` passes descriptors over D-Bus, which SELinux forbids
    /// the message bus to receive from a service — every package operation on
    /// every RHEL box failed for as long as this flag was here.
    #[test]
    fn escape_hatch_passes_no_file_descriptors() {
        let cmd = safe_command_unsandboxed("dnf", &[]);
        assert!(
            !argv_of(&cmd).iter().any(|a| a == "--pipe"),
            "--pipe is back: this breaks every package operation on every SELinux box"
        );
    }

    /// Without `--wait`, systemd-run returns as soon as PID1 accepts the job and
    /// every caller's `output.status.success()` becomes meaningless.
    #[test]
    fn escape_hatch_waits_for_the_inner_command() {
        assert!(argv_of(&safe_command_unsandboxed("dnf", &[]))
            .iter()
            .any(|a| a == "--wait"));
    }

    #[test]
    fn inner_env_is_forwarded_and_argv_order_is_preserved() {
        let mut cmd = safe_command_unsandboxed("apt-get", &[("PGPASSWORD", "s3cret")]);
        cmd.arg("install").args(["-y", "redis-server"]);
        let argv = argv_of(&cmd);
        assert!(argv.contains(&"--setenv=PGPASSWORD=s3cret".to_string()));
        let sep = argv.iter().position(|a| a == "--").expect("separator");
        assert_eq!(&argv[sep + 1..], ["apt-get", "install", "-y", "redis-server"]);
    }

    /// PID1 runs as `init_t` and may not write `tmp_t`, so a capture directory
    /// under /tmp would silently lose every command's output on RHEL.
    #[test]
    fn capture_dir_is_somewhere_pid1_may_write() {
        assert!(
            !CAPTURE_DIR.starts_with("/tmp") && !CAPTURE_DIR.starts_with("/var/tmp"),
            "capture files in {CAPTURE_DIR} would be denied to init_t on the RHEL family"
        );
    }

    /// THE WHOLE POINT: the secret must be in the FILE, never in the path we'd
    /// hand to `docker`'s own argv (which `ps`/`/proc/<pid>/cmdline` exposes).
    #[test]
    fn docker_env_file_keeps_secret_out_of_its_own_path() {
        let f = DockerEnvFile::new(&[("MYSQL_PWD", "t0psecret")]).unwrap();
        let path_str = f.path().to_string_lossy();
        assert!(!path_str.contains("t0psecret"), "secret leaked into the path: {path_str}");
        let body = std::fs::read_to_string(f.path()).unwrap();
        assert_eq!(body, "MYSQL_PWD=t0psecret\n");
    }

    #[test]
    fn docker_env_file_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let f = DockerEnvFile::new(&[("PGPASSWORD", "x")]).unwrap();
        let mode = std::fs::metadata(f.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "env file must not be group/world readable");
    }

    #[test]
    fn docker_env_file_removed_on_drop() {
        let path = {
            let f = DockerEnvFile::new(&[("MYSQL_PWD", "x")]).unwrap();
            f.path().to_path_buf()
        };
        assert!(!path.exists(), "env file must not outlive its guard");
    }

    #[test]
    fn docker_env_file_multiple_vars_and_rejects_embedded_newline() {
        let f = DockerEnvFile::new(&[("A", "1"), ("B", "2")]).unwrap();
        let body = std::fs::read_to_string(f.path()).unwrap();
        assert_eq!(body, "A=1\nB=2\n");

        assert!(DockerEnvFile::new(&[("MYSQL_PWD", "line1\nline2")]).is_err());
    }

    #[test]
    fn capture_properties_keep_stdout_and_stderr_apart() {
        let cap = Capture {
            out: "/var/lib/dockpanel/run/x.out".into(),
            err: "/var/lib/dockpanel/run/x.err".into(),
        };
        let props = cap.properties();
        assert!(props.iter().any(|p| p == "StandardOutput=file:/var/lib/dockpanel/run/x.out"));
        assert!(props.iter().any(|p| p == "StandardError=file:/var/lib/dockpanel/run/x.err"));
    }
}
