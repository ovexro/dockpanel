use axum::{
    extract::State,
    http::StatusCode,
    Json,
};
use std::collections::HashMap;

use crate::auth::{AdminUser, ServerScope};
use crate::error::{internal_error, err, agent_error, ApiError};
use crate::services::activity;
use crate::AppState;

#[derive(sqlx::FromRow)]
struct SettingRow {
    key: String,
    value: String,
}

/// The single writable-settings whitelist, shared by `update` and `import_config`.
/// It used to be spelled out separately in each, and the two had already drifted:
/// import's copy was missing the registration gates and every `security_*` toggle,
/// so exporting a config and importing it silently dropped your security posture
/// while its comment claimed it used "the same whitelist as update()".
///
/// A key belongs here when an operator is meant to change it. Every entry must
/// also have a control in the panel — `tests/settings-controls-pin-e2e.sh` §9
/// fails when one does not, and §1 fails when a key here is read by nothing.
///
/// The §9 half of that sentence was untrue from s276 until v2.48.0: the suite
/// computed the frontend's key list into a variable and never read it, so this
/// comment described a check that did not exist while fourteen keys — the OAuth
/// client credentials, the notification templates, the Stripe price IDs and
/// hide_branding — were settable only by hand-crafting a PUT.
pub const ALLOWED_KEYS: &[&str] = &[
    "panel_name", "smtp_host", "smtp_port", "smtp_username", "smtp_password",
    "smtp_from", "smtp_from_name", "smtp_encryption",
    "stripe_price_starter", "stripe_price_pro", "stripe_price_agency",
    "agent_auto_update_enabled",
    "pdns_api_url", "pdns_api_key",
    "auto_heal_enabled", "auto_heal_docker_reclaim", "status_page_enabled", "enforce_2fa",
    "logo_url", "accent_color",
    "oauth_google_client_id", "oauth_google_client_secret",
    "oauth_github_client_id", "oauth_github_client_secret",
    "oauth_gitlab_client_id", "oauth_gitlab_client_secret",
    "oauth_auto_create", "hide_branding",
    "reverse_proxy",
    // Gap #70: Customizable notification templates
    "notif_template_email", "notif_template_slack",
    "notif_template_discord", "notif_template_webhook",
    // Telemetry
    "telemetry_enabled", "telemetry_endpoint",
    // Registration + approval gates (read by routes/auth.rs)
    "self_registration_enabled", "security_approval_required",
    // Security toggles (read by services/security_hardening.rs)
    "security_geo_alert_enabled", "security_session_recording",
    "security_db_backup_enabled", "security_canary_enabled",
    "security_lockdown_threshold", "security_lockdown_window_minutes",
    "security_site_rate_limit",
    // Access gates that were readable-but-unsettable until v2.46.0: the login IP
    // allowlist (routes/auth.rs) and the server-terminal kill switch
    // (routes/terminal.rs). Both were documented or relied upon while no API
    // accepted them and no control existed.
    "allowed_panel_ips", "server_terminal_disabled",
    // The same class again, found in the v2.110.0 door census: the panel's own
    // public URL, read by services/notifications.rs to build the "Open runbook"
    // link every alert carries. It was read from this table, absent from this
    // list, and absent from the settings page — so the lookup fell through to a
    // BASE_URL env var and then to the empty string, and the payload builder
    // answers an empty base by omitting the link rather than by failing. Every
    // alert therefore shipped without its runbook link, on every install that
    // did not happen to set an environment variable no UI mentions, with no way
    // for an operator to notice or correct it.
    "base_url",
];

/// Settings keys masked in the GET response and encrypted at rest, alongside
/// every key ending `_client_secret`. Used to be declared separately, once
/// inside `update` and once inside `import_config` — the two happened to
/// still match, but nothing tied them together, the exact severed-pair shape
/// `services::credential_reencrypt`'s own header warns about. `pub(crate)` so
/// that module can also assert its `SENSITIVE_SETTINGS_SQL` predicate covers
/// every key here, rather than hand-duplicating this list a third time.
pub(crate) const SENSITIVE_KEYS: &[&str] = &["smtp_password", "pdns_api_key"];

/// GET /api/settings — Returns all settings as a key/value map (admin only).
pub async fn list(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<HashMap<String, String>>, ApiError> {

    let rows: Vec<SettingRow> = sqlx::query_as("SELECT key, value FROM settings")
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("list settings", e))?;

    let map: HashMap<String, String> = rows
        .into_iter()
        .map(|r| {
            if (r.key == "smtp_password" || r.key == "pdns_api_key"
                || r.key == "oauth_google_client_secret"
                || r.key == "oauth_github_client_secret"
                || r.key == "oauth_gitlab_client_secret") && !r.value.is_empty() {
                (r.key, "********".to_string())
            } else {
                (r.key, r.value)
            }
        })
        .collect();

    Ok(Json(map))
}

