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
    AdminUser(_claims): AdminUser,
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
/// The role values an operator may assign through this module.
///
/// ONE list, because there were three identical copies — create, update and the
/// un-suspend restore — and adding `client` to the database CHECK and to the user
/// editor still left all three refusing it, so the role existed everywhere except
/// where it is written. `suspended` is deliberately absent: it is reached through
/// `toggle_suspend`, never assigned directly.
const ASSIGNABLE_ROLES: [&str; 4] = ["admin", "reseller", "user", "client"];

/// The message that names them. `err` takes a `&str`, so this cannot be built
/// from the list at runtime — a unit test asserts it mentions every one instead,
/// which is the same guarantee by a different route.
const ASSIGNABLE_ROLES_MSG: &str = "Role must be one of: admin, reseller, user, client";


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
    if !ASSIGNABLE_ROLES.contains(&role) {
        return Err(err(StatusCode::BAD_REQUEST, ASSIGNABLE_ROLES_MSG));
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
        if !ASSIGNABLE_ROLES.contains(&role.as_str()) {
            return Err(err(StatusCode::BAD_REQUEST, ASSIGNABLE_ROLES_MSG));
        }
        let role_changed = role.as_str() != _user.role;
        // An admin cannot change their OWN role (mirrors the self-guards on
        // toggle_suspend/remove): prevents accidental self-lockout AND closes the
        // re-escalation path where a stale admin token flips itself back to admin.
        if id == claims.sub && role_changed {
            return Err(err(StatusCode::BAD_REQUEST, "Cannot change your own role"));
        }
        // Only mutate + revoke on an ACTUAL change — a no-op self-submit must not log the
        // caller out. The last-admin guard is folded into the UPDATE's WHERE so a demotion
        // can never orphan the panel of admins (the row updates only if the new role is
        // admin, the current role isn't admin, or more than one admin remains); this is
        // atomic-as-a-statement rather than the check-then-act it replaced. Clearing
        // reseller_id on promotion keeps a tenant-root from staying owned by another
        // reseller (PRIV-03).
        if role_changed {
            let res = sqlx::query(
                "UPDATE users SET role = $1, \
                 reseller_id = CASE WHEN $1 IN ('reseller','admin') THEN NULL ELSE reseller_id END, \
                 updated_at = NOW() \
                 WHERE id = $2 \
                   AND ($1 = 'admin' OR role <> 'admin' OR (SELECT COUNT(*) FROM users WHERE role = 'admin') > 1)",
            )
            .bind(role)
            .bind(id)
            .execute(&state.db)
            .await
            .map_err(|e| internal_error("update users", e))?;

            if res.rows_affected() == 0 {
                // The user exists (fetched above), so a 0-row update means the last-admin
                // guard in the WHERE blocked the demotion.
                return Err(err(StatusCode::BAD_REQUEST, "Cannot demote the last admin"));
            }

            // Role change must take effect immediately — actually revoke the victim's live
            // token(s). A plain DELETE of user_sessions is a no-op against the stateless JWT.
            crate::routes::auth::revoke_all_user_sessions(&state, id).await;
        }
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

        // A password change must evict the user's live token(s) — same invariant as
        // reset_password (admin + self-service) and reseller update_user.
        crate::routes::auth::revoke_all_user_sessions(&state, id).await;
    }

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "user.update",
        Some("user"), Some(&_user.email), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/users/{id}/toggle-suspend — Suspend or un-suspend a user (admin only).
///
/// When suspended, the user's role is set to "suspended" (previous role is stored in reset_token
/// field temporarily). Un-suspending restores the original role. All sessions are invalidated.
pub async fn toggle_suspend(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if id == claims.sub {
        return Err(err(StatusCode::BAD_REQUEST, "Cannot suspend your own account"));
    }

    let user: User = sqlx::query_as("SELECT * FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("toggle_suspend", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "User not found"))?;

    let (new_role, action) = if user.role == "suspended" {
        // Un-suspend: restore previous role (stored in reset_token) or default to "user"
        let original_role = user.reset_token.as_deref().unwrap_or("user");
        let role = if ASSIGNABLE_ROLES.contains(&original_role) {
            original_role.to_string()
        } else {
            "user".to_string()
        };
        (role, "user.unsuspend")
    } else {
        // Suspend: save current role in reset_token, set role to "suspended"
        sqlx::query("UPDATE users SET reset_token = $1 WHERE id = $2")
            .bind(&user.role)
            .bind(id)
            .execute(&state.db)
            .await
            .map_err(|e| internal_error("toggle_suspend", e))?;
        ("suspended".to_string(), "user.suspend")
    };

    sqlx::query("UPDATE users SET role = $1, updated_at = NOW() WHERE id = $2")
        .bind(&new_role)
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("toggle_suspend", e))?;

    // Actually revoke the user's live token(s) — a plain DELETE of user_sessions does
    // not invalidate the stateless JWT, so suspension would otherwise be a no-op for
    // up to 2h (and the suspended user could keep whatever role their token carries).
    crate::routes::auth::revoke_all_user_sessions(&state, id).await;

    tracing::info!("{action} by {}: {} -> role={new_role}", claims.email, user.email);
    let ip = crate::routes::client_ip(&headers);
    activity::log_activity(
        &state.db, claims.sub, &claims.email, action,
        Some("user"), Some(&user.email), Some(&new_role), ip.as_deref(),
    ).await;

    Ok(Json(serde_json::json!({
        "ok": true,
        "email": user.email,
        "role": new_role,
        "suspended": new_role == "suspended",
    })))
}

