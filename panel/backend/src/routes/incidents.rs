use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::{AdminUser, AuthUser};
use crate::error::{internal_error, err, paginate, ApiError};
use crate::services::activity;
use crate::services::extensions::fire_event;
use crate::services::notifications;
use crate::AppState;

// ── Types ───────────────────────────────────────────────────────────────────

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct ManagedIncident {
    pub id: Uuid,
    pub user_id: Uuid,
    pub title: String,
    pub status: String,
    pub severity: String,
    pub description: Option<String>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub resolved_at: Option<chrono::DateTime<chrono::Utc>>,
    pub postmortem: Option<String>,
    pub postmortem_published: bool,
    pub visible_on_status_page: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct IncidentUpdate {
    pub id: Uuid,
    pub incident_id: Uuid,
    pub status: String,
    pub message: String,
    pub author_email: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Deserialize)]
pub struct CreateIncidentRequest {
    pub title: String,
    pub severity: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub component_ids: Option<Vec<Uuid>>,
    pub visible_on_status_page: Option<bool>,
}

#[derive(serde::Deserialize)]
pub struct UpdateIncidentRequest {
    pub title: Option<String>,
    pub status: Option<String>,
    pub severity: Option<String>,
    pub description: Option<String>,
    pub message: Option<String>,
    pub postmortem: Option<String>,
    pub postmortem_published: Option<bool>,
    pub visible_on_status_page: Option<bool>,
}

#[derive(serde::Deserialize)]
pub struct IncidentListQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub status: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct PostUpdateRequest {
    pub status: String,
    pub message: String,
}

const VALID_STATUSES: &[&str] = &["investigating", "identified", "monitoring", "resolved", "postmortem"];
const VALID_SEVERITIES: &[&str] = &["minor", "major", "critical", "maintenance"];

/// The two statuses that close an incident. Everything else in `VALID_STATUSES`
/// is open, which is the definition `dashboard.rs`, `deploy.rs` and
/// `git_deploys.rs` already spell as `status NOT IN ('resolved', 'postmortem')`.
/// Named here so a count of open incidents cannot quietly mean a third thing.
const CLOSED_STATUSES: &[&str] = &["resolved", "postmortem"];

// ── Incident CRUD ───────────────────────────────────────────────────────────

/// GET /api/incidents — List incidents.
pub async fn list(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Query(params): Query<IncidentListQuery>,
) -> Result<Json<Vec<ManagedIncident>>, ApiError> {
    let (limit, offset) = paginate(params.limit, params.offset);

    let incidents: Vec<ManagedIncident> = if let Some(status) = &params.status {
        sqlx::query_as(
            "SELECT * FROM managed_incidents WHERE user_id = $1 AND status = $2 ORDER BY started_at DESC LIMIT $3 OFFSET $4"
        )
        .bind(claims.sub).bind(status).bind(limit).bind(offset)
        .fetch_all(&state.db).await
    } else {
        sqlx::query_as(
            "SELECT * FROM managed_incidents WHERE user_id = $1 ORDER BY started_at DESC LIMIT $2 OFFSET $3"
        )
        .bind(claims.sub).bind(limit).bind(offset)
        .fetch_all(&state.db).await
    }
    .map_err(|e| internal_error("list incidents", e))?;

    Ok(Json(incidents))
}

/// GET /api/incidents/summary — How many incidents are open, and in what state.
///
/// The sidebar badge used to derive this itself by fetching `?status=investigating`
/// and taking the array length. That asked a narrower question than the panel's
/// own: `investigating` is one of THREE open statuses, and the first thing the
/// incident screen offers is to move a new incident to `identified` — at which
/// point it left the badge while the dashboard went on counting it. A count that
/// disagrees with the page underneath it is worse than no count, so there is now
/// one place that answers it.
pub async fn summary(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let counts: Vec<(String, i64)> = sqlx::query_as(
        "SELECT status, COUNT(*) FROM managed_incidents WHERE user_id = $1 GROUP BY status",
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("incident summary", e))?;

    let by_status: serde_json::Map<String, serde_json::Value> = counts
        .iter()
        .map(|(s, c)| (s.clone(), serde_json::json!(c)))
        .collect();

    let open: i64 = counts
        .iter()
        .filter(|(s, _)| !CLOSED_STATUSES.contains(&s.as_str()))
        .map(|(_, c)| *c)
        .sum();

    Ok(Json(serde_json::json!({ "open": open, "by_status": by_status })))
}

