use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    Json,
};
use std::time::Instant;
use jsonwebtoken::{encode, decode, EncodingKey, DecodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};
use subtle::ConstantTimeEq;

use argon2::{
    password_hash::{rand_core::OsRng, SaltString},
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
};
use totp_rs::{Algorithm, TOTP, Secret};

use crate::auth::{AuthUser, Claims};
use crate::error::{internal_error, err, ApiError};

/// A real Argon2 PHC string verified against on the paths where there is no
/// stored hash to verify against, so that "no such account" and "this account
/// signs in with an identity provider" cost what a real verification costs.
///
/// Module-level rather than a local so a test can assert it actually parses —
/// the branches that use it call `.expect()`, and the whole point of those
/// branches is that they must not be the ones that fail.
const DUMMY_PASSWORD_HASH: &str =
    "$argon2id$v=19$m=19456,t=2,p=1$ZHVtbXlzYWx0MTIzNA$K1PqGlDJpiBFSguVJXKDBIuXQ5baiAOXSgWAGkuJYxk";
use crate::models::User;
use crate::services::{activity, email, notifications, security_hardening};
use crate::AppState;

/// Generate a secure random token and its SHA-256 hash.
fn generate_token() -> (String, String) {
    let token = uuid::Uuid::new_v4().to_string().replace('-', "");
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let hash = hex::encode(hasher.finalize());
    (token, hash)
}

/// Hash a token with SHA-256 for DB comparison.
pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

#[derive(Deserialize)]
pub struct SetupRequest {
    pub email: String,
    pub password: String,
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

/// GET /api/auth/setup-status — Check if setup is needed (no users exist).
pub async fn setup_status(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(&state.db)
        .await
        .map_err(|e| internal_error("check setup status", e))?;

    Ok(Json(serde_json::json!({ "needs_setup": count.0 == 0 })))
}

/// POST /api/auth/setup — Create the initial admin user. Only works when no users exist.
pub async fn setup(
    State(state): State<AppState>,
    Json(body): Json<SetupRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    // Fast path: reject if setup already completed (avoids expensive Argon2 hash)
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(&state.db)
        .await
        .map_err(|e| internal_error("initial setup", e))?;

    if count.0 > 0 {
        return Err(err(StatusCode::FORBIDDEN, "Setup already completed"));
    }

    // Validate input
    if body.email.is_empty() || body.email.len() > 254 || !body.email.contains('@') {
        return Err(err(StatusCode::BAD_REQUEST, "Valid email address is required"));
    }

    if body.password.len() < 8 {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Password must be at least 8 characters",
        ));
    }

    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(body.password.as_bytes(), &salt)
        .map_err(|e| internal_error("initial setup", e))?
        .to_string();

    // Atomic check-and-insert to prevent TOCTOU race.
    //
    // `email_verified` is TRUE here, and that is the whole difference between this
    // door and `register`. This is the bootstrap door: it runs once, only while the
    // table is empty, for the person who already has the box. It issues no
    // `email_token` and sends no mail — so unlike a registration, there is no
    // ceremony to complete and nobody to complete it against.
    //
    // Leaving it FALSE made the first administrator of every install a latent
    // lockout. `login` (:268) refuses an unverified account whenever `smtp_host` is
    // merely NON-EMPTY, so the gate armed the moment the operator configured mail —
    // against their own account — and BOTH exits needed the mail that was being
    // configured. The token door (:833) matches `WHERE email_token = $1`, and this
    // user has no token at all, so that door does not exist for them; the only other
    // writer is `reset_password`, which needs the forgot-password message. Reported
    // as #100 by an operator who reinstalled to get back in.
    let user: Option<User> = sqlx::query_as(
        "INSERT INTO users (email, password_hash, role, email_verified) \
         SELECT $1, $2, 'admin', TRUE \
         WHERE NOT EXISTS (SELECT 1 FROM users) \
         RETURNING *",
    )
    .bind(&body.email)
    .bind(&hash)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("initial setup", e))?;

    let user = user.ok_or_else(|| err(StatusCode::FORBIDDEN, "Setup already completed"))?;

    tracing::info!("Initial admin created: {}", user.email);

    // Now that we have an admin user, register the local server (deferred from startup)
    if state.agents.local_server_id().await.is_none() {
        let local_id = crate::services::agent::ensure_local_server(
            &state.db,
            &state.config.agent_token,
        )
        .await;
        if !local_id.is_nil() {
            state.agents.set_local_server_id(local_id).await;
            tracing::info!("Local server registered after setup: {local_id}");
        }
    }

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": user.id,
            "email": user.email,
            "role": user.role,
        })),
    ))
}

