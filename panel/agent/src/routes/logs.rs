use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, Query, State, WebSocketUpgrade,
    },
    http::StatusCode,
    response::Response,
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::AsyncBufReadExt;
use tokio::process::Command;

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
                &jsonwebtoken::DecodingKey::from_secret(state.token.as_bytes()),
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
    let child = Command::new("tail")
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

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/logs", get(get_logs))
        .route("/logs/search", get(search_logs))
        .route("/logs/{domain}", get(get_site_logs))
}

/// Stream route — placed outside auth middleware (validates its own JWT via query param).
pub fn stream_router() -> Router<AppState> {
    Router::new().route("/logs/stream", get(stream_handler))
}
