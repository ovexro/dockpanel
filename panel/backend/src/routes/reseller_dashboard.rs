use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;
use argon2::{
    password_hash::{rand_core::OsRng, SaltString},
    Argon2, PasswordHasher,
};

use crate::auth::ResellerUser;
use crate::error::{internal_error, err, ApiError};
use crate::services::activity;
use crate::AppState;

/// The roles a reseller may act on among its own sub-accounts.
///
/// An ALLOW-LIST, and it must stay one. `admin` and `reseller` are absent by
/// construction, so a privileged account that happens to carry a `reseller_id`
/// can never be reached through the two handlers that read this. Writing it as
/// `role <> 'admin'` would invert that property the first time a new privileged
/// role is added.
///
/// `suspended` is included deliberately: a reseller must be able to delete an
/// account the panel has suspended, and resetting the password of a suspended
/// account changes a hash that cannot be used to sign in until an administrator
/// restores the role — so including it grants no access the suspension withholds.
const RESELLER_MANAGEABLE_ROLES: &[&str] = &["user", "client", "suspended"];

#[derive(serde::Serialize)]
pub struct ResellerDashboardData {
    pub panel_name: Option<String>,
    pub used_users: i32,
    pub max_users: Option<i32>,
    pub used_sites: i32,
    pub max_sites: Option<i32>,
    pub used_databases: i32,
    pub max_databases: Option<i32>,
    pub server_count: i64,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct ResellerUserItem {
    pub id: Uuid,
    pub email: String,
    pub role: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub site_count: i64,
}

#[derive(serde::Deserialize)]
pub struct CreateUserRequest {
    pub email: String,
    pub password: String,
}

#[derive(serde::Deserialize)]
pub struct UpdateUserRequest {
    pub password: Option<String>,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct ResellerServerItem {
    pub id: Uuid,
    pub name: String,
    pub status: String,
    pub ip_address: Option<String>,
}

/// GET /api/reseller/dashboard — Reseller dashboard data (quota usage + server count).
pub async fn dashboard(
    State(state): State<AppState>,
    ResellerUser(claims): ResellerUser,
) -> Result<Json<ResellerDashboardData>, ApiError> {
    if claims.role != "reseller" {
        return Err(err(
            StatusCode::FORBIDDEN,
            "This endpoint is for resellers only (admin: use /api/resellers/{id})",
        ));
    }

    let profile: Option<(Option<String>, i32, Option<i32>, i32, Option<i32>, i32, Option<i32>)> =
        sqlx::query_as(
            "SELECT panel_name, used_users, max_users, used_sites, max_sites, \
             used_databases, max_databases \
             FROM reseller_profiles WHERE user_id = $1",
        )
        .bind(claims.sub)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("dashboard", e))?;

    let profile = profile.ok_or_else(|| err(StatusCode::NOT_FOUND, "Reseller profile not found"))?;

    let (server_count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM reseller_servers WHERE reseller_id = $1",
    )
    .bind(claims.sub)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("dashboard", e))?;

    Ok(Json(ResellerDashboardData {
        panel_name: profile.0,
        used_users: profile.1,
        max_users: profile.2,
        used_sites: profile.3,
        max_sites: profile.4,
        used_databases: profile.5,
        max_databases: profile.6,
        server_count,
    }))
}

/// GET /api/reseller/users — List users belonging to this reseller.
pub async fn list_users(
    State(state): State<AppState>,
    ResellerUser(claims): ResellerUser,
) -> Result<Json<Vec<ResellerUserItem>>, ApiError> {
    let users: Vec<ResellerUserItem> = if claims.role == "admin" {
        sqlx::query_as(
            "SELECT u.id, u.email, u.role, u.created_at, \
             COALESCE((SELECT COUNT(*) FROM sites WHERE user_id = u.id), 0) as site_count \
             FROM users u WHERE u.reseller_id IS NOT NULL \
             ORDER BY u.created_at ASC",
        )
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("list users", e))?
    } else {
        sqlx::query_as(
            "SELECT u.id, u.email, u.role, u.created_at, \
             COALESCE((SELECT COUNT(*) FROM sites WHERE user_id = u.id), 0) as site_count \
             FROM users u WHERE u.reseller_id = $1 \
             ORDER BY u.created_at ASC",
        )
        .bind(claims.sub)
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("list users", e))?
    };

    Ok(Json(users))
}

