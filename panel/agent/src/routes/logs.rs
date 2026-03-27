use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, Query, State, WebSocketUpgrade,
    },
    http::StatusCode,
    response::Response,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::AsyncBufReadExt;
use crate::safe_cmd::safe_command;

use super::AppState;
use crate::services::logs;

/// Maximum concurrent log stream WebSocket connections.
static ACTIVE_STREAMS: AtomicUsize = AtomicUsize::new(0);
const MAX_STREAMS: usize = 10;

type ApiErr = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: &str) -> ApiErr {
    (status, Json(serde_json::json!({ "error": msg })))
}

#[derive(Deserialize)]
struct LogQuery {
    r#type: Option<String>,
    lines: Option<usize>,
    filter: Option<String>,
}

#[derive(Deserialize)]
struct SearchQuery {
    r#type: Option<String>,
    pattern: Option<String>,
    max: Option<usize>,
}

#[derive(Deserialize)]
struct StreamQuery {
    token: Option<String>,
    r#type: Option<String>,
    domain: Option<String>,
}

#[derive(Deserialize)]
struct StreamTicket {
    #[allow(dead_code)]
    sub: String,
    purpose: String,
}

/// GET /logs?type=nginx_access&lines=100&filter=404
async fn get_logs(Query(q): Query<LogQuery>) -> Result<Json<Vec<String>>, ApiErr> {
    let log_type = q.r#type.as_deref().unwrap_or("syslog");
    let lines = q.lines.unwrap_or(100);
    let filter = q.filter.as_deref();

    let result = logs::read_log(log_type, lines, filter)
        .await
        .map_err(|e| {
            if e.contains("not found") || e.contains("Unknown log type") {
                err(StatusCode::NOT_FOUND, &e)
            } else {
                err(StatusCode::INTERNAL_SERVER_ERROR, &e)
            }
        })?;

    Ok(Json(result))
}

/// GET /logs/{domain}?type=access&lines=100&filter=
async fn get_site_logs(
    Path(domain): Path<String>,
    Query(q): Query<LogQuery>,
) -> Result<Json<Vec<String>>, ApiErr> {
    let short_type = q.r#type.as_deref().unwrap_or("access");
    let lines = q.lines.unwrap_or(100);
    let filter = q.filter.as_deref();

    let log_type = match short_type {
        "access" => format!("nginx_access:{domain}"),
        "error" => format!("nginx_error:{domain}"),
        other => {
            return Err(err(
                StatusCode::BAD_REQUEST,
                &format!("Invalid site log type: {other}. Use 'access' or 'error'"),
            ));
        }
    };

    let result = logs::read_log(&log_type, lines, filter)
        .await
        .map_err(|e| {
            if e.contains("not found") || e.contains("traversal") || e.contains("Invalid domain")
            {
                err(StatusCode::BAD_REQUEST, &e)
            } else {
                err(StatusCode::INTERNAL_SERVER_ERROR, &e)
            }
        })?;

    Ok(Json(result))
}

/// GET /logs/search?type=nginx_access&pattern=404&max=500
async fn search_logs(Query(q): Query<SearchQuery>) -> Result<Json<Vec<String>>, ApiErr> {
    let log_type = q.r#type.as_deref().unwrap_or("nginx_access");
    let pattern = q.pattern.as_deref().unwrap_or("");
    let max = q.max.unwrap_or(500);

    if pattern.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Pattern is required"));
    }

    let result = logs::search_log(log_type, pattern, max)
        .await
        .map_err(|e| {
            if e.contains("not found") {
                err(StatusCode::NOT_FOUND, &e)
            } else {
                err(StatusCode::UNPROCESSABLE_ENTITY, &e)
            }
        })?;

    Ok(Json(result))
}

