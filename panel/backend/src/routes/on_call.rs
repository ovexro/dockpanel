//! Phase 4 W3: on-call rotation admin API.
//!
//! Admin-only CRUD over `on_call_schedules` plus a `/whoami` endpoint that
//! lets non-admin operators check whether they're currently on-call without
//! exposing the schedule layout to them.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::{AdminUser, AuthUser};
use crate::error::{err, internal_error, ApiError};
use crate::services::on_call::resolve_on_call_user;
use crate::AppState;

#[derive(Serialize)]
pub struct MemberInfo {
    pub id: Uuid,
    pub email: String,
}

/// Surface shape: members + current-rotation pointer resolved to `{id, email}`
/// pairs so the UI can render email chips without N+1 round-trips.
#[derive(Serialize)]
pub struct OnCallScheduleDto {
    pub id: Uuid,
    pub name: String,
    pub cadence_days: i32,
    pub anchor_at: DateTime<Utc>,
    /// Members in rotation order. Orphan UUIDs (FK target deleted) appear
    /// with `email = "(deleted user)"` so the operator can prune them.
    pub members: Vec<MemberInfo>,
    pub current_on_call: Option<MemberInfo>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Deserialize)]
pub struct ScheduleInput {
    pub name: String,
    pub members: Vec<Uuid>,
    pub cadence_days: i32,
    /// Optional anchor override. Defaults to NOW() on create; on update the
    /// stored value is preserved when this field is absent so cadence math
    /// doesn't drift every PUT.
    #[serde(default)]
    pub anchor_at: Option<DateTime<Utc>>,
}

async fn load_member_emails(pool: &sqlx::PgPool, ids: &[Uuid]) -> Vec<MemberInfo> {
    if ids.is_empty() {
        return Vec::new();
    }
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, email FROM users WHERE id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    // Preserve the schedule's stored ordering rather than DB row order.
    ids.iter()
        .map(|id| {
            let email = rows
                .iter()
                .find(|(uid, _)| uid == id)
                .map(|(_, e)| e.clone())
                .unwrap_or_else(|| "(deleted user)".to_string());
            MemberInfo { id: *id, email }
        })
        .collect()
}

async fn schedule_to_dto(
    pool: &sqlx::PgPool,
    id: Uuid,
    name: String,
    members: Vec<Uuid>,
    cadence_days: i32,
    anchor_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
) -> OnCallScheduleDto {
    let member_info = load_member_emails(pool, &members).await;
    let current_uid = resolve_on_call_user(pool, id, Utc::now()).await;
    let current = current_uid.and_then(|uid| {
        member_info.iter().find(|m| m.id == uid).map(|m| MemberInfo {
            id: m.id,
            email: m.email.clone(),
        })
    });
    OnCallScheduleDto {
        id,
        name,
        cadence_days,
        anchor_at,
        members: member_info,
        current_on_call: current,
        created_at,
        updated_at,
    }
}

fn validate_input(input: &ScheduleInput) -> Result<(), ApiError> {
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 200 {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "name must be 1-200 characters",
        ));
    }
    if input.members.is_empty() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "members list cannot be empty",
        ));
    }
    let mut seen = std::collections::HashSet::with_capacity(input.members.len());
    for m in &input.members {
        if !seen.insert(*m) {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "members list contains duplicate user IDs",
            ));
        }
    }
    if !(1..=90).contains(&input.cadence_days) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "cadence_days must be between 1 and 90",
        ));
    }
    Ok(())
}

/// Reject a member UUID the caller may not page: anyone other than
/// themselves, or a user they directly manage (`users.reseller_id = caller`)
/// — the same self-or-managed boundary `escalation_policies.rs`'s
/// `validate_user_routes` enforces for `user:<uuid>` escalation steps.
///
/// ⚠ SECURITY: this used to only check the IDs existed (`SELECT COUNT(*)...
/// WHERE id = ANY($1)`), with no ownership term. A schedule's members feed
/// straight into `on_call_schedule:<uuid>` escalation routing, so an admin
/// could add ANY other user's UUID (trivially discoverable via `GET
/// /api/users`, which lists every user on the install) as a rotation member
/// of a schedule they own, and pages routed at that schedule would deliver
/// to the victim's real email/Slack/Discord/PagerDuty/webhook — the exact
/// sibling gap `validate_user_routes` closed at s437 for the `user:` route,
/// missed here because this table's own member list is a different path
/// into the same sink.
async fn validate_members_exist(
    pool: &sqlx::PgPool,
    members: &[Uuid],
    owner_id: Uuid,
) -> Result<(), ApiError> {
    let found: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM users WHERE id = ANY($1) AND (id = $2 OR reseller_id = $2)",
    )
    .bind(members)
    .bind(owner_id)
    .fetch_one(pool)
    .await
    .map_err(|e| internal_error("validate members", e))?;
    if (found as usize) != members.len() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "one or more member IDs do not match existing users",
        ));
    }
    Ok(())
}

