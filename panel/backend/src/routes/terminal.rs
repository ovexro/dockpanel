use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use jsonwebtoken::{encode, EncodingKey, Header};

use crate::auth::{AuthUser, ServerScope};
use crate::error::{internal_error, err, require_admin, ApiError};
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

    // Block server terminal when disabled via settings (e.g., demo mode)
    if q.site_id.is_none() {
        let disabled: Option<(String,)> =
            sqlx::query_as("SELECT value FROM settings WHERE key = 'server_terminal_disabled'")
                .fetch_optional(&state.db)
                .await
                .map_err(|e| internal_error("ws token", e))?;
        if disabled.map(|r| r.0 == "true").unwrap_or(false) {
            return Err(err(StatusCode::FORBIDDEN, "Server terminal is disabled"));
        }
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
                .map_err(|e| internal_error("ws token", e))?;

        match row {
            Some((d,)) => Some(d),
            None => return Err(err(StatusCode::FORBIDDEN, "Site not found or not owned by you")),
        }
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
        &EncodingKey::from_secret(agent.token().await.as_bytes()),
    )
    .map_err(|e| internal_error("ws token", e))?;

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

    // Store in settings table with timestamp prefix for crash-resilient cleanup
    let value = format!("{}|{}", chrono::Utc::now().timestamp(), content);
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ($1, $2) ON CONFLICT (key) DO UPDATE SET value = $2",
    )
    .bind(format!("terminal_share_{share_id}"))
    .bind(&value)
    .execute(&state.db)
    .await
    .ok();

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

    let raw = content
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Share expired or not found"))?
        .0;

    // Strip timestamp prefix (format: "unix_ts|content") and compute remaining seconds
    let (created_ts, content) = if let Some(pos) = raw.find('|') {
        let ts: i64 = raw[..pos].parse().unwrap_or(0);
        (ts, &raw[pos + 1..])
    } else {
        (0i64, raw.as_str())
    };

    let now = chrono::Utc::now().timestamp();
    let remaining = if created_ts > 0 {
        std::cmp::max(0, 3600 - (now - created_ts))
    } else {
        3600i64
    };

    let escaped = content.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>DockPanel — Shared Terminal Output</title>
