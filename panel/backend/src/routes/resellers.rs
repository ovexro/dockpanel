use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::AdminUser;
use crate::error::{internal_error, err, ApiError};
use crate::services::activity;
use crate::AppState;

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct ResellerProfile {
    pub id: Uuid,
    pub user_id: Uuid,
    pub panel_name: Option<String>,
    pub max_users: Option<i32>,
    pub max_sites: Option<i32>,
    pub max_databases: Option<i32>,
    pub max_disk_mb: Option<i64>,
    pub max_email_accounts: Option<i32>,
    pub used_users: i32,
    pub used_sites: i32,
    pub used_databases: i32,
    pub logo_url: Option<String>,
    pub accent_color: Option<String>,
    pub hide_branding: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct ResellerListItem {
    pub id: Uuid,
    pub user_id: Uuid,
    pub email: String,
    pub panel_name: Option<String>,
    pub max_users: Option<i32>,
    pub max_sites: Option<i32>,
    pub max_databases: Option<i32>,
    pub used_users: i32,
    pub used_sites: i32,
    pub used_databases: i32,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Deserialize)]
pub struct CreateResellerRequest {
    pub user_id: Uuid,
    pub panel_name: Option<String>,
    pub max_users: Option<i32>,
    pub max_sites: Option<i32>,
    pub max_databases: Option<i32>,
    pub logo_url: Option<String>,
    pub accent_color: Option<String>,
    pub hide_branding: Option<bool>,
}

#[derive(serde::Deserialize)]
pub struct UpdateResellerRequest {
    pub panel_name: Option<String>,
    pub max_users: Option<i32>,
    pub max_sites: Option<i32>,
    pub max_databases: Option<i32>,
    pub max_disk_mb: Option<i64>,
    pub max_email_accounts: Option<i32>,
    pub logo_url: Option<String>,
    pub accent_color: Option<String>,
    pub hide_branding: Option<bool>,
}

#[derive(serde::Deserialize)]
pub struct AllocateServerRequest {
    pub server_id: Uuid,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct ResellerServer {
    pub id: Uuid,
    pub reseller_id: Uuid,
    pub server_id: Uuid,
    pub server_name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// GET /api/resellers — List all resellers with their profiles (admin only).
pub async fn list(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<Vec<ResellerListItem>>, ApiError> {
    let resellers: Vec<ResellerListItem> = sqlx::query_as(
        "SELECT rp.id, rp.user_id, u.email, rp.panel_name, \
         rp.max_users, rp.max_sites, rp.max_databases, \
         rp.used_users, rp.used_sites, rp.used_databases, \
         rp.created_at \
         FROM reseller_profiles rp \
         JOIN users u ON u.id = rp.user_id \
         WHERE u.role = 'reseller' \
         ORDER BY rp.created_at ASC",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("list resellers", e))?;

    Ok(Json(resellers))
}

/// POST /api/resellers — Promote a user to reseller (admin only).
pub async fn create(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Json(body): Json<CreateResellerRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    // Check user exists
    let user: Option<(Uuid, String, String)> =
        sqlx::query_as("SELECT id, email, role FROM users WHERE id = $1")
            .bind(body.user_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("create resellers", e))?;

    let (user_id, email, role) =
        user.ok_or_else(|| err(StatusCode::NOT_FOUND, "User not found"))?;

    if role == "admin" {
        return Err(err(StatusCode::BAD_REQUEST, "Cannot promote an admin to reseller"));
    }

    // Check if profile already exists
    if role == "reseller" {
        let existing: Option<(Uuid,)> =
            sqlx::query_as("SELECT id FROM reseller_profiles WHERE user_id = $1")
                .bind(user_id)
                .fetch_optional(&state.db)
                .await
                .map_err(|e| internal_error("create resellers", e))?;
        if existing.is_some() {
            return Err(err(StatusCode::CONFLICT, "Reseller profile already exists"));
        }
    }

    // Update user role to reseller (idempotent if already reseller)
    sqlx::query("UPDATE users SET role = 'reseller', updated_at = NOW() WHERE id = $1")
        .bind(user_id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("create resellers", e))?;

    // Insert reseller profile
    let profile: ResellerProfile = sqlx::query_as(
        "INSERT INTO reseller_profiles (user_id, panel_name, max_users, max_sites, max_databases, logo_url, accent_color, hide_branding) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, COALESCE($8, false)) RETURNING *",
    )
    .bind(user_id)
    .bind(&body.panel_name)
    .bind(body.max_users)
    .bind(body.max_sites)
    .bind(body.max_databases)
    .bind(&body.logo_url)
    .bind(&body.accent_color)
    .bind(body.hide_branding)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("create resellers", e))?;

    tracing::info!("User promoted to reseller by {}: {}", claims.email, email);
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "reseller.create",
        Some("reseller"), Some(&email), None, None,
    ).await;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": profile.id,
            "user_id": user_id,
            "email": email,
        })),
    ))
}

/// GET /api/resellers/{id} — Get reseller profile by profile id (admin only).
pub async fn get(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<Json<ResellerProfile>, ApiError> {
    let profile: ResellerProfile = sqlx::query_as(
        "SELECT * FROM reseller_profiles WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("get resellers", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Reseller profile not found"))?;

    Ok(Json(profile))
}

