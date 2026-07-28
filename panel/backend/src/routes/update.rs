//! Phase 4 W4: panel self-update + snapshot + fleet admin API.
//!
//! All endpoints admin-gated. Mounted from `routes/mod.rs` under
//! `/api/update/*`, `/api/snapshots/*`, `/api/update/fleet*`.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use crate::auth::AdminUser;
use crate::error::{err, internal_error, ApiError};
use crate::models::{FleetUpdateRun, PanelSnapshot, UpdateChannel};
use crate::services::{panel_snapshot, panel_update, telemetry_collector};
use crate::AppState;

// ── Status & state ───────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct StatusResponse {
    #[serde(flatten)]
    pub state: panel_update::UpdateState,
    pub current_version: String,
    pub available_version: Option<String>,
    pub channel: String,
    /// Verdict of the most recent snapshot restore, if one has ever run.
    ///
    /// A restore stops and restarts this process, so its outcome cannot be
    /// returned through the request that started it — the restore writes a
    /// result file and it is surfaced here. Always present after a restore,
    /// including one that aborted partway, so a failed rollback can never look
    /// like no rollback.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_restore: Option<serde_json::Value>,
    /// Verdict of the most recent `update.sh` run, if one has ever run.
    ///
    /// Same reason as `last_restore`: a successful update kills this process, so
    /// the outcome cannot come back through the request that started it. It is
    /// also the ONLY channel for an update that failed *before* the api was
    /// stopped — nothing restarts, so nothing else ever reports it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_panel_update: Option<serde_json::Value>,
}

pub async fn get_status(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<StatusResponse>, ApiError> {
    let s = panel_update::current_state(&state.panel_update_state, &state.db).await;
    let available_version: Option<(String,)> = sqlx::query_as(
        "SELECT value FROM settings WHERE key = 'update_available_version'",
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("update status", e))?;
    let channel: Option<(String,)> =
        sqlx::query_as("SELECT value FROM settings WHERE key = 'update_channel'")
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("update status", e))?;
    Ok(Json(StatusResponse {
        state: s,
        current_version: env!("CARGO_PKG_VERSION").to_string(),
        available_version: available_version.map(|r| r.0),
        channel: channel.map(|r| r.0).unwrap_or_else(|| "stable".into()),
        last_restore: panel_snapshot::last_restore_result().await,
        last_panel_update: panel_update::last_panel_update_result().await,
    }))
}

// ── Apply update ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ApplyInput {
    pub target_version: String,
}

#[derive(Serialize)]
pub struct ApplyResponse {
    pub accepted: bool,
    pub state: panel_update::UpdateState,
}

pub async fn apply_update(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Json(body): Json<ApplyInput>,
) -> Result<(StatusCode, Json<ApplyResponse>), ApiError> {
    // Operator can only apply versions the poller has surfaced. Prevents
    // arbitrary tag jumps + downgrades through this surface (downgrade =
    // rollback to a snapshot, separate route).
    let advertised: Option<(String,)> = sqlx::query_as(
        "SELECT value FROM settings WHERE key = 'update_available_version'",
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("apply update", e))?;
    let advertised_version = advertised.map(|r| r.0).unwrap_or_default();
    let target_clean = body.target_version.trim_start_matches('v');
    if advertised_version.is_empty() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "no update is currently available — run /api/update/manual-check first",
        ));
    }
    if advertised_version.trim_start_matches('v') != target_clean {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "target_version does not match the advertised update — refresh and retry",
        ));
    }

    let new_state = panel_update::start_panel_update(
        state.panel_update_state.clone(),
        state.db.clone(),
        body.target_version.clone(),
        Some(claims.email.clone()),
    )
    .await
    .map_err(|e| match e {
        panel_update::OrchestratorError::InvalidTargetVersion(_) => {
            err(StatusCode::BAD_REQUEST, &e.to_string())
        }
        panel_update::OrchestratorError::AlreadyInFlight => {
            err(StatusCode::CONFLICT, &e.to_string())
        }
        panel_update::OrchestratorError::ScriptMissing(_) => {
            err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
        }
        panel_update::OrchestratorError::Snapshot(_)
        | panel_update::OrchestratorError::Spawn(_)
        | panel_update::OrchestratorError::Db(_) => internal_error("apply update", e),
    })?;

    Ok((
        StatusCode::ACCEPTED,
        Json(ApplyResponse {
            accepted: true,
            state: new_state,
        }),
    ))
}

