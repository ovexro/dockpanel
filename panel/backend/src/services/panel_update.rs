//! Phase 4 W4: panel self-update orchestrator.
//!
//! Lifts `scripts/update.sh`'s SSH-only flow into a panel-UI-driven action.
//! Does NOT reimplement the binary swap or rollback — those live in
//! `update.sh` (its binary-swap + `.bak` rollback path) and are battle-tested.
//! The orchestrator's job is:
//!
//!   1. Create a persistent snapshot via [`super::panel_snapshot`].
//!   2. Shell out to `update.sh` with `DOCKPANEL_NO_SELF_REFRESH=1` +
//!      `DOCKPANEL_VERSION=$target` so a single subprocess invocation does
//!      the work without mid-flight re-exec.
//!   3. Track state in two places:
//!      - In-process `Arc<RwLock<UpdateState>>` for fast guards against
//!        concurrent applies within one process lifetime.
//!      - `panel_snapshots` rows in the DB for cross-restart truth (the
//!        api process dies mid-swap when update.sh restarts services).
//!   4. On the next process boot, [`finalize_pending_on_startup`] closes
//!      out any in-flight rows by writing `to_version =
//!      CARGO_PKG_VERSION`. Equal `from_version`/`to_version` ⇒ rollback
//!      happened.
//!
//! Out of scope here:
//!   - In-flight rollback is `update.sh`'s `.bak` restore — orchestrator
//!     doesn't touch it.
//!   - Operator-triggered rollback from a snapshot is
//!     [`rollback_to_snapshot`] — restores binaries + DB + /etc/dockpanel
//!     and bounces services. Routes/UI gate this behind a destructive
//!     confirm.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::models::PanelSnapshot;
use crate::services::panel_snapshot::{self, SnapshotTrigger};

/// Path to `scripts/update.sh` on a panel install. setup.sh + update.sh
/// both lay the source tree under `/opt/dockpanel`, so this is stable.
const UPDATE_SCRIPT: &str = "/opt/dockpanel/scripts/update.sh";

/// A snapshot row is considered "in flight" if it has `to_version IS NULL`
/// and was created within this window. Older rows that never finalized are
/// dead and don't block new applies.
///
/// Manual snapshots are excluded everywhere this predicate is used: they are
/// taken on demand and legitimately keep `to_version` NULL forever (see the
/// `panel_snapshots` migration), so treating them as unfinalized updates made
/// a safety snapshot look like an update in progress — and let the
/// abandoned-sweep below stamp them `to_version = 'abandoned'`, which
/// `current_state` then reported as a successful update to "abandoned".
const IN_FLIGHT_WINDOW_MIN: i64 = 15;

/// Maximum length of the captured `update.sh` stdout tail kept in memory
/// for the in-process state.
const LOG_TAIL_MAX: usize = 64;

// ── State ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum UpdateState {
    /// No update is currently running.
    Idle,
    /// `update.sh` is executing. The orchestrator may not survive long
    /// enough to transition out of this (the api binary swap will kill
    /// this process), so the DB row is the durable signal.
    InFlight {
        target_version: String,
        snapshot_id: Uuid,
        started_at: DateTime<Utc>,
        last_log_line: Option<String>,
    },
    /// Reconstructed at request-time from the snapshot row when the api
    /// reboots into the new version. `from_version != to_version`.
    Succeeded {
        from_version: String,
        to_version: String,
        completed_at: DateTime<Utc>,
    },
    /// `update.sh`'s in-flight `.bak` rollback fired and the api came back
    /// on the original binary. `from_version == to_version`.
    RolledBack {
        attempted_version: String,
        snapshot_id: Uuid,
        completed_at: DateTime<Utc>,
    },
    /// Orchestrator failed before `update.sh` could run (snapshot error,
    /// validation, missing script, etc.).
    Failed {
        reason: String,
        at: DateTime<Utc>,
    },
}

pub type UpdateStateHandle = Arc<RwLock<UpdateState>>;

pub fn new_state_handle() -> UpdateStateHandle {
    Arc::new(RwLock::new(UpdateState::Idle))
}

// ── Validation ───────────────────────────────────────────────────────────

/// Accepts `vX.Y.Z`, `X.Y.Z`, `vX.Y.Z-rc.N`, `X.Y.Z-rc.N`. No other shapes.
/// Hand-rolled instead of pulling in `regex` for one expression.
pub fn validate_target_version(v: &str) -> bool {
    let v = v.trim_start_matches('v');
    let (core, suffix) = match v.split_once('-') {
        Some((c, s)) => (c, Some(s)),
        None => (v, None),
    };

    let core_parts: Vec<&str> = core.split('.').collect();
    if core_parts.len() != 3 {
        return false;
    }
    for p in &core_parts {
        if p.is_empty() || !p.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
    }

    if let Some(s) = suffix {
        let Some(n) = s.strip_prefix("rc.") else {
            return false;
        };
        if n.is_empty() || !n.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
    }
    true
}

