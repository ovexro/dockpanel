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

/// Marks a ticket as a server (root) shell. Not a valid domain — it contains an
/// `@`, which the agent's own domain validation rejects — so it cannot collide
/// with a site's name.
pub const SERVER_SHELL_SCOPE: &str = "@server";

/// The lockdown row's terminal switch, as `(lockdown_active, terminals_disabled)`.
///
/// A sibling of `security_hardening::is_locked_down`, and it would live beside it
/// if it were only about lockdown. It is not: it is the terminal subsystem's own
/// question, and the two columns only mean something when they are read together.
///
/// **Both columns, never one of them.** `activate_lockdown` writes
/// `terminals_disabled = TRUE` in the same statement that sets `active = TRUE`,
/// but `deactivate_lockdown` clears only `active` — nothing in the codebase ever
/// writes the terminal column back to FALSE. The migration compounds this: it
/// declares the column `DEFAULT TRUE` while seeding the singleton row with
/// `active = FALSE`, so on a panel that has never once been locked down the
/// column already reads TRUE. A gate keyed on `terminals_disabled` alone would
/// therefore refuse every shell on a fresh install, and would keep refusing them
/// for ever after the first unlock — a kill switch with no off position, wearing
/// the name of an emergency measure. `active` is what makes the flag mean
/// anything at a given moment; the flag is what says whether THIS lockdown was
/// one that reaches terminals. `is_locked_down` answers neither question: it
/// reads `active` and stops, which is right for the login and site-creation doors
/// and would be wrong here.
pub async fn lockdown_terminal_state(pool: &sqlx::PgPool) -> (bool, bool) {
    let row: Option<(bool, Option<bool>)> =
        sqlx::query_as("SELECT active, terminals_disabled FROM lockdown_state WHERE id = 1")
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();

    match row {
        // A NULL column underneath an ACTIVE lockdown is not evidence that
        // terminals were meant to stay open, so it is not read as permission.
        // Underneath an inactive lockdown it decides nothing either way.
        Some((active, disabled)) => (active, disabled.unwrap_or(true)),
        // No singleton row at all — the migration has not run, or somebody
        // deleted it. `is_locked_down` answers the same shape of question with
        // `false`, and disagreeing here would mean a panel mid-migration has no
        // terminals rather than no lockdown.
        None => (false, false),
    }
}

/// Whether a shell may be opened right now: a lockdown is active AND it is one
/// that disabled terminals.
///
/// The doors this closes are the two that CREATE something — the ticket mint and
/// the share writer — plus the unauthenticated viewer that serves what the share
/// writer stored. `revoke_share` and `list_shares` are deliberately left open:
/// they are how an operator sees a live share and closes it, and refusing them
/// during a lockdown would withdraw the remedy at the same moment as the risk.
pub async fn terminals_locked_down(pool: &sqlx::PgPool) -> bool {
    let (active, disabled) = lockdown_terminal_state(pool).await;
    active && disabled
}

#[derive(serde::Serialize)]
struct TerminalTicket {
    sub: String,
    purpose: String,
    exp: usize,
    /// Carries `security_session_recording` to the agent. It rides INSIDE the
    /// signed ticket because the browser dials the agent directly — as a query
    /// param the user could suppress the recording of their own session.
    record: bool,
    /// The shell this ticket authorises: a site's domain, or `@server` for the
    /// admin root shell. It rides inside the signed ticket for the SAME reason
    /// `record` does, and it is the fix for a real escalation: the agent used to
    /// read the scope from a `?domain=` query param the browser controls, and an
    /// EMPTY domain there spawns a root shell with no privilege drop. So any
    /// non-admin who owned a single site could mint an ownership-checked
    /// site-scoped ticket, then dial the agent with the domain omitted and get
    /// an unrestricted root shell on the host. Binding the scope into the
    /// signature makes the site-ownership check performed here the one that
    /// governs the shell that opens there.
    scope: String,
}

