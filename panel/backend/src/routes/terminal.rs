use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use jsonwebtoken::{encode, EncodingKey, Header};

use crate::auth::{AuthUser, ServerScope};
use crate::error::{err, require_admin, ApiError};
use crate::AppState;

#[derive(serde::Deserialize)]
pub struct TerminalQuery {
    pub site_id: Option<String>,
}

#[derive(serde::Serialize)]
struct TerminalTicket {
    sub: String,
    purpose: String,
    exp: usize,
}

/// GET /api/terminal/token — Generate a short-lived terminal ticket.
/// Returns a 60-second JWT signed with the agent token (never exposes the raw agent token).
pub async fn ws_token(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Query(q): Query<TerminalQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Server-level terminal requires admin role
    if q.site_id.is_none() && claims.role != "admin" {
        return Err(err(
            StatusCode::FORBIDDEN,
            "Admin access required for server terminal",
        ));
    }

    // Optionally resolve domain from site_id
    let domain = if let Some(ref sid) = q.site_id {
        let site_id: uuid::Uuid = sid
            .parse()
            .map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid site_id"))?;

        let row: Option<(String,)> =
            sqlx::query_as("SELECT domain FROM sites WHERE id = $1 AND user_id = $2")
                .bind(site_id)
                .bind(claims.sub)
                .fetch_optional(&state.db)
                .await
                .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

        row.map(|(d,)| d)
    } else {
        None
    };

    // Generate a short-lived JWT ticket (60 seconds) signed with the agent token
    let ticket = TerminalTicket {
        sub: claims.email,
        purpose: "terminal".to_string(),
        exp: (chrono::Utc::now() + chrono::Duration::seconds(60)).timestamp() as usize,
    };

    let token = encode(
        &Header::default(),
        &ticket,
        &EncodingKey::from_secret(agent.token().as_bytes()),
    )
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(serde_json::json!({
        "token": token,
        "domain": domain,
    })))
}

/// POST /api/terminal/share — Save terminal output for sharing (temporary, 1 hour expiry).
pub async fn share_output(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let content = body
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if content.is_empty() || content.len() > 500_000 {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Content required (max 500KB)",
        ));
    }

    // Generate share token (12 hex chars from UUID)
    let share_id = uuid::Uuid::new_v4()
        .to_string()
        .replace('-', "")
        .chars()
        .take(12)
        .collect::<String>();

    // Store in settings table (simple approach — no new table needed)
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ($1, $2) ON CONFLICT (key) DO UPDATE SET value = $2",
    )
    .bind(format!("terminal_share_{share_id}"))
    .bind(content)
    .execute(&state.db)
    .await
    .ok();

    // Auto-cleanup after 1 hour (best effort)
    let db = state.db.clone();
    let sid = share_id.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        sqlx::query("DELETE FROM settings WHERE key = $1")
            .bind(format!("terminal_share_{sid}"))
            .execute(&db)
            .await
            .ok();
    });

    Ok(Json(serde_json::json!({
        "share_id": share_id,
        "url": format!("/api/terminal/shared/{share_id}")
    })))
}

/// GET /api/terminal/shared/{id} — View shared terminal output (public, no auth).
pub async fn view_shared(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<axum::response::Html<String>, ApiError> {
    let content: Option<(String,)> =
        sqlx::query_as("SELECT value FROM settings WHERE key = $1")
            .bind(format!("terminal_share_{id}"))
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();

    let content = content
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Share expired or not found"))?
        .0;

    let escaped = content.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");

    let html = format!(
        r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>DockPanel Terminal Share</title>
<style>body {{ background: #1e1e2e; color: #cdd6f4; font-family: 'JetBrains Mono', monospace; padding: 20px; margin: 0; }}
pre {{ white-space: pre-wrap; word-wrap: break-word; font-size: 13px; line-height: 1.5; }}
.header {{ color: #a6adc8; font-size: 12px; margin-bottom: 10px; border-bottom: 1px solid #45475a; padding-bottom: 8px; }}
</style></head><body>
<div class="header">DockPanel Terminal Share — expires after 1 hour</div>
<pre>{}</pre>
</body></html>"#,
        escaped
    );

    Ok(axum::response::Html(html))
}
