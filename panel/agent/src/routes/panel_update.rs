//! Phase 4 W4: agent-side panel-update receiver.
//!
//! When the central panel orchestrator pushes a fleet update to a remote
//! server, it POSTs `/panel/update` here. This handler hands the work to a
//! PID1-owned transient unit and returns 202 immediately; the orchestrator
//! then confirms the outcome from `/health`, not from anything this file
//! says about itself.
//!
//! Distinct from `routes::updates` (OS-package apt-get management); this
//! is panel self-update specifically.
//!
//! ## What this used to do, and why it was wrong (s232, measured on a lab)
//!
//! It ran `/opt/dockpanel/scripts/update.sh`. Two defects, and the second is
//! why the first could not just be patched by shipping the repo:
//!
//! 1. `install-agent.sh` never creates `/opt/dockpanel`. Every fleet update
//!    against a real agent-only box died in 166 ms with `500: update script
//!    not found`, and that installer is the only documented way to add a
//!    remote server — so the feature never worked on any box it targeted.
//! 2. `update.sh` is the *panel* updater: it wants a git repo, a postgres
//!    container, an API, a frontend, nginx. Planting the repo and re-running
//!    it produced `No such container: dockpanel-postgres` → `Database backup
//!    failed, aborting upgrade` — one second *after* the panel had recorded
//!    the server as **succeeded**, because `update.sh` re-execs itself into a
//!    transient unit with `exec systemd-run` (no `--wait`), so the child this
//!    file waits on exits 0 the moment PID1 *accepts* the job. Status was
//!    being derived from the wrong process (lesson #49), and fixing only (1)
//!    would have converted a loud safe failure into a silent false success on
//!    every box in a fleet (lesson #50).
//!
//! So: agent boxes get an agent-only update (`scripts/agent-self-update.sh`,
//! embedded below), full panel installs keep `update.sh`, and success is
//! never inferred from a child's exit status.

use axum::{routing::{get, post}, Json, Router};
use serde::{Deserialize, Serialize};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use super::AppState;

/// The panel updater, present only on full DockPanel installs.
const UPDATE_SCRIPT: &str = "/opt/dockpanel/scripts/update.sh";
/// Present on a full install only; used to tell a panel box from an agent box.
const API_BIN: &str = "/usr/local/bin/dockpanel-api";

/// The agent-only updater. Compiled in rather than read from disk for the same
/// reason the restore procedure is (lessons #35/#38): a remote agent box has no
/// repo, and a script the running binary merely hopes is in sync is drift.
const AGENT_UPDATE_SCRIPT: &str = include_str!("../../../../scripts/agent-self-update.sh");
const AGENT_UPDATE_SCRIPT_PATH: &str = "/var/lib/dockpanel/agent-self-update.sh";
/// Where `agent-self-update.sh` records what actually happened. Read back after
/// the restart it performs, because the agent's in-memory state does not
/// survive its own update.
const AGENT_UPDATE_RESULT_PATH: &str = "/var/lib/dockpanel/last-agent-update.json";

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AgentUpdateState {
    Idle,
    InFlight {
        target_version: String,
        started_at: chrono::DateTime<chrono::Utc>,
        last_log_line: Option<String>,
    },
    Succeeded {
        version: String,
        completed_at: chrono::DateTime<chrono::Utc>,
    },
    Failed {
        reason: String,
        at: chrono::DateTime<chrono::Utc>,
    },
}

/// Process-local state, and deliberately only ever a *negative* signal plus a
/// progress window. An update restarts this process, so this state cannot
/// outlive the thing it describes; the orchestrator confirms success from
/// `/health` (which is what the W4 design §4.5 said to do all along). Nothing
/// here may report `Succeeded` on its own authority — that is exactly the bug
/// this file shipped with.
static STATE: Mutex<AgentUpdateState> = Mutex::new(AgentUpdateState::Idle);

#[derive(Debug, Deserialize)]
struct UpdateRequest {
    target_version: String,
}

#[derive(Debug, Serialize)]
struct ApplyResponse {
    accepted: bool,
    target_version: String,
}