// ── Errors ───────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum OrchestratorError {
    InvalidTargetVersion(String),
    AlreadyInFlight,
    ScriptMissing(String),
    Snapshot(panel_snapshot::SnapshotError),
    Spawn(std::io::Error),
    Db(sqlx::Error),
}

impl std::fmt::Display for OrchestratorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrchestratorError::InvalidTargetVersion(v) => {
                write!(f, "invalid target version: {v}")
            }
            OrchestratorError::AlreadyInFlight => write!(
                f,
                "another update is already in flight (check /api/update/status)"
            ),
            OrchestratorError::ScriptMissing(p) => {
                write!(f, "update script not found at {p}")
            }
            OrchestratorError::Snapshot(e) => write!(f, "snapshot failed: {e}"),
            OrchestratorError::Spawn(e) => write!(f, "failed to spawn update.sh: {e}"),
            OrchestratorError::Db(e) => write!(f, "db error: {e}"),
        }
    }
}

impl std::error::Error for OrchestratorError {}

impl From<panel_snapshot::SnapshotError> for OrchestratorError {
    fn from(e: panel_snapshot::SnapshotError) -> Self {
        OrchestratorError::Snapshot(e)
    }
}

impl From<sqlx::Error> for OrchestratorError {
    fn from(e: sqlx::Error) -> Self {
        OrchestratorError::Db(e)
    }
}

// ── Public API ───────────────────────────────────────────────────────────

