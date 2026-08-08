use axum::{
    extract::{
        ws::{Message, WebSocket},
        Query, State, WebSocketUpgrade,
    },
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use serde::Deserialize;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::AppState;
use crate::services::command_filter;

pub static ACTIVE_TERMINALS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Every shell this agent has spawned, by pid, so the panic button can kill the
/// ones it actually started.
///
/// The panic button used to run `pkill -u www-data -f bash`, which is a guess
/// about which processes are ours, and it guessed wrong in the one direction
/// that matters: a SERVER terminal is deliberately kept as root
/// (`open_pty_shell`), so the single most dangerous shell on the box — the one
/// an intruder with a stolen admin session is holding — was the one process the
/// emergency control could not reach. A registry is not a guess.
pub static TERMINAL_PIDS: std::sync::Mutex<Vec<(i32, bool)>> =
    std::sync::Mutex::new(Vec::new());

/// Record a shell this agent spawned. `is_root` is true for a server terminal.
pub fn register_terminal(pid: i32, is_root: bool) {
    TERMINAL_PIDS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push((pid, is_root));
}

/// Forget a shell that has already exited.
pub fn forget_terminal(pid: i32) {
    TERMINAL_PIDS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|(p, _)| *p != pid);
}
static LAST_CONNECT: AtomicU64 = AtomicU64::new(0);
const MAX_TERMINAL_SESSIONS: u32 = 20;

#[derive(Deserialize)]
struct TermQuery {
    domain: Option<String>,
    token: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
}

/// A ticket whose `scope` is this value authorises the root server shell. Any
/// other value is a site domain; the shell drops to www-data under that site's
/// root. The panel writes it; it is not a valid domain (the `@` fails the domain
/// check below), so it cannot be a site's name.
const SERVER_SHELL_SCOPE: &str = "@server";

/// What the caller is told when the site-terminal blocklist refuses a line.
/// One constant so both input arms cannot drift into saying different things.
const BLOCKED_NOTICE: &str =
    "\r\n\x1b[1;31mBlocked:\x1b[0m command not allowed in site terminal\r\n";

#[derive(Deserialize)]
struct TerminalTicket {
    sub: String,
    purpose: String,
    /// Whether this session may be recorded. A SIGNED claim, never a query
    /// param — the browser holds the ticket and dials the agent directly, so a
    /// param would let the user switch off the recording of their own session.
    /// Absent means an older panel minted the ticket: keep recording, which is
    /// what that panel already expected.
    record: Option<bool>,
    /// The shell this ticket authorises: a site's domain, or `@server` for the
    /// root shell. A SIGNED claim for the same reason `record` is one, and the
    /// fix for an escalation: the scope used to come from the `?domain=` query
    /// param, so a site-scoped ticket dialled with the domain omitted opened a
    /// ROOT shell with no privilege drop. Now the ticket decides. Absent means
    /// an older panel that did not bind it — see the fallback below.
    scope: Option<String>,
}