/// GET /api/on-call/schedules — Admin: list this tenant's rotation schedules.
///
/// ⚠ SECURITY (s437): this table carried no `user_id` at all until this
/// release — every route here gated on `AdminUser` (role) alone, so any
/// admin on the install could list, read, write or delete every OTHER
/// tenant's on-call rotations. Scoped the same way `alerts.rs`/`servers.rs`
/// scope a caller-supplied resource: never widens for admin, admin included.
pub async fn list_schedules(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<Json<Vec<OnCallScheduleDto>>, ApiError> {
    let rows: Vec<(Uuid, String, Vec<Uuid>, i32, DateTime<Utc>, DateTime<Utc>, DateTime<Utc>)> =
        sqlx::query_as(
            "SELECT id, name, members, cadence_days, anchor_at, created_at, updated_at \
             FROM on_call_schedules WHERE user_id = $1 ORDER BY name ASC",
        )
        .bind(claims.sub)
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("list schedules", e))?;

    let mut out = Vec::with_capacity(rows.len());
    for (id, name, members, cadence, anchor, created, updated) in rows {
        out.push(
            schedule_to_dto(&state.db, id, name, members, cadence, anchor, created, updated)
                .await,
        );
    }
    Ok(Json(out))
}

/// GET /api/on-call/schedules/{id} — Admin: fetch one rotation by id, scoped
/// to the caller's own tenant.
pub async fn get_schedule(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<Json<OnCallScheduleDto>, ApiError> {
    let row: Option<(Uuid, String, Vec<Uuid>, i32, DateTime<Utc>, DateTime<Utc>, DateTime<Utc>)> =
        sqlx::query_as(
            "SELECT id, name, members, cadence_days, anchor_at, created_at, updated_at \
             FROM on_call_schedules WHERE id = $1 AND user_id = $2",
        )
        .bind(id)
        .bind(claims.sub)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("get schedule", e))?;

    let Some((id, name, members, cadence, anchor, created, updated)) = row else {
        return Err(err(StatusCode::NOT_FOUND, "Schedule not found"));
    };

    Ok(Json(
        schedule_to_dto(&state.db, id, name, members, cadence, anchor, created, updated).await,
    ))
}

/// POST /api/on-call/schedules — Admin: create a rotation, owned by the
/// creating admin's own tenant.
pub async fn create_schedule(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Json(input): Json<ScheduleInput>,
) -> Result<Json<OnCallScheduleDto>, ApiError> {
    validate_input(&input)?;
    validate_members_exist(&state.db, &input.members, claims.sub).await?;

    let anchor = input.anchor_at.unwrap_or_else(Utc::now);
    let row: (Uuid, String, Vec<Uuid>, i32, DateTime<Utc>, DateTime<Utc>, DateTime<Utc>) =
        sqlx::query_as(
            "INSERT INTO on_call_schedules (user_id, name, members, cadence_days, anchor_at) \
             VALUES ($1, $2, $3, $4, $5) \
             RETURNING id, name, members, cadence_days, anchor_at, created_at, updated_at",
        )
        .bind(claims.sub)
        .bind(input.name.trim())
        .bind(&input.members)
        .bind(input.cadence_days)
        .bind(anchor)
        .fetch_one(&state.db)
        .await
        .map_err(|e| internal_error("create schedule", e))?;

    Ok(Json(
        schedule_to_dto(&state.db, row.0, row.1, row.2, row.3, row.4, row.5, row.6).await,
    ))
}