/// POST /api/auth/login — Authenticate and return JWT in HttpOnly cookie.
pub async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<LoginRequest>,
) -> Result<(StatusCode, [(header::HeaderName, String); 1], Json<serde_json::Value>), ApiError> {
    // Extract client IP for rate limiting.
    // Use X-Real-IP (set by nginx from $remote_addr — trustworthy) and fall back
    // to the peer address.  Do NOT trust X-Forwarded-For as it can be forged by
    // the client, allowing rate-limit bypass.
    let ip = headers
        .get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // Rate limit: max 5 attempts per 15 minutes
    {
        let mut attempts = state.login_attempts.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let entry = attempts.entry(ip.clone()).or_default();
        entry.retain(|t| now.duration_since(*t).as_secs() < 900);
        if entry.len() >= 5 {
            return Err(err(
                StatusCode::TOO_MANY_REQUESTS,
                "Too many login attempts. Try again in 15 minutes.",
            ));
        }
    }

    // GAP 68: IP whitelist check — block login from non-whitelisted IPs
    enforce_panel_ip_allowlist(&state.db, &headers).await?;

    let user_opt: Option<User> = sqlx::query_as("SELECT * FROM users WHERE email = $1")
        .bind(&body.email)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("login", e))?;

    // Constant-time: always run Argon2 verify, even for non-existent users (prevents timing attack)
    let dummy_hash = DUMMY_PASSWORD_HASH;
    let user = match user_opt {
        Some(u) => {
            // An account created through an identity provider has no password
            // at all — `oauth.rs` stores the empty string — and an empty string
            // is not a PHC string, so parsing it fails. Answering that with a
            // 500 made this door an oracle: a non-existent address and a
            // password account both answered 401, and the ONE class that
            // answered anything else was "this address exists and signs in with
            // SSO". Worse, the `?` returned above the two lines below it, so
            // the probe was neither counted against the rate limit nor written
            // to the activity log the login-audit surface reads — an
            // unauthenticated, unthrottled, unlogged enumeration channel.
            //
            // Having no password is failing to present one, so it is answered
            // exactly like a wrong password, and at the same cost: the dummy
            // verify runs first, or the branch would return in microseconds
            // where every other outcome takes an Argon2 hash, and the status
            // oracle would simply become a timing oracle.
            let verified = if u.password_hash.is_empty() {
                let dummy = PasswordHash::new(dummy_hash)
                    .expect("the compiled-in dummy hash is a valid PHC string");
                let _ = Argon2::default().verify_password(body.password.as_bytes(), &dummy);
                false
            } else {
                let parsed = PasswordHash::new(&u.password_hash)
                    .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Password hash error"))?;
                Argon2::default()
                    .verify_password(body.password.as_bytes(), &parsed)
                    .is_ok()
            };

            if !verified {
                record_login_attempt(&state.login_attempts, &ip);
                // Log failed login attempt (never log password)
                activity::log_activity(
                    &state.db, u.id, &u.email, "auth.login_failed",
                    None, None, None, Some(&ip),
                ).await;
                return Err(err(StatusCode::UNAUTHORIZED, "Invalid credentials"));
            }
            u
        }
        None => {
            // Run dummy verify to equalize timing, then fail
            let parsed = PasswordHash::new(dummy_hash).unwrap();
            let _ = Argon2::default().verify_password(body.password.as_bytes(), &parsed);
            record_login_attempt(&state.login_attempts, &ip);
            // Log failed login with email only (no user ID available).
            //
            // This is the branch that matters for detection and it is the one that
            // was never recorded. It used to pass the nil uuid, which
            // `fk_activity_logs_user` rejects, so the insert failed and the warning
            // was swallowed — for every release this endpoint has existed. The
            // wrong-password-for-a-real-account branch above names a real user and
            // therefore landed, so the panel recorded the one failure mode that is
            // NOT enumeration and dropped the one that is: credential stuffing and
            // username probing are, by definition, attempts against emails that do
            // not exist. `GET /api/security/login-audit` could not show them
            // because there was nothing to show.
            activity::log_activity_system(
                &state.db, &body.email, "auth.login_failed",
                None, None, Some("unknown_user"), Some(&ip), None,
            ).await;
            return Err(err(StatusCode::UNAUTHORIZED, "Invalid credentials"));
        }
    };

    // Success — clear rate limit for this IP
    {
        let mut attempts = state.login_attempts.lock().unwrap_or_else(|e| e.into_inner());
        attempts.remove(&ip);
    }

    // Block login for suspended accounts (parity with the passkey path). Without this,
    // suspension is bypassable by simply logging in again for a fresh token.
    if user.role == "suspended" {
        return Err(err(StatusCode::FORBIDDEN, "Account suspended"));
    }

    // Block login if email not verified and SMTP is configured
    if !user.email_verified {
        let smtp_configured = {
            let row: Option<(String,)> = sqlx::query_as(
                "SELECT value FROM settings WHERE key = 'smtp_host'",
            )
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
            row.map(|r| !r.0.is_empty()).unwrap_or(false)
        };
        if smtp_configured {
            return Err(err(StatusCode::FORBIDDEN, "Email not verified. Check your inbox."));
        }
    }

    // Feature 9: Block non-admin logins during lockdown
    if user.role != "admin" && security_hardening::is_locked_down(&state.db).await {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "System is in lockdown mode. Only admins can login."));
    }

    // Feature 8: Block login if user is not approved
    if let Ok(Some((approved,))) = sqlx::query_as::<_, (bool,)>(
        "SELECT COALESCE(approved, TRUE) FROM users WHERE id = $1"
    ).bind(user.id).fetch_optional(&state.db).await {
        if !approved {
            return Err(err(StatusCode::FORBIDDEN, "Account pending admin approval"));
        }
    }

    // If 2FA is enabled, return a temporary token instead of a full session
    if user.totp_enabled {
        let now = chrono::Utc::now();
        let temp_claims = TwoFaClaims {
            sub: user.id,
            purpose: "2fa".to_string(),
            exp: (now + chrono::Duration::minutes(5)).timestamp() as usize,
        };
        let temp_token = encode(
            &Header::default(),
            &temp_claims,
            &EncodingKey::from_secret(state.config.jwt_secret.as_bytes()),
        )
        .map_err(|e| internal_error("login", e))?;

        // Return empty cookie header (no session yet)
        return Ok((
            StatusCode::OK,
            [(header::SET_COOKIE, String::new())],
            Json(serde_json::json!({
                "requires_2fa": true,
                "temp_token": temp_token,
            })),
        ));
    }

    let (_token, cookie, jti) = issue_session(&state, &user, &headers)?;

    // Record session
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let _ = sqlx::query(
        "INSERT INTO user_sessions (user_id, jti, ip_address, user_agent) VALUES ($1, $2, $3, $4)"
    )
    .bind(user.id)
    .bind(&jti)
    .bind(&ip)
    .bind(&user_agent)
    .execute(&state.db)
    .await;

    // Feature 1: Geo-IP alert on login from new/suspicious IP
    // Feature 7: Write to immutable audit log
    {
        let ip_clone = ip.clone();
        let email_clone = user.email.clone();
        let pool = state.db.clone();
        tokio::spawn(async move {
            // Check if lockdown is active (Feature 9)
            if security_hardening::is_locked_down(&pool).await {
                // During lockdown, log but don't block admin logins
                security_hardening::audit_log(
                    &pool, "login.during_lockdown", Some(&email_clone), Some(&ip_clone),
                    None, None, None, None, "warning",
                ).await;
            }

            let geo = security_hardening::lookup_geo_ip(&ip_clone).await;

            // Write immutable audit log
            security_hardening::audit_log(
                &pool, "login", Some(&email_clone), Some(&ip_clone),
                Some("user"), None, None, geo.as_ref(), "info",
            ).await;

            // Check if this IP is new for this user
            if security_hardening::get_setting_bool(&pool, "security_geo_alert_enabled", true).await {
                let known: Option<(i64,)> = sqlx::query_as(
                    "SELECT COUNT(*) FROM user_sessions WHERE user_id = (SELECT id FROM users WHERE email = $1) AND ip_address = $2"
                )
                .bind(&email_clone)
                .bind(&ip_clone)
                .fetch_optional(&pool)
                .await
                .ok()
                .flatten();

                let is_new_ip = known.map(|(c,)| c <= 1).unwrap_or(true); // <=1 because we just inserted
                if let Some(ref geo) = geo {
                    if is_new_ip || geo.proxy || geo.hosting {
                        security_hardening::alert_suspicious_ip(
                            &pool, "Login", &email_clone, &ip_clone, geo,
                        ).await;
                    }
                }
            }
        });
    }

    // Check if 2FA is enforced
    let enforce_2fa: bool = sqlx::query_scalar::<_, String>(
        "SELECT COALESCE(value, 'false') FROM settings WHERE key = 'enforce_2fa'"
    )
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten()
    .map(|v| v == "true")
    .unwrap_or(false);

    let user_has_2fa: bool = user.totp_enabled;

    let mut response = serde_json::json!({
        "user": {
            "id": user.id,
            "email": user.email,
            "role": user.role,
        }
    });

    // If 2FA enforced but user hasn't set it up, include flag in response
    if enforce_2fa && !user_has_2fa {
        response["twofa_required"] = serde_json::json!(true);
    }

    Ok((
        StatusCode::OK,
        [(header::SET_COOKIE, cookie)],
        Json(response),
    ))
}

/// True when this request arrived over HTTPS, either directly or via a reverse
/// proxy that set X-Forwarded-Proto. Browsers reject Secure cookies on plain
/// HTTP, so set Secure only when the connection is actually encrypted.
/// Fixes #47 — HTTP-on-IP installs were stuck in a login bounce because the
/// previous logic set Secure whenever BASE_URL was empty.
pub fn request_is_https(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.eq_ignore_ascii_case("https"))
        .unwrap_or(false)
}

pub(crate) fn cookie_secure_flag(headers: &HeaderMap) -> &'static str {
    // Tie Secure strictly to the actual connection scheme as seen by nginx
    // (X-Forwarded-Proto $scheme). Do NOT key off BASE_URL: setup.sh writes
    // BASE_URL=https://<domain> when a domain is supplied, but the panel vhost
    // is served over plain HTTP until TLS is added. The old base_url=https
    // branch then forced a Secure cookie the browser silently dropped on the
    // HTTP response, bouncing the very first login (#71 — the case the #47 fix
    // left open). request_is_https is authoritative because the API only ever
    // listens on 127.0.0.1 behind nginx, so the scheme can only come from the
    // proxy header.
    if request_is_https(headers) {
        "; Secure"
    } else {
        ""
    }
}

/// Issue a JWT session token + cookie for a user.
/// Returns (token, cookie, jti) so callers can record the session.
fn issue_session(
    state: &AppState,
    user: &User,
    headers: &HeaderMap,
) -> Result<(String, String, String), ApiError> {
    issue_session_pub(state, user, headers)
}

/// Enforce `allowed_panel_ips` for a request that is about to establish a session.
///
/// **Call this from every door that can mint one.** The allowlist gates access to the
/// panel, not access to one handler: until v2.47.0 it lived inline in `login` alone,
/// so an operator who restricted the panel to their office range still had the
/// passkey and OAuth doors answering from anywhere. Both of those issue exactly the
/// same session cookie as the password door.
///
/// Fails CLOSED when the proxy sends no `X-Real-IP` — an allowlist that cannot
/// identify the caller must not admit them (`helpers::panel_ip_allowed`).
pub async fn enforce_panel_ip_allowlist(
    db: &sqlx::PgPool,
    headers: &HeaderMap,
) -> Result<(), ApiError> {
    let whitelist: Option<(String,)> = sqlx::query_as(
        "SELECT value FROM settings WHERE key = 'allowed_panel_ips'"
    ).fetch_optional(db).await
        .map_err(|e| internal_error("login ip whitelist", e))?;
    if let Some((ips,)) = whitelist {
        if !ips.trim().is_empty() {
            // Until v2.46.0 this compared the header to each entry as a STRING, so
            // the CIDR ranges the docs promise never matched anything — an operator
            // who entered one locked themselves out instead of restricting access.
            let client_ip = headers.get("x-real-ip").and_then(|v| v.to_str().ok()).unwrap_or("");
            if !crate::helpers::panel_ip_allowed(&ips, client_ip) {
                return Err(err(StatusCode::FORBIDDEN, "Access denied: IP not whitelisted"));
            }
        }
    }
    Ok(())
}

/// Public version for use by passkey auth (same logic).
pub fn issue_session_pub(
    state: &AppState,
    user: &User,
    headers: &HeaderMap,
) -> Result<(String, String, String), ApiError> {
    let now = chrono::Utc::now();
    let jti = uuid::Uuid::new_v4().to_string();
    let claims = Claims {
        sub: user.id,
        email: user.email.clone(),
        role: user.role.clone(),
        iat: now.timestamp() as usize,
        exp: (now + chrono::Duration::hours(2)).timestamp() as usize,
        jti: Some(jti.clone()),
    };

    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(state.config.jwt_secret.as_bytes()),
    )
    .map_err(|e| internal_error("login", e))?;

    let secure_flag = cookie_secure_flag(headers);
    let cookie = format!(
        "token={token}; Path=/; HttpOnly{secure_flag}; SameSite=Lax; Max-Age=7200"
    );
    Ok((token, cookie, jti))
}

fn record_login_attempt(
    attempts: &std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, Vec<Instant>>>>,
    ip: &str,
) {
    if let Ok(mut map) = attempts.lock() {
        map.entry(ip.to_string()).or_default().push(Instant::now());
    }
}