/// GET /terminal/ws — WebSocket terminal.
/// Auth via ?token= query param (short-lived JWT ticket signed by the API).
async fn ws_handler(
    State(state): State<AppState>,
    Query(q): Query<TermQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    // Rate limit: max 1 connection per second (prevents rapid reconnect storms)
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let last = LAST_CONNECT.swap(now, Ordering::Relaxed);
    if now == last {
        return (StatusCode::TOO_MANY_REQUESTS, "Too many terminal connections").into_response();
    }

    // Enforce terminal session limit
    let current = ACTIVE_TERMINALS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    if current >= MAX_TERMINAL_SESSIONS {
        let _ = ACTIVE_TERMINALS.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| Some(v.saturating_sub(1)));
        return Response::builder()
            .status(503)
            .body("Too many terminal sessions".into())
            .unwrap();
    }

    // Validate JWT ticket (short-lived token signed by the API using agent token as secret)
    let token_value = state.token.read().await.clone();
    let user_email = q
        .token
        .as_deref()
        .and_then(|t| {
            let mut validation =
                jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
            validation.set_required_spec_claims(&["exp", "sub"]);
            validation.validate_exp = true;
            jsonwebtoken::decode::<TerminalTicket>(
                t,
                &jsonwebtoken::DecodingKey::from_secret(token_value.as_bytes()),
                &validation,
            )
            .ok()
            .filter(|data| data.claims.purpose == "terminal")
            .map(|data| (data.claims.sub, data.claims.record.unwrap_or(true), data.claims.scope))
        });
    let (user_email, record, ticket_scope) = match user_email {
        Some((email, record, scope)) => (Some(email), record, scope),
        None => (None, true, None),
    };
    if user_email.is_none() {
        let _ = ACTIVE_TERMINALS.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| Some(v.saturating_sub(1)));
        return Response::builder()
            .status(401)
            .body("Unauthorized".into())
            .unwrap();
    }
    let user_email = user_email.unwrap();

    // The scope is the SIGNED decision, not the `?domain=` param. Trusting the
    // param let a site-scoped ticket open a root shell by omitting the domain.
    // `@server` is the root shell (the panel only mints it for an admin, and it
    // is not a valid domain); any other value is a site. A ticket with no scope
    // came from a panel too old to bind it — fall back to the param it expected,
    // and say so, so an unscoped root shell is visible in the log rather than
    // silent.
    let domain = match ticket_scope.as_deref() {
        Some(SERVER_SHELL_SCOPE) => String::new(),
        Some(scope) => scope.to_string(),
        None => {
            let legacy = q.domain.clone().unwrap_or_default();
            if legacy.is_empty() {
                tracing::warn!(
                    "Terminal ticket for {user_email} carries no scope (pre-v2.75.0 panel); \
                     opening a server shell from the query param"
                );
            }
            legacy
        }
    };

    // Validate domain format (prevent path traversal). Belt and suspenders: the
    // scope is signed, but a directory name still reaches the filesystem below.
    if !domain.is_empty()
        && (domain.contains("..") || domain.contains('/') || domain.contains('\0'))
    {
        let _ = ACTIVE_TERMINALS.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| Some(v.saturating_sub(1)));
        return (StatusCode::BAD_REQUEST, "Invalid domain").into_response();
    }

    // Verify site directory exists before upgrading to WebSocket
    if !domain.is_empty() && !std::path::Path::new(&format!("/var/www/{domain}")).is_dir() {
        let _ = ACTIVE_TERMINALS.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| Some(v.saturating_sub(1)));
        return (StatusCode::BAD_REQUEST, "Site directory not found").into_response();
    }

    let cols = q.cols.unwrap_or(80);
    let rows = q.rows.unwrap_or(24);

    ws.on_upgrade(move |socket| handle_terminal(socket, domain, user_email, cols, rows, record))
}

