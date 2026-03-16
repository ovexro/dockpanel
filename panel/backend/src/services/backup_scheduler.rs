use chrono::Datelike;
use chrono::Timelike;
use sqlx::PgPool;
use uuid::Uuid;

use crate::services::agent::AgentClient;

/// Row from the scheduler query (join of schedules + destinations + sites).
#[derive(sqlx::FromRow)]
struct ScheduleRow {
    schedule_id: Uuid,
    site_id: Uuid,
    domain: String,
    schedule: String,
    retention_count: i32,
    #[allow(dead_code)]
    dest_id: Option<Uuid>,
    dest_dtype: Option<String>,
    dest_config: Option<serde_json::Value>,
}

/// Run the backup scheduler loop — checks every 60 seconds for due schedules.
pub async fn run(db: PgPool, agent: AgentClient, mut shutdown_rx: tokio::sync::broadcast::Receiver<()>) {
    tracing::info!("Backup scheduler started");

    let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));

    loop {
        tokio::select! {
            _ = interval.tick() => {
                if let Err(e) = tick(&db, &agent).await {
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

async fn tick(db: &PgPool, agent: &AgentClient) -> Result<(), String> {
    let now = chrono::Utc::now();

    // Fetch all enabled schedules with their destination and site info
    let rows: Vec<ScheduleRow> = sqlx::query_as(
        "SELECT \
         bs.id as schedule_id, bs.site_id, s.domain, bs.schedule, bs.retention_count, \
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
        let result = run_scheduled_backup(db, agent, row).await;

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
    agent: &AgentClient,
    row: &ScheduleRow,
) -> Result<(), String> {
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

    // 1. Create backup via agent
    let agent_path = format!("/backups/{}/create", row.domain);
    let backup_result = agent
        .post(&agent_path, None)
        .await
        .map_err(|e| format!("Backup creation failed: {e}"))?;

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
    let _uploaded_remote = if let (Some(dest_dtype), Some(dest_config)) =
        (&row.dest_dtype, &row.dest_config)
    {
        let mut dest = dest_config.clone();
        if let Some(obj) = dest.as_object_mut() {
            obj.insert("type".to_string(), serde_json::json!(dest_dtype));
        }

        let upload_body = serde_json::json!({
            "filepath": filepath,
            "destination": dest,
        });

        // Retry upload with exponential backoff: 5s, 15s, 30s
        let delays = [5u64, 15, 30];
        let mut last_err = String::new();
        let mut uploaded = false;

        for (attempt, delay) in delays.iter().enumerate() {
            match agent.post("/backups/upload", Some(upload_body.clone())).await {
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

        // Prune old remote backups
        let prune_body = serde_json::json!({
            "destination": dest,
            "domain": row.domain,
            "retention": row.retention_count,
        });
        let _ = agent.post("/backups/prune", Some(prune_body)).await;

        true
    } else {
        false
    };

    // 3. Record in DB only after successful creation and upload (if configured).
    // This ensures the DB only contains backups that are fully complete.
    let _ = sqlx::query(
        "INSERT INTO backups (site_id, filename, size_bytes) VALUES ($1, $2, $3)",
    )
    .bind(row.site_id)
    .bind(filename)
    .bind(size_bytes)
    .execute(db)
    .await;

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
