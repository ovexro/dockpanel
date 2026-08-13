use chrono::Datelike;
use chrono::Timelike;
use sqlx::PgPool;
use uuid::Uuid;

use crate::services::agent::AgentRegistry;

/// Row from the scheduler query (join of schedules + destinations + sites).
#[derive(sqlx::FromRow)]
struct ScheduleRow {
    schedule_id: Uuid,
    site_id: Uuid,
    domain: String,
    schedule: String,
    retention_count: i32,
    /// The host that actually holds this site's files. NOT NULL, and stable — a
    /// site never moves between servers, so it is safe to resolve the agent from
    /// it rather than storing a copy on `backups`.
    server_id: Uuid,
    /// Recorded on the `backups` row when the upload succeeds. It carried an
    /// `#[allow(dead_code)]` for as long as this path selected it and never used
    /// it — the annotation is what kept the compiler from pointing at the gap.
    dest_id: Option<Uuid>,
    dest_dtype: Option<String>,
    dest_config: Option<serde_json::Value>,
}

/// Run the backup scheduler loop — checks every 60 seconds for due schedules.
///
/// The schedule query is fleet-wide, so this takes the REGISTRY: a site is backed up
/// on the server that holds its files. Backing a member's site up against the panel's
/// own `/var/www` either fails every night for ever (the agent refuses a webroot it
/// does not have) or — where the same domain legally exists on both hosts, which
/// `idx_sites_domain_server` permits — silently archives the wrong machine's files
/// and prunes the member's real off-site copies out of retention.
pub async fn run(db: PgPool, agents: AgentRegistry, jwt_secret: String, mut shutdown_rx: tokio::sync::broadcast::Receiver<()>) {
    tracing::info!("Backup scheduler started");

    let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));

    loop {
        tokio::select! {
            _ = interval.tick() => {
                if let Err(e) = tick(&db, &agents, &jwt_secret).await {
                    tracing::error!("Backup scheduler error: {e}");
                }
            }
            _ = shutdown_rx.recv() => {
                tracing::info!("Backup scheduler shutting down gracefully");
                break;
            }
        }
    }
}

async fn tick(db: &PgPool, agents: &AgentRegistry, jwt_secret: &str) -> Result<(), String> {
    let now = chrono::Utc::now();

    // Fetch all enabled schedules with their destination and site info
    let rows: Vec<ScheduleRow> = sqlx::query_as(
        "SELECT \
         bs.id as schedule_id, bs.site_id, s.domain, bs.schedule, bs.retention_count, \
         s.server_id, \
         bd.id as dest_id, bd.dtype as dest_dtype, bd.config as dest_config \
         FROM backup_schedules bs \
         JOIN sites s ON s.id = bs.site_id \
         LEFT JOIN backup_destinations bd ON bd.id = bs.destination_id \
         WHERE bs.enabled = true",
    )
    .fetch_all(db)
    .await
    .map_err(|e| e.to_string())?;

    for row in &rows {
        if !cron_matches_now(&row.schedule, &now) {
            continue;
        }

        // Check if we already ran this minute (prevent double-runs)
        if let Some(last) = get_last_run(db, row.schedule_id).await {
            let diff = now.signed_duration_since(last);
            if diff.num_seconds() < 90 {
                continue;
            }
        }

        tracing::info!("Running scheduled backup for {}", row.domain);
        let result = run_scheduled_backup(db, agents, jwt_secret, row).await;

        let status = if result.is_ok() { "success" } else { "failed" };
        if let Err(ref e) = result {
            tracing::error!("Scheduled backup failed for {}: {e}", row.domain);

            crate::services::system_log::log_event(
                db,
                "error",
                "backup_scheduler",
                &format!("Scheduled backup failed for {}", row.domain),
                Some(&e.to_string()),
            ).await;

            // Fire backup failure alert
            if let Ok(Some((user_id,))) = sqlx::query_as::<_, (Uuid,)>(
                "SELECT user_id FROM sites WHERE id = $1",
            )
            .bind(row.site_id)
            .fetch_optional(db)
            .await
            {
                crate::services::notifications::fire_alert(
                    db,
                    user_id,
                    None,
                    Some(row.site_id),
                    "backup_failure",
                    "",
                    "critical",
                    &format!("Backup failed: {}", row.domain),
                    &format!(
                        "Scheduled backup for {} failed: {e}",
                        row.domain
                    ),
                )
                .await;
            }
        }

        // Update last_run
        let _ = sqlx::query(
            "UPDATE backup_schedules SET last_run = NOW(), last_status = $1, updated_at = NOW() WHERE id = $2",
        )
        .bind(status)
        .bind(row.schedule_id)
        .execute(db)
        .await;
    }

    Ok(())
}