/// GET /logs/stream — WebSocket endpoint for real-time log tailing.
/// Auth via ?token= (short-lived JWT), ?type= for log type, ?domain= for site-specific.
async fn stream_handler(
    State(state): State<AppState>,
    Query(q): Query<StreamQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    // Validate JWT ticket
    let token_value = state.token.read().await.clone();
    let valid = q
        .token
        .as_deref()
        .map(|t| {
            let mut validation =
                jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
            validation.set_required_spec_claims(&["exp", "sub"]);
            validation.validate_exp = true;
            jsonwebtoken::decode::<StreamTicket>(
                t,
                &jsonwebtoken::DecodingKey::from_secret(token_value.as_bytes()),
                &validation,
            )
            .map(|data| data.claims.purpose == "log_stream")
            .unwrap_or(false)
        })
        .unwrap_or(false);

    if !valid {
        return Response::builder()
            .status(401)
            .body("Unauthorized".into())
            .unwrap();
    }

    let log_type_raw = q.r#type.clone().unwrap_or_else(|| "nginx_access".into());
    let domain = q.domain.clone();

    // Resolve the full log type (for site-specific logs)
    let log_type = if let Some(ref d) = domain {
        match log_type_raw.as_str() {
            "access" => format!("nginx_access:{d}"),
            "error" => format!("nginx_error:{d}"),
            other => other.to_string(),
        }
    } else {
        log_type_raw
    };

    // Enforce concurrent stream limit
    let current = ACTIVE_STREAMS.load(Ordering::Relaxed);
    if current >= MAX_STREAMS {
        return Response::builder()
            .status(429)
            .body("Too many active log streams".into())
            .unwrap();
    }

    // Resolve to file path
    let path = match logs::resolve_log_path(&log_type) {
        Ok(p) => p,
        Err(_) => {
            return Response::builder()
                .status(400)
                .body("Invalid log type".into())
                .unwrap();
        }
    };

    ws.on_upgrade(move |socket| handle_stream(socket, path))
}

/// RAII guard that kills the tail child process and decrements the stream counter on drop.
struct StreamGuard {
    child: tokio::process::Child,
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        // Best-effort kill — start_kill is non-async and safe in Drop
        let _ = self.child.start_kill();
        ACTIVE_STREAMS.fetch_sub(1, Ordering::Relaxed);
    }
}

async fn handle_stream(mut socket: WebSocket, path: String) {
    ACTIVE_STREAMS.fetch_add(1, Ordering::Relaxed);

    // Start tail -f with last 50 lines so the user sees recent context
    let child = safe_command("tail")
        .args(["-f", "-n", "50", &path])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();

    let mut guard = match child {
        Ok(c) => StreamGuard { child: c },
        Err(e) => {
            ACTIVE_STREAMS.fetch_sub(1, Ordering::Relaxed);
            let _ = socket
                .send(Message::Text(format!("Error: {e}").into()))
                .await;
            return;
        }
    };

    let stdout = guard.child.stdout.take().unwrap();
    let reader = tokio::io::BufReader::new(stdout);
    let mut lines = reader.lines();

    loop {
        tokio::select! {
            // New line from tail -f
            line = lines.next_line() => {
                match line {
                    Ok(Some(text)) => {
                        if socket.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    Ok(None) => break, // EOF
                    Err(_) => break,
                }
            }
            // Client message (close or ping)
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(data))) => {
                        let _ = socket.send(Message::Pong(data)).await;
                    }
                    _ => {}
                }
            }
        }
    }
    // guard dropped here → kills child process and decrements counter
}