/// POST /api/reseller/users — Create a new user under this reseller.
pub async fn create_user(
    State(state): State<AppState>,
    ResellerUser(claims): ResellerUser,
    Json(body): Json<CreateUserRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    if claims.role != "reseller" {
        return Err(err(
            StatusCode::FORBIDDEN,
            "Only resellers can create users via this endpoint",
        ));
    }

    // Validate email
    if body.email.is_empty() || body.email.len() > 254 || !body.email.contains('@') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid email"));
    }

    // Validate password
    if body.password.len() < 8 {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Password must be at least 8 characters",
        ));
    }

    // Check email uniqueness
    let existing: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM users WHERE email = $1")
            .bind(&body.email)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("create user", e))?;

    if existing.is_some() {
        return Err(err(StatusCode::CONFLICT, "Email already registered"));
    }

    // Hash password
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(body.password.as_bytes(), &salt)
        .map_err(|e| internal_error("create user", e))?
        .to_string();

    // Atomically reserve the user-quota slot BEFORE creating (closes the TOCTOU that
    // let concurrent creates exceed max_users). reserve_reseller_user_slot both checks
    // and increments in one statement; release it if the INSERT then fails.
    reserve_reseller_user_slot(&state, claims.sub).await?;

    let (user_id, user_email) = match sqlx::query_as::<_, (Uuid, String)>(
        "INSERT INTO users (email, password_hash, role, reseller_id) \
         VALUES ($1, $2, 'user', $3) RETURNING id, email",
    )
    .bind(&body.email)
    .bind(&hash)
    .bind(claims.sub)
    .fetch_one(&state.db)
    .await
    {
        Ok(v) => v,
        Err(e) => {
            release_reseller_user_slot(&state, claims.sub).await;
            return Err(internal_error("create user", e));
        }
    };

    tracing::info!(
        "Reseller {} created user: {}",
        claims.email,
        user_email
    );
    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "reseller.user.create",
        Some("user"),
        Some(&user_email),
        None,
        None,
    )
    .await;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": user_id,
            "email": user_email,
            "role": "user",
        })),
    ))
}

