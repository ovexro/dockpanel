use crate::safe_cmd::safe_command;
use axum::{
    extract::{Path, Json as AxumJson},
    http::StatusCode,
    routing::{get, post, delete},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::AppState;
use crate::services::command_filter;
// ONE fail-closed reader and ONE writer for the whole crate. These lived here
// as private helpers, which is why the WordPress auto-update twin never got
// them — see services/crontab.rs.
use crate::services::crontab::{read_crontab, write_crontab};

type ApiErr = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: &str) -> ApiErr {
    (status, Json(serde_json::json!({ "error": msg })))
}

#[derive(Deserialize)]
pub struct CronRequest {
    pub id: String,
    pub command: String,
    pub schedule: String,
    pub label: Option<String>,
    /// Whether this cron should be in the crontab.
    ///
    /// The panel now sends every cron of the site being synced, enabled or not,
    /// because the payload has to be the authoritative statement of *which lines
    /// this sync owns* — see [`sync_crons`]. Defaults to `true` so a panel older
    /// than this field still schedules what it sends.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Serialize)]
pub struct CronResult {
    pub success: bool,
    pub output: Option<String>,
    pub exit_code: Option<i32>,
}

const CRONTAB_MARKER: &str = "# dockpanel:";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/crons/sync", post(sync_crons))
        .route("/crons/run", post(run_cron))
        .route("/crons/list", get(list_crons))
        .route("/crons/remove/{id}", delete(remove_cron))
}

/// POST /crons/sync — Write this site's crons to the system crontab.
/// Receives a list of crons and writes them atomically.
///
/// **This sync owns only the lines it was sent.** It used to strip every line
/// carrying `# dockpanel:` — the marker is panel-wide, not per-site — and then
/// re-add only the one site the panel was syncing. So any tenant adding, editing
/// or deleting a single cron on their OWN site silently removed every OTHER
/// site's jobs from the crontab, box-wide. Nothing errored and nothing was
/// visible: the `crons` rows are untouched, so the panel kept listing those jobs
/// as enabled while they never ran again. On a box with several WordPress sites
/// the collateral is automatic, since each install seeds a WP-Cron row.
///
/// The payload is now the authoritative set for one site, which is why it
/// carries disabled crons too: a line is removed because its id was sent and is
/// disabled, never because some other site happened to be synced.
async fn sync_crons(
    AxumJson(crons): AxumJson<Vec<CronRequest>>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    // Read the existing crontab. A read that FAILED must not be mistaken for an
    // empty crontab — that is how a transient `crontab -l` timeout used to
    // replace root's entire crontab, operator entries included, with one site's
    // jobs.
    let existing = read_crontab()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    // Keep every line except the ones this payload is authoritative for.
    let owned: std::collections::HashSet<&str> =
        crons.iter().map(|c| c.id.as_str()).collect();
    let mut lines: Vec<String> = existing
        .lines()
        .filter(|line| match marker_id(line) {
            Some(id) => !owned.contains(id),
            None => true,
        })
        .map(|s| s.to_string())
        .collect();

    // Add dockpanel crons.
    //
    // A row that fails validation is SKIPPED, not fatal to the whole sync.
    // This endpoint used to return 400 for the entire batch on the first bad
    // command, which turned one unusable row into a permanently broken feature:
    // the panel's own WordPress auto-cron was INSERTed straight into the
    // database with `> /dev/null 2>&1` in it — a pattern this very filter
    // blocks — so from the moment a WordPress site existed, every subsequent
    // add/edit/delete of ANY cron failed with "Command contains disallowed
    // characters or patterns" and nothing reached the crontab (measured on a
    // fresh Ubuntu 24.04 box, s268).
    //
    // The writer is fixed too, but fixing only the writer would leave every
    // install that already has the row broken forever, so the reader has to
    // tolerate it. Skipping keeps the guard's security property exactly as
    // strong — the command still never reaches the crontab — while costing one
    // row instead of the feature.
    let mut skipped: Vec<String> = Vec::new();
    let mut disabled = 0usize;
    for cron in &crons {
        if !cron.enabled {
            // Its line was already dropped above; not scheduling it is the point.
            disabled += 1;
            continue;
        }
        let reject = if !is_valid_schedule(&cron.schedule) {
            Some(format!("invalid schedule {:?}", cron.schedule))
        } else if cron.command.is_empty() {
            Some("empty command".to_string())
        } else if !command_filter::is_safe_cron_command(&cron.command) {
            Some("command contains disallowed characters or patterns".to_string())
        } else {
            None
        };

        if let Some(why) = reject {
            tracing::warn!("Cron {} skipped during sync: {why}", cron.id);
            skipped.push(cron.id.clone());
            continue;
        }

        let label = cron.label.as_deref().unwrap_or("");
        lines.push(format!(
            "{} {} {}{} {}",
            cron.schedule,
            cron.command,
            CRONTAB_MARKER,
            cron.id,
            label
        ));
    }

    write_crontab(&lines.join("\n")).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    let synced = crons.len() - skipped.len() - disabled;
    if skipped.is_empty() {
        tracing::info!("Synced {synced} cron jobs to system crontab");
    } else {
        tracing::warn!(
            "Synced {synced} cron jobs; {} skipped as unsafe and NOT scheduled: {}",
            skipped.len(),
            skipped.join(", ")
        );
    }
    // Report the skips rather than swallowing them — a job the operator can see
    // in the UI but which is not in the crontab is the shape that never runs
    // and never says why.
    Ok(Json(serde_json::json!({ "synced": synced, "skipped": skipped })))
}