/// PUT /api/on-call/schedules/{id} — Admin: update an existing rotation,
/// scoped to the caller's own tenant.
pub async fn update_schedule(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
    Json(input): Json<ScheduleInput>,
) -> Result<Json<OnCallScheduleDto>, ApiError> {
    validate_input(&input)?;
    validate_members_exist(&state.db, &input.members, claims.sub).await?;

    let row: Option<(Uuid, String, Vec<Uuid>, i32, DateTime<Utc>, DateTime<Utc>, DateTime<Utc>)> =
        if let Some(anchor) = input.anchor_at {
            sqlx::query_as(
                "UPDATE on_call_schedules \
                 SET name = $2, members = $3, cadence_days = $4, anchor_at = $5, updated_at = NOW() \
                 WHERE id = $1 AND user_id = $6 \
                 RETURNING id, name, members, cadence_days, anchor_at, created_at, updated_at",
            )
            .bind(id)
            .bind(input.name.trim())
            .bind(&input.members)
            .bind(input.cadence_days)
            .bind(anchor)
            .bind(claims.sub)
            .fetch_optional(&state.db)
            .await
        } else {
            // No anchor in payload → preserve existing anchor so cadence math
            // doesn't reset on every save (e.g. when only re-ordering members).
            sqlx::query_as(
                "UPDATE on_call_schedules \
                 SET name = $2, members = $3, cadence_days = $4, updated_at = NOW() \
                 WHERE id = $1 AND user_id = $5 \
                 RETURNING id, name, members, cadence_days, anchor_at, created_at, updated_at",
            )
            .bind(id)
            .bind(input.name.trim())
            .bind(&input.members)
            .bind(input.cadence_days)
            .bind(claims.sub)
            .fetch_optional(&state.db)
            .await
        }
        .map_err(|e| internal_error("update schedule", e))?;

    let Some((id, name, members, cadence, anchor, created, updated)) = row else {
        return Err(err(StatusCode::NOT_FOUND, "Schedule not found"));
    };

    Ok(Json(
        schedule_to_dto(&state.db, id, name, members, cadence, anchor, created, updated).await,
    ))
}

/// DELETE /api/on-call/schedules/{id} — Admin: remove a rotation.
///
/// Escalation-policy steps routing to this schedule (`on_call_schedule:<uuid>`)
/// are rewritten to `all_channels` in the same transaction as the delete. The
/// reference lives inside a JSONB blob, so no FK can do it for us, and a
/// dangling route is not inert: `route_to_user_ids` resolves it to zero users
/// and the page is dropped — including the *initial* page, because
/// `try_fire_alert` dispatches step 0 and returns. Every alert bound to the
/// policy would stop reaching email/Slack/PagerDuty entirely.
///
/// (An earlier version of this comment claimed an hourly `alert_engine`
/// orphan-route sweep repaired these. No such sweep exists — the engine's only
/// hourly task is the resolved-alert purge.)
///
/// ⚠ SECURITY (s437): both the delete and the orphan-route sweep below are
/// now scoped to the caller's own tenant. Scoping the sweep matters even
/// though `validate_schedule_routes` (escalation_policies.rs) now refuses to
/// let a policy reference another tenant's schedule going forward — a delete
/// must still never touch another tenant's rows, defense-in-depth against any
/// pre-existing or future cross-tenant reference.
pub async fn delete_schedule(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut tx = state
        .db
        .begin()
        .await
        .map_err(|e| internal_error("delete schedule", e))?;

    // Delete first so a request for a schedule that does not exist (or isn't
    // owned by this tenant) fails fast, without taking a lock on this
    // tenant's escalation_policies rows and rolling it back (an admin-UI
    // double-submit would otherwise lock every policy row twice). Same
    // transaction either way: if the rewrite below fails, the schedule is
    // not deleted.
    let result = sqlx::query("DELETE FROM on_call_schedules WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(claims.sub)
        .execute(&mut *tx)
        .await
        .map_err(|e| internal_error("delete schedule", e))?;

    if result.rows_affected() == 0 {
        // Dropping `tx` rolls back — nothing was touched.
        return Err(err(StatusCode::NOT_FOUND, "Schedule not found"));
    }

    let orphan_route = format!("on_call_schedule:{id}");
    let policies: Vec<(Uuid, serde_json::Value)> = sqlx::query_as(
        "SELECT id, steps FROM escalation_policies WHERE user_id = $1 FOR UPDATE",
    )
    .bind(claims.sub)
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| internal_error("delete schedule", e))?;

    let mut rewritten = 0usize;
    for (policy_id, steps_json) in policies {
        let Ok(mut steps) =
            serde_json::from_value::<Vec<crate::models::EscalationStep>>(steps_json)
        else {
            // Undecodable steps are already inert for dispatch; leave them alone
            // rather than rewriting a blob we can't round-trip.
            continue;
        };

        let mut changed = false;
        for step in steps.iter_mut() {
            if step.route == orphan_route {
                step.route = "all_channels".to_string();
                changed = true;
            }
        }
        if !changed {
            continue;
        }

        let new_steps = serde_json::to_value(&steps)
            .map_err(|e| internal_error("delete schedule", e))?;
        sqlx::query("UPDATE escalation_policies SET steps = $1, updated_at = NOW() WHERE id = $2")
            .bind(new_steps)
            .bind(policy_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| internal_error("delete schedule", e))?;
        rewritten += 1;
    }

    tx.commit()
        .await
        .map_err(|e| internal_error("delete schedule", e))?;

    if rewritten > 0 {
        tracing::info!(
            "Deleted on-call schedule {id}; rewrote routed steps in {rewritten} escalation \
             polic{} to all_channels",
            if rewritten == 1 { "y" } else { "ies" }
        );
    }

    Ok(Json(serde_json::json!({
        "ok": true,
        "policies_rewritten": rewritten,
    })))
}