// ── Manual poll ──────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct ManualCheckResponse {
    pub checked_at: String,
    pub available_version: Option<String>,
}

pub async fn manual_check(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<ManualCheckResponse>, ApiError> {
    telemetry_collector::check_for_updates_manual(&state.db).await;
    let advertised: Option<(String,)> = sqlx::query_as(
        "SELECT value FROM settings WHERE key = 'update_available_version'",
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("manual check", e))?;
    Ok(Json(ManualCheckResponse {
        checked_at: chrono::Utc::now().to_rfc3339(),
        available_version: advertised.map(|r| r.0),
    }))
}

// ── Rollback ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct RollbackInput {
    pub snapshot_id: Uuid,
}

#[derive(Serialize)]
pub struct RollbackResponse {
    pub accepted: bool,
    pub snapshot_id: Uuid,
}

pub async fn rollback(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Json(body): Json<RollbackInput>,
) -> Result<(StatusCode, Json<RollbackResponse>), ApiError> {
    panel_update::rollback_to_snapshot(state.db.clone(), body.snapshot_id)
        .await
        .map_err(|e| match e {
            panel_update::OrchestratorError::Snapshot(
                panel_snapshot::SnapshotError::NotFound(_),
            ) => err(StatusCode::NOT_FOUND, &e.to_string()),
            panel_update::OrchestratorError::Snapshot(
                panel_snapshot::SnapshotError::FileMissing(_),
            ) => err(StatusCode::GONE, &e.to_string()),
            _ => internal_error("rollback", e),
        })?;
    Ok((
        StatusCode::ACCEPTED,
        Json(RollbackResponse {
            accepted: true,
            snapshot_id: body.snapshot_id,
        }),
    ))
}

// ── Channel ──────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct ChannelResponse {
    pub channel: String,
}

pub async fn get_channel(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<ChannelResponse>, ApiError> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT value FROM settings WHERE key = 'update_channel'")
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("get channel", e))?;
    Ok(Json(ChannelResponse {
        channel: row.map(|r| r.0).unwrap_or_else(|| "stable".into()),
    }))
}

#[derive(Deserialize)]
pub struct ChannelInput {
    pub channel: String,
}

pub async fn put_channel(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Json(body): Json<ChannelInput>,
) -> Result<Json<ChannelResponse>, ApiError> {
    let channel = body.channel.trim().to_string();
    UpdateChannel::validate(&channel).map_err(|m| err(StatusCode::BAD_REQUEST, &m))?;
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('update_channel', $1) \
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
    )
    .bind(&channel)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("put channel", e))?;
    Ok(Json(ChannelResponse { channel }))
}

// ── Snapshots ────────────────────────────────────────────────────────────

pub async fn list_snapshots_route(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<Vec<PanelSnapshot>>, ApiError> {
    let rows = panel_snapshot::list_snapshots(&state.db)
        .await
        .map_err(|e| internal_error("list snapshots", e))?;
    Ok(Json(rows))
}

pub async fn delete_snapshot_route(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    panel_snapshot::delete_snapshot(&state.db, id)
        .await
        .map_err(|e| match e {
            panel_snapshot::SnapshotError::NotFound(_) => {
                err(StatusCode::NOT_FOUND, &e.to_string())
            }
            _ => internal_error("delete snapshot", e),
        })?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize, Default)]
pub struct CreateSnapshotInput {
    /// Optional human label appended to the `trigger` field.
    #[serde(default)]
    pub label: Option<String>,
}