/// Resolve the current state. In-memory `Idle` may be a lie if a prior
/// process died mid-update; we cross-check the DB for an in-flight row
/// before returning Idle.
pub async fn current_state(handle: &UpdateStateHandle, pool: &PgPool) -> UpdateState {
    {
        let s = handle.read().await;
        if !matches!(*s, UpdateState::Idle) {
            return s.clone();
        }
    }

    // In-memory state says idle. Check DB for an unfinalized in-flight row.
    // Manual snapshots never carry a `to_version`, so they must be excluded or
    // taking a safety snapshot would masquerade as an update in progress.
    let cutoff = Utc::now() - chrono::Duration::minutes(IN_FLIGHT_WINDOW_MIN);
    if let Ok(Some(snap)) = sqlx::query_as::<_, PanelSnapshot>(
        "SELECT * FROM panel_snapshots \
         WHERE to_version IS NULL AND trigger <> 'manual' AND created_at > $1 \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(cutoff)
    .fetch_optional(pool)
    .await
    {
        let target = parse_target_from_trigger(&snap.trigger).unwrap_or_default();
        return UpdateState::InFlight {
            target_version: target,
            snapshot_id: snap.id,
            started_at: snap.created_at,
            last_log_line: None,
        };
    }

    // Most recent finalized snapshot tells us if the last completed update
    // succeeded or rolled back (read-only summary for UI).
    if let Ok(Some(snap)) = sqlx::query_as::<_, PanelSnapshot>(
        "SELECT * FROM panel_snapshots \
         WHERE to_version IS NOT NULL \
         ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    {
        let from = snap.from_version.clone();
        let to = snap.to_version.clone().unwrap_or_default();
        let attempted = parse_target_from_trigger(&snap.trigger).unwrap_or_else(|| to.clone());
        if to == from && attempted != from {
            return UpdateState::RolledBack {
                attempted_version: attempted,
                snapshot_id: snap.id,
                completed_at: snap.created_at,
            };
        }
        if to != from {
            // The row is stamped by whichever binary happened to boot after the
            // swap — which is ~30s BEFORE update.sh's health check decides
            // whether the new build is actually good. On a rollback the new api
            // starts, writes to_version = itself ("succeeded"), and only then
            // fails its health check and gets replaced by the old binary; the
            // row is already finalized, so nothing revisits it and the panel
            // reported a successful update to a version it is demonstrably not
            // running. Verified on a lab box: state=succeeded to_version=2.11.3
            // alongside current_version=2.11.2 in the SAME response.
            //
            // The running binary is ground truth, so correct the record at the
            // read site rather than trusting the stamp. No schema change, no new
            // state variant — this is what `RolledBack` already means.
            let running = env!("CARGO_PKG_VERSION");
            if to != running {
                return UpdateState::RolledBack {
                    attempted_version: to,
                    snapshot_id: snap.id,
                    completed_at: snap.created_at,
                };
            }
            return UpdateState::Succeeded {
                from_version: from,
                to_version: to,
                completed_at: snap.created_at,
            };
        }
    }

    UpdateState::Idle
}

/// Start the panel-self-update flow.
///
/// 1. Validate target_version + in-flight guard.
/// 2. Create snapshot with trigger=pre-update:<target>.
/// 3. Spawn `update.sh` detached (process group of its own so SIGTERM to
///    the api during binary swap doesn't propagate up).
/// 4. Background task streams stdout into the state handle's
///    `last_log_line` until the api dies or update.sh finishes.
///
/// Returns the InFlight state to the caller; the actual apply progress is
/// observed via `current_state` polling.
pub async fn start_panel_update(
    handle: UpdateStateHandle,
    pool: PgPool,
    target_version: String,
    operator: Option<String>,
) -> Result<UpdateState, OrchestratorError> {
    let target_version = target_version.trim().to_string();
    if !validate_target_version(&target_version) {
        return Err(OrchestratorError::InvalidTargetVersion(target_version));
    }

    if !std::path::Path::new(UPDATE_SCRIPT).exists() {
        return Err(OrchestratorError::ScriptMissing(UPDATE_SCRIPT.into()));
    }

    // Concurrent-apply guard (in-process + DB).
    {
        let s = handle.read().await;
        if matches!(*s, UpdateState::InFlight { .. }) {
            return Err(OrchestratorError::AlreadyInFlight);
        }
    }
    let cutoff = Utc::now() - chrono::Duration::minutes(IN_FLIGHT_WINDOW_MIN);
    // Same exclusion as `current_state`: a manual snapshot is not an update, so
    // it must not lock out the one thing an operator does right after taking it.
    let in_flight_count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM panel_snapshots \
         WHERE to_version IS NULL AND trigger <> 'manual' AND created_at > $1",
    )
    .bind(cutoff)
    .fetch_one(&pool)
    .await?;
    if in_flight_count.0 > 0 {
        return Err(OrchestratorError::AlreadyInFlight);
    }

    // Create the pre-update snapshot. If this fails, no state changes.
    let meta = panel_snapshot::create_snapshot(
        &pool,
        SnapshotTrigger::PreUpdate {
            target_version: target_version.clone(),
        },
        operator.clone(),
    )
    .await?;

    let started_at = Utc::now();
    let in_flight = UpdateState::InFlight {
        target_version: target_version.clone(),
        snapshot_id: meta.id,
        started_at,
        last_log_line: None,
    };
    *handle.write().await = in_flight.clone();

    // Spawn update.sh. Detached process group so systemctl stop of
    // dockpanel-api (issued by update.sh) doesn't propagate SIGTERM to
    // the script itself. PID1 reaps when complete.
    let mut cmd = Command::new("bash");
    cmd.arg(UPDATE_SCRIPT)
        .env("INSTALL_FROM_RELEASE", "1")
        .env("DOCKPANEL_NO_SELF_REFRESH", "1")
        // update.sh documents DOCKPANEL_VERSION as `vX.Y.Z` and concatenates it
        // straight into the release download URL, but the poller stores the
        // advertised version with the `v` stripped (telemetry_collector.rs) and
        // apply_update validates against that stripped form. Passing it through
        // verbatim built `releases/download/2.11.2/...`, which 404s — so every
        // self-update died at the first `curl -sfL` with exit 22, before any
        // binary was swapped. Re-add the prefix at this boundary.
        .env("DOCKPANEL_VERSION", format!("v{}", target_version.trim_start_matches('v')))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    #[cfg(unix)]
    {
        cmd.process_group(0);
    }

    let mut child = cmd.spawn().map_err(OrchestratorError::Spawn)?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let handle_clone = handle.clone();
    let handle_for_exit = handle.clone();
    let pool_for_exit = pool.clone();
    let snapshot_id_for_exit = meta.id;
    let target_clone = target_version.clone();
    tokio::spawn(async move {
        stream_update_output(handle_clone, stdout, stderr).await;
        // We may not reach here — update.sh kills the api midway. If we
        // do, log the exit status so the operator sees it in journals.
        match tokio::time::timeout(Duration::from_secs(900), child.wait()).await {
            Ok(Ok(status)) => {
                tracing::info!(
                    "update.sh (target {target_clone}) exited with status {status}"
                );
                // Reaching here at all means update.sh died before it stopped
                // the api — i.e. it failed early (a bad download, a missing
                // file), so no binary was swapped and no .bak rollback ran.
                // Nothing else closes out the row in that case, so the operator
                // was left staring at `in_flight` until the 15-minute window
                // lapsed, with the real error only in the journal. Finalising
                // to_version == from_version is what `current_state` already
                // reads as "attempted <target>, still on <current>" (RolledBack).
                if !status.success() {
                    tracing::error!(
                        "update.sh (target {target_clone}) FAILED with {status} — \
                         panel left on the previous version"
                    );
                    *handle_for_exit.write().await = UpdateState::Idle;
                    let current = env!("CARGO_PKG_VERSION");
                    if let Err(e) = sqlx::query(
                        "UPDATE panel_snapshots SET to_version = $1 \
                         WHERE id = $2 AND to_version IS NULL",
                    )
                    .bind(current)
                    .bind(snapshot_id_for_exit)
                    .execute(&pool_for_exit)
                    .await
                    {
                        tracing::warn!("failed to finalize failed-update snapshot: {e}");
                    }
                }
            }
            Ok(Err(e)) => {
                tracing::warn!("update.sh wait failed: {e}");
            }
            Err(_) => {
                tracing::warn!("update.sh wait timed out after 15min");
            }
        }
    });

    Ok(in_flight)
}