/// Hand-rolled validator (matches backend's `validate_target_version`).
fn validate_target_version(v: &str) -> bool {
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

/// POST /panel/update — kick off the update with the target version.
/// Returns 202 with the accepted target. Caller polls /panel/update/status.
///
/// A thin HTTP shell over `start_agent_self_update`, which is shared with the
/// agent's own periodic update check. There is exactly one implementation of
/// "start an update on this box", so the timer cannot drift into a second,
/// weaker one — which is precisely what it used to be (s233).
async fn apply_panel_update(
    Json(body): Json<UpdateRequest>,
) -> Result<Json<ApplyResponse>, (axum::http::StatusCode, String)> {
    let target = body.target_version.trim().to_string();
    match start_agent_self_update(&target).await {
        Ok(()) => Ok(Json(ApplyResponse {
            accepted: true,
            target_version: target,
        })),
        Err(e) => Err((e.status_code(), e.to_string())),
    }
}

/// Why an update could not be started. The HTTP path maps these to status
/// codes; the timer path logs them.
#[derive(Debug)]
pub(crate) enum StartUpdateError {
    Invalid(String),
    InFlight,
    Internal(String),
}

impl StartUpdateError {
    fn status_code(&self) -> axum::http::StatusCode {
        match self {
            Self::Invalid(_) => axum::http::StatusCode::BAD_REQUEST,
            Self::InFlight => axum::http::StatusCode::CONFLICT,
            Self::Internal(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl std::fmt::Display for StartUpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(v) => write!(f, "invalid target_version: {v}"),
            Self::InFlight => write!(f, "an update is already in flight"),
            Self::Internal(m) => write!(f, "{m}"),
        }
    }
}

/// Start an update of this box to `target`, and return as soon as the work has
/// been handed to PID1. Never blocks on the update itself and never reports
/// success — the caller confirms from `/health` and from the updater's own
/// verdict file.
///
/// Shared by the panel-driven fleet path (`POST /panel/update`) and the agent's
/// own periodic check, so both get the same validation, the same liveness
/// guard, and the same checksum-verified, health-verified, rollback-capable
/// updater.
pub(crate) async fn start_agent_self_update(target: &str) -> Result<(), StartUpdateError> {
    let target = target.trim().to_string();
    if !validate_target_version(&target) {
        return Err(StartUpdateError::Invalid(target));
    }
    // A full panel install keeps the panel updater (it has an API, a frontend
    // and a database to move). Everything else — i.e. every box added with
    // install-agent.sh, which is the entire fleet population — gets the
    // agent-only updater, which needs nothing from /opt.
    let mode = if std::path::Path::new(UPDATE_SCRIPT).exists()
        && std::path::Path::new(API_BIN).exists()
    {
        UpdateMode::FullPanel
    } else {
        UpdateMode::AgentOnly
    };

    // Already-in-flight guard. It has to ask whether the in-flight run is
    // actually still running: an updater that fails inside its own transient
    // unit never restarts this process, so `InFlight` would otherwise stick
    // for the life of the agent and every later attempt — including the one
    // fixing the operator's typo — would be refused 409 for ever. Observed on
    // the s232 lab immediately after the previous fix landed.
    //
    // It also serialises the timer against an operator-driven fleet run: two
    // updates must never race for the same binary.
    //
    // Checking and claiming happen under ONE lock acquisition. They used to be
    // two, which left a window where two callers could both observe "not in
    // flight" and both go on to spawn an updater. That was unreachable while the
    // only caller was HTTP — the orchestrator drives fleet members one at a time
    // — but s233 added the agent's own timer as a second, independent caller
    // that can fire at any moment, including the moment an operator starts a
    // fleet run. Making a dormant race reachable is exactly lesson #50, so the
    // guard is now atomic: the state may not change between the question and
    // the claim.
    {
        let mut s = STATE.lock().unwrap();
        if let AgentUpdateState::InFlight { started_at, .. } = &*s {
            if !run_has_finished(*started_at) {
                return Err(StartUpdateError::InFlight);
            }
        }
        *s = AgentUpdateState::InFlight {
            target_version: target.clone(),
            started_at: chrono::Utc::now(),
            last_log_line: None,
        };
    }

    // Materialise the embedded updater before returning, so a box that cannot
    // even write it gets a real error instead of a silent no-op.
    if matches!(mode, UpdateMode::AgentOnly) {
        if let Err(e) = write_agent_update_script().await {
            let mut s = STATE.lock().unwrap();
            *s = AgentUpdateState::Idle;
            return Err(StartUpdateError::Internal(format!(
                "could not stage the agent updater: {e}"
            )));
        }
    }

    tokio::spawn(async move {
        run_update_subprocess(target, mode).await;
    });

    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum UpdateMode {
    /// Box has an API binary and the panel updater — update the whole panel.
    FullPanel,
    /// Agent-only box (install-agent.sh). Update just the agent binary.
    AgentOnly,
}

async fn write_agent_update_script() -> std::io::Result<()> {
    tokio::fs::create_dir_all("/var/lib/dockpanel").await?;
    tokio::fs::write(AGENT_UPDATE_SCRIPT_PATH, AGENT_UPDATE_SCRIPT).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            AGENT_UPDATE_SCRIPT_PATH,
            std::fs::Permissions::from_mode(0o700),
        )?;
    }
    Ok(())
}

async fn run_update_subprocess(target: String, mode: UpdateMode) {
    // Both updaters restart dockpanel-agent.service, and that unit is
    // KillMode=control-group — so anything still in this process's cgroup when
    // that happens is SIGTERMed mid-update. PID1 has to own the work
    // (lesson #47). `update.sh` self-detaches the same way; doing it here too
    // means the agent path does not depend on the remote box's copy of a script
    // being new enough to protect itself.
    let unit = format!(
        "dockpanel-agent-update-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );

    let mut cmd = Command::new("systemd-run");
    cmd.arg("--quiet")
        .arg("--collect")
        .arg(format!("--unit={unit}"))
        .arg(format!("--setenv=DOCKPANEL_VERSION={target}"));

    match mode {
        UpdateMode::FullPanel => {
            cmd.arg("--setenv=INSTALL_FROM_RELEASE=1")
                .arg("--setenv=DOCKPANEL_NO_SELF_REFRESH=1")
                .arg("--setenv=DOCKPANEL_UPDATE_DETACHED=1")
                .arg("bash")
                .arg(UPDATE_SCRIPT);
        }
        UpdateMode::AgentOnly => {
            cmd.arg("--setenv=DOCKPANEL_AGENT_UPDATE_DETACHED=1")
                .arg("bash")
                .arg(AGENT_UPDATE_SCRIPT_PATH);
        }
    }

    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let mut s = STATE.lock().unwrap();
            *s = AgentUpdateState::Failed {
                reason: format!("spawn failed: {e}"),
                at: chrono::Utc::now(),
            };
            return;
        }
    };

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    if let Some(s) = stdout {
        tokio::spawn(async move {
            let mut reader = BufReader::new(s).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if line.is_empty() {
                    continue;
                }
                tracing::info!(target: "panel_update", "{line}");
                let mut st = STATE.lock().unwrap();
                if let AgentUpdateState::InFlight { last_log_line, .. } = &mut *st {
                    *last_log_line = Some(line.chars().take(256).collect());
                }
            }
        });
    }
    if let Some(s) = stderr {
        tokio::spawn(async move {
            let mut reader = BufReader::new(s).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if line.is_empty() {
                    continue;
                }
                tracing::warn!(target: "panel_update", "{line}");
            }
        });
    }

    // `systemd-run` without `--wait` returns as soon as PID1 has ACCEPTED the
    // job — measured at ~120 ms, while the real update runs for minutes and may
    // fail outright. So a zero exit here means "the work was handed off", never
    // "the work succeeded", and this function must not promote it to
    // `Succeeded`. It used to, and the panel therefore reported a fleet member
    // as updated one second before that member's updater aborted (lesson #49).
    //
    // A NON-zero exit is still real information: PID1 refused the job, so
    // nothing is running and the update definitively did not happen.
    match tokio::time::timeout(Duration::from_secs(900), child.wait()).await {
        Ok(Ok(status)) => {
            if !status.success() {
                let mut s = STATE.lock().unwrap();
                *s = AgentUpdateState::Failed {
                    reason: format!("could not start the update unit ({status})"),
                    at: chrono::Utc::now(),
                };
            }
            // Success case: stay InFlight. Either this process is replaced by
            // the restart (state resets to Idle and /health carries the truth),
            // or the updater's own result file explains the failure.
        }
        Ok(Err(e)) => {
            let mut s = STATE.lock().unwrap();
            *s = AgentUpdateState::Failed {
                reason: format!("wait error: {e}"),
                at: chrono::Utc::now(),
            };
        }
        Err(_) => {
            let mut s = STATE.lock().unwrap();
            *s = AgentUpdateState::Failed {
                reason: "updater did not return within 15min".into(),
                at: chrono::Utc::now(),
            };
        }
    }
}

