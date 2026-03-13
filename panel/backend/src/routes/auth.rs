use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    Json,
};
use std::time::Instant;
use jsonwebtoken::{encode, EncodingKey, Header};
use serde::Deserialize;
use sha2::{Sha256, Digest};

use argon2::{
    password_hash::{rand_core::OsRng, SaltString},
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
};

use crate::auth::{AuthUser, Claims};
use crate::error::{err, ApiError};
use crate::models::User;
use crate::services::{activity, email};
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
    if body.email.is_empty() || !body.email.contains('@') {
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
    // Extract client IP for rate limiting
    let ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|s| s.trim().to_string())
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
        })
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
            Argon2::default()
                .verify_password(body.password.as_bytes(), &parsed)
                .map_err(|_| {
                    record_login_attempt(&state.login_attempts, &ip);
                    err(StatusCode::UNAUTHORIZED, "Invalid credentials")
                })?;
            u
        }
        None => {
            // Run dummy verify to equalize timing, then fail
            let parsed = PasswordHash::new(dummy_hash).unwrap();
            let _ = Argon2::default().verify_password(body.password.as_bytes(), &parsed);
            record_login_attempt(&state.login_attempts, &ip);
            return Err(err(StatusCode::UNAUTHORIZED, "Invalid credentials"));
        }
    };

    // Success — clear rate limit for this IP
    {
        let mut attempts = state.login_attempts.lock().unwrap_or_else(|e| e.into_inner());
        attempts.remove(&ip);
    }

    let now = chrono::Utc::now();
    let claims = Claims {
        sub: user.id,
        email: user.email.clone(),
        role: user.role.clone(),
        iat: now.timestamp() as usize,
        exp: (now + chrono::Duration::hours(24)).timestamp() as usize,
    };

    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(state.config.jwt_secret.as_bytes()),
    )
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let cookie = format!(
        "token={token}; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=86400"
    );

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

fn record_login_attempt(
    attempts: &std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, Vec<Instant>>>>,
    ip: &str,
) {
    if let Ok(mut map) = attempts.lock() {
        map.entry(ip.to_string()).or_default().push(Instant::now());
    }
}

/// POST /api/auth/logout — Clear the auth cookie.
pub async fn logout() -> (StatusCode, [(header::HeaderName, String); 1], Json<serde_json::Value>) {
    let cookie = "token=; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=0".to_string();
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
    if body.email.is_empty() || !body.email.contains('@') {
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

    // Determine base URL from request headers
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
    Json(body): Json<ResetPasswordRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
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

    Ok(Json(serde_json::json!({ "ok": true, "message": "Password reset successfully" })))
}