/// PUT /api/settings — Upsert settings from key/value map (admin only).
///
/// No `ServerScope`. Every key in `ALLOWED_KEYS` is panel-global — there is one
/// `settings` table and no per-server variant of any row in it — so the caller's
/// `X-Server-Id` header never had a bearing on what this handler writes. It did
/// have two effects, both wrong. It decided which single host received the SMTP
/// configuration below, including the decrypted password; and because
/// `ServerScope` resolves the agent eagerly and fails the request when it can't,
/// saving ANY setting — a panel rename, a security toggle — returned 502 whenever
/// the member selected in the server picker happened to be down.
pub async fn update(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Json(body): Json<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, ApiError> {

    for key in body.keys() {
        if !ALLOWED_KEYS.contains(&key.as_str()) {
            return Err(err(StatusCode::BAD_REQUEST, &format!("Unknown setting: {key}")));
        }
    }

    // Reject a malformed IP allowlist BEFORE storing it. Every entry must be one
    // `panel_ip_allowed` can match, or the operator locks themselves out of the
    // panel and the only way back in is editing the settings table by hand.
    if let Some(list) = body.get("allowed_panel_ips") {
        if let Some(bad) = list
            .split(',')
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .find(|e| !crate::helpers::valid_panel_ip_entry(e))
        {
            return Err(err(
                StatusCode::BAD_REQUEST,
                &format!("Invalid IP or CIDR in allowed_panel_ips: {bad}"),
            ));
        }
    }

    // Validate logo_url
    if let Some(url) = body.get("logo_url") {
        if !url.is_empty() && !url.starts_with("https://") && !url.starts_with("http://") && !url.starts_with("/") {
            return Err(err(StatusCode::BAD_REQUEST, "logo_url must be an HTTP(S) URL or relative path"));
        }
    }

    // Validate base_url. Unlike `logo_url` a relative value is useless here: the
    // reader concatenates this with a runbook path to build a link that leaves
    // the panel, in an email or a chat message, where nothing supplies an origin.
    // Empty stays legal and is the documented "no link" state, so an operator can
    // clear it deliberately.
    if let Some(url) = body.get("base_url") {
        let trimmed = url.trim();
        if !trimmed.is_empty()
            && !(trimmed.starts_with("https://") || trimmed.starts_with("http://"))
        {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "base_url must be an absolute HTTP(S) URL, e.g. https://panel.example.com",
            ));
        }
    }

    // Validate accent_color
    if let Some(color) = body.get("accent_color") {
        if !color.is_empty() {
            let valid = color.starts_with('#') && color.len() <= 9 && color[1..].chars().all(|c| c.is_ascii_hexdigit());
            let valid = valid || color.starts_with("rgb") || color.starts_with("hsl");
            if !valid {
                return Err(err(StatusCode::BAD_REQUEST, "accent_color must be a valid hex color (#rrggbb), rgb(), or hsl()"));
            }
        }
    }

    // Update all settings atomically in a transaction
    let mut tx = state.db.begin().await
        .map_err(|e| internal_error("update settings", e))?;

    // Sensitive keys that are masked in the GET response — skip if value is the mask sentinel.
    // `SENSITIVE_KEYS` is the module-level const above, not a local copy.
    for (key, value) in &body {
        // Don't overwrite real secrets with the mask placeholder
        if SENSITIVE_KEYS.contains(&key.as_str()) && value == "********" {
            continue;
        }
        if key.ends_with("_client_secret") && value == "********" {
            continue;
        }

        // Encrypt sensitive values before storing
        let store_value = if SENSITIVE_KEYS.contains(&key.as_str()) || key.ends_with("_client_secret") {
            if value.is_empty() {
                value.clone()
            } else {
                crate::services::secrets_crypto::encrypt_credential(value, &state.config.jwt_secret)
                    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Encryption failed: {e}")))?
            }
        } else {
            value.clone()
        };

        sqlx::query(
            "INSERT INTO settings (key, value, updated_at) VALUES ($1, $2, NOW()) \
             ON CONFLICT (key) DO UPDATE SET value = $2, updated_at = NOW()",
        )
        .bind(key)
        .bind(&store_value)
        .execute(&mut *tx)
        .await
        .map_err(|e| internal_error("update settings", e))?;
    }

    tx.commit().await
        .map_err(|e| internal_error("update settings", e))?;

    tracing::info!("Settings updated by {}: {} keys", claims.email, body.len());

    let mut resp = serde_json::json!({ "ok": true });

    // If SMTP keys were updated, push config to every host that sends mail
    let smtp_keys = ["smtp_host", "smtp_port", "smtp_username", "smtp_password", "smtp_from", "smtp_from_name", "smtp_encryption"];
    if body.keys().any(|k| smtp_keys.contains(&k.as_str())) {
        // Fetch all SMTP settings to send complete config
        let rows: Vec<SettingRow> = sqlx::query_as("SELECT key, value FROM settings WHERE key LIKE 'smtp_%'")
            .fetch_all(&state.db)
            .await
            .map_err(|e| internal_error("update settings", e))?;

        let map: HashMap<String, String> = rows.into_iter().map(|r| (r.key, r.value)).collect();

        let host = map.get("smtp_host").cloned().unwrap_or_default();
        if !host.is_empty() {
            let port_str = map.get("smtp_port").cloned().unwrap_or_else(|| "587".to_string());
            let port: u16 = port_str.parse().unwrap_or(587);

            // Decrypt smtp_password before sending to agent
            let smtp_password_raw = map.get("smtp_password").cloned().unwrap_or_default();
            let smtp_password = crate::services::secrets_crypto::decrypt_credential_or_legacy(
                &smtp_password_raw, &state.config.jwt_secret,
            );

            let agent_body = serde_json::json!({
                "host": host,
                "port": port,
                "username": map.get("smtp_username").cloned().unwrap_or_default(),
                "password": smtp_password,
                "from": map.get("smtp_from").cloned().unwrap_or_default(),
                "from_name": map.get("smtp_from_name").cloned().unwrap_or_else(|| "DockPanel".to_string()),
                "encryption": map.get("smtp_encryption").cloned().unwrap_or_else(|| "starttls".to_string()),
            });

            let push = push_smtp_to_fleet(&state, agent_body).await;
            if let Some(w) = push.warning() {
                tracing::warn!("{w}");
                resp["warning"] = serde_json::json!(w);
            }
            resp["smtp"] = push.to_json();
        }
    }

    Ok(Json(resp))
}

/// Where the SMTP configuration actually landed, host by host.
///
/// Three lists rather than a ratio, because the three outcomes are three
/// different problems and only a NAME is actionable. A host that refused the
/// config is broken now; a host that was never asked is invisible until its
/// first unsent mail; and a host that took it is the only one an operator may
/// assume anything about.
struct SmtpPush {
    /// Hosts that took the configuration.
    configured: Vec<String>,
    /// Hosts that were asked and refused, with the agent's reason.
    failed: Vec<(String, String)>,
    /// Hosts the registry would not resolve an agent for, with their status.
    not_asked: Vec<(String, String)>,
}

