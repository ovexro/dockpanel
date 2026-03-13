use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{err, ApiError};
use crate::services::{activity, email};
use crate::AppState;

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct Team {
    pub id: Uuid,
    pub name: String,
    pub owner_id: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct TeamMember {
    pub id: Uuid,
    pub team_id: Uuid,
    pub user_id: Uuid,
    pub role: String,
    pub joined_at: chrono::DateTime<chrono::Utc>,
    pub email: String,
}

#[derive(serde::Deserialize)]
pub struct CreateTeam {
    pub name: String,
}

#[derive(serde::Deserialize)]
pub struct InviteRequest {
    pub email: String,
    pub role: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct AcceptInvite {
    pub token: String,
}

#[derive(serde::Deserialize)]
pub struct UpdateRole {
    pub role: String,
}

#[derive(sqlx::FromRow)]
struct TeamWithOwner {
    id: Uuid,
    name: String,
    owner_id: Uuid,
    created_at: chrono::DateTime<chrono::Utc>,
    owner_email: String,
}

/// GET /api/teams — List user's teams (owned + member of).
pub async fn list(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    // Single query: teams with owner email (eliminates N+1 for owner lookup)
    let teams: Vec<TeamWithOwner> = sqlx::query_as(
        "SELECT DISTINCT t.id, t.name, t.owner_id, t.created_at, ou.email as owner_email \
         FROM teams t \
         LEFT JOIN team_members tm ON tm.team_id = t.id \
         JOIN users ou ON ou.id = t.owner_id \
         WHERE t.owner_id = $1 OR tm.user_id = $1 \
         ORDER BY t.created_at DESC",
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    // Batch fetch all members for all teams in one query
    let team_ids: Vec<Uuid> = teams.iter().map(|t| t.id).collect();
    let members: Vec<TeamMember> = if team_ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as(
            "SELECT tm.id, tm.team_id, tm.user_id, tm.role, tm.joined_at, u.email \
             FROM team_members tm JOIN users u ON u.id = tm.user_id \
             WHERE tm.team_id = ANY($1) ORDER BY tm.joined_at ASC",
        )
        .bind(&team_ids)
        .fetch_all(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
    };

    // Group members by team_id
    let mut members_map: std::collections::HashMap<Uuid, Vec<&TeamMember>> = std::collections::HashMap::new();
    for m in &members {
        members_map.entry(m.team_id).or_default().push(m);
    }

    let result: Vec<serde_json::Value> = teams
        .iter()
        .map(|team| {
            let team_members = members_map.get(&team.id).cloned().unwrap_or_default();
            serde_json::json!({
                "id": team.id,
                "name": team.name,
                "owner_id": team.owner_id,
                "owner_email": team.owner_email,
                "created_at": team.created_at,
                "members": team_members,
                "is_owner": team.owner_id == claims.sub,
            })
        })
        .collect();

    Ok(Json(result))
}

/// POST /api/teams — Create a new team.
pub async fn create(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<CreateTeam>,
) -> Result<(StatusCode, Json<Team>), ApiError> {
    let name = body.name.trim();
    if name.is_empty() || name.len() > 100 {
        return Err(err(StatusCode::BAD_REQUEST, "Name must be 1-100 characters"));
    }

    // Limit teams per user (10)
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM teams WHERE owner_id = $1")
        .bind(claims.sub)
        .fetch_one(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if count.0 >= 10 {
        return Err(err(StatusCode::BAD_REQUEST, "Team limit reached (10)"));
    }

    let team: Team = sqlx::query_as(
        "INSERT INTO teams (name, owner_id) VALUES ($1, $2) RETURNING *",
    )
    .bind(name)
    .bind(claims.sub)
    .fetch_one(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    // Add owner as member with 'owner' role
    sqlx::query(
        "INSERT INTO team_members (team_id, user_id, role) VALUES ($1, $2, 'owner')",
    )
    .bind(team.id)
    .bind(claims.sub)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "team.create",
        Some("team"), Some(name), None, None,
    ).await;

    Ok((StatusCode::CREATED, Json(team)))
}

/// DELETE /api/teams/{id} — Delete a team (owner only).
pub async fn remove(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let team: Option<Team> = sqlx::query_as(
        "SELECT * FROM teams WHERE id = $1 AND owner_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let team = team.ok_or_else(|| err(StatusCode::NOT_FOUND, "Team not found or not owner"))?;

    sqlx::query("DELETE FROM teams WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "team.delete",
        Some("team"), Some(&team.name), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/teams/{id}/invite — Invite a user to a team (owner/admin only).
pub async fn invite(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(team_id): Path<Uuid>,
    Json(body): Json<InviteRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Verify caller is owner or admin of this team
    let member_role = get_member_role(&state, team_id, claims.sub).await?;
    if member_role != "owner" && member_role != "admin" {
        return Err(err(StatusCode::FORBIDDEN, "Only owners and admins can invite members"));
    }

    let email_addr = body.email.trim().to_lowercase();
    if email_addr.is_empty() || !email_addr.contains('@') {
        return Err(err(StatusCode::BAD_REQUEST, "Valid email required"));
    }

    let role = body.role.as_deref().unwrap_or("viewer");
    if !["admin", "developer", "viewer"].contains(&role) {
        return Err(err(StatusCode::BAD_REQUEST, "Role must be admin, developer, or viewer"));
    }

    // Check if already a member
    let existing: Option<(Uuid,)> = sqlx::query_as(
        "SELECT tm.id FROM team_members tm JOIN users u ON u.id = tm.user_id \
         WHERE tm.team_id = $1 AND u.email = $2",
    )
    .bind(team_id)
    .bind(&email_addr)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if existing.is_some() {
        return Err(err(StatusCode::CONFLICT, "User is already a member"));
    }

    // Generate invite token
    let token = format!(
        "{}{}",
        Uuid::new_v4().to_string().replace('-', ""),
        Uuid::new_v4().to_string().replace('-', ""),
    );

    let token_hash = crate::routes::auth::hash_token(&token);

    sqlx::query(
        "INSERT INTO team_invites (team_id, email, role, token) VALUES ($1, $2, $3, $4)",
    )
    .bind(team_id)
    .bind(&email_addr)
    .bind(role)
    .bind(&token_hash)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    // Get team name for email
    let team_name: (String,) = sqlx::query_as("SELECT name FROM teams WHERE id = $1")
        .bind(team_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    // Send invite email
    let base_url = &state.config.base_url;
    let invite_url = format!("{base_url}/teams/accept?token={token}");
    let _ = email::send_email(
        &state.db,
        &email_addr,
        &format!("Invitation to join {} on DockPanel", team_name.0),
        &format!(
            "<h2>You've been invited!</h2>\
             <p>{} has invited you to join <strong>{}</strong> as a <strong>{role}</strong>.</p>\
             <p><a href=\"{invite_url}\" style=\"padding:10px 20px;background:#4F46E5;color:#fff;text-decoration:none;border-radius:6px\">Accept Invitation</a></p>\
             <p>This invitation expires in 7 days.</p>",
            claims.email, team_name.0,
        ),
    )
    .await;

    Ok(Json(serde_json::json!({ "ok": true, "message": "Invitation sent" })))
}

/// POST /api/teams/accept — Accept a team invitation.
pub async fn accept_invite(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<AcceptInvite>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let token_hash = crate::routes::auth::hash_token(&body.token);

    let invite: Option<(Uuid, Uuid, String, String)> = sqlx::query_as(
        "SELECT id, team_id, email, role FROM team_invites \
         WHERE token = $1 AND expires_at > NOW()",
    )
    .bind(&token_hash)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let (invite_id, team_id, _email, role) = invite
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Invalid or expired invitation"))?;

    // Add user as team member
    sqlx::query(
        "INSERT INTO team_members (team_id, user_id, role) VALUES ($1, $2, $3) \
         ON CONFLICT (team_id, user_id) DO UPDATE SET role = $3",
    )
    .bind(team_id)
    .bind(claims.sub)
    .bind(&role)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    // Delete used invite
    sqlx::query("DELETE FROM team_invites WHERE id = $1")
        .bind(invite_id)
        .execute(&state.db)
        .await
        .ok();

    let team_name: (String,) = sqlx::query_as("SELECT name FROM teams WHERE id = $1")
        .bind(team_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "team.join",
        Some("team"), Some(&team_name.0), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true, "team": team_name.0 })))
}

/// PUT /api/teams/{id}/members/{member_id} — Update member role.
pub async fn update_member(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path((team_id, member_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<UpdateRole>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let caller_role = get_member_role(&state, team_id, claims.sub).await?;
    if caller_role != "owner" && caller_role != "admin" {
        return Err(err(StatusCode::FORBIDDEN, "Insufficient permissions"));
    }

    if !["admin", "developer", "viewer"].contains(&body.role.as_str()) {
        return Err(err(StatusCode::BAD_REQUEST, "Role must be admin, developer, or viewer"));
    }

    // Can't change owner's role
    let target: Option<(String,)> = sqlx::query_as(
        "SELECT role FROM team_members WHERE id = $1 AND team_id = $2",
    )
    .bind(member_id)
    .bind(team_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let target_role = target.ok_or_else(|| err(StatusCode::NOT_FOUND, "Member not found"))?.0;
    if target_role == "owner" {
        return Err(err(StatusCode::FORBIDDEN, "Cannot change owner's role"));
    }

    sqlx::query("UPDATE team_members SET role = $1 WHERE id = $2")
        .bind(&body.role)
        .bind(member_id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// DELETE /api/teams/{id}/members/{member_id} — Remove a team member.
pub async fn remove_member(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path((team_id, member_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let caller_role = get_member_role(&state, team_id, claims.sub).await?;
    if caller_role != "owner" && caller_role != "admin" {
        return Err(err(StatusCode::FORBIDDEN, "Insufficient permissions"));
    }

    // Can't remove owner
    let target: Option<(String,)> = sqlx::query_as(
        "SELECT role FROM team_members WHERE id = $1 AND team_id = $2",
    )
    .bind(member_id)
    .bind(team_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let target_role = target.ok_or_else(|| err(StatusCode::NOT_FOUND, "Member not found"))?.0;
    if target_role == "owner" {
        return Err(err(StatusCode::FORBIDDEN, "Cannot remove the team owner"));
    }

    sqlx::query("DELETE FROM team_members WHERE id = $1")
        .bind(member_id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn get_member_role(state: &AppState, team_id: Uuid, user_id: Uuid) -> Result<String, ApiError> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT role FROM team_members WHERE team_id = $1 AND user_id = $2",
    )
    .bind(team_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    row.map(|r| r.0)
        .ok_or_else(|| err(StatusCode::FORBIDDEN, "Not a member of this team"))
}