/// GET /logs/stats — Parse nginx access log for aggregated stats.
async fn log_stats(
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    let domain = params.get("domain").map(|s| s.as_str());

    // Validate domain to prevent path traversal
    if let Some(d) = domain {
        if !d.is_empty() && !super::is_valid_domain(d) {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid domain"));
        }
    }

    let log_path = match domain {
        Some(d) if !d.is_empty() => format!("/var/log/nginx/{d}.access.log"),
        _ => "/var/log/nginx/access.log".to_string(),
    };

    if !std::path::Path::new(&log_path).exists() {
        return Ok(Json(serde_json::json!({
            "errors_per_hour": {},
            "top_urls": [],
            "status_breakdown": {},
            "requests_total": 0,
            "errors_5xx": 0
        })));
    }

    // Read last 50000 lines
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        safe_command("tail")
            .args(["-n", "50000", &log_path])
            .output(),
    )
    .await
    .map_err(|_| err(StatusCode::GATEWAY_TIMEOUT, "Timeout reading log file"))?
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let content = String::from_utf8_lossy(&output.stdout);

    let mut status_map: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut url_map: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut errors_by_hour: std::collections::HashMap<String, u32> =
        std::collections::HashMap::new();
    let mut total = 0u32;

    for line in content.lines() {
        if line.is_empty() {
            continue;
        }
        total += 1;

        // Extract status code: after "HTTP/x.x" STATUS
        if let Some(rest) = line.split("\" ").nth(1) {
            let status = rest.split_whitespace().next().unwrap_or("");
            *status_map.entry(status.to_string()).or_insert(0) += 1;

            // Track 5xx errors by hour
            if status.starts_with('5') {
                // Extract hour from log date [18/Mar/2026:14:30:00
                if let Some(date_part) = line.split('[').nth(1) {
                    if let Some(time_part) = date_part.split(':').nth(1) {
                        let hour = format!("{time_part}:00");
                        *errors_by_hour.entry(hour).or_insert(0) += 1;
                    }
                }
            }
        }

        // Extract URL path
        if let Some(request_line) = line.split('"').nth(1) {
            let parts: Vec<&str> = request_line.split_whitespace().collect();
            if let Some(path) = parts.get(1) {
                let clean = path.split('?').next().unwrap_or(path);
                if clean != "/favicon.ico" && !clean.starts_with("/api/") {
                    *url_map.entry(clean.to_string()).or_insert(0) += 1;
                }
            }
        }
    }

    // Top 10 URLs
    let mut top_urls: Vec<(&String, &u32)> = url_map.iter().collect();
    top_urls.sort_by(|a, b| b.1.cmp(a.1));
    let top: Vec<serde_json::Value> = top_urls
        .iter()
        .take(10)
        .map(|(url, count)| serde_json::json!({ "url": url, "count": count }))
        .collect();

    // Total 5xx errors
    let error_5xx: u32 = status_map
        .iter()
        .filter(|(k, _)| k.starts_with('5'))
        .map(|(_, v)| v)
        .sum();

    Ok(Json(serde_json::json!({
        "requests_total": total,
        "status_breakdown": status_map,
        "top_urls": top,
        "errors_5xx": error_5xx,
        "errors_per_hour": errors_by_hour,
    })))
}

/// GET /logs/docker — List Docker containers with dockpanel.managed label.
async fn docker_containers() -> Json<serde_json::Value> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        safe_command("docker")
            .args([
                "ps",
                "--filter",
                "label=dockpanel.managed=true",
                "--format",
                "{{.Names}}",
            ])
            .output(),
    )
    .await;

    let names: Vec<String> = output
        .ok().and_then(|r| r.ok())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.to_string())
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default();

    Json(serde_json::json!({ "containers": names }))
}

/// GET /logs/docker/{container} — Get Docker container logs.
async fn docker_logs(
    Path(container): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    // Validate container name
    if container.is_empty()
        || !container
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container name"));
    }

    let lines = params
        .get("lines")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(200)
        .min(1000);

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        safe_command("docker")
            .args(["logs", "--tail", &lines.to_string(), &container])
            .output(),
    )
    .await
    .map_err(|_| err(StatusCode::GATEWAY_TIMEOUT, "Timeout"))?
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let content = format!("{stdout}{stderr}");

    Ok(Json(serde_json::json!({
        "logs": content,
        "lines": content.lines().count()
    })))
}

/// GET /logs/service/{service} — Get systemd service logs via journalctl.
async fn service_logs(
    Path(service): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    // Whitelist of allowed services
    let allowed = [
        "dockpanel-agent",
        "dockpanel-api",
        "nginx",
        "postfix",
        "dovecot",
        "fail2ban",
        "docker",
        "opendkim",
        "rspamd",
        "redis-server",
        "php8.3-fpm",
        "php8.2-fpm",
    ];

    if !allowed.contains(&service.as_str()) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            &format!("Service '{}' not in allowed list", service),
        ));
    }

    let lines = params
        .get("lines")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(100)
        .min(500);

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        safe_command("journalctl")
            .args([
                "-u",
                &service,
                "-n",
                &lines.to_string(),
                "--no-pager",
                "-o",
                "short-iso",
            ])
            .output(),
    )
    .await
    .map_err(|_| err(StatusCode::GATEWAY_TIMEOUT, "Timeout"))?
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let content = String::from_utf8_lossy(&output.stdout).to_string();

    Ok(Json(serde_json::json!({
        "logs": content,
        "service": service,
        "lines": content.lines().count()
    })))
}

