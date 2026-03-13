use axum::{
    extract::{
        ws::{Message, WebSocket},
        Query, State, WebSocketUpgrade,
    },
    response::Response,
    routing::get,
    Router,
};
use serde::Deserialize;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::AppState;

#[derive(Deserialize)]
struct TermQuery {
    domain: Option<String>,
    token: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
}

#[derive(Deserialize)]
struct TerminalTicket {
    #[allow(dead_code)]
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
    // Validate JWT ticket (short-lived token signed by the API using agent token as secret)
    let valid = q
        .token
        .as_deref()
        .map(|t| {
            let mut validation =
                jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
            validation.set_required_spec_claims(&["exp", "sub"]);
            jsonwebtoken::decode::<TerminalTicket>(
                t,
                &jsonwebtoken::DecodingKey::from_secret(state.token.as_bytes()),
                &validation,
            )
            .map(|data| data.claims.purpose == "terminal")
            .unwrap_or(false)
        })
        .unwrap_or(false);
    if !valid {
        return Response::builder()
            .status(401)
            .body("Unauthorized".into())
            .unwrap();
    }

    let domain = q.domain.clone().unwrap_or_default();

    // Validate domain format if provided (prevent path traversal)
    if !domain.is_empty()
        && (domain.contains("..") || domain.contains('/') || domain.contains('\0'))
    {
        return Response::builder()
            .status(400)
            .body("Invalid domain".into())
            .unwrap();
    }

    let cols = q.cols.unwrap_or(80);
    let rows = q.rows.unwrap_or(24);

    ws.on_upgrade(move |socket| handle_terminal(socket, domain, cols, rows))
}

/// Open a PTY pair and spawn a shell in the child side.
fn open_pty_shell(cwd: &str, cols: u16, rows: u16) -> Result<(OwnedFd, u32), String> {
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

            let home = std::ffi::CString::new("HOME=/root").unwrap();
            libc::putenv(home.as_ptr() as *mut _);

            // Change directory
            let cwd_cstr = std::ffi::CString::new(cwd).unwrap();
            libc::chdir(cwd_cstr.as_ptr());

            // Exec bash (or sh)
            let shell_path = if std::path::Path::new("/bin/bash").exists() {
                std::ffi::CString::new("/bin/bash").unwrap()
            } else {
                std::ffi::CString::new("/bin/sh").unwrap()
            };

            let login_arg = std::ffi::CString::new("--login").unwrap();
            let args = [shell_path.as_ptr(), login_arg.as_ptr(), std::ptr::null()];
            libc::execv(shell_path.as_ptr(), args.as_ptr());

            // If exec fails
            libc::_exit(1);
        }
    }

    // ── Parent process ──
    Ok((master_fd, pid as u32))
}

async fn handle_terminal(mut socket: WebSocket, domain: String, cols: u16, rows: u16) {
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
            return;
        }
    } else {
        "/root".to_string()
    };

    // Spawn shell with PTY
    let (master_fd, child_pid) = match open_pty_shell(&cwd, cols, rows) {
        Ok(v) => v,
        Err(e) => {
            let _ = socket
                .send(Message::Text(format!("Failed to spawn shell: {e}").into()))
                .await;
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

    // Main loop: multiplex PTY output and WebSocket input
    loop {
        tokio::select! {
            // PTY output → WebSocket
            Some(data) = rx.recv() => {
                let text = String::from_utf8_lossy(&data).to_string();
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
                                        if writer.write_all(data.as_bytes()).await.is_err() {
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
                            // Raw text input
                            if writer.write_all(text.as_bytes()).await.is_err() {
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

    // Cleanup: kill child process
    read_task.abort();
    unsafe {
        libc::kill(child_pid as i32, libc::SIGTERM);
        // Reap zombie
        libc::waitpid(child_pid as i32, std::ptr::null_mut(), libc::WNOHANG);
    }
}

/// The terminal WebSocket route bypasses standard auth middleware
/// (token is validated inside the handler via query param).
pub fn router() -> Router<AppState> {
    Router::new().route("/terminal/ws", get(ws_handler))
}