async fn get_last_run(db: &PgPool, schedule_id: Uuid) -> Option<chrono::DateTime<chrono::Utc>> {
    sqlx::query_scalar("SELECT last_run FROM backup_schedules WHERE id = $1")
        .bind(schedule_id)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
        .flatten()
}

async fn run_scheduled_backup(
    db: &PgPool,
    agents: &AgentRegistry,
    jwt_secret: &str,
    row: &ScheduleRow,
) -> Result<(), String> {
    // Resolve the site's OWN server before anything else. Returning Err here routes
    // into tick()'s existing failure arm, which marks the schedule failed, writes a
    // system_log entry and fires a critical alert — the "refuse out loud" contract,
    // reusing the plumbing that is already there. Never fall back to the local agent:
    // the disk pre-flight, the archive, the upload and the prune below would all then
    // act on a host that does not hold this site.
    let agent = agents.for_server(row.server_id).await.map_err(|e| {
        format!(
            "server {} is unreachable ({e}) — refusing to back up {} on a different host",
            row.server_id, row.domain
        )
    })?;

    // 0. Pre-flight: check disk space via agent before creating backup
    if let Ok(sys_info) = agent.get("/system/info").await {
        let disk_pct = sys_info
            .get("disk_usage_pct")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        if disk_pct > 90.0 {
            // Fire alert about low disk space preventing backup
            if let Ok(Some((user_id,))) = sqlx::query_as::<_, (Uuid,)>(
                "SELECT user_id FROM sites WHERE id = $1",
            )
            .bind(row.site_id)
            .fetch_optional(db)
            .await
            {
                crate::services::notifications::fire_alert(
                    db,
                    user_id,
                    None,
                    Some(row.site_id),
                    "backup_failure",
                    "",
                    "warning",
                    &format!("Backup skipped (low disk): {}", row.domain),
                    &format!(
                        "Scheduled backup for {} was skipped because disk usage is {:.1}% (>90%). \
                         Free up disk space to resume automatic backups.",
                        row.domain, disk_pct
                    ),
                )
                .await;
            }
            crate::services::system_log::log_event(
                db,
                "warning",
                "backup_scheduler",
                &format!("Backup skipped for {} — disk at {disk_pct:.1}%", row.domain),
                None,
            ).await;

            return Err(format!(
                "Disk usage too high ({disk_pct:.1}% > 90%) — backup skipped"
            ));
        }
    }

    // 1. Create backup via agent — WITH the site's databases. A scheduled
    // backup that quietly omitted them would be the exact defect v2.34.0
    // closed, surviving in the path people rely on most.
    let site_dbs = crate::routes::backups::site_database_specs(db, jwt_secret, row.site_id).await;
    let db_expected = site_dbs.expected() as i32;
    let agent_body = serde_json::json!({ "databases": site_dbs.specs });

    let agent_path = format!("/backups/{}/create", row.domain);
    let backup_result = agent
        .post(&agent_path, Some(agent_body))
        .await
        .map_err(|e| format!("Backup creation failed: {e}"))?;

    let db_included = backup_result
        .get("databases_included")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0) as i32;
    if db_included < db_expected {
        // Loud, because nobody is watching a scheduled run.
        let missing: Vec<String> = backup_result
            .get("databases_failed")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|f| {
                let n = f.get("db_name")?.as_str()?;
                let w = f.get("error").and_then(|v| v.as_str()).unwrap_or("unknown error");
                Some(format!("{n}: {w}"))
            }).collect())
            .unwrap_or_default();
        tracing::error!(
            "Scheduled backup for {} is INCOMPLETE — {db_included} of {db_expected} databases included: {}",
            row.domain, missing.join("; ")
        );
        crate::services::system_log::log_event(
            db,
            "warning",
            "backup_scheduler",
            &format!(
                "Scheduled backup for {} does not contain {} of its {} database(s) — restoring it will not bring that content back",
                row.domain, db_expected - db_included, db_expected
            ),
            None,
        ).await;
    }

    let filename = backup_result
        .get("filename")
        .and_then(|v| v.as_str())
        .ok_or("No filename in backup result")?;
    let size_bytes = backup_result
        .get("size_bytes")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as i64;
    let filepath = format!("/var/backups/dockpanel/{}/{}", row.domain, filename);

    // 2. Upload to remote destination (if configured)
    let uploaded_remote = if let (Some(dest_dtype), Some(dest_config)) =
        (&row.dest_dtype, &row.dest_config)
    {
        let dest = crate::routes::backup_destinations::agent_destination_payload(
            dest_dtype,
            dest_config,
        );

        let upload_body = serde_json::json!({
            "filepath": filepath,
            "destination": dest,
        });

        // Retry upload with exponential backoff: 5s, 15s, 30s
        let delays = [5u64, 15, 30];
        let mut last_err = String::new();
        let mut uploaded = false;

        for (attempt, delay) in delays.iter().enumerate() {
            // `post_long`, not `post`: `post` caps every agent call at 60s, while the
            // agent budgets 600s for this exact upload (services/remote_backup.rs).
            // An off-site copy that takes longer than a minute — a ~12MB archive on a
            // slow uplink was enough to measure it — therefore had the panel give up
            // while curl/scp kept running and the bytes DID land. The panel then
            // retried the whole file twice more, recorded the backup as local-only,
            // and alerted that it had failed. 660s so the AGENT's own timeout always
            // fires first and the operator gets its real error, not "timed out".
            match agent
                .post_long("/backups/upload", Some(upload_body.clone()), 660)
                .await
            {
                Ok(_) => {
                    uploaded = true;
                    break;
                }
                Err(e) => {
                    last_err = e.to_string();
                    if attempt < delays.len() - 1 {
                        tracing::warn!(
                            "Backup upload attempt {} failed for {}: {last_err} — retrying in {delay}s",
                            attempt + 1,
                            row.domain
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(*delay)).await;
                    }
                }
            }
        }

        if !uploaded {
            // All retries exhausted — don't record in DB since the upload failed.
            // The local file still exists on disk for manual recovery.
            crate::services::system_log::log_event(
                db,
                "error",
                "backup_scheduler",
                &format!("Backup upload failed for {} after 3 attempts", row.domain),
                Some(&last_err),
            ).await;

            return Err(format!("Upload failed after 3 attempts: {last_err}"));
        }

        // Prune old remote backups.
        //
        // The agent answers 200 with an explanatory `message` when it cannot
        // prune — SFTP has no supported prune path, so it returns pruned:0 and
        // says so. Discarding the body meant the schedule kept rendering its
        // retention count as an enforced setting while remote copies accumulated
        // forever, and the only trace was a warn in a journal that gets vacuumed.
        let prune_body = serde_json::json!({
            "destination": dest,
            "domain": row.domain,
            "retention": row.retention_count,
        });
        match agent.post("/backups/prune", Some(prune_body)).await {
            Ok(resp) => {
                if let Some(msg) = resp.get("message").and_then(|v| v.as_str()) {
                    // "warning", not "warn" — this file's two other warning writers
                    // (:201, :248) already spell it the long way, and only that
                    // spelling is filterable and countable.
                    crate::services::system_log::log_event(
                        db,
                        "warning",
                        "backup_scheduler",
                        &format!("Remote retention was not enforced for {}", row.domain),
                        Some(msg),
                    ).await;
                }
            }
            Err(e) => {
                tracing::warn!("Remote prune failed for {}: {e}", row.domain);
            }
        }

        true
    } else {
        false
    };

    // 3. Record in DB only after successful creation and upload (if configured).
    // This ensures the DB only contains backups that are fully complete.
    //
    // `uploaded` and `destination_id` are bound here because migration
    // 20260726000000 added them to `backups` for precisely this, and this path did
    // not write them: it computed the upload result into a discarded `_`-prefixed
    // binding and inserted five columns. So a per-site schedule with a destination
    // attached shipped the archive off-site and then filed it as local-only — the
    // "remote" badge in All Backups could never light for a scheduled backup, and
    // nothing recorded where the bytes went. Measured on a live box (s289): the
    // SFTP copy was sitting at the destination while its row read
    // uploaded=f, destination_id=NULL. The sibling policy path had been wired to
    // these columns from the start, which is what made the gap invisible.
    //
    // The Result is checked because this row IS the backup as far as the product
    // is concerned — every list and restore path reads it. Returning Err here
    // routes into `tick`'s existing failure arm, which writes last_status='failed'
    // plus a system_log entry and a critical backup_failure alert; discarding it
    // left the schedule reading 'success' over an archive with no row.
    if let Err(e) = sqlx::query(
        "INSERT INTO backups (site_id, filename, size_bytes, databases_included, databases_expected, uploaded, destination_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(row.site_id)
    .bind(filename)
    .bind(size_bytes)
    .bind(db_included)
    .bind(db_expected)
    .bind(uploaded_remote)
    .bind(if uploaded_remote { row.dest_id } else { None })
    .execute(db)
    .await {
        return Err(format!("Backup created but could not be recorded: {e}"));
    }

    tracing::info!("Scheduled backup complete for {}", row.domain);
    Ok(())
}