<link rel="preconnect" href="https://fonts.googleapis.com">
<link href="https://fonts.googleapis.com/css2?family=JetBrains+Mono:wght@400;500&display=swap" rel="stylesheet">
<style>
*,*::before,*::after{{box-sizing:border-box;margin:0;padding:0}}
body{{background:#0f0f17;color:#cdd6f4;font-family:'JetBrains Mono',monospace;min-height:100vh}}
.container{{max-width:1200px;margin:0 auto;padding:24px 20px}}
.header{{display:flex;align-items:center;justify-content:space-between;flex-wrap:wrap;gap:12px;margin-bottom:20px;padding-bottom:16px;border-bottom:1px solid #313244}}
.brand{{display:flex;align-items:center;gap:12px}}
.brand-name{{font-size:14px;font-weight:500;color:#a6adc8;letter-spacing:0.1em;text-transform:uppercase}}
.badge{{display:inline-flex;align-items:center;gap:6px;background:rgba(243,139,168,0.1);border:1px solid rgba(243,139,168,0.13);color:#f38ba8;font-size:11px;font-weight:500;padding:4px 12px;border-radius:20px;text-transform:uppercase;letter-spacing:0.05em}}
.badge::before{{content:'';width:6px;height:6px;background:#f38ba8;border-radius:50%;animation:pulse-dot 2s ease-in-out infinite}}
@keyframes pulse-dot{{0%,100%{{opacity:1}}50%{{opacity:0.4}}}}
.actions{{display:flex;align-items:center;gap:8px}}
.btn{{display:inline-flex;align-items:center;gap:6px;padding:6px 14px;border-radius:6px;font-size:12px;font-family:inherit;cursor:pointer;border:1px solid #45475a;background:#1e1e2e;color:#cdd6f4;transition:all 0.15s}}
.btn:hover{{background:#313244;border-color:#585b70}}
.expiry{{font-size:12px;color:#6c7086;display:flex;align-items:center;gap:6px}}
.expiry-time{{color:#f9e2af;font-weight:500}}
.terminal-wrap{{background:#1e1e2e;border:1px solid #313244;border-radius:8px;overflow:hidden}}
.terminal-bar{{display:flex;align-items:center;gap:8px;padding:10px 16px;background:#181825;border-bottom:1px solid #313244}}
.dots{{display:flex;gap:6px}}
.dot{{width:10px;height:10px;border-radius:50%;background:#45475a}}
.dot:nth-child(1){{background:#f38ba8}}
.dot:nth-child(2){{background:#f9e2af}}
.dot:nth-child(3){{background:#a6e3a1}}
.terminal-title{{color:#6c7086;font-size:11px;flex:1;text-align:center}}
pre{{padding:16px 20px;margin:0;white-space:pre-wrap;word-wrap:break-word;font-size:13px;line-height:1.6;overflow-x:auto;max-height:80vh;overflow-y:auto}}
pre::-webkit-scrollbar{{width:6px;height:6px}}
pre::-webkit-scrollbar-track{{background:#1e1e2e}}
pre::-webkit-scrollbar-thumb{{background:#45475a;border-radius:3px}}
pre::-webkit-scrollbar-thumb:hover{{background:#585b70}}
.footer{{margin-top:16px;text-align:center;color:#45475a;font-size:11px}}
.toast{{position:fixed;bottom:24px;left:50%;transform:translateX(-50%) translateY(100px);background:#a6e3a1;color:#1e1e2e;padding:8px 20px;border-radius:6px;font-size:12px;font-weight:500;opacity:0;transition:all 0.3s ease;pointer-events:none;z-index:100}}
.toast.show{{transform:translateX(-50%) translateY(0);opacity:1}}
@media(max-width:600px){{.container{{padding:12px}}.header{{flex-direction:column;align-items:flex-start}}pre{{font-size:11px;padding:12px}}}}
</style></head><body>
<div class="container">
  <div class="header">
    <div class="brand">
      <span class="brand-name">DockPanel</span>
      <span class="badge">Read-Only Snapshot</span>
    </div>
    <div class="actions">
      <button class="btn" onclick="copyOutput()" id="copyBtn">Copy All</button>
      <div class="expiry">Expires in <span class="expiry-time" id="countdown">--:--</span></div>
    </div>
  </div>
  <div class="terminal-wrap">
    <div class="terminal-bar">
      <div class="dots"><span class="dot"></span><span class="dot"></span><span class="dot"></span></div>
      <span class="terminal-title">shared terminal output</span>
    </div>
    <pre id="output">{escaped}</pre>
  </div>
  <div class="footer">Shared via DockPanel &mdash; self-hosted server management</div>
</div>
<div class="toast" id="toast">Copied to clipboard</div>
<script>
function copyOutput(){{
  var text=document.getElementById('output').textContent;
  if(navigator.clipboard){{
    navigator.clipboard.writeText(text).then(function(){{showToast('Copied to clipboard')}}).catch(function(){{fallbackCopy(text)}});
  }}else{{fallbackCopy(text)}}
}}
function fallbackCopy(t){{var a=document.createElement('textarea');a.value=t;a.style.position='fixed';a.style.opacity='0';document.body.appendChild(a);a.select();try{{document.execCommand('copy');showToast('Copied to clipboard')}}catch(e){{showToast('Copy failed')}}document.body.removeChild(a)}}
function showToast(msg){{var el=document.getElementById('toast');el.textContent=msg;el.classList.add('show');setTimeout(function(){{el.classList.remove('show')}},2000)}}
(function(){{
  var remaining={remaining};
  var el=document.getElementById('countdown');
  function update(){{
    if(remaining<=0){{el.textContent='Expired';el.style.color='#f38ba8';return}}
    var m=Math.floor(remaining/60);var s=remaining%60;
    el.textContent=m+':'+(s<10?'0':'')+s;
    remaining--;
    setTimeout(update,1000);
  }}
  update();
}})();
</script>
</body></html>"#,
        escaped = escaped,
        remaining = remaining
    );

    Ok(axum::response::Html(html))
}