/// PUT /api/reseller/users/{id} — Update a reseller's user (password only).
pub async fn update_user(
    State(state): State<AppState>,
    ResellerUser(claims): ResellerUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateUserRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Verify user belongs to this reseller (or admin has universal access)
    let user: Option<(Uuid, String)> = if claims.role == "admin" {
        sqlx::query_as("SELECT id, email FROM users WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("update user", e))?
    } else {
        sqlx::query_as(
            // ⚠ `role = ANY(...)`, not `role = 'user'`, and NOT `role <> 'admin'`.
            //
            // `list_users` above scopes on `reseller_id` alone, so a reseller's
            // table shows every account beneath them whatever its role — and it
            // renders Reset Password and Delete on every row (`ResellerUsers.tsx`
            // has no role column at all). This predicate then resolved to `None`
            // for anything that was not exactly `user`, and the caller met
            // "User not found" about an account listed on the same screen with its
            // site count beside it.
            //
            // Both ways in are ordinary admin actions, and one of them is the flow
            // `docs/guides/roles-and-ownership.md` prescribes: setting an account's
            // role to Client (`users::ASSIGNABLE_ROLES` contains it) or suspending
            // it (which writes `role = 'suspended'`). `helpers.rs:235` and `:343`
            // describe this exact mechanism — twice, as the reason an un-suspend
            // must never guess a role — and the reseller side was never brought
            // into step with it.
            //
            // Enumerated rather than negated on purpose: a role added to
            // `ASSIGNABLE_ROLES` tomorrow must not silently become something a
            // reseller may act on. `admin` and `reseller` are absent by
            // construction, so no widening of this list can hand a reseller the
            // password of an administrator who happens to carry their id.
            "SELECT id, email FROM users \
             WHERE id = $1 AND reseller_id = $2 AND role = ANY($3)",
        )
        .bind(id)
        .bind(claims.sub)
        .bind(RESELLER_MANAGEABLE_ROLES)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("update user", e))?
    };

    let user = user.ok_or_else(|| err(StatusCode::NOT_FOUND, "User not found"))?;

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
            .map_err(|e| internal_error("update user", e))?
            .to_string();

        sqlx::query("UPDATE users SET password_hash = $1, updated_at = NOW() WHERE id = $2")
            .bind(&hash)
            .bind(id)
            .execute(&state.db)
            .await
            .map_err(|e| internal_error("update user", e))?;

        // A password reset must evict the user's live token(s) (mirrors the admin
        // users::reset_password fix) — otherwise a stolen session survives the reset.
        crate::routes::auth::revoke_all_user_sessions(&state, id).await;
    }

    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "reseller.user.update",
        Some("user"),
        Some(&user.1),
        None,
        None,
    )
    .await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// DELETE /api/reseller/users/{id} — Delete a user under this reseller.
pub async fn delete_user(
    State(state): State<AppState>,
    ResellerUser(claims): ResellerUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if id == claims.sub {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Cannot delete your own account",
        ));
    }

    // Verify user belongs to this reseller (or admin has universal access)
    let user: Option<(Uuid, String)> = if claims.role == "admin" {
        sqlx::query_as("SELECT id, email FROM users WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("delete user", e))?
    } else {
        sqlx::query_as(
            // Same predicate as `update_user` — see the note there. Kept as one
            // shared constant rather than two literals precisely because these two
            // drifted apart from `list_users` and nothing noticed.
            "SELECT id, email FROM users \
             WHERE id = $1 AND reseller_id = $2 AND role = ANY($3)",
        )
        .bind(id)
        .bind(claims.sub)
        .bind(RESELLER_MANAGEABLE_ROLES)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("delete user", e))?
    };

    let user = user.ok_or_else(|| err(StatusCode::NOT_FOUND, "User not found"))?;

    // Revoke the user's live token(s) before deletion — otherwise the stateless JWT
    // keeps authenticating as a now-nonexistent principal until it expires (mirrors
    // users::remove).
    crate::routes::auth::revoke_all_user_sessions(&state, id).await;

    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("delete user", e))?;

    // Decrement used_users counter (floor at 0)
    if claims.role == "reseller" {
        sqlx::query(
            "UPDATE reseller_profiles SET used_users = GREATEST(used_users - 1, 0) WHERE user_id = $1",
        )
        .bind(claims.sub)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("delete user", e))?;
    }

    tracing::info!(
        "Reseller {} deleted user: {}",
        claims.email,
        user.1
    );
    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "reseller.user.delete",
        Some("user"),
        Some(&user.1),
        None,
        None,
    )
    .await;

    Ok(Json(serde_json::json!({ "ok": true, "email": user.1 })))
}

/// GET /api/reseller/servers — List servers allocated to this reseller.
pub async fn list_servers(
    State(state): State<AppState>,
    ResellerUser(claims): ResellerUser,
) -> Result<Json<Vec<ResellerServerItem>>, ApiError> {
    // Always scope to the calling user's reseller allocations
    // (admin uses /api/resellers/{id}/servers to see a specific reseller's servers)
    let servers: Vec<ResellerServerItem> = sqlx::query_as(
        "SELECT s.id, s.name, s.status, s.ip_address \
         FROM servers s \
         JOIN reseller_servers rs ON rs.server_id = s.id \
         WHERE rs.reseller_id = $1 \
         ORDER BY s.name ASC",
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("list servers", e))?;

    Ok(Json(servers))
}

