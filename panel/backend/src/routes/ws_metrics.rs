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

/// True once ANY of the checks `handler()` runs at handshake would now reject `claims` —
/// expiry, blacklist, or global session revocation. Shared by the handshake gate and the
/// per-tick recheck in `handle_socket` so the two can't drift (see the comment at the
/// handshake's revocation check).
async fn claims_now_invalid(state: &AppState, claims: &Claims) -> bool {
    if (claims.exp as i64) < chrono::Utc::now().timestamp() {
        return true;
    }
    if let Some(ref jti) = claims.jti
        && state.token_blacklist.read().await.contains(jti)
    {
        return true;
    }
    if let Some(ts) = *state.sessions_revoked_at.read().await
        && (claims.iat as i64) < ts
    {
        return true;
    }
    false
}

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

    // Global session revocation — the panic button and `POST /auth/revoke-all`
    // both write `sessions_revoked_at`, and `AuthUser` refuses any token minted
    // before it. This handler decodes the JWT by hand rather than going through
    // that extractor, so without this block it was the one authenticated channel
    // panic could not close: a stolen admin token could open a NEW socket after
    // the button was pressed and stream the process list every 5 seconds until
    // its own 2h expiry — a live view of the operator's incident response, for
    // the intruder it was pressed against.
    //
    // The comparison is deliberately IDENTICAL to the one in `AuthUser` — two
    // gates enforcing one invariant must not drift. `iat` is whole seconds, so
    // a token minted in the same second as the revoke survives at both sites;
    // tightening that is a single change to BOTH, not an asymmetric one here
    // (which would also reject the admin's own re-login inside that second).
    {
        let revoked_at = state.sessions_revoked_at.read().await;
        if let Some(ts) = *revoked_at {
            if (claims.iat as i64) < ts {
                return axum::http::Response::builder()
                    .status(401)
                    .body(axum::body::Body::from("Session revoked. Please log in again."))
                    .unwrap()
                    .into_response();
            }
        }
    }

    // Live host telemetry (system info, full process list, network connections, GPU)
    // is admin-only — the equivalent REST endpoints (system::info, logs::processes,
    // logs::network) all require AdminUser. ServerScope's no-X-Server-Id fallback
    // resolves the local/admin-owned server WITHOUT a role check, so gate here.
    if claims.role != "admin" {
        return axum::http::Response::builder()
            .status(403)
            .body(axum::body::Body::from("Admin access required"))
            .unwrap()
            .into_response();
    }

    // Enforce connection limit
    let current = WS_CONNECTIONS.fetch_add(1, Ordering::SeqCst);
    if current >= MAX_WS_CONNECTIONS {
        WS_CONNECTIONS.fetch_sub(1, Ordering::SeqCst);
        return (StatusCode::TOO_MANY_REQUESTS, "Too many WebSocket connections").into_response();
    }

    ws.on_upgrade(move |socket| handle_socket(socket, agent, state, claims))
}

async fn handle_socket(mut socket: WebSocket, agent: AgentHandle, state: AppState, claims: Claims) {
    tracing::debug!("WebSocket metrics client connected");

    loop {
        // Re-validate every tick, not just at handshake. Without this, logout, the panic
        // button, and token-blacklist revocation all closed the door to NEW connections
        // but left an ALREADY-OPEN socket streaming the full process/network/GPU/system
        // feed for as long as the underlying TCP connection stayed alive — unbounded by
        // even the token's own nominal expiry, since nothing in this loop ever re-checked
        // `exp` either. This is exactly the "live view of the operator's incident
        // response" scenario the handshake's own revocation comment says panic closes.
        if claims_now_invalid(&state, &claims).await {
            tracing::info!("WebSocket metrics: session no longer valid, closing");
            break;
        }

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