/// Check if a cron expression matches the current time.
fn cron_matches_now(schedule: &str, now: &chrono::DateTime<chrono::Utc>) -> bool {
    let parts: Vec<&str> = schedule.split_whitespace().collect();
    if parts.len() != 5 {
        return false;
    }

    field_matches(parts[0], now.minute())
        && field_matches(parts[1], now.hour())
        && field_matches(parts[2], now.day())
        && field_matches(parts[3], now.month())
        && field_matches(parts[4], now.weekday().num_days_from_sunday())
}

/// Check if a single cron field matches a value.
fn field_matches(field: &str, value: u32) -> bool {
    if field == "*" {
        return true;
    }

    // Handle */N (step)
    if let Some(step) = field.strip_prefix("*/") {
        if let Ok(s) = step.parse::<u32>() {
            return s > 0 && value % s == 0;
        }
    }

    // Handle comma-separated values and ranges
    for part in field.split(',') {
        if let Some((start, end)) = part.split_once('-') {
            if let (Ok(s), Ok(e)) = (start.parse::<u32>(), end.parse::<u32>()) {
                if value >= s && value <= e {
                    return true;
                }
            }
        } else if let Ok(v) = part.parse::<u32>() {
            if v == value {
                return true;
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_field_matches() {
        assert!(field_matches("*", 0));
        assert!(field_matches("*", 59));
        assert!(field_matches("5", 5));
        assert!(!field_matches("5", 6));
        assert!(field_matches("*/5", 0));
        assert!(field_matches("*/5", 15));
        assert!(!field_matches("*/5", 13));
        assert!(field_matches("1,5,10", 5));
        assert!(!field_matches("1,5,10", 6));
        assert!(field_matches("1-5", 3));
        assert!(!field_matches("1-5", 6));
    }
}