/// Revoke ALL active sessions for a user: delete their `user_sessions` rows and
/// blacklist every associated JWT `jti` — both in the in-memory blacklist (which
/// `AuthUser` actually consults) AND the persistent `token_blacklist` table (so the
/// revocation survives a restart). This is the ONLY correct way to cut off a
/// still-valid token, because `AuthUser` authorizes from the stateless JWT and never
/// reads `user_sessions`. Used by every admin-initiated de-escalation (suspend /
/// role change / password reset / delete); mirrors the self-service reset_password path.
pub async fn revoke_all_user_sessions(state: &AppState, user_id: uuid::Uuid) {
    let sessions: Vec<(String,)> = sqlx::query_as(
        "DELETE FROM user_sessions WHERE user_id = $1 RETURNING jti",
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    if sessions.is_empty() {
        return;
    }
    {
        let mut blacklist = state.token_blacklist.write().await;
        for (jti,) in &sessions {
            blacklist.insert(jti.clone());
        }
    }
    for (jti,) in &sessions {
        let _ = sqlx::query(
            "INSERT INTO token_blacklist (jti, expires_at) VALUES ($1, NOW() + INTERVAL '2 hours') ON CONFLICT DO NOTHING",
        )
        .bind(jti)
        .execute(&state.db)
        .await;
    }
}

/// POST /api/auth/logout — Clear the auth cookie and blacklist the token JTI.
pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Result<AuthUser, crate::error::ApiError>,
) -> (StatusCode, [(header::HeaderName, String); 1], Json<serde_json::Value>) {
    // Blacklist the token's JTI so it cannot be reused
    if let Ok(AuthUser(claims)) = auth {
        if let Some(jti) = claims.jti {
            let mut blacklist = state.token_blacklist.write().await;
            blacklist.insert(jti.clone());
            drop(blacklist);
            // GAP 66: Persist to DB (survives restart)
            let _ = sqlx::query("INSERT INTO token_blacklist (jti, expires_at) VALUES ($1, NOW() + INTERVAL '2 hours') ON CONFLICT DO NOTHING")
                .bind(&jti).execute(&state.db).await;
            // Remove session record
            let _ = sqlx::query("DELETE FROM user_sessions WHERE jti = $1")
                .bind(&jti)
                .execute(&state.db)
                .await;
        }
    }

    let secure_flag = cookie_secure_flag(&headers);
    let cookie = format!("token=; Path=/; HttpOnly{secure_flag}; SameSite=Lax; Max-Age=0");
    (
        StatusCode::OK,
        [(header::SET_COOKIE, cookie)],
        Json(serde_json::json!({ "ok": true })),
    )
}

/// GET /api/auth/me — Return current authenticated user.
pub async fn me(AuthUser(claims): AuthUser) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "id": claims.sub,
        "email": claims.email,
        "role": claims.role,
    }))
}

// ─── SaaS Registration & Password Reset ────────────────────────────────────

#[derive(Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
}

/// POST /api/auth/register — Self-registration (creates unverified user account).
pub async fn register(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<RegisterRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    // Feature 9/11: Block registration during lockdown
    if security_hardening::is_locked_down(&state.db).await {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "System is in lockdown mode. Registration disabled."));
    }

    // Check if self-registration is enabled (default: disabled)
    let reg_enabled: Option<(String,)> =
        sqlx::query_as("SELECT value FROM settings WHERE key = 'self_registration_enabled'")
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("register user", e))?;
    let allowed = reg_enabled
        .map(|r| r.0 == "true")
        .unwrap_or(false);
    if !allowed {
        return Err(err(StatusCode::FORBIDDEN, "Registration is disabled"));
    }

    // Rate limit: 3 registrations per IP per hour
    let ip = headers
        .get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    {
        let rate_key = format!("register:{ip}");
        let mut attempts = state.login_attempts.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let entry = attempts.entry(rate_key).or_default();
        entry.retain(|t| now.duration_since(*t).as_secs() < 3600);
        if entry.len() >= 3 {
            return Err(err(
                StatusCode::TOO_MANY_REQUESTS,
                "Too many registration attempts. Try again later.",
            ));
        }
        entry.push(now);
    }

    if body.email.is_empty() || body.email.len() > 254 || !body.email.contains('@') {
        return Err(err(StatusCode::BAD_REQUEST, "Valid email address is required"));
    }
    if body.password.len() < 8 {
        return Err(err(StatusCode::BAD_REQUEST, "Password must be at least 8 characters"));
    }

    // Check email uniqueness — return generic success to prevent account enumeration
    let existing: Option<(uuid::Uuid,)> =
        sqlx::query_as("SELECT id FROM users WHERE email = $1")
            .bind(&body.email)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("register user", e))?;

    if existing.is_some() {
        // Return same success response to prevent account enumeration
        return Ok((
            StatusCode::CREATED,
            Json(serde_json::json!({
                "message": "Account created. Check your email to verify.",
            })),
        ));
    }

    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(body.password.as_bytes(), &salt)
        .map_err(|e| internal_error("register user", e))?
        .to_string();

    let (token, token_hash) = generate_token();

    let user: User = sqlx::query_as(
        "INSERT INTO users (email, password_hash, role, email_verified, email_token) \
         VALUES ($1, $2, 'user', FALSE, $3) RETURNING *",
    )
    .bind(&body.email)
    .bind(&hash)
    .bind(&token_hash)
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        if e.to_string().contains("unique") {
            // Return generic error to prevent account enumeration (race condition path)
            err(StatusCode::INTERNAL_SERVER_ERROR, "Registration failed. Please try again.")
        } else {
            internal_error("register user", e)
        }
    })?;

    // Use the configured BASE_URL — never derive from request headers (attacker-controlled)
    let base_url = state.config.base_url.clone();

    // Send verification email
    // Check if SMTP is configured — if not, auto-verify (self-hosted convenience)
    let smtp_configured = {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT value FROM settings WHERE key = 'smtp_host'",
        )
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
        row.map(|r| !r.0.is_empty()).unwrap_or(false)
    };

    if !smtp_configured {
        // No SMTP configured — auto-verify for self-hosted convenience
        tracing::info!("SMTP not configured — auto-verifying {}", body.email);
        sqlx::query(
            "UPDATE users SET email_verified = TRUE, email_token = NULL, updated_at = NOW() WHERE id = $1",
        )
        .bind(user.id)
        .execute(&state.db)
        .await
        .ok();
    } else {
        match email::send_verification_email(&state.db, &body.email, &token, &base_url).await {
            Ok(()) => {
                tracing::info!("Verification email sent to {}", body.email);
            }
            Err(e) => {
                tracing::warn!("Failed to send verification email to {}: {e}", body.email);
                // Don't auto-verify — user must retry or admin must verify manually
            }
        }
    }

    activity::log_activity(
        &state.db, user.id, &user.email, "auth.register",
        None, None, None, Some(&ip),
    ).await;

    // Feature 1/7: Geo-IP alert and immutable audit log on registration
    {
        let ip_clone = ip.clone();
        let email_clone = user.email.clone();
        let pool = state.db.clone();
        tokio::spawn(async move {
            let geo = security_hardening::lookup_geo_ip(&ip_clone).await;

            // Immutable audit log
            security_hardening::audit_log(
                &pool, "register", Some(&email_clone), Some(&ip_clone),
                Some("user"), None, None, geo.as_ref(), "info",
            ).await;

            // Alert admins about new registration (especially from proxy/datacenter IPs)
            if security_hardening::get_setting_bool(&pool, "security_geo_alert_enabled", true).await {
                if let Some(ref geo) = geo {
                    security_hardening::alert_suspicious_ip(
                        &pool, "Registration", &email_clone, &ip_clone, geo,
                    ).await;
                }
            }

            // Feature 4: Record as suspicious if from proxy/datacenter
            if let Some(ref geo) = geo {
                if geo.proxy || geo.hosting {
                    security_hardening::record_suspicious_event(
                        &pool, "register.proxy_ip", Some(&email_clone), Some(&ip_clone),
                        Some(&format!("Registration from {}/{} ({})", geo.country, geo.city, geo.isp)),
                    ).await;
                }
            }
        });
    }

    // Feature 8: If approval mode is on, set user as unapproved
    if security_hardening::get_setting_bool(&state.db, "security_approval_required", false).await {
        let _ = sqlx::query("UPDATE users SET approved = FALSE WHERE id = $1")
            .bind(user.id)
            .execute(&state.db)
            .await;

        return Ok((
            StatusCode::CREATED,
            Json(serde_json::json!({
                "id": user.id,
                "email": user.email,
                "message": "Account created. Awaiting admin approval.",
                "pending_approval": true,
            })),
        ));
    }

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": user.id,
            "email": user.email,
            "message": "Account created. Check your email to verify.",
        })),
    ))
}

#[derive(Deserialize)]
pub struct VerifyEmailRequest {
    pub token: String,
}