pub async fn create_snapshot_route(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    body: Option<Json<CreateSnapshotInput>>,
) -> Result<(StatusCode, Json<PanelSnapshot>), ApiError> {
    let _label = body.as_ref().and_then(|b| b.label.clone());
    let meta = panel_snapshot::create_snapshot(
        &state.db,
        panel_snapshot::SnapshotTrigger::Manual,
        Some(claims.email.clone()),
    )
    .await
    .map_err(|e| internal_error("create snapshot", e))?;
    let row: PanelSnapshot =
        sqlx::query_as("SELECT * FROM panel_snapshots WHERE id = $1")
            .bind(meta.id)
            .fetch_one(&state.db)
            .await
            .map_err(|e| internal_error("create snapshot read", e))?;
    Ok((StatusCode::CREATED, Json(row)))
}

// ── Fleet update ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct FleetApplyInput {
    pub target_version: String,
    #[serde(default = "default_true")]
    pub halt_on_failure: bool,
    #[serde(default)]
    pub include_panel: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Serialize)]
pub struct FleetApplyResponse {
    pub run_id: Uuid,
    pub plan_size: usize,
}

pub async fn apply_fleet(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Json(body): Json<FleetApplyInput>,
) -> Result<(StatusCode, Json<FleetApplyResponse>), ApiError> {
    if !panel_update::validate_target_version(&body.target_version) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "invalid target_version (must be vX.Y.Z[-rc.N])",
        ));
    }

    let plan = panel_update::build_fleet_plan(&state.db, claims.sub, &body.target_version)
        .await
        .map_err(|e| internal_error("fleet plan", e))?;
    if plan.is_empty() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "no reachable servers needing update — fleet plan is empty",
        ));
    }

    let plan_size = plan.len();
    let run_id = panel_update::create_fleet_run(
        &state.db,
        &body.target_version,
        &plan,
        body.halt_on_failure,
        body.include_panel,
        Some(claims.sub),
    )
    .await
    .map_err(|e| internal_error("fleet create run", e))?;

    let pool = state.db.clone();
    let agents = state.agents.clone();
    let target = body.target_version.clone();
    let halt = body.halt_on_failure;
    // Passing the handle is what makes the operator's "include this panel"
    // checkbox do anything — it was persisted and read by nobody until s232.
    let panel_handle = body
        .include_panel
        .then(|| state.panel_update_state.clone());
    tokio::spawn(async move {
        panel_update::execute_fleet_plan(pool, agents, run_id, plan, target, halt, panel_handle)
            .await;
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(FleetApplyResponse { run_id, plan_size }),
    ))
}

pub async fn get_fleet_run(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<Json<FleetUpdateRun>, ApiError> {
    let row: Option<FleetUpdateRun> =
        sqlx::query_as("SELECT * FROM fleet_update_runs WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("fleet run", e))?;
    row.map(Json)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "fleet run not found"))
}

pub async fn list_fleet_runs(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Query(_q): Query<HashMap<String, String>>,
) -> Result<Json<Vec<FleetUpdateRun>>, ApiError> {
    let rows: Vec<FleetUpdateRun> = sqlx::query_as(
        "SELECT * FROM fleet_update_runs ORDER BY started_at DESC LIMIT 100",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("list fleet runs", e))?;
    Ok(Json(rows))
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_input_default_true_helper() {
        assert!(default_true());
    }

    #[test]
    fn channel_validate_through_route_input() {
        // The PUT route uses UpdateChannel::validate; verify it rejects the
        // garbage values a route handler would surface as 400.
        assert!(UpdateChannel::validate("nightly").is_err());
        assert!(UpdateChannel::validate("STABLE").is_err());
        assert!(UpdateChannel::validate("stable").is_ok());
    }

    #[test]
    fn fleet_input_serde_defaults() {
        // halt_on_failure defaults to true, include_panel to false.
        let body: FleetApplyInput =
            serde_json::from_str(r#"{"target_version":"v2.10.0"}"#).unwrap();
        assert!(body.halt_on_failure);
        assert!(!body.include_panel);
    }

    #[test]
    fn fleet_input_serde_overrides() {
        let body: FleetApplyInput = serde_json::from_str(
            r#"{"target_version":"v2.10.0","halt_on_failure":false,"include_panel":true}"#,
        )
        .unwrap();
        assert!(!body.halt_on_failure);
        assert!(body.include_panel);
    }
}
