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

/// POST /api/incidents — Create an incident.
pub async fn create(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
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
    notify_subscribers(&state.db, &incident.title, status, req.description.as_deref().unwrap_or("")).await;

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
    AuthUser(claims): AuthUser,
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
    let resolved_at_clause = if req.status.as_deref() == Some("resolved") {
        ", resolved_at = NOW()"
    } else {
        ""
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
        notify_subscribers(&state.db, &incident.title, update_status, message).await;
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
    AuthUser(claims): AuthUser,
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
    AuthUser(claims): AuthUser,
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
            let _ = sqlx::query(
                "UPDATE alerts SET status = 'resolved', resolved_at = NOW() \
                 WHERE title = $1 AND status IN ('firing', 'acknowledged')"
            ).bind(title).execute(&state.db).await;
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
        let _ = sqlx::query("UPDATE managed_incidents SET status = $2, updated_at = NOW() WHERE id = $1")
            .bind(id).bind(&req.status).execute(&state.db).await;
    }

    let update: IncidentUpdate = sqlx::query_as(
        "INSERT INTO incident_updates (incident_id, status, message, author_email) VALUES ($1, $2, $3, $4) RETURNING *"
    )
    .bind(id).bind(&req.status).bind(&req.message).bind(&claims.email)
    .fetch_one(&state.db).await
    .map_err(|e| internal_error("post update", e))?;

    // Notify subscribers
    let title: Option<(String,)> = sqlx::query_as("SELECT title FROM managed_incidents WHERE id = $1")
        .bind(id).fetch_optional(&state.db).await
        .map_err(|e| internal_error("incident notify title lookup", e))?;
    if let Some((title,)) = title {
        notify_subscribers(&state.db, &title, &req.status, &req.message).await;
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
    // Ensure config exists
    let _ = sqlx::query(
        "INSERT INTO status_page_config (user_id) VALUES ($1) ON CONFLICT DO NOTHING"
    )
    .bind(claims.sub)
    .execute(&state.db).await;

    let config: StatusPageConfig = sqlx::query_as(
        "UPDATE status_page_config SET \
         title = COALESCE(NULLIF($2, ''), title), \
         description = COALESCE(NULLIF($3, ''), description), \
         logo_url = COALESCE($4, logo_url), \
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
    .bind(req.description.as_deref().unwrap_or(""))
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
    if req.email.is_empty() || !req.email.contains('@') || req.email.len() > 255 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid email address"));
    }

    let token = uuid::Uuid::new_v4().to_string().replace('-', "");

    let _ = sqlx::query(
        "INSERT INTO status_page_subscribers (email, verify_token, verified) \
         VALUES ($1, $2, TRUE) \
         ON CONFLICT (email) DO NOTHING"
    )
    .bind(&req.email)
    .bind(&token)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("subscribe", e))?;

    Ok(Json(serde_json::json!({ "ok": true, "message": "Subscribed to status updates" })))
}