/// POST /api/incidents — Create an incident.
pub async fn create(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Json(req): Json<CreateIncidentRequest>,
) -> Result<(StatusCode, Json<ManagedIncident>), ApiError> {
    if req.title.is_empty() || req.title.len() > 200 {
        return Err(err(StatusCode::BAD_REQUEST, "Title must be 1-200 characters"));
    }

    let status = req.status.as_deref().unwrap_or("investigating");
    if !VALID_STATUSES.contains(&status) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid status"));
    }

    let severity = req.severity.as_deref().unwrap_or("major");
    if !VALID_SEVERITIES.contains(&severity) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid severity"));
    }

    let incident: ManagedIncident = sqlx::query_as(
        "INSERT INTO managed_incidents (user_id, title, status, severity, description, visible_on_status_page) \
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING *"
    )
    .bind(claims.sub)
    .bind(&req.title)
    .bind(status)
    .bind(severity)
    .bind(&req.description)
    .bind(req.visible_on_status_page.unwrap_or(true))
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("create incidents", e))?;

    // Link affected components
    if let Some(component_ids) = &req.component_ids {
        for cid in component_ids {
            let _ = sqlx::query(
                "INSERT INTO managed_incident_components (incident_id, component_id) VALUES ($1, $2) ON CONFLICT DO NOTHING"
            )
            .bind(incident.id).bind(cid)
            .execute(&state.db).await;
        }
    }

    // Create initial update
    let _ = sqlx::query(
        "INSERT INTO incident_updates (incident_id, status, message, author_email) VALUES ($1, $2, $3, $4)"
    )
    .bind(incident.id)
    .bind(status)
    .bind(req.description.as_deref().unwrap_or("Incident created"))
    .bind(&claims.email)
    .execute(&state.db)
    .await;

    // Notify subscribers
    notify_subscribers(&incident.title, status, req.description.as_deref().unwrap_or(""), incident.user_id);

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "incident.create",
        Some("incident"), Some(&req.title), None, None,
    ).await;

    fire_event(&state.db, "incident.created", serde_json::json!({
        "incident_id": incident.id, "title": &req.title, "severity": &incident.severity, "status": &incident.status,
    }));

    // Panel notification
    notifications::notify_panel(&state.db, Some(claims.sub), &format!("Incident: {}", req.title), req.description.as_deref().unwrap_or("New incident created"), severity, "incident", Some("/incidents")).await;

    Ok((StatusCode::CREATED, Json(incident)))
}