/// POST /api/auth/verify-email — Verify email with token.
pub async fn verify_email(
    State(state): State<AppState>,
    Json(body): Json<VerifyEmailRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let token_hash = hash_token(&body.token);

    let result = sqlx::query(
        "UPDATE users SET email_verified = TRUE, email_token = NULL, updated_at = NOW() \
         WHERE email_token = $1 AND email_verified = FALSE",
    )
    .bind(&token_hash)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("verify email", e))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid or expired verification token"));
    }

    Ok(Json(serde_json::json!({ "ok": true, "message": "Email verified successfully" })))
}

#[derive(Deserialize)]
pub struct ForgotPasswordRequest {
    pub email: String,
}

/// POST /api/auth/forgot-password — Request password reset email.
pub async fn forgot_password(
    State(state): State<AppState>,
    _headers: HeaderMap,
    Json(body): Json<ForgotPasswordRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Rate limit: 3 requests per email per 15 minutes
    {
        let rate_key = format!("forgot:{}", body.email.to_lowercase());
        let mut attempts = state.login_attempts.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let entry = attempts.entry(rate_key).or_default();
        entry.retain(|t| now.duration_since(*t).as_secs() < 900);
        if entry.len() >= 3 {
            return Err(err(
                StatusCode::TOO_MANY_REQUESTS,
                "Too many password reset requests. Try again later.",
            ));
        }
        entry.push(now);
    }

    // Always return success to prevent email enumeration
    let success_msg = serde_json::json!({
        "ok": true,
        "message": "If an account exists with that email, a reset link has been sent.",
    });

    let user: Option<User> = sqlx::query_as("SELECT * FROM users WHERE email = $1")
        .bind(&body.email)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("forgot password", e))?;

    let user = match user {
        Some(u) => u,
        None => return Ok(Json(success_msg)),
    };

    // A suspended account gets no reset token. Every other door already refuses
    // one — login (:228), the JWT middleware (auth.rs:105), 2FA (:1184),
    // passkeys and OAuth — and this was the single entry point with no role test
    // at all, so a reset issued here could never be used for anything anyway.
    //
    // ⚠ Return the SAME success body, never an error. The handler above answers
    // identically for every address on purpose, to defeat account enumeration; a
    // refusal here would turn it into an oracle that additionally discloses
    // WHICH addresses are suspended — a worse leak than the one being closed,
    // handed to an unauthenticated caller.
    if user.role == "suspended" {
        return Ok(Json(success_msg));
    }

    let (token, token_hash) = generate_token();
    let expires = chrono::Utc::now() + chrono::Duration::hours(1);

    sqlx::query(
        "UPDATE users SET reset_token = $1, reset_expires = $2, updated_at = NOW() WHERE id = $3",
    )
    .bind(&token_hash)
    .bind(expires)
    .bind(user.id)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("forgot password", e))?;

    // Use the configured BASE_URL — never derive from request headers (attacker-controlled)
    let base_url = state.config.base_url.clone();

    if let Err(e) = email::send_reset_email(&state.db, &body.email, &token, &base_url).await {
        tracing::warn!("Could not send reset email to {}: {e}", body.email);
    }

    Ok(Json(success_msg))
}

#[derive(Deserialize)]
pub struct ResetPasswordRequest {
    pub token: String,
    pub password: String,
}

/// POST /api/auth/reset-password — Reset password with token.
pub async fn reset_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ResetPasswordRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Rate limit: 5 attempts per IP per 15 minutes
    let ip = headers
        .get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    {
        let rate_key = format!("reset:{ip}");
        let mut attempts = state.login_attempts.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let entry = attempts.entry(rate_key).or_default();
        entry.retain(|t| now.duration_since(*t).as_secs() < 900);
        if entry.len() >= 5 {
            return Err(err(
                StatusCode::TOO_MANY_REQUESTS,
                "Too many reset attempts. Try again later.",
            ));
        }
        entry.push(now);
    }

    if body.password.len() < 8 {
        return Err(err(StatusCode::BAD_REQUEST, "Password must be at least 8 characters"));
    }

    let token_hash = hash_token(&body.token);
    let now = chrono::Utc::now();

    let user: Option<User> = sqlx::query_as(
        "SELECT * FROM users WHERE reset_token = $1 AND reset_expires > $2",
    )
    .bind(&token_hash)
    .bind(now)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("reset password", e))?;

    let user = user.ok_or_else(|| err(StatusCode::BAD_REQUEST, "Invalid or expired reset token"))?;

    // The same refusal as `forgot_password`, for a token minted before the
    // account was suspended. Answers with the sentence an unknown token gets, so
    // it stays silent about why — the response shape is the disclosure here, and
    // this endpoint has no enumeration defence of its own to lean on.
    if user.role == "suspended" {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid or expired reset token"));
    }

    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(body.password.as_bytes(), &salt)
        .map_err(|e| internal_error("reset password", e))?
        .to_string();

    sqlx::query(
        "UPDATE users SET password_hash = $1, reset_token = NULL, reset_expires = NULL, \
         email_verified = TRUE, updated_at = NOW() WHERE id = $2",
    )
    .bind(&hash)
    .bind(user.id)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("reset password", e))?;

    // Invalidate ALL sessions for this user (no current session to preserve)
    let all_sessions: Vec<(String,)> = sqlx::query_as(
        "DELETE FROM user_sessions WHERE user_id = $1 RETURNING jti"
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    if !all_sessions.is_empty() {
        let mut blacklist = state.token_blacklist.write().await;
        for (jti,) in &all_sessions {
            blacklist.insert(jti.clone());
        }
        drop(blacklist);
        // Persist to DB so blacklist survives restart
        for (jti,) in &all_sessions {
            let _ = sqlx::query("INSERT INTO token_blacklist (jti, expires_at) VALUES ($1, NOW() + INTERVAL '2 hours') ON CONFLICT DO NOTHING")
                .bind(jti).execute(&state.db).await;
        }
    }

    activity::log_activity(
        &state.db, user.id, &user.email, "auth.password_reset",
        None, None, None, None,
    ).await;

    // Panel notification
    notifications::notify_panel(&state.db, Some(user.id), "Password reset", "Your password was reset successfully", "warning", "security", None).await;

    Ok(Json(serde_json::json!({ "ok": true, "message": "Password reset successfully" })))
}

// ─── Two-Factor Authentication (TOTP) ─────────────────────────────────────

/// Claims for the temporary 2FA token (5-minute expiry).
#[derive(Debug, Serialize, Deserialize)]
struct TwoFaClaims {
    sub: uuid::Uuid,
    purpose: String,
    exp: usize,
}

/// Generate 10 recovery codes (16 chars each, hex).
///
/// ⚠ These were 4 bytes — **32 bits** — and they are stored as an UNSALTED
/// SHA-256 (`hash_token`), which is a fast hash. A 32-bit preimage space under a
/// fast hash is exhaustible on ordinary hardware in seconds, so anyone holding a
/// copy of the `users` table (a leaked backup, a read on a restored snapshot)
/// could recover the plaintext codes. That is precisely the situation 2FA exists
/// to survive: the recovery code is the factor that is supposed to still mean
/// something after the password hash has leaked.
///
/// Widened to 8 bytes / 64 bits, which makes the stored hash a real preimage
/// problem again. The hash is deliberately NOT changed to Argon2: matching is
/// by hash, so every code already issued would keep working under one scheme and
/// not the other, and a migration cannot re-hash values it cannot read. Length is
/// the lever that works without invalidating anybody's set.
///
/// Older 8-character codes keep working — `try_recovery_code` compares hashes and
/// never looks at length — and the field that accepts them takes both widths.
fn generate_recovery_codes() -> Vec<String> {
    use rand::Rng;
    let mut rng = rand::rng();
    (0..10)
        .map(|_| {
            let bytes: [u8; 8] = rng.random();
            hex::encode(bytes)
        })
        .collect()
}

/// The 2FA challenge limiter — 5 attempts per 5 minutes, keyed on the user.
///
/// This lived inline inside `twofa_verify` and nowhere else, so it guarded the
/// login door and only the login door. `twofa_disable` checks the SAME 6-digit
/// code against the SAME secret in order to make the change that switches the
/// factor OFF, and it could be guessed without limit: `check_current` runs with
/// a skew of 1, so 3 codes out of 10^6 are live at any instant, and nothing sits
/// above the handler either — the router layers only CORS, timeout and tracing
/// (`main.rs`), and the `/api/` block `setup.sh` writes for the panel vhost
/// carries no `limit_req`. The door that mints a session was rate-limited
/// because a 6-digit code is guessable; the door that removes the requirement
/// for one was not, and since v2.83.0 it also accepts recovery codes.
///
/// Kept as three small functions rather than one guard object because the
/// success path has to CLEAR the counter, and that only happens once the
/// handler knows the code was good.
fn twofa_rate_limit_check(state: &AppState, user_id: uuid::Uuid) -> Result<(), ApiError> {
    let attempts = state.twofa_attempts.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    if let Some((count, window_start)) = attempts.get(&user_id) {
        if now.duration_since(*window_start).as_secs() < 300 && *count >= 5 {
            let remaining = 300 - now.duration_since(*window_start).as_secs();
            let mins = (remaining + 59) / 60;
            return Err(err(
                StatusCode::TOO_MANY_REQUESTS,
                &format!("Too many attempts. Try again in {mins} minutes."),
            ));
        }
    }
    Ok(())
}