/// Open a PTY pair and spawn a shell in the child side.
/// If `site_domain` is Some, drop privileges to www-data for the site terminal.
fn open_pty_shell(cwd: &str, cols: u16, rows: u16, site_domain: Option<&str>) -> Result<(OwnedFd, u32), String> {
    // Open PTY master
    let master_fd = rustix::pty::openpt(rustix::pty::OpenptFlags::RDWR | rustix::pty::OpenptFlags::NOCTTY)
        .map_err(|e| format!("openpt: {e}"))?;
    rustix::pty::grantpt(&master_fd).map_err(|e| format!("grantpt: {e}"))?;
    rustix::pty::unlockpt(&master_fd).map_err(|e| format!("unlockpt: {e}"))?;

    // Get slave path
    let slave_name_buf = vec![0u8; 256];
    let slave_cstring = rustix::pty::ptsname(&master_fd, slave_name_buf)
        .map_err(|e| format!("ptsname: {e}"))?;
    let slave_name = slave_cstring
        .to_str()
        .map_err(|e| format!("ptsname utf8: {e}"))?
        .to_string();

    // Set window size on master
    unsafe {
        let ws = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        libc::ioctl(master_fd.as_raw_fd(), libc::TIOCSWINSZ, &ws);
    }

    // Fork
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err("fork failed".into());
    }

    if pid == 0 {
        // ── Child process ──
        unsafe {
            // New session (detach from parent terminal)
            libc::setsid();

            // Open slave side — this becomes our controlling terminal
            let slave_cstr = std::ffi::CString::new(slave_name.as_str()).unwrap();
            let slave_fd = libc::open(slave_cstr.as_ptr(), libc::O_RDWR);
            if slave_fd < 0 {
                libc::_exit(1);
            }

            // Set controlling terminal
            libc::ioctl(slave_fd, libc::TIOCSCTTY as _, 0);

            // Redirect stdin/stdout/stderr to slave
            libc::dup2(slave_fd, 0);
            libc::dup2(slave_fd, 1);
            libc::dup2(slave_fd, 2);
            if slave_fd > 2 {
                libc::close(slave_fd);
            }

            // Close all inherited FDs > 2 (master PTY, DB connections, TLS sockets, etc.)
            // Must use raw libc (opendir/readdir/closedir) — NOT std::fs::read_dir which
            // allocates memory (violates async-signal-safety after fork) and the iterator
            // holds its own FD which we'd close from under it (causing closedir EBADF panic).
            let proc_fd_dir = libc::opendir(b"/proc/self/fd\0".as_ptr() as *const _);
            if !proc_fd_dir.is_null() {
                let dir_fd = libc::dirfd(proc_fd_dir);
                loop {
                    let entry = libc::readdir(proc_fd_dir);
                    if entry.is_null() { break; }
                    let name = std::ffi::CStr::from_ptr((*entry).d_name.as_ptr());
                    if let Ok(fd) = name.to_str().unwrap_or("").parse::<i32>() {
                        // Close everything > 2 except the slave PTY and the /proc/self/fd dir itself
                        if fd > 2 && fd != slave_fd && fd != dir_fd {
                            libc::close(fd);
                        }
                    }
                }
                libc::closedir(proc_fd_dir);
            }

            // Clear inherited environment to prevent leaking agent secrets
            libc::clearenv();

            // Set env vars.
            //
            // ⚠ `putenv` STORES THE POINTER — it does not copy. Every variable
            // below used to be a `CString` bound inside the block that sets it,
            // so Rust freed it at that block's closing brace while `environ`
            // kept pointing at the freed chunk, and `execv` runs AFTER those
            // braces — by which time the five `CString`s built for the exec
            // itself had reused the memory. It was not theoretical: `/etc/
            // bash.bashrc` here sets `PS1` with `\w`, which prints `~` when the
            // cwd equals `$HOME`, and the child chdirs to `/root` and intends
            // `HOME=/root`. All 17 asciicast recordings on the demo box open
            // `root@host:/root#`; NONE opens `root@host:~#`. So HOME has been
            // dead in every root terminal this panel has ever served — `--login`
            // could not expand `~/.bash_profile`, and a bare `cd` failed.
            //
            // `setenv` COPIES both halves, so the lifetime question disappears
            // instead of being patched per call site.
            let k_term = std::ffi::CString::new("TERM").unwrap();
            let v_term = std::ffi::CString::new("xterm-256color").unwrap();
            libc::setenv(k_term.as_ptr(), v_term.as_ptr(), 1);

            // Change directory
            let cwd_cstr = std::ffi::CString::new(cwd).unwrap();
            libc::chdir(cwd_cstr.as_ptr());

            // Drop privileges for site terminals (run as www-data instead of root)
            if site_domain.is_some() {
                let username = b"www-data\0".as_ptr() as *const libc::c_char;
                let pw = libc::getpwnam(username);
                if !pw.is_null() {
                    let uid = (*pw).pw_uid;
                    let gid = (*pw).pw_gid;
                    // EVERY ONE OF THESE MUST SUCCEED, and their results used to
                    // be discarded. `setuid` returns EAGAIN when the target uid
                    // is at RLIMIT_NPROC — reachable on a busy box, since
                    // www-data also runs every PHP-FPM worker — and a discarded
                    // failure falls straight through to the `execv` below and
                    // hands the SITE'S OWNER a root shell in their own webroot.
                    // The `_exit(1)` in the else-arm already states the
                    // invariant ("abort rather than run as root"); these checks
                    // are what actually enforce it.
                    //
                    // `write(2, …)` is async-signal-safe, so the user is told
                    // why the shell refused to open instead of seeing it vanish.
                    const DROP_FAILED: &[u8] =
                        b"dockpanel: refusing to open this shell (privilege drop failed)\r\n";
                    // Group first: setgid/initgroups need the privilege setuid drops.
                    if libc::setgid(gid) != 0
                        || libc::initgroups(username, gid) != 0
                        || libc::setuid(uid) != 0
                    {
                        libc::write(2, DROP_FAILED.as_ptr() as *const _, DROP_FAILED.len());
                        libc::_exit(1);
                    }
                    // Ask the kernel who we ACTUALLY are rather than trusting the
                    // three return codes above. This is the arm that cannot be
                    // fooled, and it is the property the pin suite asserts.
                    if libc::getuid() != uid || libc::geteuid() != uid || libc::getgid() != gid {
                        libc::write(2, DROP_FAILED.as_ptr() as *const _, DROP_FAILED.len());
                        libc::_exit(1);
                    }

                    // PR_SET_NO_NEW_PRIVS: prevent any privilege escalation
                    // (blocks setuid binaries like su, sudo, pkexec, etc.)
                    libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0);

                    // Set HOME to site directory. `setenv`, not `putenv` — see
                    // the note above `TERM`; these three are the variables the
                    // use-after-free destroyed.
                    let k_home = std::ffi::CString::new("HOME").unwrap();
                    let v_home = std::ffi::CString::new(cwd).unwrap();
                    libc::setenv(k_home.as_ptr(), v_home.as_ptr(), 1);

                    let k_user = std::ffi::CString::new("USER").unwrap();
                    let v_user = std::ffi::CString::new("www-data").unwrap();
                    libc::setenv(k_user.as_ptr(), v_user.as_ptr(), 1);

                    // Restricted PATH: only standard user bins, no sbin
                    let k_path = std::ffi::CString::new("PATH").unwrap();
                    let v_path = std::ffi::CString::new("/usr/local/bin:/usr/bin:/bin").unwrap();
                    libc::setenv(k_path.as_ptr(), v_path.as_ptr(), 1);

                    // Restrict umask so created files aren't world-readable
                    libc::umask(0o077);
                } else {
                    // www-data user not found — abort rather than run as root
                    libc::_exit(1);
                }
            } else {
                // Server terminal — keep root
                let k_home = std::ffi::CString::new("HOME").unwrap();
                let v_home = std::ffi::CString::new("/root").unwrap();
                libc::setenv(k_home.as_ptr(), v_home.as_ptr(), 1);
            }

            // Exec shell
            if site_domain.is_some() {
                // Site terminal: use bash --restricted (rbash) to prevent:
                //   - changing directory with cd
                //   - setting/unsetting PATH
                //   - specifying commands with /
                //   - redirecting output
                let bash_path = std::ffi::CString::new("/bin/bash").unwrap();
                let sh_path = std::ffi::CString::new("/bin/sh").unwrap();
                let restricted_arg = std::ffi::CString::new("--restricted").unwrap();
                let norc_arg = std::ffi::CString::new("--norc").unwrap();
                let noprofile_arg = std::ffi::CString::new("--noprofile").unwrap();

                // Try restricted bash (--norc/--noprofile prevents .bashrc from
                // overriding the restricted mode)
                let args = [
                    bash_path.as_ptr(),
                    restricted_arg.as_ptr(),
                    norc_arg.as_ptr(),
                    noprofile_arg.as_ptr(),
                    std::ptr::null(),
                ];
                libc::execv(bash_path.as_ptr(), args.as_ptr());

                // Fallback: plain sh
                let args = [sh_path.as_ptr(), std::ptr::null()];
                libc::execv(sh_path.as_ptr(), args.as_ptr());
            } else {
                // Server terminal: full bash
                let shell_path = if std::path::Path::new("/bin/bash").exists() {
                    std::ffi::CString::new("/bin/bash").unwrap()
                } else {
                    std::ffi::CString::new("/bin/sh").unwrap()
                };
                let login_arg = std::ffi::CString::new("--login").unwrap();
                let args = [shell_path.as_ptr(), login_arg.as_ptr(), std::ptr::null()];
                libc::execv(shell_path.as_ptr(), args.as_ptr());
            }

            // If exec fails
            libc::_exit(1);
        }
    }

    // ── Parent process ──
    Ok((master_fd, pid as u32))
}