/// DELETE /api/status-page/unsubscribe — Unsubscribe (public).
pub async fn unsubscribe(
    State(state): State<AppState>,
    Json(req): Json<SubscribeRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _ = sqlx::query("DELETE FROM status_page_subscribers WHERE email = $1")
        .bind(&req.email)
        .execute(&state.db).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /api/status-page/subscribers — List subscribers (admin).
pub async fn list_subscribers(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let rows: Vec<(String, bool, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT email, verified, created_at FROM status_page_subscribers ORDER BY created_at DESC LIMIT 1000"
    )
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
    // Get config
    let config: Option<(String, String, Option<String>, String, bool, bool, i32, bool)> = sqlx::query_as(
        "SELECT title, description, logo_url, accent_color, show_subscribe, show_incident_history, history_days, enabled \
         FROM status_page_config LIMIT 1"
    )
    .fetch_optional(&state.db).await
    .map_err(|e| internal_error("public status page", e))?;

    let (title, description, logo_url, accent_color, show_subscribe, show_history, history_days, enabled) =
        config.unwrap_or(("Service Status".into(), "Current status of our services".into(), None, "#22c55e".into(), true, true, 90, true));

    if !enabled {
        return Err(err(StatusCode::NOT_FOUND, "Status page is disabled"));
    }

    // Get components with their monitor statuses
    let components: Vec<(Uuid, String, Option<String>, i32, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT id, name, description, sort_order, status_override, group_name \
         FROM status_page_components ORDER BY sort_order ASC, created_at ASC"
    )
    .fetch_all(&state.db).await
    .unwrap_or_default();

    let mut component_list = Vec::new();
    for (comp_id, name, desc, _sort, status_override, group) in &components {
        let monitor_statuses: Vec<(String,)> = sqlx::query_as(
            "SELECT m.status FROM monitors m \
             JOIN status_page_component_monitors cm ON cm.monitor_id = m.id \
             WHERE cm.component_id = $1 AND m.enabled = TRUE"
        )
        .bind(comp_id)
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

    // Overall status
    let overall = if component_list.iter().all(|c| c["status"] == "operational") {
        "operational"
    } else if component_list.iter().any(|c| c["status"] == "major_outage") {
        "major_outage"
    } else {
        "degraded"
    };

    // Active + recent incidents
    let incidents: Vec<ManagedIncident> = sqlx::query_as(
        "SELECT * FROM managed_incidents WHERE visible_on_status_page = TRUE \
         AND (status != 'resolved' OR resolved_at > NOW() - ($1 || ' days')::interval) \
         ORDER BY started_at DESC LIMIT 50"
    )
    .bind(history_days)
    .fetch_all(&state.db).await
    .unwrap_or_default();

    let mut incident_list = Vec::new();
    for inc in &incidents {
        let updates: Vec<IncidentUpdate> = sqlx::query_as(
            "SELECT * FROM incident_updates WHERE incident_id = $1 ORDER BY created_at ASC LIMIT 500"
        )
        .bind(inc.id)
        .fetch_all(&state.db).await
        .unwrap_or_default();

        incident_list.push(serde_json::json!({
            "id": inc.id,
            "title": inc.title,
            "status": inc.status,
            "severity": inc.severity,
            "started_at": inc.started_at,
            "resolved_at": inc.resolved_at,
            "updates": updates,
        }));
    }

    // Also include legacy monitor-based incidents (auto-detected downtime)
    let auto_incidents: Vec<(Uuid, String, chrono::DateTime<chrono::Utc>, Option<chrono::DateTime<chrono::Utc>>, Option<String>)> = sqlx::query_as(
        "SELECT i.id, m.name, i.started_at, i.resolved_at, i.cause \
         FROM incidents i JOIN monitors m ON m.id = i.monitor_id \
         WHERE i.started_at > NOW() - INTERVAL '7 days' \
         ORDER BY i.started_at DESC LIMIT 20"
    )
    .fetch_all(&state.db).await
    .unwrap_or_default();

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

async fn notify_subscribers(db: &sqlx::PgPool, title: &str, status: &str, message: &str) {
    let emails: Vec<(String,)> = sqlx::query_as(
        "SELECT email FROM status_page_subscribers WHERE verified = TRUE AND notify_incidents = TRUE"
    )
    .fetch_all(db).await
    .unwrap_or_default();

    if emails.is_empty() {
        return;
    }

    let subject = format!("[Status Update] {title} — {status}");
    let body = format!("{title}\nStatus: {status}\n\n{message}");

    // Get SMTP settings for sending
    let smtp_host: Option<(String,)> = match sqlx::query_as(
        "SELECT value FROM settings WHERE key = 'smtp_host'"
    ).fetch_optional(db).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("DB error fetching SMTP settings for subscriber notification: {e}");
            return;
        }
    };

    if smtp_host.is_none() {
        tracing::debug!("No SMTP configured — skipping subscriber notifications for {title}");
        return;
    }

    for (email,) in &emails {
        tracing::info!("Notifying subscriber {email} about incident update: {title}");
        // Use the existing email notification system
        crate::services::notifications::send_notification(
            db,
            &crate::services::notifications::NotifyChannels {
                email: Some(email.clone()),
                slack_url: None,
                discord_url: None,
                pagerduty_key: None,
                webhook_url: None,
                muted_types: String::new(),
            },
            &subject,
            &body,
            &body,
        ).await;
    }
}