/// Record one failed 2FA attempt, rolling the window if it has expired.
fn twofa_rate_limit_record(state: &AppState, user_id: uuid::Uuid) {
    let mut attempts = state.twofa_attempts.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    let entry = attempts.entry(user_id).or_insert((0, now));
    if now.duration_since(entry.1).as_secs() >= 300 {
        *entry = (1, now);
    } else {
        entry.0 += 1;
    }
}

/// Clear the counter after a code has been accepted.
fn twofa_rate_limit_clear(state: &AppState, user_id: uuid::Uuid) {
    let mut attempts = state.twofa_attempts.lock().unwrap_or_else(|e| e.into_inner());
    attempts.remove(&user_id);
}

/// How stale a session may be before an account that can present *nothing* is
/// asked to sign in again.
///
/// Deliberately far shorter than the session cookie's own `Max-Age` (7200s, in
/// `issue_session_pub` below). A window at or above the cookie lifetime is
/// satisfied by every live session by construction: the check would compile,
/// run, and guard nothing.
const REAUTH_MAX_SESSION_AGE_SECS: i64 = 300;

/// Proof presented when enrolling a new authenticator. Both fields are optional
/// because the *first* request carries neither — that request IS the challenge,
/// and the refusal it earns is what tells the client which fields to collect.
#[derive(Deserialize, Default)]
pub struct EnrolmentProof {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub current_password: Option<String>,
}

/// The refusal that asks a caller to re-present a credential.
///
/// `methods` is a LIST, never a single chosen value: an account holding both a
/// password and an authenticator must be offered both, for the reason spelled
/// out in [`require_enrolment_proof`].
fn reauth_required(msg: &str, methods: &[&str], provider: Option<&str>) -> ApiError {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({
            "error": msg,
            "code": crate::error::CODE_REAUTH_REQUIRED,
            "methods": methods,
            "provider": provider,
        })),
    )
}

/// Require proof of identity before this account may enrol a new authenticator.
///
/// **Why enrolment and not removal.** A passkey is the only credential this
/// panel mints that survives every remediation it offers: resetting the
/// password, clearing 2FA and revoking every session all leave the row in
/// `passkeys` intact and immediately usable, and passkey sign-in does not take
/// the 2FA step. So a session hijacker who enrols one converts a transient
/// compromise into a permanent, 2FA-free door. The door to guard is the one
/// that PLANTS the credential.
///
/// **Why any one factor, and not the strongest.** The branches below are
/// non-exclusive on purpose. The security-hardening guide tells a sole
/// administrator who has lost both authenticator and recovery codes to register
/// a passkey — that is the documented way back. Demanding a TOTP code here
/// would refuse exactly those accounts at exactly the door that repairs them,
/// which is the mistake `twofa_disable` above already had to be corrected for.
/// Against the threat actually being closed — a hijacker holding a session
/// cookie and neither the password nor the authenticator — a password is just
/// as unanswerable as a code.
///
/// **Why a freshness fallback rather than a refusal.** An account created
/// through an identity provider has an empty password hash by construction and
/// may hold no authenticator, so it can present nothing at all. Refusing it
/// would bar those accounts from passkeys permanently — a subtraction wearing a
/// safeguard's clothes. It falls back to session age instead, which is a real
/// but weaker bound: it stops a cookie replayed from another machine and buys
/// little against script injection on the panel's own origin. Stated plainly so
/// the next reader does not mistake it for equivalence.
pub async fn require_enrolment_proof(
    state: &AppState,
    claims: &Claims,
    user: &User,
    proof: &EnrolmentProof,
) -> Result<(), ApiError> {
    let has_password = !user.password_hash.is_empty();
    let has_totp = user.totp_enabled;

    // Nothing to present. Fall back to how recently this session authenticated.
    // `iat` is authentication time here: it is stamped once when the session is
    // minted and there is no refresh route that would re-stamp it.
    if !has_password && !has_totp {
        let age = chrono::Utc::now().timestamp() - claims.iat as i64;
        if age <= REAUTH_MAX_SESSION_AGE_SECS {
            return Ok(());
        }
        let provider = user.oauth_provider.as_deref();
        let label = provider.unwrap_or("your identity provider");
        return Err(reauth_required(
            &format!("For security, sign in again with {label} before adding a passkey."),
            &["oauth"],
            provider,
        ));
    }

    // Checked before any verification so a locked-out caller is refused without
    // spending an Argon2 hash on them.
    twofa_rate_limit_check(state, user.id)?;

    let mut passed = false;

    if has_password {
        if let Some(candidate) = proof.current_password.as_deref().filter(|c| !c.is_empty()) {
            // Unreachable on an empty hash, so unlike `login` this cannot parse
            // an empty string and fail internally.
            if let Ok(parsed) = PasswordHash::new(&user.password_hash) {
                passed = Argon2::default()
                    .verify_password(candidate.as_bytes(), &parsed)
                    .is_ok();
            }
        }
    }

    if !passed && has_totp {
        if let Some(candidate) = proof.code.as_deref().filter(|c| !c.is_empty()) {
            if let Some(secret_b32_enc) = user.totp_secret.as_ref() {
                let secret_b32 = crate::services::secrets_crypto::decrypt_credential_or_legacy(
                    secret_b32_enc,
                    &state.config.jwt_secret,
                );
                if let Ok(secret) = Secret::Encoded(secret_b32).to_bytes() {
                    if let Ok(totp) = TOTP::new(
                        Algorithm::SHA1,
                        6,
                        1,
                        30,
                        secret,
                        Some("DockPanel".to_string()),
                        user.email.clone(),
                    ) {
                        passed = totp.check_current(candidate).unwrap_or(false);
                    }
                }
            }
            // The recovery codes are the fallback for a lost authenticator, and
            // this is one of the doors that repairs the account.
            if !passed {
                passed = try_recovery_code(&state.db, user, candidate).await?;
            }
        }
    }

    if passed {
        twofa_rate_limit_clear(state, user.id);
        return Ok(());
    }

    // Only an ATTEMPT counts against the limiter. The bare challenge request
    // carries no proof, and five visits to the account page must not lock the
    // user out of `/auth/2fa/disable` and the recovery-code reissue route,
    // which share this counter keyed on user id alone.
    let attempted = proof.code.is_some() || proof.current_password.is_some();
    if attempted {
        twofa_rate_limit_record(state, user.id);
    }

    let mut methods: Vec<&str> = Vec::new();
    if has_password {
        methods.push("password");
    }
    if has_totp {
        methods.push("totp");
    }

    let msg = match (has_password, has_totp) {
        (true, true) => "Confirm your password, or enter a code from your authenticator app or a recovery code, to add a passkey.",
        (true, false) => "Confirm your password to add a passkey.",
        _ => "Enter a code from your authenticator app, or one of your recovery codes, to add a passkey.",
    };

    Err(reauth_required(msg, &methods, None))
}

/// How many unused recovery codes an account is holding.
///
/// Returns 0 for both "never enrolled" and "corrupted JSON", because this feeds
/// a warning and a warning that fails closed is the safe direction: the account
/// is told it has none left, which is the state it must act on.
fn recovery_codes_remaining(stored: &Option<String>) -> usize {
    stored
        .as_deref()
        .and_then(|c| serde_json::from_str::<Vec<String>>(c).ok())
        .map(|v| v.len())
        .unwrap_or(0)
}

/// POST /api/auth/2fa/setup — Generate TOTP secret and return QR code.
pub async fn twofa_setup(
    AuthUser(claims): AuthUser,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Check if already enabled
    let user: User = sqlx::query_as("SELECT * FROM users WHERE id = $1")
        .bind(claims.sub)
        .fetch_one(&state.db)
        .await
        .map_err(|e| internal_error("2FA setup", e))?;

    if user.totp_enabled {
        return Err(err(StatusCode::BAD_REQUEST, "2FA is already enabled"));
    }

    // Generate secret
    let secret = Secret::generate_secret();
    let secret_base32 = secret.to_encoded().to_string();

    let totp = TOTP::new(
        Algorithm::SHA1,
        6,
        1,
        30,
        secret.to_bytes().map_err(|e| internal_error("2FA setup", e))?,
        Some("DockPanel".to_string()),
        user.email.clone(),
    )
    .map_err(|e| internal_error("2FA setup", e))?;

    let otpauth_url = totp.get_url();

    // Generate QR code as SVG
    let qr = qrcode::QrCode::new(otpauth_url.as_bytes())
        .map_err(|e| internal_error("2FA setup", e))?;
    let svg = qr.render::<qrcode::render::svg::Color>()
        .min_dimensions(200, 200)
        .build();

    // Encrypt and store secret in DB (not yet enabled)
    let encrypted_secret = crate::services::secrets_crypto::encrypt_credential(&secret_base32, &state.config.jwt_secret)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Encryption failed: {e}")))?;
    sqlx::query(
        "UPDATE users SET totp_secret = $1, updated_at = NOW() WHERE id = $2",
    )
    .bind(&encrypted_secret)
    .bind(claims.sub)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("2FA setup", e))?;

    Ok(Json(serde_json::json!({
        "secret": secret_base32,
        "otpauth_url": otpauth_url,
        "qr_svg": svg,
    })))
}

