//! GAP 1: Backup Policy Executor — runs every 60s, evaluates cron schedules,
//! executes backup_policies across sites, databases, and volumes.

use chrono::{Datelike, Timelike};
use sqlx::PgPool;
use uuid::Uuid;

use crate::services::agent::AgentClient;
use crate::services::notifications;

/// Row from the policy query.
#[derive(sqlx::FromRow)]
struct PolicyRow {
    id: Uuid,
    user_id: Uuid,
    server_id: Option<Uuid>,
    name: String,
    backup_sites: bool,
    backup_databases: bool,
    backup_volumes: bool,
    schedule: String,
    #[allow(dead_code)]
    destination_id: Option<Uuid>,
    retention_count: i32,
    encrypt: bool,
    verify_after_backup: bool,
    last_run: Option<chrono::DateTime<chrono::Utc>>,
}

/// Run the backup policy executor loop — checks every 60 seconds for due policies.
pub async fn run(db: PgPool, agent: AgentClient, mut shutdown_rx: tokio::sync::broadcast::Receiver<()>) {
    tracing::info!("Backup policy executor started");

    let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));

    loop {
        tokio::select! {
            _ = interval.tick() => {
                if let Err(e) = tick(&db, &agent).await {
                    tracing::error!("Backup policy executor error: {e}");
                }
            }
            _ = shutdown_rx.recv() => {
                tracing::info!("Backup policy executor shutting down gracefully");
                break;
            }
        }
    }
}

/// Track last stale-backup check to avoid spamming (once per hour).
static LAST_STALE_CHECK: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

async fn tick(db: &PgPool, agent: &AgentClient) -> Result<(), String> {
    let now = chrono::Utc::now();

    // Fetch all enabled policies
    let policies: Vec<PolicyRow> = sqlx::query_as(
        "SELECT id, user_id, server_id, name, backup_sites, backup_databases, backup_volumes, \
         schedule, destination_id, retention_count, encrypt, verify_after_backup, last_run \
         FROM backup_policies WHERE enabled = TRUE"
    )
    .fetch_all(db).await.map_err(|e| e.to_string())?;

    for policy in &policies {
        // Check if cron matches current time
        if !cron_matches_now(&policy.schedule, &now) {
            continue;
        }

        // Prevent double-runs within 90 seconds
        if let Some(last_run) = policy.last_run {
            if (now - last_run).num_seconds() < 90 {
                continue;
            }
        }

        tracing::info!("Executing backup policy '{}' ({})", policy.name, policy.id);
        execute_policy(db, agent, policy).await;
    }

    // Record backup storage metric for growth tracking
    let total_storage: Option<(i64,)> = sqlx::query_as(
        "SELECT COALESCE(SUM(size_bytes), 0) FROM ( \
            SELECT size_bytes FROM backups UNION ALL \
            SELECT size_bytes FROM database_backups UNION ALL \
            SELECT size_bytes FROM volume_backups \
        ) t"
    ).fetch_one(db).await.ok();
    if let Some((bytes,)) = total_storage {
        let _ = sqlx::query(
            "INSERT INTO system_logs (level, source, message) VALUES ('info', 'backup_storage', $1)"
        ).bind(format!("{}", bytes)).execute(db).await;
    }

    // Proactive backup freshness alerting — once per hour
    let now_ts = now.timestamp();
    let last_check = LAST_STALE_CHECK.load(std::sync::atomic::Ordering::Relaxed);
    if now_ts - last_check >= 3600 {
        LAST_STALE_CHECK.store(now_ts, std::sync::atomic::Ordering::Relaxed);

        let stale: Vec<(String,)> = sqlx::query_as(
            "SELECT s.domain FROM sites s WHERE s.status = 'active' \
             AND NOT EXISTS (SELECT 1 FROM backups b WHERE b.site_id = s.id AND b.created_at > NOW() - INTERVAL '48 hours')"
        ).fetch_all(db).await.unwrap_or_default();

        if !stale.is_empty() {
            let domains: Vec<&str> = stale.iter().map(|s| s.0.as_str()).collect();
            notifications::notify_panel(db, None,
                &format!("{} site(s) have stale backups", stale.len()),
                &format!("These sites have no backup in 48+ hours: {}", domains.join(", ")),
                "warning", "backup", Some("/backup-orchestrator")
            ).await;
            tracing::warn!("Stale backup alert: {} sites without recent backups", stale.len());
        }
    }

    Ok(())
}