/// POST /crons/run — Execute a cron command immediately and return output.
async fn run_cron(
    AxumJson(body): AxumJson<CronRequest>,
) -> Result<Json<CronResult>, ApiErr> {
    if body.command.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Command cannot be empty"));
    }
    if !command_filter::is_safe_cron_command(&body.command) {
        return Err(err(StatusCode::BAD_REQUEST, "Command contains disallowed characters or patterns"));
    }

    let output = tokio::time::timeout(
        Duration::from_secs(30),
        safe_command("bash")
            .arg("-c")
            .arg(&body.command)
            .output(),
    )
    .await
    .map_err(|_| err(StatusCode::GATEWAY_TIMEOUT, "Command timed out after 30s"))?
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to execute: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let combined = if stderr.is_empty() {
        stdout
    } else {
        format!("{stdout}\n--- stderr ---\n{stderr}")
    };

    // Truncate output to 10KB
    let truncated = if combined.len() > 10240 {
        format!("{}...(truncated)", &combined[..10240])
    } else {
        combined
    };

    Ok(Json(CronResult {
        success: output.status.success(),
        output: Some(truncated),
        exit_code: output.status.code(),
    }))
}

/// GET /crons/list — Read dockpanel crons from system crontab.
async fn list_crons() -> Result<Json<Vec<serde_json::Value>>, ApiErr> {
    // Read-only: a failed read destroys nothing here, so it degrades to an empty
    // listing rather than a 500. The panel's own `crons` rows are the record.
    let crontab = read_crontab().await.unwrap_or_else(|e| {
        tracing::warn!("Could not read the crontab to list crons: {e}");
        String::new()
    });
    let crons: Vec<serde_json::Value> = crontab
        .lines()
        .filter(|line| line.contains(CRONTAB_MARKER))
        .filter_map(|line| {
            // Format: schedule command # dockpanel:id label
            let marker_pos = line.find(CRONTAB_MARKER)?;
            let before_marker = &line[..marker_pos].trim();
            let after_marker = &line[marker_pos + CRONTAB_MARKER.len()..];

            // Split the after_marker into id and label
            let (id, label) = match after_marker.find(' ') {
                Some(pos) => (&after_marker[..pos], after_marker[pos + 1..].trim()),
                None => (after_marker.trim(), ""),
            };

            Some(serde_json::json!({
                "id": id,
                "entry": before_marker,
                "label": label,
            }))
        })
        .collect();

    Ok(Json(crons))
}

/// DELETE /crons/remove/{id} — Remove a specific cron by ID from crontab.
async fn remove_cron(
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    let existing = read_crontab()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    let lines: Vec<&str> = existing
        .lines()
        .filter(|line| marker_id(line) != Some(id.as_str()))
        .collect();

    write_crontab(&lines.join("\n")).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    Ok(Json(serde_json::json!({ "removed": id })))
}

/// The dockpanel cron id a crontab line carries, if any.
///
/// The marker is written as `# dockpanel:{id} {label}`, so the id is the token
/// immediately after the marker. Parsed rather than substring-matched so that a
/// line belonging to one cron cannot be attributed to another whose id merely
/// extends it.
fn marker_id(line: &str) -> Option<&str> {
    let rest = line.split(CRONTAB_MARKER).nth(1)?;
    let id = rest.split_whitespace().next()?;
    (!id.is_empty()).then_some(id)
}

/// Basic cron schedule validation (5 fields).
fn is_valid_schedule(schedule: &str) -> bool {
    let parts: Vec<&str> = schedule.split_whitespace().collect();
    if parts.len() != 5 {
        return false;
    }
    // Each field should only contain digits, *, /, -, and ,
    parts.iter().all(|part| {
        part.chars().all(|c| c.is_ascii_digit() || c == '*' || c == '/' || c == '-' || c == ',')
    })
}

#[cfg(test)]
mod sync_scope_tests {
    use super::marker_id;

    #[test]
    fn the_id_is_the_token_after_the_marker() {
        assert_eq!(
            marker_id("*/5 * * * * /bin/true # dockpanel:abc-123 My label"),
            Some("abc-123")
        );
        assert_eq!(marker_id("*/5 * * * * /bin/true # dockpanel:abc-123"), Some("abc-123"));
        assert_eq!(marker_id("0 3 * * * /usr/bin/certbot renew"), None);
        assert_eq!(marker_id("# a plain operator comment"), None);
    }

    /// The wipe this replaced: every dockpanel line on the box was stripped and
    /// only the synced site's re-added. A line whose id was NOT sent must
    /// survive, whoever owns it.
    #[test]
    fn a_line_this_sync_does_not_own_is_kept() {
        let crontab = "\
*/15 * * * * php wp-cron.php # dockpanel:site-a-cron WordPress Cron
0 2 * * * /usr/local/bin/backup # dockpanel:site-b-cron Nightly backup
0 4 * * * /usr/bin/certbot renew";
        let owned: std::collections::HashSet<&str> = ["site-a-cron"].into_iter().collect();
        let kept: Vec<&str> = crontab
            .lines()
            .filter(|l| match marker_id(l) {
                Some(id) => !owned.contains(id),
                None => true,
            })
            .collect();
        assert_eq!(kept.len(), 2, "kept: {kept:?}");
        assert!(kept.iter().any(|l| l.contains("site-b-cron")), "another site's cron was removed");
        assert!(kept.iter().any(|l| l.contains("certbot")), "an operator line was removed");
    }

    /// An id that is a PREFIX of another must not carry it off — the same
    /// unanchored-match class as the WordPress auto-update marker.
    #[test]
    fn a_prefix_id_does_not_match_a_longer_one() {
        let line = "*/5 * * * * /bin/true # dockpanel:abc-1234 label";
        assert_ne!(marker_id(line), Some("abc-123"));
        assert_eq!(marker_id(line), Some("abc-1234"));
    }
}
