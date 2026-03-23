use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    Json,
};
use std::time::Instant;
use jsonwebtoken::{encode, decode, EncodingKey, DecodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};

use argon2::{
    password_hash::{rand_core::OsRng, SaltString},
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
};
use totp_rs::{Algorithm, TOTP, Secret};

use crate::auth::{AuthUser, Claims};
use crate::error::{err, ApiError};
use crate::models::User;
use crate::services::{activity, email, notifications};
use crate::AppState;

/// A zero-valued UUID used for activity logging when there is no authenticated user.
fn zero_uuid() -> uuid::Uuid {
    uuid::Uuid::nil()
}

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
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

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
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

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
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
        .to_string();

    // Atomic check-and-insert to prevent TOCTOU race
    let user: Option<User> = sqlx::query_as(
        "INSERT INTO users (email, password_hash, role) \
         SELECT $1, $2, 'admin' \
         WHERE NOT EXISTS (SELECT 1 FROM users) \
         RETURNING *",
    )
    .bind(&body.email)
    .bind(&hash)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

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
    let whitelist: Option<(String,)> = sqlx::query_as(
        "SELECT value FROM settings WHERE key = 'allowed_panel_ips'"
    ).fetch_optional(&state.db).await.ok().flatten();
    if let Some((ips,)) = whitelist {
        if !ips.is_empty() {
            let client_ip = headers.get("x-real-ip").and_then(|v| v.to_str().ok()).unwrap_or("");
            if !ips.split(',').any(|allowed| allowed.trim() == client_ip) {
                return Err(err(StatusCode::FORBIDDEN, "Access denied: IP not whitelisted"));
            }
        }
    }

    let user_opt: Option<User> = sqlx::query_as("SELECT * FROM users WHERE email = $1")
        .bind(&body.email)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    // Constant-time: always run Argon2 verify, even for non-existent users (prevents timing attack)
    let dummy_hash = "$argon2id$v=19$m=19456,t=2,p=1$ZHVtbXlzYWx0MTIzNA$K1PqGlDJpiBFSguVJXKDBIuXQ5baiAOXSgWAGkuJYxk";
    let user = match user_opt {
        Some(u) => {
            let parsed = PasswordHash::new(&u.password_hash)
                .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Password hash error"))?;
            match Argon2::default().verify_password(body.password.as_bytes(), &parsed) {
                Ok(()) => u,
                Err(_) => {
                    record_login_attempt(&state.login_attempts, &ip);
                    // Log failed login attempt (never log password)
                    activity::log_activity(
                        &state.db, u.id, &u.email, "auth.login_failed",
                        None, None, None, Some(&ip),
                    ).await;
                    return Err(err(StatusCode::UNAUTHORIZED, "Invalid credentials"));
                }
            }
        }
        None => {
            // Run dummy verify to equalize timing, then fail
            let parsed = PasswordHash::new(dummy_hash).unwrap();
            let _ = Argon2::default().verify_password(body.password.as_bytes(), &parsed);
            record_login_attempt(&state.login_attempts, &ip);
            // Log failed login with email only (no user ID available)
            activity::log_activity(
                &state.db, zero_uuid(), &body.email, "auth.login_failed",
                None, None, Some("unknown_user"), Some(&ip),
            ).await;
            return Err(err(StatusCode::UNAUTHORIZED, "Invalid credentials"));
        }
    };

    // Success — clear rate limit for this IP
    {
        let mut attempts = state.login_attempts.lock().unwrap_or_else(|e| e.into_inner());
        attempts.remove(&ip);
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
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

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

    let (_token, cookie, jti) = issue_session(&state, &user)?;

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

/// Issue a JWT session token + cookie for a user.
/// Returns (token, cookie, jti) so callers can record the session.
fn issue_session(state: &AppState, user: &User) -> Result<(String, String, String), ApiError> {
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
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    // Default to Secure when BASE_URL is not set (most deployments use HTTPS)
    let secure_flag = if state.config.base_url.is_empty() || state.config.base_url.starts_with("https") {
        "; Secure"
    } else {
        ""
    };
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

/// POST /api/auth/logout — Clear the auth cookie and blacklist the token JTI.
pub async fn logout(
    State(state): State<AppState>,
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

    // Default to Secure when BASE_URL is not set (most deployments use HTTPS)
    let secure_flag = if state.config.base_url.is_empty() || state.config.base_url.starts_with("https") {
        "; Secure"
    } else {
        ""
    };
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
    // Check if self-registration is enabled (default: disabled)
    let reg_enabled: Option<(String,)> =
        sqlx::query_as("SELECT value FROM settings WHERE key = 'self_registration_enabled'")
            .fetch_optional(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
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

    // Check email uniqueness
    let existing: Option<(uuid::Uuid,)> =
        sqlx::query_as("SELECT id FROM users WHERE email = $1")
            .bind(&body.email)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if existing.is_some() {
        return Err(err(StatusCode::CONFLICT, "Email already registered"));
    }

    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(body.password.as_bytes(), &salt)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
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
            err(StatusCode::CONFLICT, "Email already registered")
        } else {
            err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
        }
    })?;

    // Determine base URL from Host header only (never trust Origin — attacker-controlled)
    let base_url = {
        let host = headers
            .get("host")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("localhost");
        format!("https://{host}")
    };

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
        None, None, None, None,
    ).await;

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
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

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
    headers: HeaderMap,
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
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let user = match user {
        Some(u) => u,
        None => return Ok(Json(success_msg)),
    };

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
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let base_url = headers
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            let host = headers
                .get("host")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("localhost");
            format!("https://{host}")
        });

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
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let user = user.ok_or_else(|| err(StatusCode::BAD_REQUEST, "Invalid or expired reset token"))?;

    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(body.password.as_bytes(), &salt)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
        .to_string();

    sqlx::query(
        "UPDATE users SET password_hash = $1, reset_token = NULL, reset_expires = NULL, \
         email_verified = TRUE, updated_at = NOW() WHERE id = $2",
    )
    .bind(&hash)
    .bind(user.id)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

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