async fn execute_policy(db: &PgPool, agent: &AgentClient, policy: &PolicyRow) {
    let mut successes = 0;
    let mut failures = 0;

    // Get encryption key if encrypt is enabled
    let encryption_key: Option<String> = if policy.encrypt {
        std::env::var("JWT_SECRET").ok().map(|jwt| {
            // Use a derived key for backup encryption
            format!("backup-enc-{}", &jwt[..32.min(jwt.len())])
        })
    } else {
        None
    };

    // Backup sites
    if policy.backup_sites {
        let sites: Vec<(Uuid, String)> = sqlx::query_as(
            "SELECT id, domain FROM sites WHERE user_id = $1"
        )
        .bind(policy.user_id)
        .fetch_all(db).await.unwrap_or_default();

        for (site_id, domain) in &sites {
            let mut result = agent.post(&format!("/backups/{domain}/create"), None).await;
            // Retry once on failure
            if result.is_err() {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                result = agent.post(&format!("/backups/{domain}/create"), None).await;
            }
            match result {
                Ok(resp) => {
                    let filename = resp.get("filename").and_then(|v| v.as_str()).unwrap_or("");
                    let size_bytes = resp.get("size_bytes").and_then(|v| v.as_u64()).unwrap_or(0) as i64;

                    let _ = sqlx::query(
                        "INSERT INTO backups (site_id, filename, size_bytes) VALUES ($1, $2, $3)"
                    )
                    .bind(site_id).bind(filename).bind(size_bytes)
                    .execute(db).await;

                    successes += 1;
                }
                Err(e) => {
                    tracing::error!("Policy '{}': site backup failed for {domain} (after retry): {e}", policy.name);
                    failures += 1;
                }
            }
        }
    }

    // Backup databases
    if policy.backup_databases {
        let databases: Vec<(Uuid, String, String, String, String)> = sqlx::query_as(
            "SELECT d.id, d.name, d.engine, d.db_user, d.db_password_enc \
             FROM databases d JOIN sites s ON d.site_id = s.id WHERE s.user_id = $1"
        )
        .bind(policy.user_id)
        .fetch_all(db).await.unwrap_or_default();

        for (db_id, db_name, engine, user, password) in &databases {
            let container_name = format!("dockpanel-db-{db_name}");
            let body = serde_json::json!({
                "container_name": container_name,
                "db_name": db_name,
                "db_type": engine,
                "user": user,
                "password": password,
                "encryption_key": encryption_key,
            });

            let mut result = agent.post("/db-backups/dump", Some(body.clone())).await;
            // Retry once on failure
            if result.is_err() {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                result = agent.post("/db-backups/dump", Some(body)).await;
            }
            match result {
                Ok(resp) => {
                    let filename = resp.get("filename").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let size_bytes = resp.get("size_bytes").and_then(|v| v.as_u64()).unwrap_or(0) as i64;
                    let encrypted = encryption_key.is_some();

                    let _ = sqlx::query(
                        "INSERT INTO database_backups (database_id, server_id, filename, size_bytes, db_type, db_name, encrypted, policy_id) \
                         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"
                    )
                    .bind(db_id).bind(policy.server_id).bind(&filename).bind(size_bytes)
                    .bind(engine).bind(db_name).bind(encrypted).bind(policy.id)
                    .execute(db).await;

                    successes += 1;
                }
                Err(e) => {
                    tracing::error!("Policy '{}': DB backup failed for {db_name} (after retry): {e}", policy.name);
                    failures += 1;
                }
            }
        }
    }

    // Backup volumes (Docker app volumes)
    if policy.backup_volumes {
        // Get Docker containers with volumes
        match agent.get("/apps").await {
            Ok(apps) => {
                if let Some(apps) = apps.as_array() {
                    for app in apps {
                        let container_name = app.get("name").and_then(|v| v.as_str()).unwrap_or("");
                        let container_id = app.get("container_id").and_then(|v| v.as_str()).unwrap_or("");

                        if container_name.is_empty() { continue; }

                        // Get volumes for this container
                        if let Ok(vol_resp) = agent.get(&format!("/apps/{container_id}/volumes")).await {
                            if let Some(volumes) = vol_resp.as_array() {
                                for vol in volumes {
                                    let vol_name = vol.get("name").and_then(|v| v.as_str()).unwrap_or("");
                                    if vol_name.is_empty() { continue; }

                                    let body = serde_json::json!({
                                        "volume_name": vol_name,
                                        "container_name": container_name,
                                    });

                                    let mut result = agent.post("/volume-backups/create", Some(body.clone())).await;
                                    // Retry once on failure
                                    if result.is_err() {
                                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                                        result = agent.post("/volume-backups/create", Some(body)).await;
                                    }
                                    match result {
                                        Ok(resp) => {
                                            let filename = resp.get("filename").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                            let size_bytes = resp.get("size_bytes").and_then(|v| v.as_u64()).unwrap_or(0) as i64;

                                            let _ = sqlx::query(
                                                "INSERT INTO volume_backups (container_id, container_name, server_id, volume_name, filename, size_bytes, policy_id) \
                                                 VALUES ($1, $2, $3, $4, $5, $6, $7)"
                                            )
                                            .bind(container_id).bind(container_name).bind(policy.server_id)
                                            .bind(vol_name).bind(&filename).bind(size_bytes).bind(policy.id)
                                            .execute(db).await;

                                            successes += 1;
                                        }
                                        Err(e) => {
                                            tracing::error!("Policy '{}': volume backup failed for {container_name}/{vol_name} (after retry): {e}", policy.name);
                                            failures += 1;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                tracing::error!("Policy '{}': failed to list Docker apps for volume backup: {e}", policy.name);
                failures += 1;
            }
        }
    }

    // Update policy status
    let status = if failures == 0 { "success" } else if successes > 0 { "partial" } else { "failed" };
    let _ = sqlx::query(
        "UPDATE backup_policies SET last_run = NOW(), last_status = $2, updated_at = NOW() WHERE id = $1"
    )
    .bind(policy.id).bind(status)
    .execute(db).await;

    tracing::info!(
        "Policy '{}' completed: {} successes, {} failures (status: {status})",
        policy.name, successes, failures
    );

    // GAP 10: If backup failures, create managed incident
    if failures > 0 {
        let _ = sqlx::query(
            "INSERT INTO managed_incidents (user_id, title, status, severity, description, visible_on_status_page) \
             VALUES ($1, $2, 'investigating', 'major', $3, FALSE)"
        )
        .bind(policy.user_id)
        .bind(format!("Backup policy '{}' had failures", policy.name))
        .bind(format!("{failures} backup(s) failed, {successes} succeeded"))
        .execute(db).await;
    }

    // Fire alert on failure
    if failures > 0 {
        notifications::fire_alert(
            db, policy.user_id, policy.server_id, None,
            "backup_failure", "critical",
            &format!("Backup policy '{}' failed", policy.name),
            &format!("{failures} backup(s) failed out of {} total", successes + failures),
        ).await;
    }

    // GAP 2: If verify_after_backup, trigger verification for newly created backups
    if policy.verify_after_backup && successes > 0 {
        tracing::info!("Policy '{}': triggering post-backup verification", policy.name);
        // The backup_verifier service will pick these up on its next cycle
        // since they'll be unverified backups created in the last 7 days
    }
}

/// Simple cron matcher (5 fields: minute hour day month weekday).
fn cron_matches_now(schedule: &str, now: &chrono::DateTime<chrono::Utc>) -> bool {
    let fields: Vec<&str> = schedule.split_whitespace().collect();
    if fields.len() != 5 {
        return false;
    }

    let checks = [
        (fields[0], now.minute() as i32),
        (fields[1], now.hour() as i32),
        (fields[2], now.day() as i32),
        (fields[3], now.month() as i32),
        (fields[4], now.weekday().num_days_from_sunday() as i32),
    ];

    checks.iter().all(|(field, value)| field_matches(field, *value))
}

fn field_matches(field: &&str, value: i32) -> bool {
    let field = *field;
    if field == "*" {
        return true;
    }

    // Step: */N
    if let Some(step) = field.strip_prefix("*/") {
        if let Ok(n) = step.parse::<i32>() {
            return n > 0 && value % n == 0;
        }
    }

    // List: 1,5,10
    for part in field.split(',') {
        // Range: 1-5
        if let Some((start, end)) = part.split_once('-') {
            if let (Ok(s), Ok(e)) = (start.parse::<i32>(), end.parse::<i32>()) {
                if value >= s && value <= e {
                    return true;
                }
            }
        } else if let Ok(v) = part.parse::<i32>() {
            if v == value {
                return true;
            }
        }
    }

    false
}