/// GET /api/terminal/token — Generate a short-lived terminal ticket.
/// Returns a 60-second JWT signed with the agent token (never exposes the raw agent token).
pub async fn ws_token(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(server_id, agent): ServerScope,
    Query(q): Query<TerminalQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Feature 9/11: the lockdown's terminal switch, finally read by somebody.
    //
    // This is THE door into a shell. The browser dials the agent's WebSocket
    // directly — the panel deliberately does not proxy it — so this handler is
    // not one gate among several on the way to a terminal, it is the only one
    // the panel ever gets to apply. Nothing downstream can refuse: the ticket it
    // returns is signed with the agent's own token, and an agent that verifies
    // the signature opens the shell.
    //
    // `activate_lockdown` has written `terminals_disabled = TRUE` since the
    // column existed and nothing anywhere has ever read it. Meanwhile the panic
    // button revokes every session on purpose and the design lets an admin log
    // straight back in — that concession is correct, it is how an operator keeps
    // control of a panel under attack. What made it dangerous was that logging
    // back in also restored the ability to mint a fresh root shell, seconds
    // after the button whose entire promise was that root shells had been
    // stopped. The panic response counted the shells it killed and said nothing
    // about the next one, because there was nothing to say: no code path
    // consulted the column that claimed to prevent it.
    //
    // Placed above every other check in this handler, including the role gate
    // and the per-site branch, because `terminals_disabled` does not mean "no
    // server shell". The sweep it belongs to kills SITE shells too — `killed` is
    // the total and `server_terminals_killed` is only the root subset — so a
    // site owner's shell is exactly as much a live session on the host as an
    // admin's, and refusing one while minting the other would leave the flag
    // half-honoured in the direction that helps an intruder.
    if terminals_locked_down(&state.db).await {
        return Err(err(
            StatusCode::SERVICE_UNAVAILABLE,
            "System is in lockdown mode. Terminal access is disabled until an \
             administrator unlocks the panel (Security → Lockdown).",
        ));
    }

    // Answer here rather than minting a ticket the receiving agent must reject.
    crate::helpers::require_local_agent_scope(&state, server_id, "The web terminal").await?;

    // Server-level terminal requires admin role. This gate is load-bearing — see
    // v2.75.0, where it guarded only the MINTING of a site-less ticket and the
    // decision never travelled to the agent, so a site owner could drop the scope
    // and reach a root shell. It stays exactly as strict as it is.
    //
    // What it should NOT do is read like a dead end. A site owner has a shell on
    // every site they own; the page simply used to ask for the wrong one by
    // default. Name the thing that works, the way the disabled-switch message
    // below names its switch.
    if q.site_id.is_none() && claims.role != "admin" {
        return Err(err(
            StatusCode::FORBIDDEN,
            "The server shell is available to administrators only. \
             Choose one of your own sites in the selector to open a shell inside it.",
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
            // Name the switch and where it lives. The page cannot read this
            // setting itself, so this sentence is the only thing that tells an
            // operator the shell is off by choice rather than broken.
            return Err(err(
                StatusCode::FORBIDDEN,
                "The server shell is switched off for this panel \
                 (Settings → Security → Server terminal). Per-site shells are unaffected.",
            ));
        }
    }

    // Optionally resolve domain from site_id
    let domain = if let Some(ref sid) = q.site_id {
        let site_id: uuid::Uuid = sid
            .parse()
            .map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid site_id"))?;

        let row: Option<(String,)> =
            sqlx::query_as(&format!("SELECT s.domain FROM sites s WHERE {}", crate::helpers::SITE_CALLER_PREDICATE))
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
    let record = crate::services::security_hardening::get_setting_bool(
        &state.db,
        "security_session_recording",
        true,
    )
    .await;
    // `domain` is Some only for a site shell whose ownership was checked above,
    // and None only on the admin-gated server-shell branch — so this is the
    // authorisation decision, signed, not a hint the client can override.
    let scope = domain
        .clone()
        .unwrap_or_else(|| SERVER_SHELL_SCOPE.to_string());
    let ticket = TerminalTicket {
        sub: claims.email,
        purpose: "terminal".to_string(),
        exp: (chrono::Utc::now() + chrono::Duration::seconds(60)).timestamp() as usize,
        record,
        scope,
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

/// A share's stated lifetime: one hour. Quoted back to the operator by
/// `list_shares` and counted down on the public page.
const SHARE_TTL_SECS: i64 = 3600;

/// `share_output` mints exactly 12 hex characters. `revoke_share` has always
/// checked that shape before touching the database; the public viewer never did.
fn valid_share_id(id: &str) -> bool {
    id.len() == 12 && id.chars().all(|c| c.is_ascii_hexdigit())
}

/// Split a stored share into `(created_at, seconds_left, owner_id, content)`,
/// or `None` when it is expired or carries no usable timestamp.
///
/// **Both readers must go through here.** Until this was written they disagreed:
/// the operator's own share list skipped anything past its hour, while the public
/// viewer computed the same number, clamped it to zero and rendered the content
/// regardless — so the panel reported a share as gone while an unauthenticated
/// URL still served it. What it serves is root terminal output, and the only
/// thing that eventually removed the row was a retention sweep that runs once a
/// day. A daily sweep is a housekeeper, not an access control; expiry has to be
/// decided by whoever answers the request.
///
/// `owner_id` is the creating admin's `claims.sub`, added so `list_shares` and
/// `revoke_share` can scope to it — a multi-admin/reseller install must not let
/// one admin enumerate (and, via the share_id, read) another admin's root
/// terminal output. A row written before this field existed has no second `|`,
/// so it parses with `owner_id == ""` — matching no real admin, it simply
/// becomes invisible to `list_shares`/`revoke_share` until it expires within
/// `SHARE_TTL_SECS`, which is the correct fail-closed behavior for a value this
/// short-lived. `view_shared` never reads `owner_id` — its link-based access
/// is unauthenticated by design and unaffected by this scoping.
fn share_lifetime(raw: &str, now: i64) -> Option<(i64, i64, &str, &str)> {
    let (created_ts, rest) = match raw.find('|') {
        Some(pos) => (raw[..pos].parse::<i64>().unwrap_or(0), &raw[pos + 1..]),
        None => (0, raw),
    };
    let (owner, content) = match rest.find('|') {
        Some(pos) => (&rest[..pos], &rest[pos + 1..]),
        None => ("", rest),
    };

    // A row whose timestamp will not parse cannot be shown to have life left,
    // so it is not granted any. The operator's list has always read it that way.
    if created_ts <= 0 {
        return None;
    }

    let remaining = SHARE_TTL_SECS - (now - created_ts);
    if remaining <= 0 {
        return None;
    }

    Some((created_ts, remaining, owner, content))
}

/// POST /api/terminal/share — Save terminal output for sharing (temporary, 1 hour expiry).
pub async fn share_output(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    // A share is an UNAUTHENTICATED public URL holding up to 500KB of root
    // terminal output, and the panic button deletes every existing one for
    // exactly that reason. Deleting handles what already exists; nothing handled
    // what arrives next, so the sweep lasted precisely until the following POST
    // and the operator was told "all sessions revoked" either way. The two
    // halves only add up to a closed door together.
    if terminals_locked_down(&state.db).await {
        return Err(err(
            StatusCode::SERVICE_UNAVAILABLE,
            "System is in lockdown mode. Terminal output cannot be shared while \
             the panel is locked down.",
        ));
    }

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

    // Store in settings table with timestamp + owner prefix for crash-resilient
    // cleanup and per-admin scoping (see share_lifetime).
    let value = format!("{}|{}|{}", chrono::Utc::now().timestamp(), claims.sub, content);
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

/// DELETE /api/terminal/share/{id} — Revoke (delete) a terminal share early.
pub async fn revoke_share(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    // Validate share_id format (12 hex chars)
    if !valid_share_id(&id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid share ID"));
    }

    let key = format!("terminal_share_{id}");
    // Scoped to the calling admin's own share (see share_lifetime). 0 rows
    // affected covers "doesn't exist", "already expired", AND "belongs to a
    // different admin" — this file's own established pattern is to never let
    // a response distinguish those cases (see view_shared's identical choice).
    let result = sqlx::query(
        "DELETE FROM settings WHERE key = $1 AND split_part(value, '|', 2) = $2",
    )
        .bind(&key)
        .bind(claims.sub.to_string())
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("revoke share", e))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Share not found or already expired"));
    }

    Ok(Json(serde_json::json!({ "ok": true, "share_id": id })))
}

/// GET /api/terminal/shares — List active terminal shares (admin only).
pub async fn list_shares(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT key, value FROM settings WHERE key LIKE 'terminal_share_%'"
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("list shares", e))?;

    let now = chrono::Utc::now().timestamp();
    let my_id = claims.sub.to_string();
    let mut shares = Vec::new();

    for (key, value) in &rows {
        let share_id = key.strip_prefix("terminal_share_").unwrap_or(key);

        // Already expired, or unparsable: retention will collect the row. The
        // public viewer applies this same rule, so the two answers agree.
        let Some((created_ts, remaining, owner, _content)) = share_lifetime(value, now) else {
            continue;
        };
        // Scope to the calling admin's own shares (see share_lifetime) — a
        // multi-admin/reseller install must not let one admin enumerate (and
        // then, via the exposed share_id, read) another admin's root
        // terminal output.
        if owner != my_id {
            continue;
        }

        shares.push(serde_json::json!({
            "share_id": share_id,
            "created_at": created_ts,
            "expires_in_seconds": remaining,
            "url": format!("/api/terminal/shared/{share_id}"),
        }));
    }

    Ok(Json(serde_json::json!({ "shares": shares })))
}

/// GET /api/terminal/shared/{id} — View shared terminal output (public, no auth).
pub async fn view_shared(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<axum::response::Html<String>, ApiError> {
    // This route takes no authentication, so the id is the only credential and
    // it gets the same shape check the revoke path applies.
    if !valid_share_id(&id) {
        return Err(err(StatusCode::NOT_FOUND, "Share expired or not found"));
    }

    // The panic button deletes every share, so on that path there is nothing
    // here to serve. A manually activated lockdown does not delete them, and
    // this route carries no authentication at all — during a declared emergency
    // an anonymous URL still handing out root terminal output is the exact thing
    // the lockdown was declared about.
    //
    // Answered with the same status and the same sentence an expired or unknown
    // id gets, so the property that response established still holds: the reply
    // cannot be used to tell those two apart, and now it also does not announce
    // the panel's emergency state to an anonymous caller. The lockdown is a
    // panel-wide fact, not a per-id one, so answering uniformly leaks nothing
    // about which ids exist.
    if terminals_locked_down(&state.db).await {
        return Err(err(StatusCode::NOT_FOUND, "Share expired or not found"));
    }

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

    // Expiry is enforced HERE, on the way out, not by the daily retention sweep.
    // A row that outlived its hour is answered exactly like a row that never
    // existed — same status, same wording, so the response cannot be used to
    // tell an expired share apart from an unknown id.
    let now = chrono::Utc::now().timestamp();
    let (_created_ts, remaining, _owner, content) = share_lifetime(&raw, now)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Share expired or not found"))?;

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