impl SmtpPush {
    fn total(&self) -> usize {
        self.configured.len() + self.failed.len() + self.not_asked.len()
    }

    /// One sentence for the operator, or `None` when every host took it.
    ///
    /// It says how many of how many, then names the rest. "Saved" on its own is
    /// the claim that has to stop being made: the write to `settings` always
    /// succeeds, and it is the only part of this operation the old response
    /// described.
    fn warning(&self) -> Option<String> {
        if self.failed.is_empty() && self.not_asked.is_empty() {
            return None;
        }
        let listed = |v: &[(String, String)]| {
            v.iter()
                .map(|(name, why)| format!("{name} ({why})"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let mut parts = Vec::new();
        if !self.failed.is_empty() {
            parts.push(format!("rejected by {}", listed(&self.failed)));
        }
        if !self.not_asked.is_empty() {
            parts.push(format!("never reached {}", listed(&self.not_asked)));
        }
        Some(format!(
            "SMTP settings saved, but they reached only {} of {} servers — {}. \
             Mail sent from the others will keep using whatever they were last given; \
             save SMTP again once they are back online.",
            self.configured.len(),
            self.total(),
            parts.join("; "),
        ))
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "configured": self.configured,
            "failed": self.failed.iter()
                .map(|(server, error)| serde_json::json!({ "server": server, "error": error }))
                .collect::<Vec<_>>(),
            "not_asked": self.not_asked.iter()
                .map(|(server, status)| serde_json::json!({ "server": server, "status": status }))
                .collect::<Vec<_>>(),
        })
    }
}

/// Push SMTP configuration at every member of the fleet.
///
/// The setting is panel-global, so its destination is too. This used to post to
/// the single agent `ServerScope` resolved from the caller's `X-Server-Id`
/// header, which produced both halves of the same defect: the DECRYPTED password
/// landed on whichever box the browser happened to have in its picker, and every
/// other member was never configured at all — so mail from those hosts failed for
/// ever, reported by nothing but a `tracing::warn!`. Same shape as
/// `mail::sync_mail_config`, which syncs Postfix maps to one header-chosen host.
///
/// ⚠ This is the panel's ONLY caller of the agent's `/smtp/configure`. Nothing
/// re-pushes on check-in or on server registration, so a member that is offline
/// at save time stays unconfigured until an operator saves SMTP again. That is
/// precisely why a host that could not be asked comes back as a named result
/// instead of being skipped quietly — the report is the only reconciliation there
/// is.
async fn push_smtp_to_fleet(state: &AppState, agent_body: serde_json::Value) -> SmtpPush {
    let fleet = state.agents.online_fleet().await;

    let mut push = SmtpPush {
        configured: Vec::new(),
        failed: Vec::new(),
        not_asked: Vec::new(),
    };

    for member in &fleet {
        match member.agent.post("/smtp/configure", Some(agent_body.clone())).await {
            Ok(_) => push.configured.push(member.name.clone()),
            Err(e) => {
                tracing::warn!(
                    "SMTP configure failed on {} ({}): {e}",
                    member.name,
                    member.id
                );
                push.failed.push((member.name.clone(), e.to_string()));
            }
        }
    }

    // `online_fleet` yields only servers that are online AND resolve to an agent;
    // it skips the rest rather than substituting, which is right for a background
    // loop and not enough here. An operator pressing Save is entitled to know that
    // a registered server was left out, so the ones it skipped are counted by
    // difference — no second predicate to drift away from the first.
    let asked: Vec<uuid::Uuid> = fleet.iter().map(|m| m.id).collect();
    push.not_asked = sqlx::query_as(
        "SELECT name, status FROM servers WHERE NOT (id = ANY($1)) ORDER BY name",
    )
    .bind(asked.as_slice())
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    push
}

/// POST /api/settings/smtp/test — Send a test email (admin only).
///
/// Deliberately single-host, unlike the save above, and that is a difference in
/// the QUESTION rather than an inconsistency. Saving is a fleet-wide change and
/// has to reach every host that will send mail; a test asks whether ONE host can
/// send, and fanning it out would put N copies of the same message in the
/// operator's inbox and take as long as the slowest box to answer. The caller's
/// selected server is the right subject for that question.
///
/// It is only defensible while the answer says which host it came from, so the
/// reply names it. An unattributed "Email sent" read as a claim about the panel
/// is how one box's success came to stand for the fleet's in the first place.
pub async fn test_email(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(server_id, agent): ServerScope,
    Json(body): Json<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, ApiError> {

    let to = body.get("to").cloned().unwrap_or_else(|| claims.email.clone());
    if to.is_empty() || !to.contains('@') {
        return Err(err(StatusCode::BAD_REQUEST, "Valid email address required"));
    }

    // Get stored from address
    let rows: Vec<SettingRow> = sqlx::query_as("SELECT key, value FROM settings WHERE key LIKE 'smtp_%'")
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("test email", e))?;

    let map: HashMap<String, String> = rows.into_iter().map(|r| (r.key, r.value)).collect();
    let from = map.get("smtp_from").cloned().unwrap_or_default();
    let from_name = map.get("smtp_from_name").cloned().unwrap_or_else(|| "DockPanel".to_string());

    if from.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "SMTP not configured — save SMTP settings first"));
    }

    let agent_body = serde_json::json!({
        "to": to,
        "from": from,
        "from_name": from_name,
    });

    // Resolved before the send so the answer can be attributed even when the row
    // has since gone; a missing name is left off rather than guessed, because
    // "sent from <the wrong host>" would be worse than saying nothing.
    let server_row: Option<(String,)> = sqlx::query_as("SELECT name FROM servers WHERE id = $1")
        .bind(server_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
    let server_name = server_row.map(|(n,)| n);

    let result = agent
        .post("/smtp/test", Some(agent_body))
        .await
        .map_err(|e| agent_error("SMTP test email", e))?;

    let message = result.get("message").and_then(|v| v.as_str()).unwrap_or("Email sent");
    let message = match &server_name {
        Some(name) => format!("{message} (sent from {name})"),
        None => message.to_string(),
    };

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "smtp.test",
        Some("settings"), None, Some(&to), None,
    ).await;

    Ok(Json(serde_json::json!({
        "ok": true,
        "message": message,
        "server": server_name,
    })))
}