/// The agent-only updater's verdict file. This is the record that survives the
/// restart the update performs, which the in-memory state by definition cannot.
/// `Ok(target)` on success, `Err((stage, reason))` on failure, `None` if no
/// update has ever run here.
fn last_agent_update_result() -> Option<Result<String, (String, String)>> {
    let v = read_agent_update_result()?;
    let field = |k: &str, d: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .unwrap_or(d)
            .to_string()
    };
    match v.get("ok").and_then(|o| o.as_bool()) {
        Some(true) => Some(Ok(field("target_version", "").trim_start_matches('v').to_string())),
        Some(false) => Some(Err((
            field("stage", "unknown"),
            field("reason", "agent update failed"),
        ))),
        None => None,
    }
}

fn read_agent_update_result() -> Option<serde_json::Value> {
    let raw = std::fs::read_to_string(AGENT_UPDATE_RESULT_PATH).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Stages `agent-self-update.sh` can only reach once it has ALREADY replaced the
/// binary. A failure at one of these has cost a download, a swap and two
/// restarts of this service; a failure before them cost a download at most.
/// `unit` belongs here even though it replaces a text file rather than the
/// binary: it runs AFTER the swap, so a run that died in it has already left a
/// new binary on disk, and it can itself leave a unit the agent cannot start
/// under. Retrying that unattended is the loop this list exists to stop.
const POST_SWAP_STAGES: &[&str] = &["swap", "unit", "restart", "verify-running", "rollback"];

/// Did a previous run already fail this exact target *after* replacing the
/// binary? Returns the stage and reason if so.
///
/// This exists to stop an unattended retry loop. The agent's timer decides to
/// update by comparing the panel's target against its own version — and a failed
/// update leaves its version unchanged, so that comparison stays true for ever.
/// With a target that cannot come up on this box, the timer would otherwise
/// re-run the whole destructive sequence indefinitely: download, swap, restart,
/// fail the health poll, roll back, restart again. And because the rollback
/// restarts the process, the loop begins again at its *initial* delay rather
/// than the 6-hour one — so the pathological case would repeat roughly HOURLY,
/// on every box in the fleet, for ever.
///
/// Deliberately NOT consulted by `start_agent_self_update`: an operator starting
/// a fleet update is making an explicit decision and must be able to override
/// this. Only the unattended path refuses.
pub(crate) fn target_already_failed_destructively(target: &str) -> Option<String> {
    let v = read_agent_update_result()?;
    if v.get("ok").and_then(|o| o.as_bool()) != Some(false) {
        return None;
    }
    let recorded = v.get("target_version").and_then(|t| t.as_str())?;
    if recorded.trim_start_matches('v') != target.trim_start_matches('v') {
        return None;
    }
    let stage = v.get("stage").and_then(|s| s.as_str())?;
    if !POST_SWAP_STAGES.contains(&stage) {
        return None;
    }
    Some(format!(
        "stage {stage}: {}",
        v.get("reason")
            .and_then(|r| r.as_str())
            .unwrap_or("no reason recorded")
    ))
}

/// Has the run that began at `started_at` already reached a verdict?
///
/// This is the one predicate that decides both "may a new update start" and
/// "does the verdict on disk describe the run we are being asked about". The
/// in-memory state cannot answer either on its own: a successful update
/// replaces this process (so the state resets and says nothing), and a failure
/// inside the transient unit never touches it at all.
///
/// A verdict older than the run's start belongs to a previous run and is ignored.
///
/// The comparison is at WHOLE-SECOND resolution on both sides, and that is
/// load-bearing, not sloppy. The verdict's `at` is written by the shell updater
/// with `date +%S` — whole seconds — while `started_at` is `Utc::now()` with
/// sub-second precision. Comparing them directly means a run that reaches its
/// verdict *inside the same wall-clock second it began* has its floored verdict
/// timestamp fall below `started_at` and be judged stale — so the state stays
/// `InFlight` for ever and every later attempt is refused `409`. That is the
/// exact wedge this predicate exists to prevent, and the FASTEST failures hit
/// it every time: a 404 download resolves in ~0.25s, well within one second, so
/// a mistyped or unreleased target would reliably wedge the box (measured on the
/// s233 lab — it is why the fleet regression looked flaky). Flooring
/// `started_at` to its second makes a verdict written in the same second count
/// as belonging to this run, which is what "conservative" was reaching for.
fn run_has_finished(started_at: chrono::DateTime<chrono::Utc>) -> bool {
    let Some(v) = read_agent_update_result() else {
        return false;
    };
    let Some(at) = v.get("at").and_then(|a| a.as_str()) else {
        return false;
    };
    chrono::DateTime::parse_from_rfc3339(at)
        .map(|t| t.with_timezone(&chrono::Utc).timestamp() >= started_at.timestamp())
        .unwrap_or(false)
}

/// GET /panel/update/status — a *negative* signal plus a progress window.
///
/// `idle` after the update restarts this process, which is why the caller must
/// treat the running version (always included below, and on `/health`) as the
/// authority on whether an update landed. What this endpoint can say reliably
/// is that something FAILED: either in-process, or — across the restart — from
/// the updater's own result file.
async fn get_panel_update_status() -> Json<serde_json::Value> {
    let s = STATE.lock().unwrap().clone();
    let mut value = serde_json::to_value(&s).unwrap_or(serde_json::json!({ "state": "idle" }));

    // Consult the updater's verdict whenever it describes THIS run, not only
    // once the process has restarted. A failure that stops before the restart
    // (a target release that does not exist, say) leaves this state `in_flight`
    // for ever, and the caller would then wait out its whole 10-minute deadline
    // and report a timeout — when the real, actionable reason had been on disk
    // one second in. Measured on the s232 lab.
    let state_str = value
        .get("state")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let verdict_applies = match state_str.as_str() {
        "idle" => true,
        "in_flight" => value
            .get("started_at")
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|started| run_has_finished(started.with_timezone(&chrono::Utc)))
            .unwrap_or(false),
        _ => false,
    };

    if verdict_applies {
        match last_agent_update_result() {
            Some(Err((stage, reason))) => {
                value = serde_json::json!({
                    "state": "failed",
                    "reason": format!("{reason} (stage {stage})"),
                });
            }
            // The one place `Succeeded` may be constructed, and note WHERE it
            // comes from: the updater's on-disk verdict AND the version this
            // very process reports for itself. It is a statement about the
            // binary that is executing, not a prediction made by the process
            // that launched the update. A panel too old to check `/health`
            // still gets a truthful answer from this.
            Some(Ok(version)) if version == env!("CARGO_PKG_VERSION") => {
                value = serde_json::to_value(AgentUpdateState::Succeeded {
                    version,
                    completed_at: chrono::Utc::now(),
                })
                .unwrap_or(value);
            }
            _ => {}
        }
    }

    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "running_version".into(),
            serde_json::Value::String(env!("CARGO_PKG_VERSION").to_string()),
        );
    }
    Json(value)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/panel/update", post(apply_panel_update))
        .route("/panel/update/status", get(get_panel_update_status))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_version_validator_matches_the_panel_side() {
        for good in ["v2.11.7", "2.11.7", "v2.11.7-rc.1", "10.20.30"] {
            assert!(validate_target_version(good), "{good} should be accepted");
        }
        for bad in ["", "v", "v2.11", "latest", "2.11.7; rm -rf /", "v2.11.7-alpha"] {
            assert!(!validate_target_version(bad), "{bad} should be rejected");
        }
    }

    /// The embedded updater must never `cp` onto the binary it is replacing:
    /// writing to an executing file fails ETXTBSY, and a restore path that
    /// cannot restore is not a safety net (lesson #48). Renames only.
    #[test]
    fn agent_updater_installs_by_rename_not_by_copy_onto_the_target() {
        for line in AGENT_UPDATE_SCRIPT.lines() {
            let l = line.trim();
            if l.starts_with('#') {
                continue;
            }
            // The hazard is writing onto the file that is CURRENTLY EXECUTING,
            // which fails ETXTBSY. So the rule is about the destination, not
            // about `cp` as such: nothing may copy onto $AGENT_BIN. Copying the
            // live binary INTO the backup path is the documented exception
            // (reading a running executable is always allowed), and s271 added
            // a second, unrelated copy — the systemd unit into its rollback
            // backup, a plain text file no process holds open.
            if l.starts_with("cp ") || l.starts_with("cp -") {
                assert!(
                    !l.contains("\"$AGENT_BIN\"") || l.contains("\"$BACKUP\""),
                    "nothing may copy onto the executing binary: {l}"
                );
            }
        }
        assert!(AGENT_UPDATE_SCRIPT.contains("mv -f \"$STAGED\" \"$AGENT_BIN\""));
    }

    /// Download, then verify, then install — in that order and as three
    /// separate steps. Once bytes have been streamed into the consumer there is
    /// no verification window left (lesson #25).
    #[test]
    fn agent_updater_verifies_the_download_before_installing_it() {
        let s = AGENT_UPDATE_SCRIPT;
        assert!(s.contains("sha256sum"), "must checksum the download");
        assert!(s.contains("checksums.txt"), "must fetch the release checksums");
        let verify_at = s.find("sha256 mismatch").expect("mismatch guard missing");
        let install_at = s.find("could not move the new binary into place").unwrap();
        assert!(
            verify_at < install_at,
            "the checksum guard must come before the install"
        );
        assert!(
            !s.contains("curl -fsSL --max-time 600 \"${BASE}/${ASSET}\" |"),
            "the binary must be materialised to a file, never piped into a consumer"
        );
    }

    /// It restarts the unit it runs under, so PID1 has to own it (lesson #47),
    /// and a scope is not a substitute — a scope dies with the caller's session.
    #[test]
    fn agent_updater_runs_under_a_pid1_owned_transient_service() {
        assert!(AGENT_UPDATE_SCRIPT.contains("systemd-run"));
        assert!(
            !AGENT_UPDATE_SCRIPT.contains("--scope"),
            "a transient service, never a scope"
        );
        assert!(AGENT_UPDATE_SCRIPT.contains("--collect"));
    }

    /// The rollback branch must not claim it restored anything it did not, and
    /// must not swallow the failure of a restore — that combination is what made
    /// `update.sh`'s own rollback print "Rolled back to previous binaries" over
    /// a box that had not been rolled back at all (lesson #48/#58).
    #[test]
    fn the_rollback_branch_never_swallows_or_overclaims_the_restore() {
        let s = AGENT_UPDATE_SCRIPT;
        let rb = &s[s.find("stage=\"rollback\"").expect("no rollback branch")..];
        let rb = &rb[..rb.find("rm -f \"$BACKUP\"").unwrap_or(rb.len())];
        assert!(
            !rb.contains(concat!("mv -f \"$BACKUP\" \"$AGENT_BIN\" && ", "systemctl restart dockpanel-agent || true")),
            "a restore must not be suffixed with a status-swallowing || true"
        );
        assert!(
            rb.contains("if mv -f \"$BACKUP\" \"$AGENT_BIN\""),
            "the restore's own status has to be branched on"
        );
        assert!(
            rb.contains("COULD NOT RESTORE"),
            "a failed restore needs to say so, loudly, in the verdict"
        );
        assert!(
            rb.contains("running_version"),
            "and the claim must be checked against what the agent reports afterwards"
        );
    }

    /// A run that failed and a run that never happened must be distinguishable
    /// after the restart wipes this process's memory (lesson #52).
    #[test]
    fn agent_updater_always_writes_a_verdict_including_on_abort() {
        let s = AGENT_UPDATE_SCRIPT;
        assert!(s.contains("trap on_exit EXIT"), "needs an abort trap");
        assert!(
            s.contains("write_result false \"aborted at stage"),
            "the trap must record the abort"
        );
        assert!(s.contains("write_result true"), "and record success");
    }

    /// The regression this whole change exists for: nothing in the spawn path
    /// may promote a zero exit status into a success claim, because the process
    /// being waited on is the one that merely HANDS OFF the work.
    #[test]
    fn a_zero_exit_from_the_launcher_is_never_reported_as_success() {
        let src = include_str!("panel_update.rs");
        // Negative control: the exact shape that shipped, and that the s232 lab
        // caught reporting a fleet member updated 124ms after it was asked to be.
        // Assembled rather than written out, because this test greps its own
        // file and a literal here would match itself.
        let bad = concat!("if status.", "success() {");
        let code_hits = src
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .filter(|l| l.contains(bad))
            .count();
        assert_eq!(
            code_hits, 0,
            "a successful launch is not a successful update"
        );
        assert!(
            src.contains("if !status.success() {"),
            "a FAILED launch is still real information and must be recorded"
        );
    }

    /// A failure that stops before the restart leaves the in-memory state
    /// `in_flight` for ever. The caller would then wait out its full deadline
    /// and report a timeout, when the real reason had been on disk one second
    /// in — so the verdict must be consulted while in flight too, and only when
    /// it describes THIS run.
    #[test]
    fn an_in_flight_run_still_surfaces_a_verdict_written_after_it_started() {
        let src = include_str!("panel_update.rs");
        let f = &src[src.find("async fn get_panel_update_status").unwrap()..];
        let f = &f[..f.find("pub fn router").unwrap()];
        assert!(
            f.contains("\"in_flight\" =>"),
            "in_flight must be able to resolve from the verdict file"
        );
        assert!(
            f.contains("run_has_finished("),
            "and only for a verdict at least as new as this run's start"
        );
    }

    /// A failed run must not wedge the box: the in-flight guard and the status
    /// endpoint have to agree on when a run is over, or one failed update makes
    /// the box permanently un-updatable with a 409.
    #[test]
    fn the_in_flight_guard_and_the_status_endpoint_share_one_liveness_predicate() {
        let src = include_str!("panel_update.rs");
        let guard = &src[src.find("async fn apply_panel_update").unwrap()
            ..src.find("async fn run_update_subprocess").unwrap()];
        assert!(
            guard.contains("run_has_finished("),
            "the 409 guard must ask whether the run is actually still running"
        );
        // Negative control: the unconditional form that wedged the lab box.
        assert!(
            !guard.contains(concat!("if matches!(*s, AgentUpdateState::", "InFlight { .. }) {")),
            "an InFlight state alone is not evidence that an update is still running"
        );
    }

    /// The verdict timestamp is whole-second (`date +%S`) but `started_at` is
    /// sub-second, so `run_has_finished` must compare at second resolution — or
    /// a run that fails within the same second it began (every 404-fast failure)
    /// is judged stale and the box wedges at 409 for ever. This pins the
    /// comparison to `.timestamp()` (whole seconds) on both sides.
    #[test]
    fn run_finished_recognises_a_verdict_written_in_the_same_second_as_the_start() {
        let src = include_str!("panel_update.rs");
        let f = &src[src.find("fn run_has_finished").unwrap()..];
        let f = &f[..f.find("\n}").unwrap()];
        // Negative control: the bare comparison that wedged on sub-second failures.
        assert!(
            !f.contains(concat!(".with_timezone(&chrono::Utc) ", ">= started_at)")),
            "comparing a whole-second verdict against a sub-second start wedges fast failures"
        );
        assert!(
            f.contains(".timestamp() >= started_at.timestamp()"),
            "the comparison must be at whole-second resolution on both sides"
        );
    }

    /// The unattended path must not repeat a destructive update for ever.
    ///
    /// A failed update leaves the agent's version unchanged, so the timer's
    /// "am I behind?" test stays true permanently; without this the box would
    /// re-download, re-swap, re-fail and roll back on every cycle. Only stages
    /// reached AFTER the binary was replaced count — a 404 or a checksum
    /// mismatch costs nothing and should still be retried.
    #[test]
    fn a_target_that_already_failed_after_the_swap_is_not_retried_unattended() {
        for stage in ["swap", "unit", "restart", "verify-running", "rollback"] {
            assert!(
                POST_SWAP_STAGES.contains(&stage),
                "{stage} replaces or restores the binary or the unit it starts under, \
                 and must count as destructive"
            );
        }
        for stage in ["download", "verify", "arch", "probe", "init"] {
            assert!(
                !POST_SWAP_STAGES.contains(&stage),
                "{stage} happens before the swap and is safe to retry"
            );
        }
        // Every stage the updater can record must be classified, or a new one
        // silently lands in the retry-for-ever bucket.
        for line in AGENT_UPDATE_SCRIPT.lines() {
            let l = line.trim();
            let Some(rest) = l.strip_prefix("stage=\"") else { continue };
            let Some(stage) = rest.split('"').next() else { continue };
            assert!(
                POST_SWAP_STAGES.contains(&stage) || matches!(stage, "init" | "probe" | "arch" | "download" | "verify" | "complete"),
                "unclassified updater stage {stage:?} — decide whether it is destructive"
            );
        }
        // And the timer, not the shared launcher, is what consults it: an
        // operator starting a fleet update must always be able to override.
        let ph = include_str!("../services/phone_home.rs");
        assert!(
            ph.contains("target_already_failed_destructively("),
            "the unattended path must consult the previous verdict"
        );
        let src = include_str!("panel_update.rs");
        let launcher = &src[src.find("pub(crate) async fn start_agent_self_update").unwrap()
            ..src.find("async fn run_update_subprocess").unwrap()];
        assert!(
            !launcher.contains("target_already_failed_destructively("),
            "the operator-driven path must NOT be blocked by a previous failure"
        );
    }

    /// Two callers may now start an update — the panel over HTTP and the
    /// agent's own timer — so asking "is one in flight?" and claiming the slot
    /// must be a single atomic step. Split across two lock acquisitions, both
    /// callers can observe "no" and both spawn an updater for the same binary.
    #[test]
    fn the_in_flight_guard_claims_under_the_same_lock_it_checked() {
        let src = include_str!("panel_update.rs");
        let f = &src[src.find("pub(crate) async fn start_agent_self_update").unwrap()..];
        let f = &f[..f.find("// Materialise the embedded updater").unwrap()];

        let check = f.find("run_has_finished(").expect("no liveness check in the guard");
        let claim = f
            .find("*s = AgentUpdateState::InFlight")
            .expect("the guard never claims the slot");
        assert!(check < claim, "the guard must check before it claims");
        // Negative control: the two-acquisition form this shipped with until
        // s233, where the state could change between the question and the claim.
        assert!(
            !f[check..claim].contains("STATE.lock()"),
            "check and claim must happen under ONE lock acquisition"
        );
    }

    /// A full panel install keeps the panel updater; everything else — the
    /// entire fleet population — gets the agent-only one.
    #[test]
    fn update_mode_is_decided_by_what_the_box_actually_has() {
        let src = include_str!("panel_update.rs");
        assert!(src.contains("UpdateMode::FullPanel"));
        assert!(src.contains("UpdateMode::AgentOnly"));
        assert!(
            src.contains("std::path::Path::new(API_BIN).exists()"),
            "an agent-only box has no API binary — that is the discriminator"
        );
    }
}