/// GET /logs/sizes — Get log file sizes.
async fn log_sizes() -> Json<serde_json::Value> {
    let log_files = [
        ("/var/log/nginx/access.log", "Nginx Access"),
        ("/var/log/nginx/error.log", "Nginx Error"),
        ("/var/log/syslog", "Syslog"),
        ("/var/log/auth.log", "Auth"),
        ("/var/log/mail.log", "Mail"),
    ];

    let mut files = Vec::new();
    let mut total_bytes: u64 = 0;

    for (path, label) in &log_files {
        let size = tokio::fs::metadata(path)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        total_bytes += size;
        files.push(serde_json::json!({
            "path": path,
            "label": label,
            "size": size,
            "size_mb": (size as f64 / 1024.0 / 1024.0 * 10.0).round() / 10.0,
        }));
    }

    // Also check per-site logs
    if let Ok(mut entries) = tokio::fs::read_dir("/var/log/nginx").await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".access.log") && name != "access.log" {
                let size = entry.metadata().await.map(|m| m.len()).unwrap_or(0);
                total_bytes += size;
                let domain = name.trim_end_matches(".access.log");
                files.push(serde_json::json!({
                    "path": format!("/var/log/nginx/{name}"),
                    "label": format!("{domain} Access"),
                    "size": size,
                    "size_mb": (size as f64 / 1024.0 / 1024.0 * 10.0).round() / 10.0,
                }));
            }
        }
    }

    // Sort by size descending
    files.sort_by(|a, b| {
        b["size"]
            .as_u64()
            .unwrap_or(0)
            .cmp(&a["size"].as_u64().unwrap_or(0))
    });

    // Check logrotate status
    let logrotate_installed = std::path::Path::new("/etc/logrotate.d/nginx").exists();

    Json(serde_json::json!({
        "files": files,
        "total_bytes": total_bytes,
        "total_mb": (total_bytes as f64 / 1024.0 / 1024.0 * 10.0).round() / 10.0,
        "logrotate": logrotate_installed,
    }))
}

/// POST /logs/truncate — Truncate a specific log file (clear it).
async fn truncate_log(
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    let path = body.get("path").and_then(|v| v.as_str()).unwrap_or("");

    // Only allow truncating under /var/log/
    if path.is_empty() || !path.starts_with("/var/log/") || path.contains("..") {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid log path"));
    }

    // Canonicalize to resolve symlinks, then re-verify path is under /var/log/
    let canonical = std::fs::canonicalize(path)
        .map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid path"))?;
    if !canonical.starts_with("/var/log/") {
        return Err(err(StatusCode::FORBIDDEN, "Path escapes /var/log/"));
    }

    // Verify it's a regular file, not a symlink
    let meta = std::fs::symlink_metadata(path)
        .map_err(|_| err(StatusCode::BAD_REQUEST, "Cannot stat file"))?;
    if meta.file_type().is_symlink() {
        return Err(err(StatusCode::FORBIDDEN, "Symlinks not allowed"));
    }

    tokio::fs::write(&canonical, "")
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    tracing::info!("Log file truncated: {}", canonical.display());
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/logs", get(get_logs))
        .route("/logs/search", get(search_logs))
        .route("/logs/stats", get(log_stats))
        .route("/logs/docker", get(docker_containers))
        .route("/logs/docker/{container}", get(docker_logs))
        .route("/logs/service/{service}", get(service_logs))
        .route("/logs/sizes", get(log_sizes))
        .route("/logs/truncate", post(truncate_log))
        .route("/logs/{domain}", get(get_site_logs))
}

/// Stream route — placed outside auth middleware (validates its own JWT via query param).
pub fn stream_router() -> Router<AppState> {
    Router::new().route("/logs/stream", get(stream_handler))
}