/// GET /api/incidents/{id} — Get incident with updates.
pub async fn get_one(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let incident: ManagedIncident = sqlx::query_as(
        "SELECT * FROM managed_incidents WHERE id = $1 AND user_id = $2"
    )
    .bind(id).bind(claims.sub)
    .fetch_optional(&state.db).await
    .map_err(|e| internal_error("get_one incidents", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Incident not found"))?;

    let updates: Vec<IncidentUpdate> = sqlx::query_as(
        "SELECT * FROM incident_updates WHERE incident_id = $1 ORDER BY created_at ASC LIMIT 500"
    )
    .bind(id)
    .fetch_all(&state.db).await
    .map_err(|e| internal_error("get_one incidents", e))?;

    let component_ids: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT component_id FROM managed_incident_components WHERE incident_id = $1"
    )
    .bind(id)
    .fetch_all(&state.db).await
    .unwrap_or_default();

    Ok(Json(serde_json::json!({
        "incident": incident,
        "updates": updates,
        "component_ids": component_ids.iter().map(|(id,)| id).collect::<Vec<_>>(),
    })))
}

/// PUT /api/incidents/{id} — Update an incident (status change, add update).
pub async fn update(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateIncidentRequest>,
) -> Result<Json<ManagedIncident>, ApiError> {
    // Verify ownership
    let existing: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM managed_incidents WHERE id = $1 AND user_id = $2"
    )
    .bind(id).bind(claims.sub)
    .fetch_optional(&state.db).await
    .map_err(|e| internal_error("update incidents", e))?;

    if existing.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Incident not found"));
    }

    if let Some(ref s) = req.status {
        if !VALID_STATUSES.contains(&s.as_str()) {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid status"));
        }
    }

    if let Some(ref s) = req.severity {
        if !VALID_SEVERITIES.contains(&s.as_str()) {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid severity"));
        }
    }

    // Handle resolved_at
    //
    // Reopening had to clear it and did not. The stamp survived the transition
    // back to a live status, so the public status page rendered "Resolved <n>
    // ago" (PublicStatusPage.tsx) for an incident whose own status said
    // `investigating` — the page contradicted itself, and the more prominent
    // half was the reassuring one.
    //
    // `postmortem` follows a resolution rather than undoing it, so it keeps the
    // stamp. `None` and `""` mean the caller is editing something else (a
    // description, a severity) and must not disturb the timestamp at all.
    let resolved_at_clause = match req.status.as_deref() {
        Some("resolved") => ", resolved_at = NOW()",
        Some("investigating") | Some("identified") | Some("monitoring") => ", resolved_at = NULL",
        _ => "",
    };

    let query = format!(
        "UPDATE managed_incidents SET \
         title = COALESCE(NULLIF($2, ''), title), \
         status = COALESCE(NULLIF($3, ''), status), \
         severity = COALESCE(NULLIF($4, ''), severity), \
         description = COALESCE($5, description), \
         postmortem = COALESCE($6, postmortem), \
         postmortem_published = COALESCE($7, postmortem_published), \
         visible_on_status_page = COALESCE($8, visible_on_status_page), \
         updated_at = NOW(){resolved_at_clause} \
         WHERE id = $1 RETURNING *"
    );

    let incident: ManagedIncident = sqlx::query_as(&query)
        .bind(id)
        .bind(req.title.as_deref().unwrap_or(""))
        .bind(req.status.as_deref().unwrap_or(""))
        .bind(req.severity.as_deref().unwrap_or(""))
        .bind(&req.description)
        .bind(&req.postmortem)
        .bind(req.postmortem_published)
        .bind(req.visible_on_status_page)
        .fetch_one(&state.db)
        .await
        .map_err(|e| internal_error("update incidents", e))?;

    // GAP 34: Auto-populate postmortem template when transitioning to "postmortem"
    if req.status.as_deref() == Some("postmortem") && req.postmortem.is_none() {
        // Check if postmortem is currently empty
        let current_pm: Option<(Option<String>,)> = sqlx::query_as(
            "SELECT postmortem FROM managed_incidents WHERE id = $1"
        ).bind(id).fetch_optional(&state.db).await
            .map_err(|e| internal_error("incident postmortem check", e))?;

        let pm_empty = current_pm
            .as_ref()
            .map(|(pm,)| pm.as_ref().map_or(true, |s| s.is_empty()))
            .unwrap_or(true);

        if pm_empty {
            // Fetch timeline from incident updates
            let updates: Vec<(String, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
                "SELECT status, message, created_at FROM incident_updates WHERE incident_id = $1 ORDER BY created_at"
            ).bind(id).fetch_all(&state.db).await.unwrap_or_default();

            let timeline = updates.iter()
                .map(|(s, m, t)| format!("- **{}** [{}]: {}", t.format("%H:%M UTC"), s, m))
                .collect::<Vec<_>>()
                .join("\n");

            let template = format!(
                "## Incident Postmortem\n\n\
                 ### Summary\n[Describe the incident]\n\n\
                 ### Timeline\n{}\n\n\
                 ### Root Cause\n[What caused this?]\n\n\
                 ### Resolution\n[How was it fixed?]\n\n\
                 ### Action Items\n- [ ] \n",
                timeline
            );

            let _ = sqlx::query(
                "UPDATE managed_incidents SET postmortem = $1 WHERE id = $2 AND (postmortem IS NULL OR postmortem = '')"
            ).bind(&template).bind(id).execute(&state.db).await;
        }
    }

    // If a status change message was provided, create an update
    if let Some(ref message) = req.message {
        let update_status = req.status.as_deref().unwrap_or(&incident.status);
        let _ = sqlx::query(
            "INSERT INTO incident_updates (incident_id, status, message, author_email) VALUES ($1, $2, $3, $4)"
        )
        .bind(id).bind(update_status).bind(message).bind(&claims.email)
        .execute(&state.db).await;

        // Notify subscribers of update
        notify_subscribers(&incident.title, update_status, message, incident.user_id);
    }

    // Re-fetch after postmortem auto-populate to return the complete record
    let incident: ManagedIncident = sqlx::query_as(
        "SELECT * FROM managed_incidents WHERE id = $1"
    ).bind(id).fetch_one(&state.db).await
    .map_err(|e| internal_error("update incidents", e))?;

    Ok(Json(incident))
}