/// Stream `update.sh` stdout + stderr into the handle's `last_log_line`.
/// This loop exits when both pipes hit EOF — typically because the api
/// process is being killed by `systemctl stop dockpanel-api`.
async fn stream_update_output(
    handle: UpdateStateHandle,
    stdout: Option<tokio::process::ChildStdout>,
    stderr: Option<tokio::process::ChildStderr>,
) {
    let stdout_task = stdout.map(|s| {
        let h = handle.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(s).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if line.is_empty() {
                    continue;
                }
                update_last_log(&h, &line).await;
                tracing::info!(target: "panel_update", "{line}");
            }
        })
    });
    let stderr_task = stderr.map(|s| {
        let h = handle.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(s).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if line.is_empty() {
                    continue;
                }
                update_last_log(&h, &line).await;
                tracing::warn!(target: "panel_update", "{line}");
            }
        })
    });
    if let Some(t) = stdout_task {
        let _ = t.await;
    }
    if let Some(t) = stderr_task {
        let _ = t.await;
    }
}

async fn update_last_log(handle: &UpdateStateHandle, line: &str) {
    let truncated = if line.len() > LOG_TAIL_MAX * 4 {
        line.chars().take(LOG_TAIL_MAX * 4).collect::<String>()
    } else {
        line.to_string()
    };
    let mut s = handle.write().await;
    if let UpdateState::InFlight { last_log_line, .. } = &mut *s {
        *last_log_line = Some(truncated);
    }
}

fn parse_target_from_trigger(trigger: &str) -> Option<String> {
    trigger.strip_prefix("pre-update:").map(|s| s.to_string())
}

/// Operator-triggered rollback. Validates the snapshot, then hands the whole
/// restore — stop services, database, binaries, /etc, start services — to a
/// detached PID1-owned unit and returns.
///
/// It CANNOT be awaited. The first thing the restore does is stop the api
/// process this code is running in, so there is no "after" in which to observe
/// it from here; the outcome is read back from
/// [`panel_snapshot::last_restore_result`] once the panel is up again.
///
/// This shape is the fix, not an implementation detail. Until v2.11.5 the
/// restore ran INLINE in the request handler, which meant:
///   * it competed with the panel's own 300s HTTP timeout (`main.rs` TimeoutLayer
///     and nginx `proxy_read_timeout`) — a restore measured at 394.9s on a lab
///     box, so the future was dropped mid-flight;
///   * dropping it broke the `gunzip | psql` pipe, which psql read as a normal
///     end of input and exited 0 for, so the failure was recorded as a success;
///   * the database was left with 1 of 92 tables while the panel reported the
///     rollback had worked.
/// Both halves are now closed: the work is detached (so nothing cancels it) and
/// the database stage is atomic and verified (so it cannot half-apply).
pub async fn rollback_to_snapshot(
    pool: PgPool,
    snapshot_id: Uuid,
) -> Result<(), OrchestratorError> {
    // Everything cheap is validated synchronously so the operator gets a real
    // 4xx now rather than a 202 and a result file to go hunting for. The row,
    // the file on disk and its sha256 are all checked inside spawn_restore.
    panel_snapshot::spawn_restore(&pool, snapshot_id).await?;
    Ok(())
}

