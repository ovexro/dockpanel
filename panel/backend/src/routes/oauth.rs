use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Redirect, Response},
};
use std::collections::HashMap;
use uuid::Uuid;

use crate::error::{internal_error, err, ApiError};
use crate::AppState;

#[derive(serde::Deserialize)]
pub struct CallbackQuery {
    pub code: Option<String>,
    pub state: String,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub error_description: Option<String>,
}

/// OAuth provider configuration
struct OAuthProvider {
    auth_url: &'static str,
    token_url: &'static str,
    userinfo_url: &'static str,
    scopes: &'static str,
}

fn get_provider(name: &str) -> Option<OAuthProvider> {
    match name {
        "google" => Some(OAuthProvider {
            auth_url: "https://accounts.google.com/o/oauth2/v2/auth",
            token_url: "https://oauth2.googleapis.com/token",
            userinfo_url: "https://www.googleapis.com/oauth2/v3/userinfo",
            scopes: "openid email profile",
        }),
        "github" => Some(OAuthProvider {
            auth_url: "https://github.com/login/oauth/authorize",
            token_url: "https://github.com/login/oauth/access_token",
            userinfo_url: "https://api.github.com/user",
            scopes: "user:email",
        }),
        "gitlab" => Some(OAuthProvider {
            auth_url: "https://gitlab.com/oauth/authorize",
            token_url: "https://gitlab.com/oauth/token",
            userinfo_url: "https://gitlab.com/api/v4/user",
            scopes: "read_user",
        }),
        _ => None,
    }
}

/// GET /api/auth/oauth/{provider} — Redirect to OAuth provider
pub async fn authorize(
    State(state): State<AppState>,
    Path(provider_name): Path<String>,
) -> Result<Response, ApiError> {
    let provider = get_provider(&provider_name)
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Unknown OAuth provider"))?;

    // Read client_id from settings
    let key = format!("oauth_{provider_name}_client_id");
    let client_id: Option<(String,)> = sqlx::query_as(
        "SELECT value FROM settings WHERE key = $1"
    )
    .bind(&key)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("authorize", e))?;

    let client_id = client_id
        .map(|(v,)| v)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, &format!("{provider_name} OAuth not configured")))?;

    // Generate CSRF state token
    let csrf_state = Uuid::new_v4().to_string();
    {
        let mut states = state.oauth_states.lock().unwrap_or_else(|e| e.into_inner());
        states.insert(csrf_state.clone(), (provider_name.clone(), std::time::Instant::now()));
    }

    let redirect_uri = format!("{}/api/auth/oauth/{provider_name}/callback", state.config.base_url);

    let auth_url = format!(
        "{}?client_id={}&redirect_uri={}&scope={}&state={}&response_type=code",
        provider.auth_url,
        urlencoding::encode(&client_id),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode(provider.scopes),
        urlencoding::encode(&csrf_state),
    );

    Ok(Redirect::temporary(&auth_url).into_response())
}