#[derive(Deserialize)]
pub struct TwoFaVerifyRequest {
    pub code: String,
}

/// POST /api/auth/2fa/enable — Verify a TOTP code and enable 2FA.
pub async fn twofa_enable(
    AuthUser(claims): AuthUser,
    State(state): State<AppState>,
    Json(body): Json<TwoFaVerifyRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user: User = sqlx::query_as("SELECT * FROM users WHERE id = $1")
        .bind(claims.sub)
        .fetch_one(&state.db)
        .await
        .map_err(|e| internal_error("2FA enable", e))?;

    if user.totp_enabled {
        return Err(err(StatusCode::BAD_REQUEST, "2FA is already enabled"));
    }

    let secret_b32_enc = user.totp_secret.ok_or_else(|| {
        err(StatusCode::BAD_REQUEST, "Call /api/auth/2fa/setup first")
    })?;

    // Decrypt TOTP secret (with legacy plaintext fallback)
    let secret_b32 = crate::services::secrets_crypto::decrypt_credential_or_legacy(&secret_b32_enc, &state.config.jwt_secret);

    let secret = Secret::Encoded(secret_b32)
        .to_bytes()
        .map_err(|e| internal_error("2FA enable", e))?;

    let totp = TOTP::new(Algorithm::SHA1, 6, 1, 30, secret, Some("DockPanel".to_string()), user.email.clone())
        .map_err(|e| internal_error("2FA enable", e))?;

    if !totp.check_current(&body.code).map_err(|e| internal_error("2FA enable", e))? {
        return Err(err(StatusCode::UNAUTHORIZED, "Invalid 2FA code"));
    }

    // Generate recovery codes
    let codes = generate_recovery_codes();
    let _codes_json = serde_json::to_string(&codes)
        .map_err(|e| internal_error("2FA enable", e))?;

    // Hash each code for storage (hashing is one-way, no need for reversible encryption)
    let hashed_codes: Vec<String> = codes.iter().map(|c| hash_token(c)).collect();
    let hashed_json = serde_json::to_string(&hashed_codes)
        .map_err(|e| internal_error("2FA enable", e))?;

    sqlx::query(
        "UPDATE users SET totp_enabled = TRUE, recovery_codes = $1, updated_at = NOW() WHERE id = $2",
    )
    .bind(&hashed_json)
    .bind(claims.sub)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("2FA enable", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "auth.2fa_enabled",
        None, None, None, None,
    ).await;

    Ok(Json(serde_json::json!({
        "ok": true,
        "recovery_codes": codes,
        "message": "2FA enabled successfully. Save your recovery codes!"
    })))
}

#[derive(Deserialize)]
pub struct TwoFaLoginRequest {
    pub temp_token: String,
    pub code: String,
}

/// POST /api/auth/2fa/verify — Complete login with TOTP code.
pub async fn twofa_verify(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<TwoFaLoginRequest>,
) -> Result<(StatusCode, [(header::HeaderName, String); 1], Json<serde_json::Value>), ApiError> {
    // Decode temp token with explicit HS256 validation (consistent with main auth flow)
    let mut twofa_validation = Validation::new(jsonwebtoken::Algorithm::HS256);
    twofa_validation.validate_exp = true;
    twofa_validation.leeway = 0;
    let token_data = decode::<TwoFaClaims>(
        &body.temp_token,
        &DecodingKey::from_secret(state.config.jwt_secret.as_bytes()),
        &twofa_validation,
    )
    .map_err(|_| err(StatusCode::UNAUTHORIZED, "Invalid or expired 2FA token"))?;

    if token_data.claims.purpose != "2fa" {
        return Err(err(StatusCode::UNAUTHORIZED, "Invalid token purpose"));
    }

    let user_id = token_data.claims.sub;

    // Rate limit: max 5 failed 2FA attempts per 5 minutes
    twofa_rate_limit_check(&state, user_id)?;

    let user: User = sqlx::query_as("SELECT * FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| internal_error("2FA verify", e))?;

    // Block suspended accounts at 2FA redemption too — closes the window where a
    // temp token was minted just before suspension.
    if user.role == "suspended" {
        return Err(err(StatusCode::FORBIDDEN, "Account suspended"));
    }

    // Same reasoning as the suspension check above, for the other two gates: this
    // is a second request, holding a 5-minute bearer token, and it is what actually
    // emits the session cookie. Re-checking means the temp token cannot be carried
    // to an address the allowlist excludes, or redeemed after a lockdown began.
    enforce_panel_ip_allowlist(&state.db, &headers).await?;
    if user.role != "admin" && security_hardening::is_locked_down(&state.db).await {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "System is in lockdown mode"));
    }

    // No secret here does NOT mean the server is broken: an administrator can now
    // reset this account's 2FA (`users::reset_2fa`), and the temp token issued by
    // the first step stays valid for five minutes after it. Somebody holding one
    // when that happens used to get a 500 telling them nothing. They no longer
    // need a second factor at all, so the honest answer is "start again".
    let secret_b32_enc = user.totp_secret.as_ref().ok_or_else(|| {
        err(
            StatusCode::UNAUTHORIZED,
            "Two-factor authentication is no longer set up on this account. Sign in again.",
        )
    })?;

    // Decrypt TOTP secret (with legacy plaintext fallback)
    let secret_b32 = crate::services::secrets_crypto::decrypt_credential_or_legacy(secret_b32_enc, &state.config.jwt_secret);

    let secret = Secret::Encoded(secret_b32)
        .to_bytes()
        .map_err(|e| internal_error("2FA verify", e))?;

    let totp = TOTP::new(Algorithm::SHA1, 6, 1, 30, secret, Some("DockPanel".to_string()), user.email.clone())
        .map_err(|e| internal_error("2FA verify", e))?;

    let code_valid = totp.check_current(&body.code)
        .map_err(|e| internal_error("2FA verify", e))?;

    if !code_valid {
        // Try recovery codes
        let used_recovery = try_recovery_code(&state.db, &user, &body.code).await?;
        if !used_recovery {
            twofa_rate_limit_record(&state, user_id);
            return Err(err(StatusCode::UNAUTHORIZED, "Invalid 2FA code"));
        }
    }

    // Successful verification — clear rate limit
    twofa_rate_limit_clear(&state, user_id);

    let (_token, cookie, jti) = issue_session(&state, &user, &headers)?;

    // Record session
    let ip = headers
        .get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let _ = sqlx::query(
        "INSERT INTO user_sessions (user_id, jti, ip_address, user_agent) VALUES ($1, $2, $3, $4)"
    )
    .bind(user.id)
    .bind(&jti)
    .bind(&ip)
    .bind(&user_agent)
    .execute(&state.db)
    .await;

    Ok((
        StatusCode::OK,
        [(header::SET_COOKIE, cookie)],
        Json(serde_json::json!({
            "user": {
                "id": user.id,
                "email": user.email,
                "role": user.role,
            }
        })),
    ))
}

/// Try to use a recovery code. Returns true if a code matched and was consumed.
async fn try_recovery_code(
    db: &sqlx::PgPool,
    user: &User,
    code: &str,
) -> Result<bool, ApiError> {
    let codes_json = match &user.recovery_codes {
        Some(c) => c,
        None => return Ok(false),
    };

    let hashed_codes: Vec<String> = serde_json::from_str(codes_json)
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Recovery codes corrupted"))?;

    let code_hash = hash_token(code);
    // Constant-time scan of all codes to prevent timing-based brute force
    let mut matched_idx: Option<usize> = None;
    for (idx, stored_hash) in hashed_codes.iter().enumerate() {
        if stored_hash.as_bytes().ct_eq(code_hash.as_bytes()).into() {
            matched_idx = Some(idx);
        }
    }
    if let Some(idx) = matched_idx {
        let mut remaining = hashed_codes;
        remaining.remove(idx);
        let updated = serde_json::to_string(&remaining)
            .map_err(|e| internal_error("2FA verify", e))?;

        sqlx::query("UPDATE users SET recovery_codes = $1, updated_at = NOW() WHERE id = $2")
            .bind(&updated)
            .bind(user.id)
            .execute(db)
            .await
            .map_err(|e| internal_error("2FA verify", e))?;

        Ok(true)
    } else {
        Ok(false)
    }
}

#[derive(Deserialize)]
pub struct TwoFaDisableRequest {
    pub code: String,
}