/// At process startup, close out any in-flight snapshot rows by writing
/// `to_version = CARGO_PKG_VERSION`. Equal `from_version`/`to_version`
/// indicates an in-flight rollback by `update.sh`; differing values are a
/// successful apply.
pub async fn finalize_pending_on_startup(pool: &PgPool) {
    let cutoff = Utc::now() - chrono::Duration::minutes(IN_FLIGHT_WINDOW_MIN);
    let pending: Result<Vec<PanelSnapshot>, _> = sqlx::query_as(
        "SELECT * FROM panel_snapshots \
         WHERE to_version IS NULL AND trigger <> 'manual' AND created_at > $1 \
         ORDER BY created_at ASC",
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await;

    let pending = match pending {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("finalize_pending: read failed: {e}");
            return;
        }
    };

    let current_version = env!("CARGO_PKG_VERSION").to_string();
    for snap in &pending {
        let result = sqlx::query("UPDATE panel_snapshots SET to_version = $1 WHERE id = $2")
            .bind(&current_version)
            .bind(snap.id)
            .execute(pool)
            .await;
        match result {
            Ok(_) => {
                if current_version == snap.from_version {
                    tracing::warn!(
                        "Snapshot {} finalized as rolled-back (process restarted on \
                         pre-update version {})",
                        snap.id,
                        current_version
                    );
                } else {
                    tracing::info!(
                        "Snapshot {} finalized as succeeded ({} -> {})",
                        snap.id,
                        snap.from_version,
                        current_version
                    );
                }
            }
            Err(e) => {
                tracing::warn!("finalize_pending: row {} update failed: {e}", snap.id);
            }
        }
    }

    // Mark older, abandoned in-flight rows with a sentinel so they don't
    // perpetually block /api/update/apply. Manual snapshots are exempt — they
    // are never in flight, and stamping one 'abandoned' made it the newest
    // finalized row, which `current_state` reads as a completed update from the
    // running version to a version literally called "abandoned".
    if let Err(e) = sqlx::query(
        "UPDATE panel_snapshots SET to_version = 'abandoned' \
         WHERE to_version IS NULL AND trigger <> 'manual' AND created_at <= $1",
    )
    .bind(cutoff)
    .execute(pool)
    .await
    {
        tracing::warn!("finalize_pending: abandoned-sweep failed: {e}");
    }
}

// ── Fleet (§4.5) ─────────────────────────────────────────────────────────
//
// Operator-initiated rolling update across the user's remote agents.
// Walks plan in order (oldest agent_version first; reachability gate
// excludes servers `last_seen_at > 5 min stale`), POSTs to each agent's
// `/panel/update`, polls `/panel/update/status` until terminal, records
// per-server progress in `fleet_update_runs.progress` JSONB. Halts on
// first failure unless `force_continue: true`.
//
// Per design memo §3.D5: `include_panel: false` default — fleet rolls
// first, panel itself last with a separate explicit click.