/// GET /api/auth/oauth/{provider}/callback — Handle OAuth callback
pub async fn callback(
    State(state): State<AppState>,
    Path(provider_name): Path<String>,
    headers: axum::http::HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> Result<Response, ApiError> {
    // This door mints the same session cookie as the password and passkey doors and
    // owed the same two gates. It had neither: the panel IP allowlist gates the
    // panel, and lockdown is supposed to hold non-admins out until it expires —
    // both were enforced at the other doors and skipped here, so with SSO
    // configured, restricting the panel by IP and putting it into lockdown each
    // left this path open.
    crate::routes::auth::enforce_panel_ip_allowlist(&state.db, &headers).await?;

    // Check for OAuth error response from provider
    if let Some(ref error) = query.error {
        let desc = query.error_description.as_deref().unwrap_or("Unknown error");
        return Err(err(StatusCode::BAD_REQUEST, &format!("OAuth error: {error} — {desc}")));
    }
    let code = query.code.as_ref()
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing authorization code"))?;

    let provider = get_provider(&provider_name)
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Unknown OAuth provider"))?;

    // Validate CSRF state
    {
        let mut states = state.oauth_states.lock().unwrap_or_else(|e| e.into_inner());
        let entry = states.remove(&query.state);
        match entry {
            Some((name, created)) if name == provider_name && created.elapsed().as_secs() < 600 => {}
            _ => return Err(err(StatusCode::BAD_REQUEST, "Invalid or expired OAuth state")),
        }
    }

    // Read client credentials
    let client_id_key = format!("oauth_{provider_name}_client_id");
    let client_secret_key = format!("oauth_{provider_name}_client_secret");

    let creds: Vec<(String, String)> = sqlx::query_as(
        "SELECT key, value FROM settings WHERE key IN ($1, $2)"
    )
    .bind(&client_id_key)
    .bind(&client_secret_key)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("callback", e))?;

    let cred_map: HashMap<String, String> = creds.into_iter().collect();
    let client_id = cred_map.get(&client_id_key).cloned().unwrap_or_default();
    let client_secret_enc = cred_map.get(&client_secret_key).cloned().unwrap_or_default();

    if client_id.is_empty() || client_secret_enc.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "OAuth not fully configured"));
    }

    // Decrypt the client secret (with legacy plaintext fallback)
    let client_secret = crate::services::secrets_crypto::decrypt_credential_or_legacy(
        &client_secret_enc, &state.config.jwt_secret,
    );

    let redirect_uri = format!("{}/api/auth/oauth/{provider_name}/callback", state.config.base_url);

    // Exchange code for token
    let http = reqwest::Client::new();
    let token_resp = http.post(provider.token_url)
        .header("Accept", "application/json")
        .form(&[
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Token exchange failed: {e}")))?;

    let token_data: serde_json::Value = token_resp.json().await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Token parse failed: {e}")))?;

    let access_token = token_data.get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| err(StatusCode::BAD_GATEWAY, "No access_token in OAuth response"))?;

    // Fetch user info
    let userinfo_resp = http.get(provider.userinfo_url)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("User-Agent", "DockPanel")
        .send()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Userinfo fetch failed: {e}")))?;

    let userinfo: serde_json::Value = userinfo_resp.json().await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Userinfo parse failed: {e}")))?;

    // Extract email based on provider
    let email = match provider_name.as_str() {
        "google" => userinfo.get("email").and_then(|v| v.as_str()).map(|s| s.to_string()),
        "github" => {
            // GitHub might not return email in profile if it's private
            let email_from_profile = userinfo.get("email").and_then(|v| v.as_str()).map(|s| s.to_string());
            if email_from_profile.is_some() && !email_from_profile.as_ref().unwrap().is_empty() {
                email_from_profile
            } else {
                // Fetch from /user/emails endpoint
                let emails_resp = http.get("https://api.github.com/user/emails")
                    .header("Authorization", format!("Bearer {access_token}"))
                    .header("User-Agent", "DockPanel")
                    .send().await.ok();
                if let Some(resp) = emails_resp {
                    let emails: Vec<serde_json::Value> = resp.json().await.unwrap_or_default();
                    emails.iter()
                        .find(|e| e.get("primary").and_then(|v| v.as_bool()).unwrap_or(false) && e.get("verified").and_then(|v| v.as_bool()).unwrap_or(false))
                        .and_then(|e| e.get("email").and_then(|v| v.as_str()).map(|s| s.to_string()))
                } else {
                    None
                }
            }
        }
        "gitlab" => userinfo.get("email").and_then(|v| v.as_str()).map(|s| s.to_string()),
        _ => None,
    };

    let email = email.ok_or_else(|| err(StatusCode::BAD_GATEWAY, "Could not retrieve email from OAuth provider"))?;
    let oauth_id = userinfo.get("id").map(|v| v.to_string()).unwrap_or_else(|| userinfo.get("sub").map(|v| v.to_string()).unwrap_or_default());

    if oauth_id.is_empty() {
        return Err(err(StatusCode::BAD_GATEWAY, "OAuth provider did not return a user ID"));
    }

    // Find or create user
    let user: Option<crate::models::User> = sqlx::query_as(
        "SELECT * FROM users WHERE email = $1"
    )
    .bind(&email)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("callback", e))?;

    let user = match user {
        Some(mut u) => {
            // Only auto-link if user has no password (OAuth-only) or already same provider
            if u.oauth_provider.is_none() && u.password_hash.is_empty() {
                sqlx::query("UPDATE users SET oauth_provider = $1, oauth_id = $2 WHERE id = $3")
                    .bind(&provider_name)
                    .bind(&oauth_id)
                    .bind(u.id)
                    .execute(&state.db)
                    .await
                    .ok();
                u.oauth_provider = Some(provider_name.clone());
            } else if u.oauth_provider.is_none() {
                // User has a password — don't auto-link, require manual linking
                return Err(err(StatusCode::CONFLICT,
                    "An account with this email exists. Log in with your password and link OAuth in Settings."));
            }
            u
        }
        None => {
            // Auto-create user.
            //
            // TWO gates, because there are two ways to open this door and until
            // s275 they disagreed with each other.
            //
            // `self_registration_enabled` (routes/auth.rs) governs password
            // registration and defaults CLOSED — an absent row means disabled.
            // `oauth_auto_create` governed this path and defaulted OPEN — an
            // absent row meant allowed. Both rows are absent on a fresh install,
            // so the panel's answer to "is registration open?" depended on which
            // door you knocked on. An operator who turned the visible
            // Self-Registration toggle off and later configured a provider had
            // self-registration silently back on, through a setting that had no
            // UI control at all.
            //
            // So an EXPLICIT `self_registration_enabled=false` now closes this
            // path too: off means off, whichever door. This deliberately reads
            // the explicit value rather than the effective one — an install that
            // never set the row keeps today's behaviour, so no existing OAuth
            // deployment breaks on upgrade; only operators who actually asked
            // for registration to be off get what they asked for.
            let self_reg: Option<(String,)> = sqlx::query_as(
                "SELECT value FROM settings WHERE key = 'self_registration_enabled'"
            )
            .fetch_optional(&state.db).await
                .map_err(|e| internal_error("self-registration setting", e))?;
            if self_reg.map(|(v,)| v == "false").unwrap_or(false) {
                return Err(err(StatusCode::FORBIDDEN,
                    "Registration is disabled. Contact your administrator."));
            }

            let auto_create: Option<(String,)> = sqlx::query_as(
                "SELECT value FROM settings WHERE key = 'oauth_auto_create'"
            )
            .fetch_optional(&state.db).await
                .map_err(|e| internal_error("oauth auto-create setting", e))?;
            let auto_create = auto_create.map(|(v,)| v != "false").unwrap_or(true);

            if !auto_create {
                return Err(err(StatusCode::FORBIDDEN, "OAuth auto-registration is disabled. Contact your administrator."));
            }

            let new_user: crate::models::User = sqlx::query_as(
                "INSERT INTO users (email, password_hash, role, email_verified, oauth_provider, oauth_id) \
                 VALUES ($1, '', 'user', true, $2, $3) RETURNING *"
            )
            .bind(&email)
            .bind(&provider_name)
            .bind(&oauth_id)
            .fetch_one(&state.db)
            .await
            .map_err(|e| {
                if e.to_string().contains("duplicate key") {
                    err(StatusCode::CONFLICT, "Email already registered")
                } else {
                    internal_error("callback", e)
                }
            })?;

            tracing::info!("OAuth user created: {} via {}", email, provider_name);
            new_user
        }
    };

    // Block suspended accounts on the OAuth login path too (parity with password and
    // passkey login) — otherwise suspension is bypassable by logging in via OAuth.
    if user.role == "suspended" {
        return Err(err(StatusCode::FORBIDDEN, "Account suspended"));
    }

    // Lockdown, same rule and same admin escape hatch as the other two doors: an
    // admin can always get in to lift it, everyone else waits. Checked here rather
    // than at the top of the handler because the exemption is per-user.
    if user.role != "admin"
        && crate::services::security_hardening::is_locked_down(&state.db).await
    {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "System is in lockdown mode"));
    }

    // If 2FA is enabled, issue a temporary token and redirect to 2FA challenge
    if user.totp_enabled {
        let now = chrono::Utc::now().timestamp() as usize;
        #[derive(serde::Serialize)]
        struct TwoFaClaims {
            sub: uuid::Uuid,
            purpose: String,
            exp: usize,
        }
        let temp_claims = TwoFaClaims {
            sub: user.id,
            purpose: "2fa".to_string(),
            exp: now + 300, // 5 minutes
        };
        let temp_token = jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
            &temp_claims,
            &jsonwebtoken::EncodingKey::from_secret(state.config.jwt_secret.as_bytes()),
        )
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("JWT encode failed: {e}")))?;

        crate::services::activity::log_activity(
            &state.db, user.id, &user.email, "auth.oauth_login_2fa_required",
            Some("user"), Some(&provider_name), None, None,
        ).await;

        // Redirect to frontend 2FA page with temp token
        let redirect_url = format!("/login?oauth_2fa={temp_token}");
        return Ok(Response::builder()
            .status(StatusCode::FOUND)
            .header(header::LOCATION, redirect_url)
            .body(axum::body::Body::empty())
            .unwrap()
            .into_response());
    }

    // Issue JWT session (no 2FA required)
    let now = chrono::Utc::now().timestamp() as usize;
    let jti = Uuid::new_v4().to_string();
    let claims = crate::auth::Claims {
        sub: user.id,
        email: user.email.clone(),
        role: user.role.clone(),
        iat: now,
        exp: now + 7200, // 2 hours
        jti: Some(jti.clone()),
    };

    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(state.config.jwt_secret.as_bytes()),
    )
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("JWT encode failed: {e}")))?;

    // Record the session so it is visible to session management AND — critically —
    // revocable: the DELETE user_sessions RETURNING jti -> blacklist sweeps (logout,
    // admin de-escalation, self-service reset) can only reach a session that has a row.
    let ua = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).map(|s| s.to_string());
    let ip = headers.get("x-real-ip").and_then(|v| v.to_str().ok()).map(|s| s.trim().to_string());
    let _ = sqlx::query(
        "INSERT INTO user_sessions (user_id, jti, ip_address, user_agent) VALUES ($1, $2, $3, $4)"
    )
    .bind(user.id)
    .bind(&jti)
    .bind(&ip)
    .bind(&ua)
    .execute(&state.db)
    .await;

    crate::services::activity::log_activity(
        &state.db, user.id, &user.email, "auth.oauth_login",
        Some("user"), Some(&provider_name), None, None,
    ).await;

    // Set cookie and redirect to dashboard.
    // Browsers reject Secure cookies on HTTP — only set Secure when the request
    // actually arrived over HTTPS (#47, v2.8.14; #71 — must NOT key off BASE_URL,
    // which is https for a domain even while the vhost is served over HTTP).
    let secure_flag = crate::routes::auth::cookie_secure_flag(&headers);
    let cookie = format!(
        "token={token}; HttpOnly{secure_flag}; SameSite=Lax; Path=/; Max-Age=7200"
    );

    Ok(Response::builder()
        .status(StatusCode::FOUND)
        .header(header::SET_COOKIE, cookie)
        .header(header::LOCATION, "/")
        .body(axum::body::Body::empty())
        .unwrap()
        .into_response())
}