/// POST /api/settings/test-webhook — Test Slack/Discord webhook
pub async fn test_webhook(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Json(body): Json<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let url = body.get("url").ok_or_else(|| err(StatusCode::BAD_REQUEST, "URL required"))?;
    let service = body.get("service").unwrap_or(&"webhook".to_string()).clone();

    if url.is_empty() || !url.starts_with("https://") {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid webhook URL"));
    }

    // SSRF: the destination is user-supplied and blind-followed; block internal
    // addresses (admin-triggered, but defense-in-depth + parity with the send path).
    //
    // Pinned to the address this check approves, via a fresh client built for
    // THIS request — a shared client's own resolver would otherwise look
    // `url`'s host up a second, independent time for the real connection,
    // reopening the validate/connect gap the check above exists to close.
    let (host, port) = crate::helpers::url_authority(url)
        .map_err(|e| err(StatusCode::BAD_REQUEST, &format!("Invalid webhook URL: {e}")))?;

    let payload = if service == "slack" {
        serde_json::json!({ "text": "DockPanel test notification — your Slack webhook is working!" })
    } else {
        serde_json::json!({ "content": "DockPanel test notification — your Discord webhook is working!" })
    };

    let client = crate::helpers::pinned_client(
        &host,
        port,
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            // Refuse redirects: a public destination that 3xx-redirects to an internal
            // address would otherwise bypass the SSRF allow-check above.
            .redirect(reqwest::redirect::Policy::none()),
    )
    .await
    .map_err(|e| err(StatusCode::BAD_REQUEST, &format!("Invalid webhook URL: {e}")))?;
    let resp = client.post(url).json(&payload).send().await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Webhook request failed: {e}")))?;

    if !resp.status().is_success() {
        return Err(err(StatusCode::BAD_GATEWAY, &format!("Webhook returned {}", resp.status())));
    }

    Ok(Json(serde_json::json!({ "ok": true, "message": format!("{} test sent", service) })))
}

/// GET /api/branding — Public branding configuration (for login page + authenticated users).
/// Returns reseller branding if user belongs to one, otherwise global settings.
pub async fn branding(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Load global branding settings
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT key, value FROM settings WHERE key IN ('panel_name', 'logo_url', 'accent_color', 'hide_branding')"
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("branding", e))?;

    let map: HashMap<String, String> = rows.into_iter().collect();

    let global_name = map.get("panel_name").cloned().unwrap_or_else(|| "DockPanel".into());
    let global_logo = map.get("logo_url").cloned().unwrap_or_default();
    let global_accent = map.get("accent_color").cloned().unwrap_or_default();
    let global_hide = map.get("hide_branding").map(|v| v == "true").unwrap_or(false);

    // GAP 41: Check if authenticated user belongs to a reseller with custom branding
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .or_else(|| {
            headers
                .get(axum::http::header::COOKIE)
                .and_then(|v| v.to_str().ok())
                .and_then(|cookies| {
                    cookies.split(';').find_map(|s| s.trim().strip_prefix("token="))
                })
        });

    if let Some(token) = token {
        // Try to decode JWT — ignore errors (unauthenticated users just get global branding)
        let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
        validation.validate_exp = true;
        validation.leeway = 0;
        if let Ok(data) = jsonwebtoken::decode::<crate::auth::Claims>(
            token,
            &jsonwebtoken::DecodingKey::from_secret(state.config.jwt_secret.as_bytes()),
            &validation,
        ) {
            // Look up user's reseller_id and reseller branding
            let reseller_branding: Option<(Option<String>, Option<String>, Option<String>, bool)> = sqlx::query_as(
                "SELECT rp.logo_url, rp.accent_color, rp.panel_name, rp.hide_branding \
                 FROM reseller_profiles rp \
                 JOIN users u ON u.reseller_id = rp.user_id \
                 WHERE u.id = $1"
            )
            .bind(data.claims.sub)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();

            if let Some((logo, accent, name, hide)) = reseller_branding {
                if logo.is_some() || accent.is_some() || name.is_some() || hide {
                    // Check which OAuth providers are configured
                    let oauth_rows: Vec<(String, String)> = sqlx::query_as(
                        "SELECT key, value FROM settings WHERE key LIKE 'oauth_%_client_id' AND value != ''"
                    )
                    .fetch_all(&state.db)
                    .await
                    .unwrap_or_default();

                    let oauth_providers: Vec<String> = oauth_rows.iter()
                        .filter_map(|(k, _)| {
                            k.strip_prefix("oauth_")
                                .and_then(|s| s.strip_suffix("_client_id"))
                                .map(|s| s.to_string())
                        })
                        .collect();

                    return Ok(Json(serde_json::json!({
                        "panel_name": name.unwrap_or(global_name),
                        "logo_url": logo.unwrap_or(global_logo),
                        "accent_color": accent.unwrap_or(global_accent),
                        "hide_branding": hide,
                        "oauth_providers": oauth_providers,
                    })));
                }
            }
        }
    }

    // Check which OAuth providers are configured
    let oauth_rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT key, value FROM settings WHERE key LIKE 'oauth_%_client_id' AND value != ''"
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let oauth_providers: Vec<String> = oauth_rows.iter()
        .filter_map(|(k, _)| {
            k.strip_prefix("oauth_")
                .and_then(|s| s.strip_suffix("_client_id"))
                .map(|s| s.to_string())
        })
        .collect();

    Ok(Json(serde_json::json!({
        "panel_name": global_name,
        "logo_url": global_logo,
        "accent_color": global_accent,
        "hide_branding": global_hide,
        "oauth_providers": oauth_providers,
    })))
}