use serde::Deserialize;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetPlanRow {
    pub server_id: Uuid,
    pub name: String,
    pub agent_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetProgressRow {
    pub server_id: Uuid,
    pub status: String, // "pending" | "updating" | "succeeded" | "failed" | "skipped"
    pub duration_ms: Option<i64>,
    pub error: Option<String>,
}

/// Build the ordered plan: oldest version first, ties broken by
/// `last_seen_at desc`. Skips servers staler than 5 minutes (the
/// reachability gate). Skips servers already at target_version.
pub async fn build_fleet_plan(
    pool: &PgPool,
    user_id: Uuid,
    target_version: &str,
) -> Result<Vec<FleetPlanRow>, sqlx::Error> {
    let target_clean = target_version.trim_start_matches('v').to_string();
    // NOT ordered in SQL: `agent_version` is text, so postgres would sort it
    // lexicographically and put 2.9.0 AFTER 2.10.0 — i.e. it would roll the
    // newest box first and call it the oldest. Ordering happens below, on
    // parsed numeric components.
    let rows: Vec<(Uuid, String, Option<String>)> = sqlx::query_as(
        "SELECT id, name, agent_version FROM servers \
         WHERE user_id = $1 \
           AND last_seen_at > NOW() - INTERVAL '5 minutes' \
           AND is_local = false \
         ORDER BY last_seen_at DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    let mut plan: Vec<FleetPlanRow> = rows
        .into_iter()
        .filter(|(_, _, v)| {
            v.as_deref()
                .map(|cur| cur.trim_start_matches('v') != target_clean)
                .unwrap_or(true)
        })
        .map(|(server_id, name, agent_version)| FleetPlanRow {
            server_id,
            name,
            agent_version,
        })
        .collect();

    // Oldest first, unknown version first (a box that has never reported one is
    // the most suspect). `last_seen_at DESC` from the query is preserved within
    // a version by the stable sort.
    plan.sort_by_key(|r| semver_key(r.agent_version.as_deref()));
    Ok(plan)
}

/// Sortable numeric key for a `X.Y.Z` (optionally `v`-prefixed) version.
/// `None` sorts first. Anything unparseable sorts with whatever it parsed,
/// which keeps a malformed row from jumping to the end of the queue.
pub(crate) fn semver_key(v: Option<&str>) -> (u8, u64, u64, u64) {
    let Some(v) = v else {
        return (0, 0, 0, 0);
    };
    let core = v.trim_start_matches('v');
    let core = core.split('-').next().unwrap_or(core);
    let mut it = core.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
    (
        1,
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

/// Create a new fleet_update_runs row and return its id. The caller
/// then spawns a background task that walks the plan via
/// [`execute_fleet_plan`].
pub async fn create_fleet_run(
    pool: &PgPool,
    target_version: &str,
    plan: &[FleetPlanRow],
    halt_on_failure: bool,
    include_panel: bool,
    started_by: Option<Uuid>,
) -> Result<Uuid, sqlx::Error> {
    let plan_json = serde_json::to_value(plan).unwrap_or(serde_json::json!([]));
    let initial_progress: Vec<FleetProgressRow> = plan
        .iter()
        .map(|p| FleetProgressRow {
            server_id: p.server_id,
            status: "pending".into(),
            duration_ms: None,
            error: None,
        })
        .collect();
    let progress_json = serde_json::to_value(&initial_progress).unwrap_or(serde_json::json!([]));

    let row: (Uuid,) = sqlx::query_as(
        "INSERT INTO fleet_update_runs \
            (target_version, plan, progress, halt_on_failure, include_panel, started_by) \
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(target_version)
    .bind(plan_json)
    .bind(progress_json)
    .bind(halt_on_failure)
    .bind(include_panel)
    .bind(started_by)
    .fetch_one(pool)
    .await?;

    Ok(row.0)
}

/// Walk the plan, POST `/panel/update` to each agent, poll status until
/// terminal, record per-server progress. Writes terminal `outcome` field
/// on completion. Long-running — spawn as a tokio task.
#[allow(clippy::too_many_arguments)]
pub async fn execute_fleet_plan(
    pool: PgPool,
    agents: crate::services::agent::AgentRegistry,
    run_id: Uuid,
    plan: Vec<FleetPlanRow>,
    target_version: String,
    halt_on_failure: bool,
    include_panel: Option<UpdateStateHandle>,
) {
    let mut progress: Vec<FleetProgressRow> = plan
        .iter()
        .map(|p| FleetProgressRow {
            server_id: p.server_id,
            status: "pending".into(),
            duration_ms: None,
            error: None,
        })
        .collect();

    let mut any_failed = false;
    let mut halted = false;

    for (idx, row) in plan.iter().enumerate() {
        if halted {
            progress[idx].status = "skipped".into();
            continue;
        }
        let started = std::time::Instant::now();
        progress[idx].status = "updating".into();
        let _ = persist_progress(&pool, run_id, &progress).await;

        let result = update_one_server(&agents, row.server_id, &target_version).await;
        let elapsed_ms = started.elapsed().as_millis() as i64;
        progress[idx].duration_ms = Some(elapsed_ms);

        match result {
            Ok(()) => {
                progress[idx].status = "succeeded".into();
            }
            Err(e) => {
                progress[idx].status = "failed".into();
                progress[idx].error = Some(e.to_string());
                any_failed = true;
                if halt_on_failure {
                    halted = true;
                }
            }
        }
        let _ = persist_progress(&pool, run_id, &progress).await;
    }

    let outcome = if !any_failed {
        "success"
    } else if halted {
        "halted"
    } else {
        "partial"
    };

    let _ = sqlx::query(
        "UPDATE fleet_update_runs SET finished_at = NOW(), outcome = $1 WHERE id = $2",
    )
    .bind(outcome)
    .bind(run_id)
    .execute(&pool)
    .await;

    tracing::info!("Fleet update run {run_id} completed with outcome {outcome}");

    // `include_panel` (design memo §3.D5: fleet rolls first, the panel itself
    // last) was stored on the run row and read by nothing — the operator's
    // checkbox has been inert since v2.10.0. Honour it now, and only when every
    // member actually made it: updating the panel on top of a half-rolled fleet
    // is the one ordering the design explicitly rules out.
    if let Some(handle) = include_panel {
        if any_failed {
            tracing::warn!(
                "Fleet run {run_id}: include_panel was requested but the fleet did not \
                 fully succeed ({outcome}) — leaving the panel on its current version"
            );
            return;
        }
        match start_panel_update(handle, pool.clone(), target_version.clone(), None).await {
            Ok(_) => tracing::info!(
                "Fleet run {run_id}: fleet complete, panel self-update to {target_version} started"
            ),
            Err(e) => tracing::error!(
                "Fleet run {run_id}: fleet complete but the panel self-update could not start: {e}"
            ),
        }
    }
}

async fn persist_progress(
    pool: &PgPool,
    run_id: Uuid,
    progress: &[FleetProgressRow],
) -> Result<(), sqlx::Error> {
    let json = serde_json::to_value(progress).unwrap_or(serde_json::json!([]));
    sqlx::query("UPDATE fleet_update_runs SET progress = $1 WHERE id = $2")
        .bind(json)
        .bind(run_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// POST `/panel/update` to a remote agent, then wait for the agent to actually
/// be RUNNING the target version. Returns Ok only on that observation.
///
/// It used to return Ok as soon as the agent's own `/panel/update/status` said
/// `"succeeded"`. That state is process-local and was being set from the exit
/// status of `systemd-run`, which returns 0 the instant PID1 accepts the job —
/// so on the s232 two-box lab the panel recorded a fleet member as **succeeded
/// in 5.1s** while that member never moved off 2.11.2 and its updater aborted a
/// second later. A restart is a step, not an outcome (lesson #49); the only
/// honest evidence that an update landed is the version the agent reports for
/// itself afterwards, which is what the W4 design §4.5 specified in the first
/// place ("Polls `/health` for the new version").
async fn update_one_server(
    agents: &crate::services::agent::AgentRegistry,
    server_id: Uuid,
    target_version: &str,
) -> Result<(), String> {
    let handle = agents
        .for_server(server_id)
        .await
        .map_err(|e| format!("agent handle: {e}"))?;

    // Normalise to the `vX.Y.Z` release-tag spelling at this boundary, exactly
    // as `start_panel_update` does for the local path. The operator's input is
    // free text and `validate_target_version` accepts both spellings, so a bare
    // `2.11.4` reached the remote agent verbatim, became DOCKPANEL_VERSION, and
    // update.sh concatenated it into `releases/download/2.11.4/...` — a 404 and
    // `curl -sfL` exit 22 before any swap. The local path was repaired in
    // v2.11.3; this sibling call site was missed. update.sh's own bare-semver
    // tolerance does not rescue it, because fleet targets are by definition
    // servers on OLDER builds whose on-disk update.sh predates that heal.
    let normalized = format!("v{}", target_version.trim_start_matches('v'));
    let payload = Some(serde_json::json!({ "target_version": normalized }));
    if let Err(e) = handle.post("/panel/update", payload).await {
        let raw = e.to_string();
        // Agents before 2.11.8 could only run the PANEL updater, which is not
        // present on an agent-only box — so they refuse with this exact message
        // and there is nothing the panel can do about it remotely. Say what to
        // do instead of handing the operator a path that does not exist.
        if raw.contains("update script not found") {
            return Err(format!(
                "this agent is too old to be updated from the panel (it looks for the panel \
                 updater, which agent-only boxes do not have). Re-run install-agent.sh on it \
                 once to reach 2.11.8+, after which fleet updates work — original error: {raw}"
            ));
        }
        return Err(format!("POST /panel/update: {raw}"));
    }

    // Total budget ~10 min: the remote agent downloads a ~21MB binary, restarts,
    // and has to come back up.
    let target_clean = target_version.trim_start_matches('v').to_string();
    let deadline = std::time::Instant::now() + Duration::from_secs(600);
    let mut last_seen_version = String::new();

    while std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_secs(5)).await;

        // GROUND TRUTH first. `/health` is served by whichever binary is
        // actually running, so it cannot report a version that is not installed.
        // Connection errors are expected here — the agent restarts itself
        // partway through — and are never treated as failure.
        if let Ok(health) = handle.get("/health").await {
            if let Some(v) = health.get("version").and_then(|v| v.as_str()) {
                last_seen_version = v.to_string();
                if v.trim_start_matches('v') == target_clean {
                    return Ok(());
                }
            }
        }

        // Then the agent's own report, which is only trusted when it says
        // something went WRONG — a claim it can make honestly, unlike success.
        // (After the restart this also surfaces the updater's on-disk verdict,
        // which is the only record that survives the process that wrote it.)
        if let Ok(resp) = handle.get("/panel/update/status").await {
            let state = resp.get("state").and_then(|s| s.as_str()).unwrap_or("");
            if state == "failed" || state == "rolled_back" {
                return Err(format!(
                    "agent reported {state}: {}",
                    resp.get("reason")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unspecified")
                ));
            }
        }
    }

    Err(format!(
        "timed out after 10min — agent still reports version {}, wanted {target_clean}",
        if last_seen_version.is_empty() {
            "nothing".into()
        } else {
            last_seen_version
        }
    ))
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_version_regex_accepts_canonical_shapes() {
        assert!(validate_target_version("v2.10.0"));
        assert!(validate_target_version("2.10.0"));
        assert!(validate_target_version("v2.10.0-rc.1"));
        assert!(validate_target_version("2.10.0-rc.12"));
        assert!(validate_target_version("v10.20.30"));
    }

    #[test]
    fn target_version_regex_rejects_garbage() {
        assert!(!validate_target_version(""));
        assert!(!validate_target_version("v"));
        assert!(!validate_target_version("v2"));
        assert!(!validate_target_version("v2.10"));
        assert!(!validate_target_version("v2.10.0.1"));
        assert!(!validate_target_version("v2.10.0-alpha"));
        assert!(!validate_target_version("v2.10.0-rc"));
        assert!(!validate_target_version("v2.10.0-rc."));
        assert!(!validate_target_version("v2.10.0-rc.a"));
        assert!(!validate_target_version("v2.10.0 "));
        assert!(!validate_target_version("2.10.0; rm -rf /"));
        assert!(!validate_target_version("latest"));
    }

    #[test]
    fn parse_target_from_trigger_matches_pre_update() {
        assert_eq!(
            parse_target_from_trigger("pre-update:v2.10.0"),
            Some("v2.10.0".into())
        );
        assert_eq!(
            parse_target_from_trigger("pre-update:v2.10.0-rc.1"),
            Some("v2.10.0-rc.1".into())
        );
        assert_eq!(parse_target_from_trigger("manual"), None);
        let uuid = Uuid::new_v4();
        assert_eq!(
            parse_target_from_trigger(&format!("fleet:{uuid}")),
            None
        );
    }

    #[test]
    fn fleet_plan_orders_by_semver_not_lexicographically() {
        // The bug this replaces: postgres sorting `agent_version` as text puts
        // "2.9.0" AFTER "2.10.0", so the rolling update starts with the NEWEST
        // box while claiming to start with the oldest.
        let mut vs = vec![
            Some("2.10.0"),
            Some("v2.9.0"),
            None,
            Some("2.11.7"),
            Some("2.9.10"),
        ];
        vs.sort_by_key(|v| semver_key(*v));
        assert_eq!(
            vs,
            vec![
                None,
                Some("v2.9.0"),
                Some("2.9.10"),
                Some("2.10.0"),
                Some("2.11.7")
            ]
        );
    }

    #[test]
    fn semver_key_tolerates_junk_without_reordering_the_queue() {
        assert_eq!(semver_key(None), (0, 0, 0, 0));
        assert_eq!(semver_key(Some("v2.11.7-rc.3")), (1, 2, 11, 7));
        assert_eq!(semver_key(Some("garbage")), (1, 0, 0, 0));
        assert_eq!(semver_key(Some("2.11")), (1, 2, 11, 0));
    }

    /// `settings::recording_coverage` decides which fleet members will obey the
    /// terminal-recording toggle by comparing this key against the release that
    /// began honouring the signed `record` claim. Two edges matter and both are
    /// easy to get backwards: the release itself must count as covered, and a
    /// server that has never checked in (no version) must count as NOT covered —
    /// reporting an unverified member as covered is the exact claim the endpoint
    /// exists to stop the UI making.
    #[test]
    fn recording_gate_boundary_sorts_the_way_coverage_depends_on() {
        let gate = semver_key(Some("2.46.0"));

        assert!(semver_key(Some("2.45.1")) < gate, "an older agent is not covered");
        assert!(semver_key(Some("2.9.0")) < gate, "2.9 < 2.46 numerically, not lexicographically");
        assert!(semver_key(None) < gate, "a server that never checked in is not covered");
        assert!(semver_key(Some("unknown")) < gate, "an unparseable version is not covered");

        assert!(semver_key(Some("2.46.0")) >= gate, "the gate release itself is covered");
        assert!(semver_key(Some("2.46.1")) >= gate, "a newer patch is covered");
        assert!(semver_key(Some("v2.47.0")) >= gate, "a v-prefixed newer release is covered");
        assert!(semver_key(Some("2.100.0")) >= gate, "2.100 > 2.46 numerically");
    }

    /// The fleet orchestrator must decide "updated" from what the agent is
    /// RUNNING, not from what it says about itself — the agent's own state was
    /// being set from the exit status of the process that merely launched the
    /// update, and reported success 124ms in (lesson #49, measured s232).
    #[test]
    fn update_one_server_confirms_from_ground_truth() {
        let src = include_str!("panel_update.rs");
        let body = &src[src.find("async fn update_one_server").unwrap()..];
        let body = &body[..body.find("// ── Tests").unwrap_or(body.len())];
        assert!(
            body.contains("handle.get(\"/health\")"),
            "must read the version the agent is actually running"
        );
        let ok_at = body.find("return Ok(())").expect("no success path");
        let health_at = body.find("handle.get(\"/health\")").unwrap();
        assert!(
            health_at < ok_at,
            "the only success path must be downstream of the health probe"
        );
        // Negative control: the exact line that shipped.
        assert!(
            !body.contains("\"succeeded\" => return Ok(())"),
            "a self-reported state string is not evidence an update landed"
        );
    }

    #[tokio::test]
    async fn idle_state_handle_starts_idle() {
        let handle = new_state_handle();
        let s = handle.read().await;
        assert!(matches!(*s, UpdateState::Idle));
    }
}