/// Generate 10 recovery codes (8 chars each, hex).
fn generate_recovery_codes() -> Vec<String> {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..10)
        .map(|_| {
            let bytes: [u8; 4] = rng.r#gen();
            hex::encode(bytes)
        })
        .collect()
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
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

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
        secret.to_bytes().map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?,
        Some("DockPanel".to_string()),
        user.email.clone(),
    )
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let otpauth_url = totp.get_url();

    // Generate QR code as SVG
    let qr = qrcode::QrCode::new(otpauth_url.as_bytes())
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    let svg = qr.render::<qrcode::render::svg::Color>()
        .min_dimensions(200, 200)
        .build();

    // Store secret in DB (not yet enabled)
    sqlx::query(
        "UPDATE users SET totp_secret = $1, updated_at = NOW() WHERE id = $2",
    )
    .bind(&secret_base32)
    .bind(claims.sub)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

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
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if user.totp_enabled {
        return Err(err(StatusCode::BAD_REQUEST, "2FA is already enabled"));
    }

    let secret_b32 = user.totp_secret.ok_or_else(|| {
        err(StatusCode::BAD_REQUEST, "Call /api/auth/2fa/setup first")
    })?;

    let secret = Secret::Encoded(secret_b32)
        .to_bytes()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let totp = TOTP::new(Algorithm::SHA1, 6, 1, 30, secret, Some("DockPanel".to_string()), user.email.clone())
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if !totp.check_current(&body.code).map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))? {
        return Err(err(StatusCode::UNAUTHORIZED, "Invalid 2FA code"));
    }

    // Generate recovery codes
    let codes = generate_recovery_codes();
    let codes_json = serde_json::to_string(&codes)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    // Hash each code for storage
    let hashed_codes: Vec<String> = codes.iter().map(|c| hash_token(c)).collect();
    let hashed_json = serde_json::to_string(&hashed_codes)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    sqlx::query(
        "UPDATE users SET totp_enabled = TRUE, recovery_codes = $1, updated_at = NOW() WHERE id = $2",
    )
    .bind(&hashed_json)
    .bind(claims.sub)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

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
    // Decode temp token
    let token_data = decode::<TwoFaClaims>(
        &body.temp_token,
        &DecodingKey::from_secret(state.config.jwt_secret.as_bytes()),
        &Validation::default(),
    )
    .map_err(|_| err(StatusCode::UNAUTHORIZED, "Invalid or expired 2FA token"))?;

    if token_data.claims.purpose != "2fa" {
        return Err(err(StatusCode::UNAUTHORIZED, "Invalid token purpose"));
    }

    let user_id = token_data.claims.sub;

    // Rate limit: max 5 failed 2FA attempts per 5 minutes
    {
        let mut attempts = state.twofa_attempts.lock().unwrap_or_else(|e| e.into_inner());
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
    }

    let user: User = sqlx::query_as("SELECT * FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let secret_b32 = user.totp_secret.as_ref().ok_or_else(|| {
        err(StatusCode::INTERNAL_SERVER_ERROR, "2FA secret missing")
    })?;

    let secret = Secret::Encoded(secret_b32.clone())
        .to_bytes()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let totp = TOTP::new(Algorithm::SHA1, 6, 1, 30, secret, Some("DockPanel".to_string()), user.email.clone())
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let code_valid = totp.check_current(&body.code)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if !code_valid {
        // Try recovery codes
        let used_recovery = try_recovery_code(&state.db, &user, &body.code).await?;
        if !used_recovery {
            // Record failed attempt
            {
                let mut attempts = state.twofa_attempts.lock().unwrap_or_else(|e| e.into_inner());
                let now = Instant::now();
                let entry = attempts.entry(user_id).or_insert((0, now));
                if now.duration_since(entry.1).as_secs() >= 300 {
                    // Window expired, reset
                    *entry = (1, now);
                } else {
                    entry.0 += 1;
                }
            }
            return Err(err(StatusCode::UNAUTHORIZED, "Invalid 2FA code"));
        }
    }

    // Successful verification — clear rate limit
    {
        let mut attempts = state.twofa_attempts.lock().unwrap_or_else(|e| e.into_inner());
        attempts.remove(&user_id);
    }

    let (_token, cookie, jti) = issue_session(&state, &user)?;

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
    if let Some(idx) = hashed_codes.iter().position(|h| h == &code_hash) {
        let mut remaining = hashed_codes;
        remaining.remove(idx);
        let updated = serde_json::to_string(&remaining)
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

        sqlx::query("UPDATE users SET recovery_codes = $1, updated_at = NOW() WHERE id = $2")
            .bind(&updated)
            .bind(user.id)
            .execute(db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

        Ok(true)
    } else {
        Ok(false)
    }
}