/// DELETE /api/incidents/{id} — Delete an incident.
pub async fn remove(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = sqlx::query("DELETE FROM managed_incidents WHERE id = $1 AND user_id = $2")
        .bind(id).bind(claims.sub)
        .execute(&state.db).await
        .map_err(|e| internal_error("remove incidents", e))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Incident not found"));
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/incidents/{id}/updates — Post an incident update.
pub async fn post_update(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
    Json(req): Json<PostUpdateRequest>,
) -> Result<(StatusCode, Json<IncidentUpdate>), ApiError> {
    // Verify ownership
    let existing: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM managed_incidents WHERE id = $1 AND user_id = $2"
    )
    .bind(id).bind(claims.sub)
    .fetch_optional(&state.db).await
    .map_err(|e| internal_error("post update", e))?;

    if existing.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Incident not found"));
    }

    if !VALID_STATUSES.contains(&req.status.as_str()) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid status"));
    }

    // Update incident status
    if req.status == "resolved" {
        let _ = sqlx::query("UPDATE managed_incidents SET status = $2, resolved_at = NOW(), updated_at = NOW() WHERE id = $1")
            .bind(id).bind(&req.status).execute(&state.db).await;

        // GAP 16: Auto-resolve linked alerts when incident is resolved
        let incident_title: Option<(String,)> = sqlx::query_as("SELECT title FROM managed_incidents WHERE id = $1")
            .bind(id).fetch_optional(&state.db).await
            .map_err(|e| internal_error("incident auto-resolve title lookup", e))?;
        if let Some((ref title,)) = incident_title {
            // Scoped to the incident owner. alerts.title is auto-generated from
            // the server/service name ("Server vps is offline"), so a title-only
            // match reached across the whole table: resolving your own incident
            // silently resolved every other tenant's identically-titled firing
            // alert, clearing it from their dashboard and stopping escalation on
            // a live outage. Ownership was verified on the incident (above) but
            // never carried into this UPDATE.
            let _ = sqlx::query(
                "UPDATE alerts SET status = 'resolved', resolved_at = NOW() \
                 WHERE title = $1 AND user_id = $2 AND status IN ('firing', 'acknowledged')"
            ).bind(title).bind(claims.sub).execute(&state.db).await;
        }

        // Clear status_override on linked components
        let _ = sqlx::query(
            "UPDATE status_page_components SET status_override = NULL \
             WHERE id IN (SELECT component_id FROM managed_incident_components WHERE incident_id = $1)"
        ).bind(id).execute(&state.db).await;

        fire_event(&state.db, "incident.resolved", serde_json::json!({ "incident_id": id }));

        // Panel notification for resolution
        if let Some((ref title,)) = incident_title {
            notifications::notify_panel(&state.db, Some(claims.sub), &format!("Resolved: {}", title), "Incident has been resolved", "info", "incident", Some("/incidents")).await;
        }
    } else {
        // Same reopen rule as the PUT path: a move back to a live status clears
        // the resolution stamp, `postmortem` keeps it. Both entry points write
        // this column, so fixing one and not the other would leave the defect
        // reachable through whichever endpoint the UI happens to call.
        let sql = if matches!(req.status.as_str(), "investigating" | "identified" | "monitoring") {
            "UPDATE managed_incidents SET status = $2, resolved_at = NULL, updated_at = NOW() WHERE id = $1"
        } else {
            "UPDATE managed_incidents SET status = $2, updated_at = NOW() WHERE id = $1"
        };
        let _ = sqlx::query(sql)
            .bind(id).bind(&req.status).execute(&state.db).await;
    }

    let update: IncidentUpdate = sqlx::query_as(
        "INSERT INTO incident_updates (incident_id, status, message, author_email) VALUES ($1, $2, $3, $4) RETURNING *"
    )
    .bind(id).bind(&req.status).bind(&req.message).bind(&claims.email)
    .fetch_one(&state.db).await
    .map_err(|e| internal_error("post update", e))?;

    // Notify subscribers
    let title: Option<(String, Uuid)> = sqlx::query_as("SELECT title, user_id FROM managed_incidents WHERE id = $1")
        .bind(id).fetch_optional(&state.db).await
        .map_err(|e| internal_error("incident notify title lookup", e))?;
    if let Some((title, owner_id)) = title {
        notify_subscribers(&title, &req.status, &req.message, owner_id);
    }

    Ok((StatusCode::CREATED, Json(update)))
}

