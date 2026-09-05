// A second client, alongside `client.rs`, for the ONE class of operation the
// agent structurally cannot do: anything needing the panel's own database —
// site/database ownership, credentials, and (the reason this file exists) a
// restore that must load a database dump as well as replace files. `client.rs`
// talks to the agent over its local Unix socket with the agent's own token;
// this one talks to the panel API over HTTP with a separate, admin-minted key.
//
// See routes/api_keys.rs (backend) — a `dp_`-prefixed key, hashed the same way
// `agent.token` is, authenticated by auth.rs's `AuthUser` extractor exactly
// like a session JWT. On the common single-box install this key authorizes no
// more than `agent.token` already does (both cap at "everything on this
// machine"); on a multi-server fleet an admin-minted key reaches every server
// that admin registered, not only the one the CLI runs on — no narrower scope
// exists yet (`iac_tokens.scopes` is an unused column that could start one).

const API_ENV_PATH: &str = "/etc/dockpanel/api.env";
const TOKEN_PATH: &str = "/etc/dockpanel/backend.token";

pub fn load_token() -> Result<String, String> {
    let not_configured = || {
        format!(
            "No panel API key configured at {TOKEN_PATH}.\n\
             Mint one from the panel (Settings \u{2192} API Keys), then save it to \
             {TOKEN_PATH} (root:root, mode 600).\nAre you running as root?"
        )
    };
    let raw = std::fs::read_to_string(TOKEN_PATH).map_err(|_| not_configured())?;
    let token = raw.trim().to_string();
    // An empty (or whitespace-only) file reads as "present" to a bare
    // read_to_string — without this check a leftover file from a reverted
    // setup silently routes every restore into a panel-API call that 401s,
    // instead of falling back to the agent-only path like a genuinely
    // unconfigured install does.
    if token.is_empty() {
        return Err(not_configured());
    }
    Ok(token)
}

/// Resolve the panel API's base URL from the SAME `LISTEN_ADDR` its own
/// systemd unit's `EnvironmentFile=` feeds it — always in sync with what the
/// backend actually bound, no new operator config for the common single-box
/// install. This is deliberately NOT `BASE_URL` (also in api.env): that is the
/// panel's PUBLIC address behind nginx/Cloudflare, which the CLI running
/// locally on the box has no need to round-trip through.
fn base_url() -> Result<String, String> {
    let env = std::fs::read_to_string(API_ENV_PATH).map_err(|e| {
        format!("Cannot read {API_ENV_PATH}: {e}\nAre you running as root?")
    })?;
    let addr = env
        .lines()
        .find_map(|l| l.trim().strip_prefix("LISTEN_ADDR="))
        .ok_or_else(|| format!("{API_ENV_PATH} has no LISTEN_ADDR — cannot reach the panel API"))?
        .trim();
    Ok(format!("http://{addr}"))
}

async fn request(
    method: reqwest::Method,
    path: &str,
    body: Option<&serde_json::Value>,
    token: &str,
) -> Result<serde_json::Value, String> {
    let url = format!("{}{path}", base_url()?);
    let client = reqwest::Client::new();
    let mut req = client.request(method, &url).bearer_auth(token);
    if let Some(b) = body {
        req = req.json(b);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("Cannot reach the panel API at {url}: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        if let Some(msg) = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("error").and_then(|m| m.as_str()).map(str::to_string))
        {
            return Err(msg);
        }
        return Err(format!("Panel API returned {status}: {text}"));
    }

    if text.is_empty() {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_str(&text).map_err(|e| {
        let preview = &text[..text.len().min(200)];
        format!("Invalid JSON from panel API: {e}\nBody: {preview}")
    })
}

pub async fn get(path: &str, token: &str) -> Result<serde_json::Value, String> {
    request(reqwest::Method::GET, path, None, token).await
}

pub async fn post_empty(path: &str, token: &str) -> Result<serde_json::Value, String> {
    request(reqwest::Method::POST, path, None, token).await
}

/// Consume the panel's provisioning-log SSE stream (`routes/system.rs::install_log`,
/// shared by every async panel job — restores included) until it emits a `step
/// == "complete"` event or the connection ends, printing each step as it
/// arrives so a long restore isn't silent.
///
/// Pass/fail is decided ONLY by the terminal `step == "complete"` event's own
/// `status`, matching the panel UI's own rule
/// (`ProvisionLog.tsx`: `completeStep?.status === "error"`) — NOT by whether
/// any earlier step carried `status: "error"`. The restore handler
/// deliberately emits a non-fatal advisory with `status: "error"` for a
/// files-only archive restored onto a DB-attached site (it restores the files
/// and still finishes with `complete`/`"done"`); latching on any error step
/// would report that as a failure the app itself does not consider one.
///
/// Uses `Response::chunk()` rather than `bytes_stream()` so this file needs no
/// `futures`/`futures_util` dependency of its own — SSE framing here is exactly
/// what axum's `Event::default().data(json)` always produces: one `data: <json
/// with no embedded newlines>` line per event, frames separated by a blank
/// line. The keep-alive comment line (`: ping`) doesn't start with `data: ` and
/// is silently skipped, same as a real EventSource would. Buffers raw BYTES
/// (not a `String`) across chunks and only UTF-8-decodes a complete frame once
/// its terminating `\n\n` has actually arrived — chunk boundaries have no
/// obligation to land on a UTF-8 character boundary, and decoding each raw
/// chunk independently (as an earlier version of this function did) could
/// turn a multi-byte character split across two reads into a stray
/// replacement character in a step's printed message.
pub async fn follow_progress(install_id: &str, token: &str) -> Result<(), String> {
    let url = format!("{}/api/services/install/{install_id}/log", base_url()?);
    let client = reqwest::Client::new();
    let mut resp = client
        .get(&url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| format!("Cannot reach the panel API at {url}: {e}"))?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Panel API refused the progress stream: {text}"));
    }

    let mut buf: Vec<u8> = Vec::new();

    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("Progress stream error: {e}"))?
    {
        buf.extend_from_slice(&chunk);

        while let Some(pos) = buf.windows(2).position(|w| w == b"\n\n") {
            let frame: Vec<u8> = buf.drain(..pos + 2).collect();
            let frame = String::from_utf8_lossy(&frame);

            for line in frame.lines() {
                let Some(data) = line.strip_prefix("data: ") else { continue };
                let Ok(step) = serde_json::from_str::<serde_json::Value>(data) else { continue };

                let step_name = step["step"].as_str().unwrap_or("");
                let label = step["label"].as_str().unwrap_or("");
                let status = step["status"].as_str().unwrap_or("");
                let message = step["message"].as_str();

                let icon = match status {
                    "done" => "\x1b[32m\u{2713}\x1b[0m",
                    "error" => "\x1b[31m\u{2717}\x1b[0m",
                    _ => "\x1b[33m\u{2026}\x1b[0m",
                };
                println!("  {icon} {label}");
                if let Some(msg) = message {
                    println!("      {msg}");
                }

                if step_name == "complete" {
                    return if status == "error" {
                        Err("Restore failed \u{2014} see the messages above.".to_string())
                    } else {
                        Ok(())
                    };
                }
            }
        }
    }

    Err("Progress stream ended before the restore completed.".to_string())
}
