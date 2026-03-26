use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::{AuthUser, ServerScope};
use crate::error::{internal_error, err, agent_error, paginate, ApiError};
use crate::services::activity;
use crate::services::agent::AgentHandle;
use crate::AppState;

#[derive(serde::Deserialize)]
pub struct CronListQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(serde::Deserialize)]
pub struct CreateCronRequest {
    pub label: Option<String>,
    pub command: String,
    pub schedule: String,
}

#[derive(serde::Deserialize)]
pub struct UpdateCronRequest {
    pub label: Option<String>,
    pub command: Option<String>,
    pub schedule: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct Cron {
    pub id: Uuid,
    pub site_id: Uuid,
    pub label: String,
    pub command: String,
    pub schedule: String,
    pub enabled: bool,
    pub last_run: Option<chrono::DateTime<chrono::Utc>>,
    pub last_status: Option<String>,
    pub last_output: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Verify site ownership, return domain.
async fn get_site_domain(state: &AppState, site_id: Uuid, user_id: Uuid) -> Result<String, ApiError> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT domain FROM sites WHERE id = $1 AND user_id = $2")
            .bind(site_id)
            .bind(user_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("unknown", e))?;

    row.map(|(d,)| d)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))
}

/// Sync all enabled crons for a site to the agent's system crontab.
async fn sync_crons_to_agent(state: &AppState, agent: &AgentHandle, site_id: Uuid) -> Result<(), ApiError> {
    let crons: Vec<Cron> = sqlx::query_as(
        "SELECT * FROM crons WHERE site_id = $1 AND enabled = true",
    )
    .bind(site_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("unknown", e))?;

    let agent_crons: Vec<serde_json::Value> = crons
        .iter()
        .map(|c| {
            serde_json::json!({
                "id": c.id.to_string(),
                "command": c.command,
                "schedule": c.schedule,
                "label": c.label,
            })
        })
        .collect();

    agent
        .post("/crons/sync", Some(serde_json::Value::Array(agent_crons)))
        .await
        .map_err(|e| agent_error("Cron sync", e))?;

    Ok(())
}

/// GET /api/sites/{id}/crons — List crons for a site.
pub async fn list(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Query(params): Query<CronListQuery>,
) -> Result<Json<Vec<Cron>>, ApiError> {
    get_site_domain(&state, id, claims.sub).await?;

    let (limit, offset) = paginate(params.limit, params.offset);

    let crons: Vec<Cron> = sqlx::query_as(
        "SELECT * FROM crons WHERE site_id = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3",
    )
    .bind(id)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("list crons", e))?;

    Ok(Json(crons))
}

/// POST /api/sites/{id}/crons — Create a cron job.
pub async fn create(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
    Json(body): Json<CreateCronRequest>,
) -> Result<(StatusCode, Json<Cron>), ApiError> {
    let domain = get_site_domain(&state, id, claims.sub).await?;

    // Validate command for injection
    if body.command.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Command is required"));
    }
    super::is_safe_shell_command(&body.command)
        .map_err(|e| err(StatusCode::BAD_REQUEST, &format!("Cron command: {e}")))?;
    if body.schedule.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Schedule is required"));
    }
    let parts: Vec<&str> = body.schedule.split_whitespace().collect();
    if parts.len() != 5 {
        return Err(err(StatusCode::BAD_REQUEST, "Schedule must have 5 fields (minute hour day month weekday)"));
    }

    let label = body.label.unwrap_or_default();

    let cron: Cron = sqlx::query_as(
        "INSERT INTO crons (site_id, label, command, schedule) VALUES ($1, $2, $3, $4) RETURNING *",
    )
    .bind(id)
    .bind(&label)
    .bind(body.command.trim())
    .bind(body.schedule.trim())
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("create crons", e))?;

    // Sync to agent
    sync_crons_to_agent(&state, &agent, id).await?;

    tracing::info!("Cron created: {} for {domain}", cron.schedule);
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "cron.create",
        Some("cron"), Some(&domain), Some(&label), None,
    ).await;

    Ok((StatusCode::CREATED, Json(cron)))
}