/// GET /api/settings/export — Export all panel settings, alert rules, monitors,
/// backup schedules, and backup policies as JSON (Gap #71).
pub async fn export_config(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let rows: Vec<SettingRow> = sqlx::query_as("SELECT key, value FROM settings")
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("export config", e))?;

    let map: HashMap<String, String> = rows
        .into_iter()
        .filter(|r| r.key != "smtp_password" && r.key != "pdns_api_key"
            && !r.key.ends_with("_client_secret"))
        .map(|r| (r.key, r.value))
        .collect();

    // Gap #71: Export alert rules (user's own rules only, exclude webhook secrets)
    let alert_rule_rows = sqlx::query(
        "SELECT server_id, cpu_threshold, cpu_duration, memory_threshold, memory_duration, \
         disk_threshold, alert_cpu, alert_memory, alert_disk, alert_offline, \
         alert_backup_failure, alert_ssl_expiry, alert_service_health, \
         ssl_warning_days, notify_email, cooldown_minutes, muted_types, \
         gpu_util_threshold, gpu_util_duration, gpu_temp_threshold, gpu_vram_threshold, alert_gpu \
         FROM alert_rules WHERE user_id = $1 ORDER BY server_id NULLS FIRST"
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let alert_rules: Vec<serde_json::Value> = alert_rule_rows.iter().map(|r| {
        use sqlx::Row;
        serde_json::json!({
            "server_id": r.get::<Option<uuid::Uuid>, _>("server_id"),
            "cpu_threshold": r.get::<i32, _>("cpu_threshold"),
            "cpu_duration": r.get::<i32, _>("cpu_duration"),
            "memory_threshold": r.get::<i32, _>("memory_threshold"),
            "memory_duration": r.get::<i32, _>("memory_duration"),
            "disk_threshold": r.get::<i32, _>("disk_threshold"),
            "alert_cpu": r.get::<bool, _>("alert_cpu"),
            "alert_memory": r.get::<bool, _>("alert_memory"),
            "alert_disk": r.get::<bool, _>("alert_disk"),
            "alert_offline": r.get::<bool, _>("alert_offline"),
            "alert_backup_failure": r.get::<bool, _>("alert_backup_failure"),
            "alert_ssl_expiry": r.get::<bool, _>("alert_ssl_expiry"),
            "alert_service_health": r.get::<bool, _>("alert_service_health"),
            "ssl_warning_days": r.get::<String, _>("ssl_warning_days"),
            "notify_email": r.get::<bool, _>("notify_email"),
            "cooldown_minutes": r.get::<i32, _>("cooldown_minutes"),
            "muted_types": r.get::<String, _>("muted_types"),
            "gpu_util_threshold": r.get::<i32, _>("gpu_util_threshold"),
            "gpu_util_duration": r.get::<i32, _>("gpu_util_duration"),
            "gpu_temp_threshold": r.get::<i32, _>("gpu_temp_threshold"),
            "gpu_vram_threshold": r.get::<i32, _>("gpu_vram_threshold"),
            "alert_gpu": r.get::<bool, _>("alert_gpu"),
        })
    }).collect();

    // Gap #71: Export monitors (name, url, type, interval, keyword — no secrets)
    let monitor_rows = sqlx::query(
        "SELECT name, url, monitor_type, check_interval, keyword \
         FROM monitors WHERE user_id = $1 ORDER BY name"
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let monitors: Vec<serde_json::Value> = monitor_rows.iter().map(|r| {
        use sqlx::Row;
        serde_json::json!({
            "name": r.get::<String, _>("name"),
            "url": r.get::<String, _>("url"),
            "monitor_type": r.get::<String, _>("monitor_type"),
            "check_interval": r.get::<i32, _>("check_interval"),
            "keyword": r.get::<Option<String>, _>("keyword"),
        })
    }).collect();

    // Gap #71: Export backup schedules
    let schedule_rows = sqlx::query(
        "SELECT site_id, schedule, retention_count, enabled FROM backup_schedules"
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let schedules: Vec<serde_json::Value> = schedule_rows.iter().map(|r| {
        use sqlx::Row;
        serde_json::json!({
            "site_id": r.get::<uuid::Uuid, _>("site_id"),
            "schedule": r.get::<String, _>("schedule"),
            "retention_count": r.get::<i32, _>("retention_count"),
            "enabled": r.get::<bool, _>("enabled"),
        })
    }).collect();

    // Gap #71: Export backup policies
    let policy_rows = sqlx::query(
        "SELECT name, schedule, backup_sites, backup_databases, backup_volumes, \
         retention_count, encrypt, verify_after_backup \
         FROM backup_policies WHERE user_id = $1 ORDER BY name"
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let policies: Vec<serde_json::Value> = policy_rows.iter().map(|r| {
        use sqlx::Row;
        serde_json::json!({
            "name": r.get::<String, _>("name"),
            "schedule": r.get::<String, _>("schedule"),
            "backup_sites": r.get::<bool, _>("backup_sites"),
            "backup_databases": r.get::<bool, _>("backup_databases"),
            "backup_volumes": r.get::<bool, _>("backup_volumes"),
            "retention_count": r.get::<i32, _>("retention_count"),
            "encrypt": r.get::<bool, _>("encrypt"),
            "verify_after_backup": r.get::<bool, _>("verify_after_backup"),
        })
    }).collect();

    Ok(Json(serde_json::json!({
        "settings": map,
        "alert_rules": alert_rules,
        "monitors": monitors,
        "backup_schedules": schedules,
        "backup_policies": policies,
        "exported_at": chrono::Utc::now().to_rfc3339(),
    })))
}

/// POST /api/settings/import — Import panel settings from JSON.
pub async fn import_config(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let settings_obj = body.get("settings").and_then(|s| s.as_object())
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Invalid format: missing 'settings' object"))?;

    // Filter imported settings through the same whitelist as update() — literally
    // the same list now, not a second copy that drifts away from it.
    let allowed_keys = ALLOWED_KEYS;

    // `SENSITIVE_KEYS` is the module-level const above, not a local copy.
    let mut imported = 0;
    let mut skipped = 0;
    for (key, value) in settings_obj {
        if !allowed_keys.contains(&key.as_str()) {
            skipped += 1;
            continue; // Skip disallowed keys
        }
        if let Some(val) = value.as_str() {
            // Encrypt sensitive values before storing (same logic as update())
            let store_value = if SENSITIVE_KEYS.contains(&key.as_str()) || key.ends_with("_client_secret") {
                if val.is_empty() {
                    val.to_string()
                } else {
                    crate::services::secrets_crypto::encrypt_credential(val, &state.config.jwt_secret)
                        .unwrap_or_else(|_| val.to_string())
                }
            } else {
                val.to_string()
            };

            sqlx::query(
                "INSERT INTO settings (key, value, updated_at) VALUES ($1, $2, NOW()) \
                 ON CONFLICT (key) DO UPDATE SET value = $2, updated_at = NOW()",
            )
            .bind(key)
            .bind(&store_value)
            .execute(&state.db)
            .await
            .ok();
            imported += 1;
        }
    }

    // Import alert rules
    let mut alert_rules_imported = 0i64;
    let mut muted_types_dropped = 0i64;
    if let Some(rules) = body.get("alert_rules").and_then(|v| v.as_array()) {
        for rule in rules {
            let cpu_threshold = rule.get("cpu_threshold").and_then(|v| v.as_i64()).unwrap_or(90) as i32;
            let cpu_duration = rule.get("cpu_duration").and_then(|v| v.as_i64()).unwrap_or(5) as i32;
            let mem_threshold = rule.get("memory_threshold").and_then(|v| v.as_i64()).unwrap_or(90) as i32;
            let mem_duration = rule.get("memory_duration").and_then(|v| v.as_i64()).unwrap_or(5) as i32;
            let disk_threshold = rule.get("disk_threshold").and_then(|v| v.as_i64()).unwrap_or(90) as i32;
            let alert_cpu = rule.get("alert_cpu").and_then(|v| v.as_bool()).unwrap_or(true);
            let alert_memory = rule.get("alert_memory").and_then(|v| v.as_bool()).unwrap_or(true);
            let alert_disk = rule.get("alert_disk").and_then(|v| v.as_bool()).unwrap_or(true);
            let alert_offline = rule.get("alert_offline").and_then(|v| v.as_bool()).unwrap_or(true);
            // ⚠ `true`, matching the schema. `alerting_system.sql` declares all
            // three `NOT NULL DEFAULT TRUE`, and the four fields above already
            // default the way the schema does — these three defaulted FALSE, so a
            // config written by an older version, hand-edited, or produced by
            // anything but this build's own export silently switched off backup,
            // SSL-expiry and service-health alerting on import. Nothing said so:
            // there is no UI widget for these columns, and `alert_ssl_expiry` off
            // also consumed the expiry warning ladder's rungs (see
            // `try_fire_alert`), so the effect outlived the import.
            let alert_backup_failure = rule.get("alert_backup_failure").and_then(|v| v.as_bool()).unwrap_or(true);
            let alert_ssl_expiry = rule.get("alert_ssl_expiry").and_then(|v| v.as_bool()).unwrap_or(true);
            let alert_service_health = rule.get("alert_service_health").and_then(|v| v.as_bool()).unwrap_or(true);
            let ssl_warning_days = rule.get("ssl_warning_days").and_then(|v| v.as_str()).unwrap_or("14");
            let notify_email = rule.get("notify_email").and_then(|v| v.as_bool()).unwrap_or(true);
            let cooldown = rule.get("cooldown_minutes").and_then(|v| v.as_i64()).unwrap_or(15) as i32;
            // A restored suppression list can name a type this version no longer
            // pages about. Kept verbatim it would suppress nothing while reading
            // back as though it worked, so drop the dead tokens and count them.
            // The live edit path rejects instead; here the convention this
            // handler already states for monitors applies — salvage the row
            // rather than fail the whole restore over one stale token.
            let muted_raw = rule.get("muted_types").and_then(|v| v.as_str()).unwrap_or("");
            let dead = crate::services::notifications::unknown_suppressible_types(muted_raw);
            muted_types_dropped += dead.len() as i64;
            let muted_owned: String = muted_raw
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .filter(|s| !dead.iter().any(|d| d == s))
                .collect::<Vec<_>>()
                .join(",");
            let muted_types = muted_owned.as_str();

            // Upsert: if server_id is null, update the global (server_id IS NULL) rule
            let server_id: Option<uuid::Uuid> = rule
                .get("server_id")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok());

            let result = if let Some(sid) = server_id {
                sqlx::query(
                    "INSERT INTO alert_rules (user_id, server_id, cpu_threshold, cpu_duration, \
                     memory_threshold, memory_duration, disk_threshold, alert_cpu, alert_memory, \
                     alert_disk, alert_offline, alert_backup_failure, alert_ssl_expiry, \
                     alert_service_health, ssl_warning_days, notify_email, cooldown_minutes, muted_types) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18) \
                     ON CONFLICT (user_id, server_id) DO UPDATE SET \
                     cpu_threshold=$3, cpu_duration=$4, memory_threshold=$5, memory_duration=$6, \
                     disk_threshold=$7, alert_cpu=$8, alert_memory=$9, alert_disk=$10, \
                     alert_offline=$11, alert_backup_failure=$12, alert_ssl_expiry=$13, \
                     alert_service_health=$14, ssl_warning_days=$15, notify_email=$16, \
                     cooldown_minutes=$17, muted_types=$18"
                )
                .bind(claims.sub).bind(sid)
                .bind(cpu_threshold).bind(cpu_duration)
                .bind(mem_threshold).bind(mem_duration)
                .bind(disk_threshold)
                .bind(alert_cpu).bind(alert_memory).bind(alert_disk)
                .bind(alert_offline).bind(alert_backup_failure)
                .bind(alert_ssl_expiry).bind(alert_service_health)
                .bind(ssl_warning_days).bind(notify_email)
                .bind(cooldown).bind(muted_types)
                .execute(&state.db).await
            } else {
                sqlx::query(
                    "INSERT INTO alert_rules (user_id, cpu_threshold, cpu_duration, \
                     memory_threshold, memory_duration, disk_threshold, alert_cpu, alert_memory, \
                     alert_disk, alert_offline, alert_backup_failure, alert_ssl_expiry, \
                     alert_service_health, ssl_warning_days, notify_email, cooldown_minutes, muted_types) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17) \
                     ON CONFLICT DO NOTHING"
                )
                .bind(claims.sub)
                .bind(cpu_threshold).bind(cpu_duration)
                .bind(mem_threshold).bind(mem_duration)
                .bind(disk_threshold)
                .bind(alert_cpu).bind(alert_memory).bind(alert_disk)
                .bind(alert_offline).bind(alert_backup_failure)
                .bind(alert_ssl_expiry).bind(alert_service_health)
                .bind(ssl_warning_days).bind(notify_email)
                .bind(cooldown).bind(muted_types)
                .execute(&state.db).await
            };
            if result.is_ok() {
                alert_rules_imported += 1;
            }
        }
    }

    // Import monitors
    let mut monitors_imported = 0i64;
    if let Some(monitors) = body.get("monitors").and_then(|v| v.as_array()) {
        for m in monitors {
            let name = m.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let url = m.get("url").and_then(|v| v.as_str()).unwrap_or("");
            let monitor_type = m.get("monitor_type").and_then(|v| v.as_str()).unwrap_or("http");
            let interval = m.get("check_interval").and_then(|v| v.as_i64()).unwrap_or(60) as i32;
            let keyword = m.get("keyword").and_then(|v| v.as_str());

            if !name.is_empty() && !url.is_empty() {
                // SSRF: validate the URL for any monitor type that will be HTTP-fetched by
                // the uptime dispatcher (anything except tcp/ping/heartbeat — see
                // uptime.rs check dispatcher's default arm), parity with the create path
                // which the import loop otherwise bypasses. Skip an offending row rather
                // than failing the whole import. (check_http also re-validates at run time.)
                if !matches!(monitor_type, "tcp" | "ping" | "heartbeat")
                    && crate::helpers::validate_url_not_internal(url).await.is_err()
                {
                    continue;
                }
                let result = sqlx::query(
                    "INSERT INTO monitors (user_id, name, url, monitor_type, check_interval, keyword) \
                     VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
                )
                .bind(claims.sub)
                .bind(name)
                .bind(url)
                .bind(monitor_type)
                .bind(interval)
                .bind(keyword)
                .execute(&state.db)
                .await;
                if result.is_ok() {
                    monitors_imported += 1;
                }
            }
        }
    }

    // Import backup schedules
    let mut schedules_imported = 0i64;
    if let Some(schedules) = body.get("backup_schedules").and_then(|v| v.as_array()) {
        for s in schedules {
            let site_id: Option<uuid::Uuid> = s
                .get("site_id")
                .and_then(|v| v.as_str())
                .and_then(|v| v.parse().ok());
            let schedule = s.get("schedule").and_then(|v| v.as_str()).unwrap_or("0 2 * * *");
            let retention = s.get("retention_count").and_then(|v| v.as_i64()).unwrap_or(7) as i32;
            let enabled = s.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);

            if let Some(sid) = site_id {
                let result = sqlx::query(
                    "INSERT INTO backup_schedules (site_id, schedule, retention_count, enabled) \
                     VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
                )
                .bind(sid)
                .bind(schedule)
                .bind(retention)
                .bind(enabled)
                .execute(&state.db)
                .await;
                if result.is_ok() {
                    schedules_imported += 1;
                }
            }
        }
    }

    // Import backup policies
    let mut policies_imported = 0i64;
    if let Some(policies) = body.get("backup_policies").and_then(|v| v.as_array()) {
        for p in policies {
            let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let schedule = p.get("schedule").and_then(|v| v.as_str()).unwrap_or("0 2 * * *");
            let backup_sites = p.get("backup_sites").and_then(|v| v.as_bool()).unwrap_or(true);
            let backup_databases = p.get("backup_databases").and_then(|v| v.as_bool()).unwrap_or(true);
            let backup_volumes = p.get("backup_volumes").and_then(|v| v.as_bool()).unwrap_or(false);
            let retention = p.get("retention_count").and_then(|v| v.as_i64()).unwrap_or(7) as i32;
            let encrypt = p.get("encrypt").and_then(|v| v.as_bool()).unwrap_or(false);
            let verify = p.get("verify_after_backup").and_then(|v| v.as_bool()).unwrap_or(false);

            if !name.is_empty() {
                let result = sqlx::query(
                    "INSERT INTO backup_policies (user_id, name, schedule, backup_sites, backup_databases, \
                     backup_volumes, retention_count, encrypt, verify_after_backup) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) ON CONFLICT DO NOTHING",
                )
                .bind(claims.sub)
                .bind(name)
                .bind(schedule)
                .bind(backup_sites)
                .bind(backup_databases)
                .bind(backup_volumes)
                .bind(retention)
                .bind(encrypt)
                .bind(verify)
                .execute(&state.db)
                .await;
                if result.is_ok() {
                    policies_imported += 1;
                }
            }
        }
    }

    crate::services::activity::log_activity(
        &state.db, claims.sub, &claims.email, "settings.import",
        Some("settings"), None, None, None,
    ).await;

    Ok(Json(serde_json::json!({
        "ok": true,
        "imported": imported,
        "skipped": skipped,
        "alert_rules_imported": alert_rules_imported,
        "muted_types_dropped": muted_types_dropped,
        "monitors_imported": monitors_imported,
        "schedules_imported": schedules_imported,
        "policies_imported": policies_imported,
    })))
}

/// GET /api/settings/health — System health check (admin only).
/// The agent release in which the terminal-recording toggle began to be honoured.
/// Before it, the agent opened a `.cast` for every session unconditionally and the
/// panel had no way to tell it otherwise.
const RECORDING_GATE_MIN_AGENT: &str = "2.46.0";

/// GET /api/settings/recording-coverage — which registered servers will actually
/// obey the terminal session-recording toggle.
///
/// The toggle is one row in `settings`, but it is enforced by each server's agent:
/// the decision rides as a signed claim in the terminal ticket, and an agent older
/// than 2.46.0 does not read that claim and keeps recording. So switching recording
/// off is a fleet-wide *claim* that is false for any member still behind — and the
/// panel already knows, because every check-in writes `servers.agent_version`.
///
/// Reporting "disabled" while a member still records is the same defect class the
/// toggle itself had before 2.46.0: a control whose confirmation ends the
/// investigation. This lets the UI say which servers are not covered instead.
pub async fn recording_coverage(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let rows: Vec<(String, Option<String>, bool)> = sqlx::query_as(
        "SELECT name, agent_version, is_local FROM servers ORDER BY is_local DESC, name"
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("recording coverage", e))?;

    // `servers.agent_version` is written by the check-in a REMOTE agent makes. The
    // local agent never checks in — it is reached over a unix socket — so its row
    // is permanently NULL. Reporting that as "unknown, assume lagging" would put a
    // warning on every single-server install, which is most of them, and a warning
    // that is always on is one operators learn to scroll past. So ask it: /health
    // is the only answer that describes what is actually running (s271 shipped a
    // stale local agent that every other read reported as current).
    let local_version: Option<String> = match agent.get("/health").await {
        Ok(v) => v.get("version").and_then(|s| s.as_str()).map(str::to_string),
        Err(_) => None,
    };

    let min = crate::services::panel_update::semver_key(Some(RECORDING_GATE_MIN_AGENT));
    let lagging: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(name, v, is_local)| {
            let v = if is_local { local_version.clone().or(v) } else { v };
            (name, v)
        })
        // A remote server that has never checked in still counts as lagging: an
        // unverified claim is exactly what this endpoint exists to stop making.
        .filter(|(_, v)| crate::services::panel_update::semver_key(v.as_deref()) < min)
        .map(|(name, v)| serde_json::json!({
            "name": name,
            "agent_version": v.unwrap_or_else(|| "unknown".to_string()),
        }))
        .collect();

    Ok(Json(serde_json::json!({
        "min_agent_version": RECORDING_GATE_MIN_AGENT,
        "lagging": lagging,
    })))
}