/// POST /api/auth/2fa/disable — Disable 2FA (requires a current TOTP code OR a
/// recovery code; the latter is the only route back for a lost authenticator).
pub async fn twofa_disable(
    AuthUser(claims): AuthUser,
    State(state): State<AppState>,
    Json(body): Json<TwoFaDisableRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user: User = sqlx::query_as("SELECT * FROM users WHERE id = $1")
        .bind(claims.sub)
        .fetch_one(&state.db)
        .await
        .map_err(|e| internal_error("2FA disable", e))?;

    if !user.totp_enabled {
        return Err(err(StatusCode::BAD_REQUEST, "2FA is not enabled"));
    }

    // Same limiter as the login door. This route verifies the same 6-digit code
    // against the same secret, and what it does on success is REMOVE the factor,
    // so it is the more attractive of the two to guess at — see
    // `twofa_rate_limit_check`.
    twofa_rate_limit_check(&state, claims.sub)?;

    let secret_b32_enc = user.totp_secret.as_ref().ok_or_else(|| {
        err(StatusCode::INTERNAL_SERVER_ERROR, "2FA secret missing")
    })?;

    // Decrypt TOTP secret (with legacy plaintext fallback)
    let secret_b32 = crate::services::secrets_crypto::decrypt_credential_or_legacy(secret_b32_enc, &state.config.jwt_secret);

    let secret = Secret::Encoded(secret_b32)
        .to_bytes()
        .map_err(|e| internal_error("2FA disable", e))?;

    let totp = TOTP::new(Algorithm::SHA1, 6, 1, 30, secret, Some("DockPanel".to_string()), user.email.clone())
        .map_err(|e| internal_error("2FA disable", e))?;

    // A current TOTP code OR one of the recovery codes, exactly like the login
    // path at `twofa_verify`.
    //
    // Accepting only `check_current` made losing the authenticator app a dead
    // end: the recovery codes let you IN (`try_recovery_code` below is the same
    // helper the login flow calls), and then nothing you held could ever turn
    // 2FA off or re-enrol a new device — while the panel offers no admin-side
    // 2FA reset either. The codes exist to be the fallback; refusing them at the
    // one door that repairs the account is a lockout wearing a safeguard's
    // clothes. This became urgent when enrolment moved to `/account`, where
    // every role now sees the button that used to refuse them.
    let code_valid = totp
        .check_current(&body.code)
        .map_err(|e| internal_error("2FA disable", e))?;
    if !code_valid && !try_recovery_code(&state.db, &user, &body.code).await? {
        twofa_rate_limit_record(&state, claims.sub);
        return Err(err(
            StatusCode::UNAUTHORIZED,
            "Invalid 2FA code. Enter a code from your authenticator app, or one of your recovery codes.",
        ));
    }
    twofa_rate_limit_clear(&state, claims.sub);

    sqlx::query(
        "UPDATE users SET totp_enabled = FALSE, totp_secret = NULL, recovery_codes = NULL, updated_at = NOW() WHERE id = $1",
    )
    .bind(claims.sub)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("2FA disable", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "auth.2fa_disabled",
        None, None, None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true, "message": "2FA has been disabled" })))
}

#[derive(Deserialize)]
pub struct TwoFaRegenerateRequest {
    pub code: String,
}

/// POST /api/auth/2fa/recovery-codes — Issue a fresh set of recovery codes.
///
/// Recovery codes were written exactly once, at enrolment, and consumed one at a
/// time: after ten recovery logins the fallback was silently gone, and there was
/// no way to mint more short of disabling 2FA and enrolling again. Worse, every
/// account that enrolled before v2.83.0 holds ten codes it was NEVER SHOWN — the
/// block that draws them sat inside the branch the enable handler cleared in the
/// same breath — so those accounts have a fallback on paper and none in fact.
/// This is the door that repairs them WITHOUT losing the enrolment, and it is
/// why the route accepts a live TOTP code: someone who still has their
/// authenticator can fix themselves, today, with no administrator involved.
///
/// A recovery code is accepted too, for the same reason `twofa_disable` accepts
/// one — it is the same authority, and this is the gentler of the two things it
/// already permits. Rate-limited on the shared 2FA limiter.
pub async fn twofa_regenerate_recovery_codes(
    AuthUser(claims): AuthUser,
    State(state): State<AppState>,
    Json(body): Json<TwoFaRegenerateRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user: User = sqlx::query_as("SELECT * FROM users WHERE id = $1")
        .bind(claims.sub)
        .fetch_one(&state.db)
        .await
        .map_err(|e| internal_error("2FA recovery codes", e))?;

    if !user.totp_enabled {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "2FA is not enabled — recovery codes are issued when you enrol",
        ));
    }

    twofa_rate_limit_check(&state, claims.sub)?;

    let secret_b32_enc = user.totp_secret.as_ref().ok_or_else(|| {
        err(StatusCode::INTERNAL_SERVER_ERROR, "2FA secret missing")
    })?;
    let secret_b32 = crate::services::secrets_crypto::decrypt_credential_or_legacy(
        secret_b32_enc,
        &state.config.jwt_secret,
    );
    let secret = Secret::Encoded(secret_b32)
        .to_bytes()
        .map_err(|e| internal_error("2FA recovery codes", e))?;
    let totp = TOTP::new(Algorithm::SHA1, 6, 1, 30, secret, Some("DockPanel".to_string()), user.email.clone())
        .map_err(|e| internal_error("2FA recovery codes", e))?;

    let code_valid = totp
        .check_current(&body.code)
        .map_err(|e| internal_error("2FA recovery codes", e))?;
    if !code_valid && !try_recovery_code(&state.db, &user, &body.code).await? {
        twofa_rate_limit_record(&state, claims.sub);
        return Err(err(
            StatusCode::UNAUTHORIZED,
            "Invalid 2FA code. Enter a code from your authenticator app, or one of your recovery codes.",
        ));
    }
    twofa_rate_limit_clear(&state, claims.sub);

    // Replaces the whole set, so any code still outstanding — including one just
    // spent getting in here — stops working. That is the point: the old set is
    // the one the user could not read.
    let codes = generate_recovery_codes();
    let hashed_codes: Vec<String> = codes.iter().map(|c| hash_token(c)).collect();
    let hashed_json = serde_json::to_string(&hashed_codes)
        .map_err(|e| internal_error("2FA recovery codes", e))?;

    sqlx::query("UPDATE users SET recovery_codes = $1, updated_at = NOW() WHERE id = $2")
        .bind(&hashed_json)
        .bind(claims.sub)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("2FA recovery codes", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "auth.2fa_recovery_codes_regenerated",
        None, None, None, None,
    ).await;

    Ok(Json(serde_json::json!({
        "ok": true,
        "recovery_codes": codes,
        "message": "New recovery codes issued. The previous set no longer works.",
    })))
}

/// GET /api/auth/2fa/status — Check if 2FA is enabled for the current user.
pub async fn twofa_status(
    AuthUser(claims): AuthUser,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // `recovery_codes` rides along so the page can say how many are LEFT. They are
    // consumed one per recovery login and were never counted anywhere the user
    // could see, so an account could arrive at zero — no fallback at all — while
    // its 2FA card still read "2FA is enabled" and nothing else.
    let row: (bool, Option<String>) =
        sqlx::query_as("SELECT totp_enabled, recovery_codes FROM users WHERE id = $1")
            .bind(claims.sub)
            .fetch_one(&state.db)
            .await
            .map_err(|e| internal_error("2FA status", e))?;

    // `enforced` rides along here because this is the only 2FA read a NON-ADMIN can
    // make. The layout's "Two-factor authentication is required" banner asked
    // `GET /api/settings` for it — an `AdminUser` route — and swallowed the 403 in
    // an empty catch, so `twoFaEnforced` stayed at its `useState(false)` for every
    // non-admin and the banner could only ever render for the one role that does
    // not need telling. The operator flipped "Require 2FA for all users" and the
    // people it was aimed at were the only ones never shown it.
    //
    // Whether the panel should also REFUSE the login is a separate decision and is
    // deliberately still not taken here. ⚠ The reason has CHANGED and the old one
    // is gone: enrolment used to live only on `/settings`, which is `adminOnly` in
    // the nav registry, so refusing a non-admin would have been a lockout with no
    // way out. v2.83.0 built `/account`, which every role carries, so that
    // objection no longer holds. What holds now is the hazard list nobody has
    // costed: OAuth-provisioned accounts that have no password to fall back on,
    // the bootstrap admin, and callers that are not browsers. Enforcement is a
    // product decision, not a missing line — and until it is taken, `enforce_2fa`
    // does not refuse anything (`login` sets a `twofa_required` flag on the
    // response body and issues the session cookie regardless).
    // `settings.value` is `TEXT NOT NULL`, so the row is either present or absent —
    // an absent key means the operator never turned it on.
    let enforced: Option<(String,)> =
        sqlx::query_as("SELECT value FROM settings WHERE key = 'enforce_2fa'")
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("2FA status", e))?;

    Ok(Json(serde_json::json!({
        "enabled": row.0,
        "enforced": enforced.map(|(v,)| v == "true").unwrap_or(false),
        "recovery_codes_remaining": recovery_codes_remaining(&row.1),
    })))
}

// ─── Password Change & Session Revocation ───────────────────────────────