/// PUT /api/sites/{id}/crons/{cron_id} — Update a cron job.
pub async fn update(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path((id, cron_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<UpdateCronRequest>,
) -> Result<Json<Cron>, ApiError> {
    let domain = get_site_domain(&state, id, claims.sub).await?;

    // Verify cron belongs to this site
    let existing: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM crons WHERE id = $1 AND site_id = $2")
            .bind(cron_id)
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("update crons", e))?;

    if existing.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Cron job not found"));
    }

    // Validate command for injection if provided
    if let Some(ref command) = body.command {
        if !command.trim().is_empty() {
            super::is_safe_shell_command(command)
                .map_err(|e| err(StatusCode::BAD_REQUEST, &format!("Cron command: {e}")))?;
        }
    }

    // Validate schedule if provided
    if let Some(ref schedule) = body.schedule {
        let parts: Vec<&str> = schedule.split_whitespace().collect();
        if parts.len() != 5 {
            return Err(err(StatusCode::BAD_REQUEST, "Schedule must have 5 fields"));
        }
    }

    let cron: Cron = sqlx::query_as(
        "UPDATE crons SET \
         label = COALESCE($1, label), \
         command = COALESCE($2, command), \
         schedule = COALESCE($3, schedule), \
         enabled = COALESCE($4, enabled), \
         updated_at = NOW() \
         WHERE id = $5 RETURNING *",
    )
    .bind(body.label.as_deref())
    .bind(body.command.as_deref())
    .bind(body.schedule.as_deref())
    .bind(body.enabled)
    .bind(cron_id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("update crons", e))?;

    // Sync to agent (re-syncs all enabled crons)
    sync_crons_to_agent(&state, &agent, id).await?;

    tracing::info!("Cron updated: {cron_id} for {domain}");
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "cron.update",
        Some("cron"), Some(&domain), Some(&cron.label), None,
    ).await;

    Ok(Json(cron))
}

/// DELETE /api/sites/{id}/crons/{cron_id} — Delete a cron job.
pub async fn remove(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path((id, cron_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = get_site_domain(&state, id, claims.sub).await?;

    let deleted = sqlx::query("DELETE FROM crons WHERE id = $1 AND site_id = $2")
        .bind(cron_id)
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("remove crons", e))?;

    if deleted.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Cron job not found"));
    }

    // Remove from agent crontab
    let _ = agent.delete(&format!("/crons/remove/{cron_id}")).await;

    // Re-sync remaining crons
    sync_crons_to_agent(&state, &agent, id).await?;

    tracing::info!("Cron deleted: {cron_id} for {domain}");
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "cron.delete",
        Some("cron"), Some(&domain), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/sites/{id}/crons/{cron_id}/run — Run a cron job immediately.
pub async fn run_now(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path((id, cron_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = get_site_domain(&state, id, claims.sub).await?;

    let cron: Cron = sqlx::query_as(
        "SELECT * FROM crons WHERE id = $1 AND site_id = $2",
    )
    .bind(cron_id)
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("run now", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Cron job not found"))?;

    // Execute via agent
    let result = agent
        .post(
            "/crons/run",
            Some(serde_json::json!({
                "id": cron.id.to_string(),
                "command": cron.command,
                "schedule": cron.schedule,
            })),
        )
        .await
        .map_err(|e| agent_error("Cron execution", e))?;

    let success = result.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
    let output = result.get("output").and_then(|v| v.as_str()).unwrap_or("");
    let status_str = if success { "success" } else { "failed" };

    // Update last_run info
    sqlx::query(
        "UPDATE crons SET last_run = NOW(), last_status = $1, last_output = $2, updated_at = NOW() WHERE id = $3",
    )
    .bind(status_str)
    .bind(output)
    .bind(cron_id)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("run now", e))?;

    tracing::info!("Cron executed manually: {cron_id} for {domain} — {status_str}");
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "cron.run",
        Some("cron"), Some(&domain), Some(&cron.label), Some(status_str),
    ).await;

    // GAP 30: Fire alert on cron job failure
    if !success {
        let exit_code = result.get("exit_code").and_then(|v| v.as_i64()).unwrap_or(-1);
        let alert_subject = format!("Cron failed: {} (exit {})", cron.label, exit_code);
        let alert_message = format!(
            "Cron job '{}' on {} failed with exit code {}.\nCommand: {}\nOutput: {}",
            cron.label, domain, exit_code, cron.command,
            if output.len() > 500 { &output[..500] } else { output }
        );

        // Fire alert via the existing system
        crate::services::notifications::fire_alert(
            &state.db,
            claims.sub,
            None,
            Some(id),
            "cron_failure",
            "warning",
            &alert_subject,
            &alert_message,
        ).await;

        crate::services::system_log::log_event(
            &state.db,
            "warning",
            "cron",
            &format!("Cron failed: {} on {}", cron.label, domain),
            Some(&format!("exit_code={}, command={}", exit_code, cron.command)),
        ).await;
    }

    Ok(Json(serde_json::json!({
        "ok": success,
        "output": output,
        "status": status_str,
    })))
}