/// PUT /api/resellers/{id} — Update reseller quotas/branding (admin only).
pub async fn update(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateResellerRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Verify profile exists
    let profile: ResellerProfile = sqlx::query_as(
        "SELECT * FROM reseller_profiles WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("update resellers", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Reseller profile not found"))?;

    sqlx::query(
        "UPDATE reseller_profiles SET \
         panel_name = COALESCE($1, panel_name), \
         max_users = COALESCE($2, max_users), \
         max_sites = COALESCE($3, max_sites), \
         max_databases = COALESCE($4, max_databases), \
         max_disk_mb = COALESCE($5, max_disk_mb), \
         max_email_accounts = COALESCE($6, max_email_accounts), \
         logo_url = COALESCE($7, logo_url), \
         accent_color = COALESCE($8, accent_color), \
         hide_branding = COALESCE($9, hide_branding), \
         updated_at = NOW() \
         WHERE id = $10",
    )
    .bind(&body.panel_name)
    .bind(body.max_users)
    .bind(body.max_sites)
    .bind(body.max_databases)
    .bind(body.max_disk_mb)
    .bind(body.max_email_accounts)
    .bind(&body.logo_url)
    .bind(&body.accent_color)
    .bind(body.hide_branding)
    .bind(id)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("update resellers", e))?;

    // Get email for activity log
    let user_email: (String,) = sqlx::query_as("SELECT email FROM users WHERE id = $1")
        .bind(profile.user_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| internal_error("update resellers", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "reseller.update",
        Some("reseller"), Some(&user_email.0), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// DELETE /api/resellers/{id} — Demote reseller back to user (admin only).
pub async fn remove(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Get the profile to find user_id
    let profile: ResellerProfile = sqlx::query_as(
        "SELECT * FROM reseller_profiles WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("remove resellers", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Reseller profile not found"))?;

    // Get email for logging before we delete
    let user_email: (String,) = sqlx::query_as("SELECT email FROM users WHERE id = $1")
        .bind(profile.user_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| internal_error("remove resellers", e))?;

    // Delete reseller profile
    sqlx::query("DELETE FROM reseller_profiles WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("remove resellers", e))?;

    // Demote user back to regular user
    sqlx::query("UPDATE users SET role = 'user', updated_at = NOW() WHERE id = $1")
        .bind(profile.user_id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("remove resellers", e))?;

    // Clear reseller_id on any users that belonged to this reseller
    sqlx::query("UPDATE users SET reseller_id = NULL, updated_at = NOW() WHERE reseller_id = $1")
        .bind(profile.user_id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("remove resellers", e))?;

    // Delete server allocations
    sqlx::query("DELETE FROM reseller_servers WHERE reseller_id = $1")
        .bind(profile.user_id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("remove resellers", e))?;

    tracing::info!("Reseller demoted by {}: {}", claims.email, user_email.0);
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "reseller.delete",
        Some("reseller"), Some(&user_email.0), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true, "email": user_email.0 })))
}

/// GET /api/resellers/{id}/servers — List servers allocated to reseller (admin only).
pub async fn list_servers(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<ResellerServer>>, ApiError> {
    let servers: Vec<ResellerServer> = sqlx::query_as(
        "SELECT rs.id, rs.reseller_id, rs.server_id, s.name as server_name, rs.created_at \
         FROM reseller_servers rs \
         JOIN servers s ON s.id = rs.server_id \
         WHERE rs.reseller_id = (SELECT user_id FROM reseller_profiles WHERE id = $1) \
         ORDER BY rs.created_at ASC",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("list servers", e))?;

    Ok(Json(servers))
}

/// POST /api/resellers/{id}/servers — Allocate server to reseller (admin only).
pub async fn allocate_server(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
    Json(body): Json<AllocateServerRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    // Verify reseller profile exists and get user_id
    let profile: ResellerProfile = sqlx::query_as(
        "SELECT * FROM reseller_profiles WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("allocate server", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Reseller profile not found"))?;

    // Verify server exists
    let server_name: Option<(String,)> =
        sqlx::query_as("SELECT name FROM servers WHERE id = $1")
            .bind(body.server_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("allocate server", e))?;

    let server_name = server_name
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Server not found"))?
        .0;

    // Insert allocation
    let row: ResellerServer = sqlx::query_as(
        "INSERT INTO reseller_servers (reseller_id, server_id) VALUES ($1, $2) \
         RETURNING id, reseller_id, server_id, \
         (SELECT name FROM servers WHERE id = $2) as server_name, created_at",
    )
    .bind(profile.user_id)
    .bind(body.server_id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("allocate server", e))?;

    tracing::info!(
        "Server {} allocated to reseller {} by {}",
        server_name, profile.user_id, claims.email
    );
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "reseller.server.allocate",
        Some("server"), Some(&server_name), None, None,
    ).await;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": row.id,
            "reseller_id": row.reseller_id,
            "server_id": row.server_id,
            "server_name": row.server_name,
        })),
    ))
}

/// DELETE /api/resellers/{id}/servers/{server_id} — Deallocate server from reseller (admin only).
pub async fn deallocate_server(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path((id, server_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Get reseller user_id from profile
    let profile: ResellerProfile = sqlx::query_as(
        "SELECT * FROM reseller_profiles WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("deallocate server", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Reseller profile not found"))?;

    let result = sqlx::query(
        "DELETE FROM reseller_servers WHERE reseller_id = $1 AND server_id = $2",
    )
    .bind(profile.user_id)
    .bind(server_id)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("deallocate server", e))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Server allocation not found"));
    }

    tracing::info!(
        "Server {} deallocated from reseller {} by {}",
        server_id, profile.user_id, claims.email
    );
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "reseller.server.deallocate",
        Some("server"), Some(&server_id.to_string()), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}
