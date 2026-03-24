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
static LAST_CONNECT: AtomicU64 = AtomicU64::new(0);
const MAX_TERMINAL_SESSIONS: u32 = 20;

#[derive(Deserialize)]
struct TermQuery {
    domain: Option<String>,
    token: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
}

#[derive(Deserialize)]
struct TerminalTicket {
    sub: String,
    purpose: String,
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
            .map(|data| data.claims.sub)
        });
    if user_email.is_none() {
        let _ = ACTIVE_TERMINALS.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| Some(v.saturating_sub(1)));
        return Response::builder()
            .status(401)
            .body("Unauthorized".into())
            .unwrap();
    }
    let user_email = user_email.unwrap();

    let domain = q.domain.clone().unwrap_or_default();

    // Validate domain format if provided (prevent path traversal)
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

    ws.on_upgrade(move |socket| handle_terminal(socket, domain, user_email, cols, rows))
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

            // Close master in child
            // (master_fd is dropped when parent returns, but we should close the raw fd)

            // Set env vars
            let term = std::ffi::CString::new("TERM=xterm-256color").unwrap();
            libc::putenv(term.as_ptr() as *mut _);

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
                    // Set group first (must happen before setuid drops root)
                    libc::setgid(gid);
                    libc::initgroups(username, gid);
                    libc::setuid(uid);

                    // PR_SET_NO_NEW_PRIVS: prevent any privilege escalation
                    // (blocks setuid binaries like su, sudo, pkexec, etc.)
                    libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0);

                    // Set HOME to site directory
                    let home = std::ffi::CString::new(format!("HOME={}", cwd)).unwrap();
                    libc::putenv(home.as_ptr() as *mut _);

                    let user_env = std::ffi::CString::new("USER=www-data").unwrap();
                    libc::putenv(user_env.as_ptr() as *mut _);

                    // Restricted PATH: only standard user bins, no sbin
                    let path_env = std::ffi::CString::new(
                        "PATH=/usr/local/bin:/usr/bin:/bin"
                    ).unwrap();
                    libc::putenv(path_env.as_ptr() as *mut _);

                    // Restrict umask so created files aren't world-readable
                    libc::umask(0o077);
                } else {
                    // www-data user not found — abort rather than run as root
                    libc::_exit(1);
                }
            } else {
                // Server terminal — keep root
                let home = std::ffi::CString::new("HOME=/root").unwrap();
                libc::putenv(home.as_ptr() as *mut _);
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

async fn handle_terminal(mut socket: WebSocket, domain: String, user_email: String, cols: u16, rows: u16) {
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

    // Feature 5: Open session recording file
    let recording_dir = "/var/lib/dockpanel/recordings";
    let _ = std::fs::create_dir_all(recording_dir);
    let session_id = uuid::Uuid::new_v4();
    let recording_path = format!("{recording_dir}/{session_id}.cast");
    let recording_start = std::time::Instant::now();
    let mut recording_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&recording_path)
        .ok();

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

    // Main loop: multiplex PTY output and WebSocket input
    loop {
        tokio::select! {
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
                                        // For site terminals: buffer input and intercept
                                        // dangerous commands BEFORE they reach the shell.
                                        // Characters are forwarded immediately for echo,
                                        // but Enter is held until the command is validated.
                                        let mut blocked = false;
                                        for ch in data.chars() {
                                            if ch == '\r' || ch == '\n' {
                                                let trimmed = cmd_buffer.trim().to_string();
                                                if !trimmed.is_empty() && is_site_terminal
                                                    && !command_filter::is_safe_terminal_command(&trimmed)
                                                {
                                                    tracing::warn!(
                                                        target: "terminal_audit",
                                                        user = %user_email,
                                                        domain = %domain_str,
                                                        command = %trimmed,
                                                        "Blocked dangerous terminal command"
                                                    );
                                                    // Send Ctrl+U (kill line) + Ctrl+C to cancel
                                                    let _ = writer.write_all(b"\x15\x03").await;
                                                    let warning = format!(
                                                        "\r\n\x1b[1;31mBlocked:\x1b[0m command not allowed in site terminal\r\n",
                                                    );
                                                    let _ = socket.send(Message::Text(warning.into())).await;
                                                    cmd_buffer.clear();
                                                    blocked = true;
                                                    break;
                                                }
                                                if !trimmed.is_empty() {
                                                    // Feature 4: Check for suspicious commands (alert even if allowed)
                                                    if command_filter::is_suspicious_command(&trimmed) {
                                                        tracing::warn!(
                                                            target: "terminal_audit",
                                                            user = %user_email,
                                                            domain = %domain_str,
                                                            command = %trimmed,
                                                            "SUSPICIOUS terminal command detected"
                                                        );
                                                    } else {
                                                        tracing::info!(
                                                            target: "terminal_audit",
                                                            user = %user_email,
                                                            domain = %domain_str,
                                                            command = %trimmed,
                                                            "Terminal command"
                                                        );
                                                    }
                                                }
                                                cmd_buffer.clear();
                                            } else if ch == '\x7f' || ch == '\x08' {
                                                cmd_buffer.pop();
                                            } else if !ch.is_control() {
                                                cmd_buffer.push(ch);
                                            }
                                        }
                                        // Only forward input to PTY if command was not blocked
                                        if !blocked {
                                            if writer.write_all(data.as_bytes()).await.is_err() {
                                                break;
                                            }
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
                            // Raw text input — apply same command filtering
                            let mut blocked = false;
                            if is_site_terminal {
                                for ch in text.chars() {
                                    if ch == '\r' || ch == '\n' {
                                        let trimmed = cmd_buffer.trim().to_string();
                                        if !trimmed.is_empty()
                                            && !command_filter::is_safe_terminal_command(&trimmed)
                                        {
                                            tracing::warn!(
                                                target: "terminal_audit",
                                                user = %user_email,
                                                domain = %domain_str,
                                                command = %trimmed,
                                                "Blocked dangerous terminal command (raw)"
                                            );
                                            let _ = writer.write_all(b"\x15\x03").await;
                                            let warning = "\r\n\x1b[1;31mBlocked:\x1b[0m command not allowed in site terminal\r\n";
                                            let _ = socket.send(Message::Text(warning.into())).await;
                                            cmd_buffer.clear();
                                            blocked = true;
                                            break;
                                        }
                                        cmd_buffer.clear();
                                    } else if ch == '\x7f' || ch == '\x08' {
                                        cmd_buffer.pop();
                                    } else if !ch.is_control() {
                                        cmd_buffer.push(ch);
                                    }
                                }
                            }
                            if !blocked {
                                if writer.write_all(text.as_bytes()).await.is_err() {
                                    break;
                                }
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

    // Release terminal session slot
    let _ = ACTIVE_TERMINALS.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| Some(v.saturating_sub(1)));
}

/// The terminal WebSocket route bypasses standard auth middleware
/// (token is validated inside the handler via query param).
pub fn router() -> Router<AppState> {
    Router::new().route("/terminal/ws", get(ws_handler))
}
