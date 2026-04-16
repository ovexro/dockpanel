use axum::{
    extract::{Query, State, WebSocketUpgrade},
    extract::ws::{Message, WebSocket},
    response::IntoResponse,
    http::{HeaderMap, StatusCode},
};
use jsonwebtoken::{decode, DecodingKey, Validation};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use crate::auth::{Claims, ServerScope};
use crate::services::agent::AgentHandle;
use crate::AppState;

static WS_CONNECTIONS: AtomicU32 = AtomicU32::new(0);
const MAX_WS_CONNECTIONS: u32 = 50;

#[derive(serde::Deserialize)]
pub struct WsQuery {
    pub token: Option<String>,
}

/// Extract JWT token from query param OR cookie header
fn extract_token(q: &WsQuery, headers: &HeaderMap) -> Option<String> {
    // Try query param first
    if let Some(ref t) = q.token {
        if !t.is_empty() {
            return Some(t.clone());
        }
    }
    // Fall back to cookie
    if let Some(cookie_header) = headers.get("cookie") {
        if let Ok(cookies) = cookie_header.to_str() {
            for cookie in cookies.split(';') {
                let cookie = cookie.trim();
                if let Some(value) = cookie.strip_prefix("token=") {
                    if !value.is_empty() {
                        return Some(value.to_string());
                    }
                }
            }
        }
    }
    None
}

/// GET /api/ws/metrics — WebSocket endpoint that pushes live system metrics every 5s.
/// Authenticates via ?token=<jwt> query param OR cookie.
pub async fn handler(
    State(state): State<AppState>,
    ServerScope(_server_id, agent): ServerScope,
    Query(q): Query<WsQuery>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let token = match extract_token(&q, &headers) {
        Some(t) => t,
        None => {
            return axum::http::Response::builder()
                .status(401)
                .body(axum::body::Body::from("Missing authentication token"))
                .unwrap()
                .into_response();
        }
    };

    // Validate JWT
    let mut validation = Validation::new(jsonwebtoken::Algorithm::HS256);
    validation.validate_exp = true;
    validation.leeway = 0;

    let claims = match decode::<Claims>(
        &token,
        &DecodingKey::from_secret(state.config.jwt_secret.as_bytes()),
        &validation,
    ) {
        Ok(data) => data.claims,
        Err(_) => {
            return axum::http::Response::builder()
                .status(401)
                .body(axum::body::Body::from("Invalid or expired token"))
                .unwrap()
                .into_response();
        }
    };

    // Check token blacklist
    {
        let blacklist = state.token_blacklist.read().await;
        if let Some(ref jti) = claims.jti {
            if blacklist.contains(jti) {
                return axum::http::Response::builder()
                    .status(401)
                    .body(axum::body::Body::from("Token has been revoked"))
                    .unwrap()
                    .into_response();
            }
        }
    }

    // Enforce connection limit
    let current = WS_CONNECTIONS.fetch_add(1, Ordering::SeqCst);
    if current >= MAX_WS_CONNECTIONS {
        WS_CONNECTIONS.fetch_sub(1, Ordering::SeqCst);
        return (StatusCode::TOO_MANY_REQUESTS, "Too many WebSocket connections").into_response();
    }

    ws.on_upgrade(move |socket| handle_socket(socket, agent))
}

async fn handle_socket(mut socket: WebSocket, agent: AgentHandle) {
    tracing::debug!("WebSocket metrics client connected");

    loop {
        // Fetch all four endpoints concurrently
        let (system_res, processes_res, network_res, gpu_res) = tokio::join!(
            agent.get("/system/info"),
            agent.get("/system/processes"),
            agent.get("/system/network"),
            agent.get("/apps/gpu-info"),
        );

        let system = system_res.unwrap_or_else(|_| serde_json::json!(null));
        let processes = processes_res.unwrap_or_else(|_| serde_json::json!(null));
        let network = network_res.unwrap_or_else(|_| serde_json::json!(null));
        let gpu = gpu_res.unwrap_or_else(|_| serde_json::json!(null));

        let payload = serde_json::json!({
            "type": "metrics",
            "system": system,
            "processes": processes,
            "network": network,
            "gpu": gpu,
        });

        let msg = Message::Text(payload.to_string().into());
        if socket.send(msg).await.is_err() {
            // Client disconnected
            break;
        }

        // Also check for incoming close/ping frames
        match tokio::time::timeout(Duration::from_secs(5), socket.recv()).await {
            Ok(Some(Ok(Message::Close(_)))) => break,
            Ok(Some(Err(_))) => break,
            Ok(None) => break,
            // Timeout (5s elapsed) or non-close message — continue the loop
            _ => {}
        }
    }

    // Send explicit close frame so client doesn't linger
    let _ = socket.send(Message::Close(None)).await;
    WS_CONNECTIONS.fetch_sub(1, Ordering::SeqCst);
    tracing::debug!("WebSocket metrics client disconnected");
}