/// Atomically reserve one user slot against the reseller's own quota (the reseller
/// owns the profile keyed by their own user_id). The conditional UPDATE ... RETURNING
/// makes the check and increment a single statement, closing the check-then-increment
/// TOCTOU. Mirrors databases::reserve_reseller_db_slot.
async fn reserve_reseller_user_slot(state: &AppState, reseller_id: Uuid) -> Result<(), ApiError> {
    let reserved: Option<i32> = sqlx::query_scalar(
        "UPDATE reseller_profiles SET used_users = used_users + 1, updated_at = NOW() \
         WHERE user_id = $1 AND (max_users IS NULL OR used_users < max_users) \
         RETURNING 1",
    )
    .bind(reseller_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("reserve reseller user quota", e))?;

    if reserved.is_some() {
        return Ok(());
    }
    // 0 rows: profile missing OR quota exhausted — distinguish for the right error.
    let has_profile: Option<(Uuid,)> = sqlx::query_as(
        "SELECT user_id FROM reseller_profiles WHERE user_id = $1",
    )
    .bind(reseller_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("reserve reseller user quota", e))?;

    if has_profile.is_some() {
        Err(err(StatusCode::FORBIDDEN, "User quota exceeded — upgrade your reseller plan"))
    } else {
        Err(err(StatusCode::NOT_FOUND, "Reseller profile not found"))
    }
}

/// Release a reserved user slot (compensating decrement) when the create fails after
/// reserve_reseller_user_slot succeeded.
async fn release_reseller_user_slot(state: &AppState, reseller_id: Uuid) {
    let _ = sqlx::query(
        "UPDATE reseller_profiles SET used_users = GREATEST(used_users - 1, 0), updated_at = NOW() \
         WHERE user_id = $1",
    )
    .bind(reseller_id)
    .execute(&state.db)
    .await;
}

/// Check if a reseller has quota remaining for a resource type.
/// Call before creating sites, databases, or users.
/// Returns Ok(()) if allowed. For direct users (no reseller), always allowed.
pub async fn check_reseller_quota(
    db: &sqlx::PgPool,
    user_id: Uuid,
    resource: &str,
) -> Result<(), ApiError> {
    // 1. Find the user's reseller_id
    let reseller_id: Option<(Option<Uuid>,)> =
        sqlx::query_as("SELECT reseller_id FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(db)
            .await
            .ok()
            .flatten();

    let rid = match reseller_id {
        Some((Some(rid),)) => rid,
        _ => return Ok(()), // No reseller = no quota enforcement
    };

    // 2. Fetch reseller profile quotas
    let profile: Option<(i32, Option<i32>, i32, Option<i32>, i32, Option<i32>)> = sqlx::query_as(
        "SELECT used_users, max_users, used_sites, max_sites, \
         used_databases, max_databases \
         FROM reseller_profiles WHERE user_id = $1",
    )
    .bind(rid)
    .fetch_optional(db)
    .await
    .map_err(|e| internal_error("check reseller quota", e))?;

    let profile = match profile {
        Some(p) => p,
        None => return Ok(()), // No profile = no quota enforcement
    };

    // 3. Check the specific resource quota
    let (used, max) = match resource {
        "users" => (profile.0, profile.1),
        "sites" => (profile.2, profile.3),
        "databases" => (profile.4, profile.5),
        _ => return Ok(()),
    };

    if let Some(max) = max {
        if used >= max {
            return Err(err(
                StatusCode::FORBIDDEN,
                &format!(
                    "Reseller quota exceeded for {resource} ({used}/{max})"
                ),
            ));
        }
    }

    Ok(())
}