async fn handle_terminal(mut socket: WebSocket, domain: String, user_email: String, cols: u16, rows: u16, record: bool) {
    // Determine working directory
    let cwd = if !domain.is_empty() {
        let path = format!("/var/www/{domain}");
        if std::path::Path::new(&path).exists() {
            path
        } else {
            let _ = socket
                .send(Message::Text(
                    format!("Site directory not found: /var/www/{domain}").into(),
                ))
                .await;
            let _ = ACTIVE_TERMINALS.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| Some(v.saturating_sub(1)));
            return;
        }
    } else {
        "/root".to_string()
    };

    // Spawn shell with PTY (drop to www-data for site terminals)
    let site_domain = if domain.is_empty() { None } else { Some(domain.as_str()) };
    let (master_fd, child_pid) = match open_pty_shell(&cwd, cols, rows, site_domain) {
        Ok(v) => v,
        Err(e) => {
            let _ = socket
                .send(Message::Text(format!("Failed to spawn shell: {e}").into()))
                .await;
            let _ = ACTIVE_TERMINALS.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| Some(v.saturating_sub(1)));
            return;
        }
    };

    register_terminal(child_pid as i32, site_domain.is_none());

    let raw_fd = master_fd.as_raw_fd();

    // Duplicate the fd so reader and writer are independent
    // (tokio::fs::File only supports one concurrent operation)
    let write_fd = unsafe { libc::dup(raw_fd) };
    if write_fd < 0 {
        let _ = socket
            .send(Message::Text("Failed to dup PTY fd".into()))
            .await;
        let _ = ACTIVE_TERMINALS.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| Some(v.saturating_sub(1)));
        return;
    }
    // Prevent OwnedFd from closing the fd since Files now own them
    std::mem::forget(master_fd);

    let reader_file = unsafe { std::fs::File::from_raw_fd(raw_fd) };
    let writer_file = unsafe { std::fs::File::from_raw_fd(write_fd) };
    let mut reader = tokio::fs::File::from_std(reader_file);
    let mut writer = tokio::fs::File::from_std(writer_file);

    // Channel for PTY output → WebSocket
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);

    // Read PTY output
    let read_task = tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Command buffer for audit logging and safety checks
    let mut cmd_buffer = String::new();
    let is_site_terminal = !domain.is_empty();
    let domain_str = if domain.is_empty() { "server" } else { &domain };

    // Feature 5: Open session recording file — only when the panel's
    // `security_session_recording` toggle allows it. Until v2.46.0 this ran
    // unconditionally and the toggle was inert, so switching it off changed
    // nothing while the UI reported "Session recording disabled".
    let recording_dir = "/var/lib/dockpanel/recordings";
    let session_id = uuid::Uuid::new_v4();
    let recording_path = format!("{recording_dir}/{session_id}.cast");
    let recording_start = std::time::Instant::now();
    let mut recording_file = if record {
        let _ = std::fs::create_dir_all(recording_dir);
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&recording_path)
            .ok()
    } else {
        None
    };

    // Write asciicast v2 header
    if let Some(ref mut f) = recording_file {
        use std::io::Write;
        let header = serde_json::json!({
            "version": 2,
            "width": cols,
            "height": rows,
            "timestamp": chrono::Utc::now().timestamp(),
            "env": {
                "TERM": "xterm-256color",
                "SHELL": "/bin/bash"
            },
            "title": format!("{}@{}", user_email, domain_str)
        });
        let _ = writeln!(f, "{}", header);
    }

    // Feature 6: Write session start to tamper-resistant audit file
    {
        let dir = "/var/lib/dockpanel/audit";
        let _ = std::fs::create_dir_all(dir);
        let date = chrono::Utc::now().format("%Y-%m-%d");
        let path = format!("{dir}/audit-{date}.log");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            use std::io::Write;
            let _ = writeln!(f, "{}\tinfo\tterminal.start\t{}\t-\tdomain={}", chrono::Utc::now().to_rfc3339(), user_email, domain_str);
        }
    }

    tracing::info!(target: "terminal_audit", user = %user_email, domain = %domain_str, "Terminal session started");

    // Feature: Terminal session timeout (30 minutes default)
    let session_timeout = Duration::from_secs(30 * 60);
    let session_deadline = tokio::time::Instant::now() + session_timeout;

    // Main loop: multiplex PTY output and WebSocket input
    loop {
        tokio::select! {
            // Timeout branch — fires even on idle sessions
            _ = tokio::time::sleep_until(session_deadline) => {
                let _ = socket.send(Message::Text(
                    "\r\n\x1b[1;33mSession timed out (30 minutes). Reconnect to continue.\x1b[0m\r\n".into()
                )).await;
                tracing::info!(target: "terminal_audit", user = %user_email, domain = %domain_str, "Terminal session timed out");
                break;
            }
            // PTY output → WebSocket
            Some(data) = rx.recv() => {
                let text = String::from_utf8_lossy(&data).to_string();

                // Feature 5: Record output to asciicast file
                if let Some(ref mut f) = recording_file {
                    use std::io::Write;
                    let elapsed = recording_start.elapsed().as_secs_f64();
                    // asciicast v2 format: [time, "o", "data"]
                    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r");
                    let _ = writeln!(f, "[{:.6}, \"o\", \"{}\"]", elapsed, escaped);
                }

                if socket.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
            // WebSocket input → PTY
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        // Try to parse as JSON command
                        if let Ok(cmd) = serde_json::from_str::<serde_json::Value>(&text) {
                            match cmd.get("type").and_then(|t| t.as_str()) {
                                Some("input") => {
                                    if let Some(data) = cmd.get("data").and_then(|d| d.as_str()) {
                                        // Characters are forwarded immediately for echo, but
                                        // Enter is held until the completed line is judged.
                                        // ONE path — see `observe_input`; the raw-text arm
                                        // below calls exactly the same three lines.
                                        let seen = observe_input(data, &mut cmd_buffer, is_site_terminal);
                                        let mut blocked = false;
                                        for obs in &seen {
                                            record_observation(obs, &user_email, domain_str);
                                            blocked |= matches!(obs, Observed::Blocked(_));
                                        }
                                        if blocked {
                                            // Send Ctrl+U (kill line) + Ctrl+C to cancel
                                            let _ = writer.write_all(b"\x15\x03").await;
                                            let _ = socket.send(Message::Text(BLOCKED_NOTICE.into())).await;
                                        } else if writer.write_all(data.as_bytes()).await.is_err() {
                                            break;
                                        }
                                    }
                                }
                                Some("resize") => {
                                    let new_cols = cmd.get("cols").and_then(|c| c.as_u64()).unwrap_or(80) as u16;
                                    let new_rows = cmd.get("rows").and_then(|r| r.as_u64()).unwrap_or(24) as u16;
                                    unsafe {
                                        let ws = libc::winsize {
                                            ws_row: new_rows,
                                            ws_col: new_cols,
                                            ws_xpixel: 0,
                                            ws_ypixel: 0,
                                        };
                                        libc::ioctl(raw_fd, libc::TIOCSWINSZ, &ws);
                                    }
                                }
                                _ => {}
                            }
                        } else {
                            // Raw text input. NOT gated on `is_site_terminal` any
                            // more, and NOT a second copy of the loop: the frame
                            // shape a client picked is not a security boundary,
                            // and a root shell is where an observation is worth
                            // the most. Identical three lines to the JSON arm.
                            let seen = observe_input(&text, &mut cmd_buffer, is_site_terminal);
                            let mut blocked = false;
                            for obs in &seen {
                                record_observation(obs, &user_email, domain_str);
                                blocked |= matches!(obs, Observed::Blocked(_));
                            }
                            if blocked {
                                let _ = writer.write_all(b"\x15\x03").await;
                                let _ = socket.send(Message::Text(BLOCKED_NOTICE.into())).await;
                            } else if writer.write_all(text.as_bytes()).await.is_err() {
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }

    // Cleanup: kill child process and reap to prevent zombies
    read_task.abort();
    unsafe {
        libc::kill(child_pid as i32, libc::SIGTERM);
    }
    // Give process 500ms to exit gracefully after SIGTERM
    tokio::time::sleep(Duration::from_millis(500)).await;
    unsafe {
        let mut status = 0i32;
        let ret = libc::waitpid(child_pid as i32, &mut status, libc::WNOHANG);
        if ret == 0 {
            // Still alive after SIGTERM — force kill and blocking reap
            libc::kill(child_pid as i32, libc::SIGKILL);
            libc::waitpid(child_pid as i32, &mut status, 0);
        }
    }

    forget_terminal(child_pid as i32);

    // Release terminal session slot
    let _ = ACTIVE_TERMINALS.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| Some(v.saturating_sub(1)));
}

/// What one completed input line turned out to be.
enum Observed {
    /// The site-terminal blocklist refuses it. Nothing is forwarded to the PTY.
    Blocked(String),
    /// It runs, and it matches the suspicious list.
    Suspicious(String),
    /// It runs and looks ordinary.
    Plain(String),
}

/// Reconstruct completed lines from one chunk of terminal input and say what
/// each one is.
///
/// **Every byte that reaches the PTY passes through here, whatever WebSocket
/// frame shape carried it.** That is the whole point of the function existing.
/// Until v2.88.0 this loop was written twice: once inside the
/// `{"type":"input"}` arm, which classified, and once inside the raw-text arm,
/// which only ever ran the BLOCK half and only for site terminals. Since
/// `Terminal.tsx` sends JSON at all seven of its senders, the raw arm is a path
/// the product's own UI never uses — so the ONE input path with no detection was
/// the one only a non-browser client takes, which is the client an intruder
/// with a stolen admin session uses. `TERMINAL_PIDS`' own doc above names that
/// adversary. Measured on the demo box at v2.87.0, same site, same account,
/// same command: JSON frame → 1 `terminal.suspicious_command`; raw frame → 0.
///
/// ⚠ HONEST BOUND, so nobody reads more into this than it delivers. A line is
/// reconstructed from keystrokes, so anything that moves the cursor makes the
/// reconstruction diverge from what the shell will execute — typing
/// `echo cmod 777`, then seven LEFTs, then `h` runs `echo chmod 777` and is
/// measured here as `echo cmod 777[D…h`, which matches nothing. That was true
/// before this change and is true after it. What this function guarantees is
/// narrower and worth having: the observation no longer depends on which frame
/// shape the caller chose. Same family as
/// `command_filter::_SITE_SHELL_IS_NOT_A_SANDBOX`, and stated for the same
/// reason.
fn observe_input(data: &str, buf: &mut String, is_site_terminal: bool) -> Vec<Observed> {
    let mut out = Vec::new();
    for ch in data.chars() {
        if ch == '\r' || ch == '\n' {
            let line = buf.trim().to_string();
            buf.clear();
            if line.is_empty() {
                continue;
            }
            // The block check stays gated on `is_site_terminal` deliberately —
            // an administrator's SERVER shell must keep `cd ..`, which
            // `client-role-and-server-ownership-pin-e2e.sh` §D pins. Only the
            // SUSPICIOUS half is ungated, because a root shell is precisely
            // where an observation is worth the most.
            if is_site_terminal && !command_filter::is_safe_terminal_command(&line) {
                out.push(Observed::Blocked(line));
                // Refused: the rest of this chunk never reaches the shell, so
                // there is nothing further to observe in it.
                return out;
            }
            if command_filter::is_suspicious_command(&line) {
                out.push(Observed::Suspicious(line));
            } else {
                out.push(Observed::Plain(line));
            }
        } else if ch == '\x7f' || ch == '\x08' {
            buf.pop();
        } else if !ch.is_control() {
            buf.push(ch);
        }
    }
    out
}

/// Write a suspicious event to the shared JSONL file for backend ingestion.
fn write_suspicious_event(event_type: &str, user_email: &str, domain: &str, command: &str) {
    use std::io::Write;
    let path = "/var/lib/dockpanel/suspicious-events.jsonl";
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let event = serde_json::json!({
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "event_type": event_type,
            "actor_email": user_email,
            "domain": domain,
            "command": command,
        });
        let _ = writeln!(f, "{}", event);
    }
}

/// Record one observation: the agent's journal always, and the panel's own
/// intrusion channel when the line is worth an operator's attention.
///
/// A REFUSAL is reported. It used to `break` out of the loop before the
/// classifier ran, so the strongest thing this product can observe — "I just
/// refused `sudo` on a shell someone is holding" — produced a single
/// `tracing::warn` on the `terminal_audit` target, which has NO consumer
/// outside the agent's own journal (0 hits across `panel/backend/src` and
/// `panel/frontend/src`). It reached neither the audit log the panel renders
/// nor anything else.
///
/// It is written as its OWN event type, and that is the load-bearing detail:
/// `security_hardening::record_suspicious_event_at` counts every row in
/// `suspicious_events` within the window against `security_lockdown_threshold`
/// (default 5 in 10 minutes) and a trip blocks every non-admin for 24h. But
/// `TERMINAL_BLOCKED_PATTERNS` contains a bare `".."` SUBSTRING — its own
/// comment admits `echo "done..."` is refused — so a tenant trips refusals by
/// accident. Counting those toward auto-lockdown would convert a blunt
/// blocklist into a self-inflicted outage. The ingest therefore audits this
/// type and does not count it; see `auto_healer::security_ingest_suspicious_events`.
fn record_observation(obs: &Observed, user_email: &str, domain_str: &str) {
    match obs {
        Observed::Blocked(cmd) => {
            tracing::warn!(
                target: "terminal_audit",
                user = %user_email, domain = %domain_str, command = %cmd,
                "Blocked dangerous terminal command"
            );
            write_suspicious_event("terminal.blocked_command", user_email, domain_str, cmd);
        }
        Observed::Suspicious(cmd) => {
            tracing::warn!(
                target: "terminal_audit",
                user = %user_email, domain = %domain_str, command = %cmd,
                "SUSPICIOUS terminal command detected"
            );
            write_suspicious_event("terminal.suspicious_command", user_email, domain_str, cmd);
        }
        Observed::Plain(cmd) => {
            tracing::info!(
                target: "terminal_audit",
                user = %user_email, domain = %domain_str, command = %cmd,
                "Terminal command"
            );
        }
    }
}

/// The terminal WebSocket route bypasses standard auth middleware
/// (token is validated inside the handler via query param).
pub fn router() -> Router<AppState> {
    Router::new().route("/terminal/ws", get(ws_handler))
}

#[cfg(test)]
mod observe_tests {
    use super::*;

    fn kinds(v: &[Observed]) -> Vec<&'static str> {
        v.iter()
            .map(|o| match o {
                Observed::Blocked(_) => "blocked",
                Observed::Suspicious(_) => "suspicious",
                Observed::Plain(_) => "plain",
            })
            .collect()
    }

    /// Deliver `input` the way the BROWSER does: one JSON frame per character.
    fn as_json_frames(input: &str, is_site: bool) -> Vec<&'static str> {
        let mut buf = String::new();
        let mut all = Vec::new();
        for ch in input.chars() {
            all.extend(observe_input(&ch.to_string(), &mut buf, is_site));
        }
        kinds(&all)
    }

    /// Deliver `input` the way a SCRIPT does: one raw frame for the whole line.
    fn as_raw_frame(input: &str, is_site: bool) -> Vec<&'static str> {
        let mut buf = String::new();
        kinds(&observe_input(input, &mut buf, is_site))
    }

    /// THE PROPERTY THIS WHOLE CHANGE EXISTS FOR, stated as the equality it is.
    ///
    /// Before v2.88.0 the raw arm ran no classifier at all, so the right-hand
    /// side of every one of these was empty. Measured live at v2.87.0 on the
    /// demo box: same site, same account, `echo chmod 777` → JSON frame wrote 1
    /// `terminal.suspicious_command`, raw frame wrote 0.
    #[test]
    fn the_frame_shape_a_client_picked_does_not_change_what_is_observed() {
        for (line, is_site) in [
            ("echo chmod 777\n", true),   // suspicious, allowed
            ("echo chmod 777\n", false),  // ...on the ROOT shell too
            ("sudo id\n", true),          // refused by the site blocklist
            ("sudo id\n", false),         // ...but a server shell may run it
            ("ls -la\n", true),           // ordinary
            ("ls -la\n", false),
        ] {
            assert_eq!(
                as_json_frames(line, is_site),
                as_raw_frame(line, is_site),
                "{line:?} (site={is_site}) is classified differently depending on \
                 whether the client sent JSON frames or raw text"
            );
        }
    }

    /// The suspicious check must NOT be gated on the terminal kind. A root shell
    /// is where an observation is worth the most, and it is the shell an
    /// intruder holding a stolen admin session opens.
    #[test]
    fn a_root_shell_is_observed_too() {
        assert_eq!(as_raw_frame("echo chmod 777\n", false), ["suspicious"]);
        assert_eq!(as_json_frames("echo chmod 777\n", false), ["suspicious"]);
    }

    /// A REFUSAL is an observation, not a silence. It used to `break` before the
    /// classifier and reach nothing but the agent's own journal.
    #[test]
    fn a_refused_command_is_reported() {
        assert_eq!(as_raw_frame("sudo id\n", true), ["blocked"]);
        assert_eq!(as_json_frames("sudo id\n", true), ["blocked"]);
    }

    /// ...and refusing stops the chunk: nothing after the refused newline may be
    /// judged, because nothing after it reaches the shell either.
    #[test]
    fn a_refusal_ends_the_chunk() {
        assert_eq!(as_raw_frame("sudo id\nls -la\n", true), ["blocked"]);
    }

    /// The block half STAYS gated on the terminal kind — widening it would take
    /// `cd ..` away from an administrator's server shell, which
    /// `client-role-and-server-ownership-pin-e2e.sh` §D pins in the other
    /// direction.
    #[test]
    fn the_block_half_is_still_site_only() {
        assert_eq!(as_raw_frame("sudo id\n", false), ["suspicious"]);
        assert_eq!(as_raw_frame("cat ../other-site/wp-config.php\n", true), ["blocked"]);
    }

    #[test]
    fn backspace_edits_the_line_and_blank_lines_are_not_observed() {
        assert_eq!(as_raw_frame("lsX\x7f -la\n", true), ["plain"]);
        assert_eq!(as_raw_frame("\n   \n", true), Vec::<&str>::new());
    }

    /// The honest bound, asserted so nobody later reads the fix as more than it
    /// is: the line is rebuilt from KEYSTROKES, so cursor movement makes the
    /// reconstruction diverge from what the shell runs. Verified live at
    /// v2.87.0 — `echo cmod 777`, seven LEFTs, `h` runs `echo chmod 777` and is
    /// observed as neither blocked nor suspicious. This test exists to make that
    /// limit explicit rather than discovered.
    #[test]
    fn reconstruction_is_not_an_oracle_and_this_fix_does_not_claim_otherwise() {
        let assembled = format!("echo cmod 777{}h\n", "\x1b[D".repeat(7));
        assert_eq!(
            as_json_frames(&assembled, true),
            ["plain"],
            "if this ever reports `suspicious`, the reconstructor became cursor-aware \
             and the bound documented on `observe_input` should be rewritten"
        );
    }
}
