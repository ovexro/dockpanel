use axum::{
    extract::{Path, Json as AxumJson},
    http::StatusCode,
    routing::{get, post, delete},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use super::AppState;

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

/// POST /crons/sync — Write all enabled crons to the system crontab.
/// Receives a list of crons and writes them atomically.
async fn sync_crons(
    AxumJson(crons): AxumJson<Vec<CronRequest>>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    // Read existing crontab, preserve non-dockpanel entries
    let existing = read_crontab().await;
    let mut lines: Vec<String> = existing
        .lines()
        .filter(|line| !line.contains(CRONTAB_MARKER))
        .map(|s| s.to_string())
        .collect();

    // Add dockpanel crons
    for cron in &crons {
        if !is_valid_schedule(&cron.schedule) {
            return Err(err(StatusCode::BAD_REQUEST, &format!("Invalid cron schedule: {}", cron.schedule)));
        }
        if cron.command.is_empty() {
            return Err(err(StatusCode::BAD_REQUEST, "Command cannot be empty"));
        }
        // Sanitize: reject shell metacharacters and dangerous patterns
        if !is_safe_command(&cron.command) {
            return Err(err(StatusCode::BAD_REQUEST, "Command contains disallowed characters or patterns"));
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

    tracing::info!("Synced {} cron jobs to system crontab", crons.len());
    Ok(Json(serde_json::json!({ "synced": crons.len() })))
}

/// POST /crons/run — Execute a cron command immediately and return output.
async fn run_cron(
    AxumJson(body): AxumJson<CronRequest>,
) -> Result<Json<CronResult>, ApiErr> {
    if body.command.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Command cannot be empty"));
    }
    if !is_safe_command(&body.command) {
        return Err(err(StatusCode::BAD_REQUEST, "Command contains disallowed characters or patterns"));
    }

    let output = tokio::process::Command::new("bash")
        .arg("-c")
        .arg(&body.command)
        .output()
        .await
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
    let crontab = read_crontab().await;
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
    let existing = read_crontab().await;
    let marker = format!("{}{}", CRONTAB_MARKER, id);
    let lines: Vec<&str> = existing
        .lines()
        .filter(|line| !line.contains(&marker))
        .collect();

    write_crontab(&lines.join("\n")).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    Ok(Json(serde_json::json!({ "removed": id })))
}

/// Read the current root crontab.
async fn read_crontab() -> String {
    let output = tokio::process::Command::new("crontab")
        .arg("-l")
        .output()
        .await;

    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => String::new(), // No crontab or error
    }
}

/// Write a new crontab for root.
async fn write_crontab(content: &str) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;

    let mut child = tokio::process::Command::new("crontab")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn crontab: {e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(content.as_bytes()).await
            .map_err(|e| format!("Failed to write crontab: {e}"))?;
        stdin.write_all(b"\n").await
            .map_err(|e| format!("Failed to write crontab newline: {e}"))?;
    }

    let status = child.wait().await
        .map_err(|e| format!("Failed to wait for crontab: {e}"))?;

    if status.success() {
        Ok(())
    } else {
        Err("crontab command failed".into())
    }
}

/// Reject commands with shell injection vectors.
fn is_safe_command(cmd: &str) -> bool {
    let dangerous = [
        "rm -rf /", "; rm ", "&& rm ",
        "$(", "`",                           // command substitution
        "| ", "||", "&&",                     // pipes and chaining
        "> /etc/", "> /root/", "> /var/",     // redirects to system dirs
        "< /etc/", "< /root/",               // reading system files
        "curl ", "wget ",                     // network access
        "chmod 777", "chown ",               // permission changes
        "mkfs", "dd if=",                    // destructive disk ops
        "shutdown", "reboot", "init ",       // system control
        "passwd", "useradd", "userdel",      // user management
        "\\x", "\\0",                        // escape sequences
    ];
    let cmd_lower = cmd.to_lowercase();
    !dangerous.iter().any(|p| cmd_lower.contains(p))
        && !cmd.contains('\0')
        && cmd.len() <= 4096
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
