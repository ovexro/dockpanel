use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use uuid::Uuid;

use argon2::{
    password_hash::{rand_core::OsRng, SaltString},
    Argon2, PasswordHasher,
};

use crate::auth::AdminUser;
use crate::error::{internal_error, err, ApiError};
use crate::models::User;
use crate::services::activity;
use crate::AppState;

#[derive(serde::Deserialize)]
pub struct CreateUserRequest {
    pub email: String,
    pub password: String,
    pub role: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct UpdateUserRequest {
    pub role: Option<String>,
    pub password: Option<String>,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct UserResponse {
    pub id: Uuid,
    pub email: String,
    pub role: String,
    pub reseller_id: Option<Uuid>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub site_count: i64,
}

/// GET /api/users — List all users (admin only).
pub async fn list(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<Json<Vec<UserResponse>>, ApiError> {

    let users: Vec<UserResponse> = sqlx::query_as(
        "SELECT u.id, u.email, u.role, u.reseller_id, u.created_at, \
         COALESCE((SELECT COUNT(*) FROM sites WHERE user_id = u.id), 0) as site_count \
         FROM users u ORDER BY u.created_at ASC",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("list users", e))?;

    Ok(Json(users))
}

/// POST /api/users — Create a new user (admin only).
pub async fn create(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    headers: HeaderMap,
    Json(body): Json<CreateUserRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {

    if body.email.is_empty() || body.email.len() > 254 || !body.email.contains('@') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid email"));
    }
    if body.password.len() < 8 {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Password must be at least 8 characters",
        ));
    }

    let role = body.role.as_deref().unwrap_or("user");
    if !["admin", "reseller", "user"].contains(&role) {
        return Err(err(StatusCode::BAD_REQUEST, "Role must be admin, reseller, or user"));
    }

    // Check email uniqueness
    let existing: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM users WHERE email = $1")
            .bind(&body.email)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("create users", e))?;

    if existing.is_some() {
        return Err(err(StatusCode::CONFLICT, "Email already registered"));
    }

    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(body.password.as_bytes(), &salt)
        .map_err(|e| internal_error("create users", e))?
        .to_string();

    let user: User = sqlx::query_as(
        "INSERT INTO users (email, password_hash, role) VALUES ($1, $2, $3) RETURNING *",
    )
    .bind(&body.email)
    .bind(&hash)
    .bind(role)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("create users", e))?;

    tracing::info!("User created by {}: {} ({})", claims.email, user.email, role);
    let ip = crate::routes::client_ip(&headers);
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "user.create",
        Some("user"), Some(&user.email), Some(role), ip.as_deref(),
    ).await;

    // GAP 42: Send welcome email (best-effort, skip silently if SMTP not configured)
    {
        let panel_url = &state.config.base_url;
        let welcome_html = format!(
            r#"<div style="font-family: sans-serif; max-width: 600px; margin: 0 auto;">
                <h2 style="color: #4f46e5;">Welcome to DockPanel</h2>
                <p>Your DockPanel account has been created by an administrator.</p>
                <p><strong>Email:</strong> {}</p>
                <p><strong>Panel URL:</strong> <a href="{}">{}</a></p>
                <p>Please log in and change your password at your earliest convenience.</p>
                <p style="color: #9ca3af; font-size: 12px;">If you did not expect this account, please contact your administrator.</p>
            </div>"#,
            user.email, panel_url, panel_url
        );
        if let Err(e) = crate::services::email::send_email(
            &state.db, &user.email, "Welcome to DockPanel", &welcome_html
        ).await {
            tracing::debug!("Welcome email not sent to {}: {e}", user.email);
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

/// PUT /api/users/{id} — Update user role or password (admin only).
pub async fn update(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateUserRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {

    // Verify user exists
    let _user: User = sqlx::query_as("SELECT * FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("update users", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "User not found"))?;

    if let Some(ref role) = body.role {
        if !["admin", "reseller", "user"].contains(&role.as_str()) {
            return Err(err(StatusCode::BAD_REQUEST, "Role must be admin, reseller, or user"));
        }
        sqlx::query("UPDATE users SET role = $1, updated_at = NOW() WHERE id = $2")
            .bind(role)
            .bind(id)
            .execute(&state.db)
            .await
            .map_err(|e| internal_error("update users", e))?;
    }

    if let Some(ref password) = body.password {
        if password.len() < 8 {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "Password must be at least 8 characters",
            ));
        }
        let salt = SaltString::generate(&mut OsRng);
        let hash = Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| internal_error("update users", e))?
            .to_string();

        sqlx::query("UPDATE users SET password_hash = $1, updated_at = NOW() WHERE id = $2")
            .bind(&hash)
            .bind(id)
            .execute(&state.db)
            .await
            .map_err(|e| internal_error("update users", e))?;
    }

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "user.update",
        Some("user"), Some(&_user.email), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// DELETE /api/users/{id} — Delete a user (admin only, cannot delete self).
pub async fn remove(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {

    if id == claims.sub {
        return Err(err(StatusCode::BAD_REQUEST, "Cannot delete your own account"));
    }

    let user: User = sqlx::query_as("SELECT * FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("remove users", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "User not found"))?;

    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("remove users", e))?;

    tracing::info!("User deleted by {}: {}", claims.email, user.email);
    let ip = crate::routes::client_ip(&headers);
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "user.delete",
        Some("user"), Some(&user.email), None, ip.as_deref(),
    ).await;

    Ok(Json(serde_json::json!({ "ok": true, "email": user.email })))
}