/// POST /api/auth/change-password — Change own password.
pub async fn change_password(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let current = body.get("current_password").and_then(|v| v.as_str())
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Current password required"))?;
    let new_pass = body.get("new_password").and_then(|v| v.as_str())
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "New password required"))?;

    if new_pass.len() < 8 {
        return Err(err(StatusCode::BAD_REQUEST, "Password must be at least 8 characters"));
    }

    // Verify current password
    let user: Option<(String,)> = sqlx::query_as("SELECT password_hash FROM users WHERE id = $1")
        .bind(claims.sub)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("change password", e))?;

    let hash = user.ok_or_else(|| err(StatusCode::NOT_FOUND, "User not found"))?.0;

    // OAuth users have no password hash — they must use their OAuth provider
    if hash.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "OAuth users cannot change passwords. Use your OAuth provider instead."));
    }

    let parsed = PasswordHash::new(&hash)
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Password hash error"))?;
    Argon2::default()
        .verify_password(current.as_bytes(), &parsed)
        .map_err(|_| err(StatusCode::UNAUTHORIZED, "Current password is incorrect"))?;

    // Hash new password
    let salt = SaltString::generate(&mut OsRng);
    let new_hash = Argon2::default()
        .hash_password(new_pass.as_bytes(), &salt)
        .map_err(|e| internal_error("change password", e))?
        .to_string();

    sqlx::query("UPDATE users SET password_hash = $1, updated_at = NOW() WHERE id = $2")
        .bind(&new_hash)
        .bind(claims.sub)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("change password", e))?;

    // Invalidate all other sessions for this user (keep current session)
    if let Some(ref current_jti) = claims.jti {
        let other_sessions: Vec<(String,)> = sqlx::query_as(
            "DELETE FROM user_sessions WHERE user_id = $1 AND jti != $2 RETURNING jti"
        )
        .bind(claims.sub)
        .bind(current_jti)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

        if !other_sessions.is_empty() {
            let mut blacklist = state.token_blacklist.write().await;
            for (jti,) in &other_sessions {
                blacklist.insert(jti.clone());
            }
            drop(blacklist);
            // Persist to DB so blacklist survives restart
            for (jti,) in &other_sessions {
                let _ = sqlx::query("INSERT INTO token_blacklist (jti, expires_at) VALUES ($1, NOW() + INTERVAL '2 hours') ON CONFLICT DO NOTHING")
                    .bind(jti).execute(&state.db).await;
            }
        }
    }

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "auth.password_change",
        None, None, None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true, "message": "Password changed successfully" })))
}

/// POST /api/auth/revoke-all — Revoke all active sessions (forces re-login for everyone).
/// Admin-only: only admins can force everyone to re-login.
pub async fn revoke_all_sessions(
    State(state): State<AppState>,
    crate::auth::AdminUser(claims): crate::auth::AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let now = chrono::Utc::now();
    // Store a timestamp marker — auth middleware checks this to invalidate older tokens
    sqlx::query(
        "INSERT INTO settings (key, value, updated_at) VALUES ('sessions_revoked_at', $1, NOW()) \
         ON CONFLICT (key) DO UPDATE SET value = $1, updated_at = NOW()",
    )
    .bind(now.to_rfc3339())
    .execute(&state.db)
    .await
    .ok();

    // Update the in-memory cache so the auth middleware enforces immediately
    {
        let mut cached = state.sessions_revoked_at.write().await;
        *cached = Some(now.timestamp());
    }

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "auth.revoke_all",
        None, None, None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true, "message": "All sessions revoked. Users will need to re-login." })))
}

// ─── Session Management ─────────────────────────────────────────────────────

/// GET /api/auth/sessions — List active sessions for the current user.
pub async fn list_sessions(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let sessions: Vec<(
        uuid::Uuid,
        String,
        Option<String>,
        Option<String>,
        chrono::DateTime<chrono::Utc>,
        chrono::DateTime<chrono::Utc>,
    )> = sqlx::query_as(
        "SELECT id, jti, ip_address, user_agent, created_at, last_seen_at \
         FROM user_sessions WHERE user_id = $1 ORDER BY last_seen_at DESC",
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("list sessions", e))?;

    let current_jti = claims.jti.unwrap_or_default();
    let result: Vec<serde_json::Value> = sessions
        .iter()
        .map(|(id, jti, ip, ua, created, seen)| {
            serde_json::json!({
                "id": id,
                "ip_address": ip,
                "user_agent": ua,
                "created_at": created,
                "last_seen_at": seen,
                "is_current": jti == &current_jti,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({ "sessions": result })))
}

/// DELETE /api/auth/sessions/{id} — Revoke a specific session.
pub async fn revoke_session(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Get the JTI for this session (must belong to the current user)
    let session: Option<(String,)> = sqlx::query_as(
        "SELECT jti FROM user_sessions WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("revoke session", e))?;

    if let Some((jti,)) = session {
        // Add to token blacklist so the JWT is immediately invalid
        let mut blacklist = state.token_blacklist.write().await;
        blacklist.insert(jti.clone());
        drop(blacklist);

        // GAP 66: Persist to DB (survives restart)
        let _ = sqlx::query("INSERT INTO token_blacklist (jti, expires_at) VALUES ($1, NOW() + INTERVAL '2 hours') ON CONFLICT DO NOTHING")
            .bind(&jti).execute(&state.db).await;

        // Delete session record
        sqlx::query("DELETE FROM user_sessions WHERE id = $1")
            .bind(id)
            .execute(&state.db)
            .await
            .ok();

        Ok(Json(serde_json::json!({ "ok": true })))
    } else {
        Err(err(StatusCode::NOT_FOUND, "Session not found"))
    }
}

/// GET /api/auth/export-my-data — Export all personal data (GDPR)
pub async fn export_my_data(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = sqlx::query_as::<_, (String, String, Option<String>, bool, chrono::DateTime<chrono::Utc>)>(
        "SELECT email, role, oauth_provider, totp_enabled, created_at FROM users WHERE id = $1"
    ).bind(claims.sub).fetch_one(&state.db).await
    .map_err(|e| internal_error("export user data", e))?;

    let sites: Vec<(String, String)> = sqlx::query_as(
        "SELECT domain, runtime FROM sites WHERE user_id = $1"
    ).bind(claims.sub).fetch_all(&state.db).await.unwrap_or_default();

    let activity: Vec<(String, Option<String>, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT action, target_name, created_at FROM activity_logs WHERE user_id = $1 ORDER BY created_at DESC LIMIT 100"
    ).bind(claims.sub).fetch_all(&state.db).await.unwrap_or_default();

    let sessions: Vec<(Option<String>, Option<String>, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT ip_address, user_agent, created_at FROM user_sessions WHERE user_id = $1"
    ).bind(claims.sub).fetch_all(&state.db).await.unwrap_or_default();

    // Passkeys belong here for the same reason sessions do, and more urgently:
    // a session can be revoked, a passkey survives every reset the panel
    // offers. Exporting the revocable credential while omitting the permanent
    // one would leave this the only self-service surface that cannot show a
    // user what signs in as them.
    let passkeys: Vec<(String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT name, created_at FROM passkeys WHERE user_id = $1"
    ).bind(claims.sub).fetch_all(&state.db).await.unwrap_or_default();

    Ok(Json(serde_json::json!({
        "user": { "email": user.0, "role": user.1, "oauth_provider": user.2, "2fa_enabled": user.3, "created_at": user.4 },
        "sites": sites.iter().map(|(d,r)| serde_json::json!({"domain": d, "runtime": r})).collect::<Vec<_>>(),
        "recent_activity": activity.iter().map(|(a,t,c)| serde_json::json!({"action": a, "target": t, "at": c})).collect::<Vec<_>>(),
        "sessions": sessions.iter().map(|(ip,ua,c)| serde_json::json!({"ip": ip, "user_agent": ua, "at": c})).collect::<Vec<_>>(),
        "passkeys": passkeys.iter().map(|(n,c)| serde_json::json!({"name": n, "at": c})).collect::<Vec<_>>(),
        "exported_at": chrono::Utc::now(),
    })))
}

#[cfg(test)]
mod login_credential_tests {
    use super::*;

    /// The mechanism behind the OAuth-only 500. `oauth.rs` creates accounts
    /// with an empty `password_hash`, and an empty string is not a PHC string,
    /// so the login handler's parse fails — which it used to answer with a 500,
    /// making the door an enumeration oracle for identity-provider accounts.
    #[test]
    fn an_empty_password_hash_cannot_be_parsed() {
        assert!(
            PasswordHash::new("").is_err(),
            "if this ever parses, the empty-hash branch in `login` is dead code \
             and the reason for it has changed"
        );
    }

    /// Control: the assertion above is about the EMPTY string specifically, not
    /// about `PasswordHash::new` rejecting everything.
    #[test]
    fn a_real_hash_still_parses() {
        assert!(PasswordHash::new(DUMMY_PASSWORD_HASH).is_ok());
    }

    /// The empty-hash branch calls `.expect()` on this. A dummy hash that did
    /// not parse would turn an enumeration fix into a panic on the same input.
    #[test]
    fn the_dummy_hash_is_verifiable_not_merely_parseable() {
        let parsed = PasswordHash::new(DUMMY_PASSWORD_HASH)
            .expect("the compiled-in dummy hash must parse");
        // Wrong password against the dummy: the call must COMPLETE (spending
        // the Argon2 time that equalises this branch), and must not match.
        assert!(Argon2::default()
            .verify_password(b"whatever-the-caller-typed", &parsed)
            .is_err());
    }
}