#[derive(Deserialize)]
pub struct TwoFaDisableRequest {
    pub code: String,
}

/// POST /api/auth/2fa/disable — Disable 2FA (requires current TOTP code).
pub async fn twofa_disable(
    AuthUser(claims): AuthUser,
    State(state): State<AppState>,
    Json(body): Json<TwoFaDisableRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user: User = sqlx::query_as("SELECT * FROM users WHERE id = $1")
        .bind(claims.sub)
        .fetch_one(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if !user.totp_enabled {
        return Err(err(StatusCode::BAD_REQUEST, "2FA is not enabled"));
    }

    let secret_b32 = user.totp_secret.as_ref().ok_or_else(|| {
        err(StatusCode::INTERNAL_SERVER_ERROR, "2FA secret missing")
    })?;

    let secret = Secret::Encoded(secret_b32.clone())
        .to_bytes()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let totp = TOTP::new(Algorithm::SHA1, 6, 1, 30, secret, Some("DockPanel".to_string()), user.email.clone())
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if !totp.check_current(&body.code).map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))? {
        return Err(err(StatusCode::UNAUTHORIZED, "Invalid 2FA code"));
    }

    sqlx::query(
        "UPDATE users SET totp_enabled = FALSE, totp_secret = NULL, recovery_codes = NULL, updated_at = NOW() WHERE id = $1",
    )
    .bind(claims.sub)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "auth.2fa_disabled",
        None, None, None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true, "message": "2FA has been disabled" })))
}

/// GET /api/auth/2fa/status — Check if 2FA is enabled for the current user.
pub async fn twofa_status(
    AuthUser(claims): AuthUser,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let row: (bool,) = sqlx::query_as("SELECT totp_enabled FROM users WHERE id = $1")
        .bind(claims.sub)
        .fetch_one(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(serde_json::json!({ "enabled": row.0 })))
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
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

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
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
        .to_string();

    sqlx::query("UPDATE users SET password_hash = $1, updated_at = NOW() WHERE id = $2")
        .bind(&new_hash)
        .bind(claims.sub)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

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
    // Store a timestamp marker — auth middleware can check this to invalidate older tokens
    sqlx::query(
        "INSERT INTO settings (key, value, updated_at) VALUES ('sessions_revoked_at', $1, NOW()) \
         ON CONFLICT (key) DO UPDATE SET value = $1, updated_at = NOW()",
    )
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&state.db)
    .await
    .ok();

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
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

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
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

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
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let sites: Vec<(String, String)> = sqlx::query_as(
        "SELECT domain, runtime FROM sites WHERE user_id = $1"
    ).bind(claims.sub).fetch_all(&state.db).await.unwrap_or_default();

    let activity: Vec<(String, Option<String>, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT action, target_name, created_at FROM activity_logs WHERE user_id = $1 ORDER BY created_at DESC LIMIT 100"
    ).bind(claims.sub).fetch_all(&state.db).await.unwrap_or_default();

    let sessions: Vec<(Option<String>, Option<String>, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT ip_address, user_agent, created_at FROM user_sessions WHERE user_id = $1"
    ).bind(claims.sub).fetch_all(&state.db).await.unwrap_or_default();

    Ok(Json(serde_json::json!({
        "user": { "email": user.0, "role": user.1, "oauth_provider": user.2, "2fa_enabled": user.3, "created_at": user.4 },
        "sites": sites.iter().map(|(d,r)| serde_json::json!({"domain": d, "runtime": r})).collect::<Vec<_>>(),
        "recent_activity": activity.iter().map(|(a,t,c)| serde_json::json!({"action": a, "target": t, "at": c})).collect::<Vec<_>>(),
        "sessions": sessions.iter().map(|(ip,ua,c)| serde_json::json!({"ip": ip, "user_agent": ua, "at": c})).collect::<Vec<_>>(),
        "exported_at": chrono::Utc::now(),
    })))
}