/// Remove a deleted user from every on-call rota and rewrite any escalation
/// step routed directly at them.
///
/// Called from the user-delete path. Neither reference can be a foreign key —
/// `on_call_schedules.members` is a `UUID[]` and `escalation_policies.steps` is
/// JSONB — so nothing else cleans them up, and a dangling reference is not
/// inert: the rotation still hands out the dead UUID and the page goes nowhere.
///
/// Best-effort and non-fatal: a failure here must not block the delete, since
/// the stale reference is strictly less harmful than a half-deleted user. Any
/// route that ends up unroutable still degrades to the alert owner in
/// `dispatch_escalation_step`.
pub async fn scrub_deleted_user(db: &sqlx::PgPool, user_id: Uuid) {
    match sqlx::query(
        "UPDATE on_call_schedules SET members = array_remove(members, $1), updated_at = NOW() \
         WHERE $1 = ANY(members)",
    )
    .bind(user_id)
    .execute(db)
    .await
    {
        Ok(r) if r.rows_affected() > 0 => {
            tracing::info!(
                "Removed deleted user {user_id} from {} on-call schedule(s)",
                r.rows_affected()
            );
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("Could not scrub deleted user {user_id} from on-call rotas: {e}"),
    }

    let dead_route = format!("user:{user_id}");
    let policies: Vec<(Uuid, serde_json::Value)> =
        match sqlx::query_as("SELECT id, steps FROM escalation_policies")
            .fetch_all(db)
            .await
        {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("Could not scan escalation policies for deleted user {user_id}: {e}");
                return;
            }
        };

    for (policy_id, steps_json) in policies {
        let Ok(mut steps) = serde_json::from_value::<Vec<crate::models::EscalationStep>>(steps_json)
        else {
            continue;
        };

        let mut changed = false;
        for step in steps.iter_mut() {
            if step.route == dead_route {
                step.route = "all_channels".to_string();
                changed = true;
            }
        }
        if !changed {
            continue;
        }

        let Ok(new_steps) = serde_json::to_value(&steps) else {
            continue;
        };
        if let Err(e) =
            sqlx::query("UPDATE escalation_policies SET steps = $1, updated_at = NOW() WHERE id = $2")
                .bind(new_steps)
                .bind(policy_id)
                .execute(db)
                .await
        {
            tracing::warn!("Could not rewrite policy {policy_id} for deleted user {user_id}: {e}");
        } else {
            tracing::info!(
                "Rewrote escalation policy {policy_id} step routed at deleted user {user_id}"
            );
        }
    }
}

/// GET /api/on-call/whoami — Any authenticated user: am I currently on-call?
///
/// Returns the list of schedules where this user is the current rotation
/// holder. Empty array = not on-call right now.
pub async fn whoami(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let schedules: Vec<(Uuid, String, Vec<Uuid>, i32, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, name, members, cadence_days, anchor_at FROM on_call_schedules",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("whoami", e))?;

    let mut out = Vec::new();
    let now = Utc::now();
    for (id, name, _members, _cadence, _anchor) in schedules {
        // Re-resolve via the helper rather than recompute inline so the math
        // stays in exactly one place.
        if let Some(current_uid) = resolve_on_call_user(&state.db, id, now).await {
            if current_uid == claims.sub {
                out.push(serde_json::json!({
                    "schedule_id": id,
                    "schedule_name": name,
                }));
            }
        }
    }
    Ok(Json(out))
}