/// GET /api/incidents/{id}/updates — List incident updates.
pub async fn list_updates(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<IncidentUpdate>>, ApiError> {
    // Verify ownership
    let _: (Uuid,) = sqlx::query_as(
        "SELECT id FROM managed_incidents WHERE id = $1 AND user_id = $2"
    )
    .bind(id).bind(claims.sub)
    .fetch_optional(&state.db).await
    .map_err(|e| internal_error("list updates", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Incident not found"))?;

    let updates: Vec<IncidentUpdate> = sqlx::query_as(
        "SELECT * FROM incident_updates WHERE incident_id = $1 ORDER BY created_at ASC LIMIT 500"
    )
    .bind(id)
    .fetch_all(&state.db).await
    .map_err(|e| internal_error("list updates", e))?;

    Ok(Json(updates))
}

// ── Status Page Config ──────────────────────────────────────────────────────

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct StatusPageConfig {
    pub id: Uuid,
    pub title: String,
    pub description: String,
    pub logo_url: Option<String>,
    pub accent_color: String,
    pub show_subscribe: bool,
    pub show_incident_history: bool,
    pub history_days: i32,
    pub enabled: bool,
}

#[derive(serde::Deserialize)]
pub struct UpdateConfigRequest {
    pub title: Option<String>,
    pub description: Option<String>,
    pub logo_url: Option<String>,
    pub accent_color: Option<String>,
    pub show_subscribe: Option<bool>,
    pub show_incident_history: Option<bool>,
    pub history_days: Option<i32>,
    pub enabled: Option<bool>,
}

/// GET /api/status-page/config — Get status page config.
pub async fn get_config(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<Json<StatusPageConfig>, ApiError> {
    let config: Option<StatusPageConfig> = sqlx::query_as(
        "SELECT id, title, description, logo_url, accent_color, show_subscribe, show_incident_history, history_days, enabled \
         FROM status_page_config WHERE user_id = $1"
    )
    .bind(claims.sub)
    .fetch_optional(&state.db).await
    .map_err(|e| internal_error("get config", e))?;

    match config {
        Some(c) => Ok(Json(c)),
        None => {
            // Auto-create default config
            let c: StatusPageConfig = sqlx::query_as(
                "INSERT INTO status_page_config (user_id) VALUES ($1) \
                 RETURNING id, title, description, logo_url, accent_color, show_subscribe, show_incident_history, history_days, enabled"
            )
            .bind(claims.sub)
            .fetch_one(&state.db).await
            .map_err(|e| internal_error("get config", e))?;
            Ok(Json(c))
        }
    }
}

/// PUT /api/status-page/config — Update status page config.
pub async fn update_config(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Json(req): Json<UpdateConfigRequest>,
) -> Result<Json<StatusPageConfig>, ApiError> {
    // Ensure config exists. The conflict TARGET is load-bearing: without it the
    // clause can only absorb a primary-key collision, and the primary key
    // defaults to gen_random_uuid(), so the guard was inert and every PUT
    // inserted another row. The UPDATE below then matched N rows and
    // `fetch_one` returned an arbitrary one — an operator could untick
    // "Enabled", get a 200 and a form showing false, and still be publishing.
    let _ = sqlx::query(
        "INSERT INTO status_page_config (user_id) VALUES ($1) ON CONFLICT (user_id) DO NOTHING"
    )
    .bind(claims.sub)
    .execute(&state.db).await;

    let config: StatusPageConfig = sqlx::query_as(
        "UPDATE status_page_config SET \
         title = COALESCE(NULLIF($2, ''), title), \
         description = CASE WHEN $3 = '' THEN NULL ELSE COALESCE($3, description) END, \
         logo_url = CASE WHEN $4 = '' THEN NULL ELSE COALESCE($4, logo_url) END, \
         accent_color = COALESCE(NULLIF($5, ''), accent_color), \
         show_subscribe = COALESCE($6, show_subscribe), \
         show_incident_history = COALESCE($7, show_incident_history), \
         history_days = COALESCE($8, history_days), \
         enabled = COALESCE($9, enabled), \
         updated_at = NOW() \
         WHERE user_id = $1 \
         RETURNING id, title, description, logo_url, accent_color, show_subscribe, show_incident_history, history_days, enabled"
    )
    .bind(claims.sub)
    .bind(req.title.as_deref().unwrap_or(""))
    // NOT `unwrap_or("")` any more. The empty string is now the instruction to
    // clear, so collapsing an absent key onto it would blank the description of
    // any client that simply did not send the field. `title` and `accent_color`
    // keep the NULLIF guard deliberately: a status page with no title and no
    // colour is not a state an operator can want, so blank stays "leave it".
    .bind(req.description.as_deref())
    .bind(&req.logo_url)
    .bind(req.accent_color.as_deref().unwrap_or(""))
    .bind(req.show_subscribe)
    .bind(req.show_incident_history)
    .bind(req.history_days)
    .bind(req.enabled)
    .fetch_one(&state.db).await
    .map_err(|e| internal_error("update config", e))?;

    Ok(Json(config))
}

// ── Status Page Components ──────────────────────────────────────────────────

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct StatusPageComponent {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub sort_order: i32,
    pub status_override: Option<String>,
    pub group_name: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Deserialize)]
pub struct CreateComponentRequest {
    pub name: String,
    pub description: Option<String>,
    pub sort_order: Option<i32>,
    pub group_name: Option<String>,
    pub monitor_ids: Option<Vec<Uuid>>,
}

/// GET /api/status-page/components — List components.
pub async fn list_components(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let components: Vec<StatusPageComponent> = sqlx::query_as(
        "SELECT * FROM status_page_components WHERE user_id = $1 ORDER BY sort_order ASC, created_at ASC"
    )
    .bind(claims.sub)
    .fetch_all(&state.db).await
    .map_err(|e| internal_error("list components", e))?;

    let mut result = Vec::new();
    for comp in &components {
        let monitor_ids: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT monitor_id FROM status_page_component_monitors WHERE component_id = $1"
        )
        .bind(comp.id)
        .fetch_all(&state.db).await
        .unwrap_or_default();

        result.push(serde_json::json!({
            "id": comp.id,
            "name": comp.name,
            "description": comp.description,
            "sort_order": comp.sort_order,
            "status_override": comp.status_override,
            "group_name": comp.group_name,
            "monitor_ids": monitor_ids.iter().map(|(id,)| id).collect::<Vec<_>>(),
        }));
    }

    Ok(Json(result))
}

/// POST /api/status-page/components — Create component.
pub async fn create_component(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Json(req): Json<CreateComponentRequest>,
) -> Result<(StatusCode, Json<StatusPageComponent>), ApiError> {
    if req.name.is_empty() || req.name.len() > 100 {
        return Err(err(StatusCode::BAD_REQUEST, "Name must be 1-100 characters"));
    }

    let comp: StatusPageComponent = sqlx::query_as(
        "INSERT INTO status_page_components (user_id, name, description, sort_order, group_name) \
         VALUES ($1, $2, $3, $4, $5) RETURNING *"
    )
    .bind(claims.sub)
    .bind(&req.name)
    .bind(&req.description)
    .bind(req.sort_order.unwrap_or(0))
    .bind(&req.group_name)
    .fetch_one(&state.db).await
    .map_err(|e| internal_error("create component", e))?;

    // Link monitors
    if let Some(monitor_ids) = &req.monitor_ids {
        for mid in monitor_ids {
            let _ = sqlx::query(
                "INSERT INTO status_page_component_monitors (component_id, monitor_id) VALUES ($1, $2) ON CONFLICT DO NOTHING"
            )
            .bind(comp.id).bind(mid)
            .execute(&state.db).await;
        }
    }

    Ok((StatusCode::CREATED, Json(comp)))
}

/// DELETE /api/status-page/components/{id} — Delete component.
pub async fn delete_component(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = sqlx::query("DELETE FROM status_page_components WHERE id = $1 AND user_id = $2")
        .bind(id).bind(claims.sub)
        .execute(&state.db).await
        .map_err(|e| internal_error("delete component", e))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Component not found"));
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Subscribers ─────────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct SubscribeRequest {
    pub email: String,
}

/// POST /api/status-page/subscribe — Subscribe to updates (public, no auth).
pub async fn subscribe(
    State(state): State<AppState>,
    Json(req): Json<SubscribeRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // A disabled status page has no subscribers to take. Ungated, this wrote
    // attacker-supplied addresses into `status_page_subscribers` as
    // `verified = TRUE` on installs that never turned the feature on.
    crate::services::public_status::require_enabled(&state.db).await?;

    if req.email.is_empty() || !req.email.contains('@') || req.email.len() > 255 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid email address"));
    }

    // s427: which tenant's page this visitor is actually looking at — see
    // `resolve_current_status_page_owner`'s own doc comment for the tie-break
    // and the no-config fallback. `None` here means the install has no users
    // at all, which cannot happen on a reachable running panel.
    let owner_id = crate::services::public_status::resolve_current_status_page_owner(&state.db)
        .await
        .ok_or_else(|| internal_error("subscribe", sqlx::Error::RowNotFound))?;

    let token = uuid::Uuid::new_v4().to_string().replace('-', "");

    let _ = sqlx::query(
        "INSERT INTO status_page_subscribers (owner_id, email, verify_token, verified) \
         VALUES ($1, $2, $3, TRUE) \
         ON CONFLICT (owner_id, email) DO NOTHING"
    )
    .bind(owner_id)
    .bind(&req.email)
    .bind(&token)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("subscribe", e))?;

    Ok(Json(serde_json::json!({ "ok": true, "message": "Subscribed to status updates" })))
}

/// DELETE (or POST) /api/status-page/unsubscribe — Unsubscribe (public).
///
/// Registered for both methods. This comment said DELETE while the router
/// accepted POST only, and the published guide followed the comment — so the
/// documented call 405'd and the working call was written down nowhere.
pub async fn unsubscribe(
    State(state): State<AppState>,
    Json(req): Json<SubscribeRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    crate::services::public_status::require_enabled(&state.db).await?;

    // s427: scoped to the page the visitor is actually looking at — `IS NOT
    // DISTINCT FROM` rather than `=` so this still matches a legacy row whose
    // owner could not be resolved (both sides NULL), instead of `NULL = NULL`
    // silently matching nothing. Without this scoping, unsubscribing from one
    // tenant's public page could remove a row that belongs to a DIFFERENT
    // tenant the visitor never subscribed through.
    let owner_id = crate::services::public_status::resolve_current_status_page_owner(&state.db).await;
    let _ = sqlx::query(
        "DELETE FROM status_page_subscribers WHERE email = $1 AND owner_id IS NOT DISTINCT FROM $2"
    )
        .bind(&req.email)
        .bind(owner_id)
        .execute(&state.db).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /api/status-page/subscribers — List subscribers (admin).
///
/// s427: scoped to the calling admin's own `owner_id`, matching every sibling
/// status-page admin endpoint (`get_config`, `list_components`, ...) — before
/// this, any admin on the install could read every OTHER tenant's subscriber
/// email list, not just their own.
pub async fn list_subscribers(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let rows: Vec<(String, bool, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT email, verified, created_at FROM status_page_subscribers \
         WHERE owner_id = $1 ORDER BY created_at DESC LIMIT 1000"
    )
    .bind(claims.sub)
    .fetch_all(&state.db).await
    .map_err(|e| internal_error("list subscribers", e))?;

    let result: Vec<serde_json::Value> = rows.into_iter().map(|(email, verified, created_at)| {
        serde_json::json!({ "email": email, "verified": verified, "created_at": created_at })
    }).collect();

    Ok(Json(result))
}

// ── Enhanced Public Status Page ─────────────────────────────────────────────

/// GET /api/status-page/public — Enhanced public status page (no auth).
pub async fn public_status_page(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // The one gate every unauthenticated status-page route passes through. This
    // endpoint is what `/status` actually fetches, and until v2.70.0 it was
    // governed by nothing the operator UI writes.
    crate::services::public_status::require_enabled(&state.db).await?;

    // Get config. ORDER BY is load-bearing, not decoration: `update_config` used
    // to insert a fresh row on every PUT (see the migration that made
    // `user_id` unique), so an install can carry duplicates, and an unordered
    // LIMIT 1 would let two reads of the same table disagree.
    //
    // ⚠ SECURITY (s418): every query below this point MUST be scoped to
    // `owner_id` — the tenant whose config row won this ORDER BY. This handler
    // is unauthenticated; on any multi-tenant/reseller install, an unscoped
    // read here serves one tenant's components/incidents to every anonymous
    // visitor of every OTHER tenant's status page. Confirmed live-reachable on
    // this box before this fix.
    let config: Option<(Uuid, String, String, Option<String>, String, bool, bool, i32, bool)> = sqlx::query_as(
        "SELECT user_id, title, description, logo_url, accent_color, show_subscribe, show_incident_history, history_days, enabled \
         FROM status_page_config ORDER BY created_at ASC, id ASC LIMIT 1"
    )
    .fetch_optional(&state.db).await
    .map_err(|e| internal_error("public status page", e))?;

    let owner_id: Option<Uuid> = config.as_ref().map(|c| c.0);
    let (title, description, logo_url, accent_color, show_subscribe, show_history, history_days, enabled) =
        config.map(|c| (c.1, c.2, c.3, c.4, c.5, c.6, c.7, c.8))
            .unwrap_or(("Service Status".into(), "Current status of our services".into(), None, "#22c55e".into(), true, true, 90, true));

    if !enabled {
        return Err(err(StatusCode::NOT_FOUND, "Status page is disabled"));
    }

    // Get components with their monitor statuses — scoped to the config's owner.
    let mut component_list = Vec::new();
    if let Some(uid) = owner_id {
        let components: Vec<(Uuid, String, Option<String>, i32, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT id, name, description, sort_order, status_override, group_name \
             FROM status_page_components WHERE user_id = $1 ORDER BY sort_order ASC, created_at ASC"
        )
        .bind(uid)
        .fetch_all(&state.db).await
        .unwrap_or_default();

        for (comp_id, name, desc, _sort, status_override, group) in &components {
            let monitor_statuses: Vec<(String,)> = sqlx::query_as(
                "SELECT m.status FROM monitors m \
                 JOIN status_page_component_monitors cm ON cm.monitor_id = m.id \
                 WHERE cm.component_id = $1 AND m.user_id = $2 AND m.enabled = TRUE"
            )
            .bind(comp_id)
            .bind(uid)
            .fetch_all(&state.db).await
            .unwrap_or_default();

            let status = if let Some(override_status) = status_override {
                override_status.clone()
            } else if monitor_statuses.is_empty() {
                "operational".to_string()
            } else if monitor_statuses.iter().all(|(s,)| s == "up") {
                "operational".to_string()
            } else if monitor_statuses.iter().any(|(s,)| s == "down") {
                "major_outage".to_string()
            } else {
                "degraded".to_string()
            };

            component_list.push(serde_json::json!({
                "id": comp_id,
                "name": name,
                "description": desc,
                "group": group,
                "status": status,
            }));
        }
    }

    // Overall status
    let overall = if component_list.iter().all(|c| c["status"] == "operational") {
        "operational"
    } else if component_list.iter().any(|c| c["status"] == "major_outage") {
        "major_outage"
    } else {
        "degraded"
    };

    // Active + recent incidents — scoped to the config's owner.
    let mut incident_list = Vec::new();
    if let Some(uid) = owner_id {
        let incidents: Vec<ManagedIncident> = sqlx::query_as(
            "SELECT * FROM managed_incidents WHERE user_id = $1 AND visible_on_status_page = TRUE \
             AND (status != 'resolved' OR resolved_at > NOW() - ($2 || ' days')::interval) \
             ORDER BY started_at DESC LIMIT 50"
        )
        .bind(uid)
        .bind(history_days)
        .fetch_all(&state.db).await
        .unwrap_or_default();

        for inc in &incidents {
            let updates: Vec<IncidentUpdate> = sqlx::query_as(
                "SELECT * FROM incident_updates WHERE incident_id = $1 ORDER BY created_at ASC LIMIT 500"
            )
            .bind(inc.id)
            .fetch_all(&state.db).await
            .unwrap_or_default();

            // Projected field by field, not serialized whole: `IncidentUpdate`
            // carries `author_email`, and this endpoint answers with no login. The
            // four fields below are exactly what `PublicStatusPage.tsx` declares.
            // `#[serde(skip_serializing)]` on the field is the smaller edit and the
            // wrong one — three authenticated handlers return the same struct and
            // the incidents guide publishes attribution as behaviour.
            let public_updates: Vec<serde_json::Value> = updates
                .iter()
                .map(|u| {
                    serde_json::json!({
                        "id": u.id,
                        "status": u.status,
                        "message": u.message,
                        "created_at": u.created_at,
                    })
                })
                .collect();

            incident_list.push(serde_json::json!({
                "id": inc.id,
                "title": inc.title,
                "status": inc.status,
                "severity": inc.severity,
                "started_at": inc.started_at,
                "resolved_at": inc.resolved_at,
                "updates": public_updates,
            }));
        }
    }

    // Also include legacy monitor-based incidents (auto-detected downtime) — scoped to the config's owner.
    let auto_incidents: Vec<(Uuid, String, chrono::DateTime<chrono::Utc>, Option<chrono::DateTime<chrono::Utc>>, Option<String>)> = match owner_id {
        Some(uid) => sqlx::query_as(
            "SELECT i.id, m.name, i.started_at, i.resolved_at, i.cause \
             FROM incidents i JOIN monitors m ON m.id = i.monitor_id \
             WHERE m.user_id = $1 AND i.started_at > NOW() - INTERVAL '7 days' \
             ORDER BY i.started_at DESC LIMIT 20"
        )
        .bind(uid)
        .fetch_all(&state.db).await
        .unwrap_or_default(),
        None => Vec::new(),
    };

    Ok(Json(serde_json::json!({
        "title": title,
        "description": description,
        "logo_url": logo_url,
        "accent_color": accent_color,
        "show_subscribe": show_subscribe,
        "show_incident_history": show_history,
        "overall_status": overall,
        "components": component_list,
        "incidents": incident_list,
        "auto_incidents": auto_incidents.iter().map(|(id, name, started, resolved, cause)| {
            serde_json::json!({
                "id": id, "monitor_name": name, "started_at": started,
                "resolved_at": resolved, "cause": cause,
            })
        }).collect::<Vec<_>>(),
        "updated_at": chrono::Utc::now(),
    })))
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Notify status-page subscribers of an incident update.
///
/// Hands off to the shared status-notice worker. This used to walk an unbounded
/// subscriber list serially over SMTP while awaited INSIDE the HTTP handler, so
/// one incident update blocked the operator's request — holding a DB pool slot —
/// until every subscriber had been mailed or the proxy gave up. The public
/// subscribe endpoint is unauthenticated and unthrottled, so that list is
/// attacker-grown. See `services::status_notices` for the ordering and
/// backpressure guarantees; it also applies the fan-out cap.
fn notify_subscribers(title: &str, status: &str, message: &str, owner_id: Uuid) {
    crate::services::status_notices::enqueue(
        title,
        format!("[Status Update] {title} — {status}"),
        format!("{title}\nStatus: {status}\n\n{message}"),
        owner_id,
    );
}