#[derive(serde::Deserialize)]
pub struct ResetPasswordRequest {
    pub password: String,
}

/// POST /api/users/{id}/reset-password — Admin resets a user's password.
///
/// Hashes the new password with Argon2, updates the DB, and invalidates all sessions.
pub async fn reset_password(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<ResetPasswordRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.password.len() < 8 {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Password must be at least 8 characters",
        ));
    }
    if body.password.len() > 128 {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Password must be at most 128 characters",
        ));
    }

    let user: User = sqlx::query_as("SELECT * FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("reset_password", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "User not found"))?;

    // Hash new password with Argon2
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(body.password.as_bytes(), &salt)
        .map_err(|e| internal_error("reset_password", e))?
        .to_string();

    // Update password
    sqlx::query("UPDATE users SET password_hash = $1, updated_at = NOW() WHERE id = $2")
        .bind(&hash)
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("reset_password", e))?;

    // Actually revoke the user's live token(s) so a stolen/active session cannot
    // survive the reset — matches the self-service reset path (auth.rs).
    crate::routes::auth::revoke_all_user_sessions(&state, id).await;

    tracing::info!("Password reset by admin {} for user {}", claims.email, user.email);
    let ip = crate::routes::client_ip(&headers);
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "user.reset_password",
        Some("user"), Some(&user.email), None, ip.as_deref(),
    ).await;

    Ok(Json(serde_json::json!({ "ok": true, "email": user.email })))
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

    // Revoke the user's live token(s) BEFORE deletion — otherwise the stateless JWT
    // keeps authenticating as a now-nonexistent principal until it expires.
    crate::routes::auth::revoke_all_user_sessions(&state, id).await;

    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("remove users", e))?;

    // Scrub the on-call references no foreign key can reach: rota membership is
    // a UUID[] and escalation routes are opaque strings inside a JSONB blob.
    // Leaving them dangling silently drops pages — `resolve_on_call_user` indexes
    // straight into `members` and hands back the deleted UUID, so the rotation
    // "pages" a principal that no longer exists. The write path already refuses
    // nonexistent members (on_call::validate_members_exist); this makes the
    // delete path reciprocate.
    //
    // Strictly AFTER the delete: several FKs to `users` are NO ACTION (maintenance
    // windows, deploy approvals), so the DELETE can legitimately fail — and this
    // scrub is irreversible. Running it first would strip a still-existing user
    // from every rotation and then return 500, leaving the operator with an
    // "internal error" and a silently gutted on-call schedule.
    crate::routes::on_call::scrub_deleted_user(&state.db, id).await;

    tracing::info!("User deleted by {}: {}", claims.email, user.email);
    let ip = crate::routes::client_ip(&headers);
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "user.delete",
        Some("user"), Some(&user.email), None, ip.as_deref(),
    ).await;

    Ok(Json(serde_json::json!({ "ok": true, "email": user.email })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_role_message_names_every_assignable_role() {
        // The list and the sentence describing it are two facts that must agree,
        // and `err` takes a `&str` so the sentence cannot be built from the list.
        // Adding a role to the array and forgetting the message would tell an
        // operator their valid role is invalid.
        for role in ASSIGNABLE_ROLES {
            assert!(
                ASSIGNABLE_ROLES_MSG.contains(role),
                "role {role:?} is assignable but the error message does not name it"
            );
        }
    }

    #[test]
    fn suspended_is_not_directly_assignable() {
        // It is reached through toggle_suspend, which also stashes the previous
        // role. Allowing it here would suspend an account without recording what
        // to restore it to.
        assert!(!ASSIGNABLE_ROLES.contains(&"suspended"));
    }

    #[test]
    fn the_client_role_is_assignable_and_survives_an_unsuspend() {
        // s312 / GitHub #51. Three separate copies of this list existed — create,
        // update and the un-suspend restore — so a role added to the database
        // CHECK and the user editor was still refused by all three. The restore
        // path matters most: a role missing from the list is silently downgraded
        // to "user" when the account is un-suspended, which would hand a client
        // back a role that can create sites.
        assert!(ASSIGNABLE_ROLES.contains(&"client"));
    }
}