pub async fn health(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {

    // Check DB
    let db_status = match sqlx::query("SELECT 1").execute(&state.db).await {
        Ok(_) => "ok",
        Err(_) => "error",
    };

    // Check agent connectivity
    let agent_status = match agent.get("/health").await {
        Ok(_) => "ok",
        Err(_) => "error",
    };

    // System uptime from /proc/uptime
    let uptime = match tokio::fs::read_to_string("/proc/uptime").await {
        Ok(contents) => {
            let secs: f64 = contents
                .split_whitespace()
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0);
            let days = (secs / 86400.0) as u64;
            let hours = ((secs % 86400.0) / 3600.0) as u64;
            let minutes = ((secs % 3600.0) / 60.0) as u64;
            if days > 0 {
                format!("{days} days, {hours}h {minutes}m")
            } else {
                format!("{hours}h {minutes}m")
            }
        }
        Err(_) => "unknown".to_string(),
    };

    Ok(Json(serde_json::json!({
        "db": db_status,
        "agent": agent_status,
        "uptime": uptime,
    })))
}

/// POST /api/settings/credentials/reencrypt — rewrite every stored credential
/// under the current primary encryption key.
///
/// The operator-facing half of the `SECRETS_ENCRYPTION_KEY` story. Since
/// v2.112.0 the decrypt chain tries every derivation a value could have been
/// written under, so changing that variable is survivable — but until the rows
/// are rewritten the install is leaning on a fallback arm and the old
/// derivation can never be retired. This endpoint does the rewriting.
///
/// Idempotent and safe to re-run: a row already under the primary key is
/// counted and skipped, and a row no candidate key can open is reported as
/// `unreadable` and **left exactly as it is** — overwriting the only copy of a
/// ciphertext we cannot read would turn a recoverable state into a permanent
/// one, which is the failure this whole change exists to prevent.
pub async fn reencrypt_credentials(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let reports =
        crate::services::credential_reencrypt::reencrypt_all(&state.db, &state.config.jwt_secret)
            .await;

    let rewritten: i64 = reports.iter().map(|r| r.rewritten).sum();
    let unreadable: i64 = reports.iter().map(|r| r.unreadable).sum();
    let examined: i64 = reports.iter().map(|r| r.examined).sum();
    let raced: i64 = reports.iter().map(|r| r.raced).sum();

    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "settings.credentials.reencrypt",
        Some("settings"),
        None,
        Some(&format!(
            "examined {examined}, rewritten {rewritten}, unreadable {unreadable}, raced {raced}"
        )),
        None,
    )
    .await;

    if unreadable > 0 {
        tracing::error!(
            "credential re-encryption finished with {unreadable} unreadable value(s) — those rows \
             were left untouched. They were encrypted with a key this process cannot derive; \
             restoring the previous {} is the way back.",
            crate::services::secrets_crypto::ENCRYPTION_KEY_ENV
        );
    }
    if raced > 0 {
        // Not an error: a normal write beat the sweep to a row and the CAS
        // guard correctly skipped it rather than clobbering the newer value.
        // Re-running the sweep re-keys it once nothing races it.
        tracing::warn!(
            "credential re-encryption skipped {raced} row(s) that changed concurrently — \
             run it again to pick them up"
        );
    }

    Ok(Json(serde_json::json!({
        "examined": examined,
        "rewritten": rewritten,
        "unreadable": unreadable,
        "raced": raced,
        "subjects": reports,
        // Which surfaces this sweep claims to cover, so the answer to "did it
        // touch my X?" is in the response rather than in someone's memory.
        "covered_modules": crate::services::credential_reencrypt::covered_module_names(),
        "swept_subjects": crate::services::credential_reencrypt::swept_subjects(),
    })))
}
