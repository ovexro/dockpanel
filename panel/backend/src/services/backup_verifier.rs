use sqlx::PgPool;
use uuid::Uuid;

use crate::services::agent::AgentClient;
use crate::services::notifications;

/// Run the backup verifier — every 6 hours, picks unverified backups and verifies them.
pub async fn run(db: PgPool, agent: AgentClient, mut shutdown_rx: tokio::sync::broadcast::Receiver<()>) {
    tracing::info!("Backup verifier started");

    // Initial delay: 5 minutes after startup
    tokio::select! {
        _ = tokio::time::sleep(std::time::Duration::from_secs(300)) => {}
        _ = shutdown_rx.recv() => {
            tracing::info!("Backup verifier shutting down (initial delay)");
            return;
        }
    }

    let mut interval = tokio::time::interval(std::time::Duration::from_secs(6 * 3600)); // 6 hours

    loop {
        tokio::select! {
            _ = interval.tick() => {
                if let Err(e) = tick(&db, &agent).await {
                    tracing::error!("Backup verifier error: {e}");
                }
            }
            _ = shutdown_rx.recv() => {
                tracing::info!("Backup verifier shutting down gracefully");
                break;
            }
        }
    }
}

async fn tick(db: &PgPool, agent: &AgentClient) -> Result<(), String> {
    // Find policies that have verify_after_backup enabled
    let policies_exist: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM backup_policies WHERE verify_after_backup = TRUE AND enabled = TRUE"
    )
    .fetch_one(db).await.map_err(|e| e.to_string())?;

    if policies_exist.0 == 0 {
        return Ok(());
    }

    // Pick up to 3 unverified site backups (created in last 7 days, not yet verified)
    let site_backups: Vec<(Uuid, String, String)> = sqlx::query_as(
        "SELECT b.id, s.domain, b.filename FROM backups b \
         JOIN sites s ON s.id = b.site_id \
         LEFT JOIN backup_verifications bv ON bv.backup_type = 'site' AND bv.backup_id = b.id \
         WHERE bv.id IS NULL AND b.created_at > NOW() - INTERVAL '7 days' \
         ORDER BY b.created_at DESC LIMIT 3"
    )
    .fetch_all(db).await.map_err(|e| e.to_string())?;

    for (backup_id, domain, filename) in &site_backups {
        verify_one(db, agent, "site", *backup_id, &domain, &filename, None, None).await;
    }

    // Pick up to 3 unverified database backups
    let db_backups: Vec<(Uuid, String, String, String)> = sqlx::query_as(
        "SELECT db.id, db.db_type, db.db_name, db.filename FROM database_backups db \
         LEFT JOIN backup_verifications bv ON bv.backup_type = 'database' AND bv.backup_id = db.id \
         WHERE bv.id IS NULL AND db.created_at > NOW() - INTERVAL '7 days' \
         ORDER BY db.created_at DESC LIMIT 3"
    )
    .fetch_all(db).await.map_err(|e| e.to_string())?;

    for (backup_id, db_type, db_name, filename) in &db_backups {
        verify_one(db, agent, "database", *backup_id, &db_name, &filename, Some(db_type), None).await;
    }

    // Pick up to 2 unverified volume backups
    let vol_backups: Vec<(Uuid, String, String)> = sqlx::query_as(
        "SELECT vb.id, vb.container_name, vb.filename FROM volume_backups vb \
         LEFT JOIN backup_verifications bv ON bv.backup_type = 'volume' AND bv.backup_id = vb.id \
         WHERE bv.id IS NULL AND vb.created_at > NOW() - INTERVAL '7 days' \
         ORDER BY vb.created_at DESC LIMIT 2"
    )
    .fetch_all(db).await.map_err(|e| e.to_string())?;

    for (backup_id, container_name, filename) in &vol_backups {
        verify_one(db, agent, "volume", *backup_id, &container_name, &filename, None, Some(container_name.as_str())).await;
    }

    let total = site_backups.len() + db_backups.len() + vol_backups.len();
    if total > 0 {
        tracing::info!("Backup verifier: verified {total} backups");
    }

    Ok(())
}

async fn verify_one(
    db: &PgPool,
    agent: &AgentClient,
    backup_type: &str,
    backup_id: Uuid,
    name: &str,
    filename: &str,
    db_type: Option<&str>,
    container_name: Option<&str>,
) {
    // Create verification record
    let verif_id: Uuid = match sqlx::query_scalar(
        "INSERT INTO backup_verifications (backup_type, backup_id, status, started_at) \
         VALUES ($1, $2, 'running', NOW()) RETURNING id"
    )
    .bind(backup_type).bind(backup_id)
    .fetch_one(db).await {
        Ok(id) => id,
        Err(e) => {
            tracing::error!("Failed to create verification record: {e}");
            return;
        }
    };

    let result = match backup_type {
        "site" => {
            let body = serde_json::json!({ "domain": name, "filename": filename });
            agent.post("/backups/verify/site", Some(body)).await
        }
        "database" => {
            let body = serde_json::json!({
                "db_type": db_type.unwrap_or("postgres"),
                "db_name": name,
                "filename": filename,
            });
            agent.post("/backups/verify/database", Some(body)).await
        }
        "volume" => {
            let body = serde_json::json!({
                "container_name": container_name.unwrap_or(name),
                "filename": filename,
            });
            agent.post("/backups/verify/volume", Some(body)).await
        }
        _ => return,
    };

    match result {
        Ok(data) => {
            let passed = data.get("passed").and_then(|v| v.as_bool()).unwrap_or(false);
            let checks_run = data.get("checks_run").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let checks_passed = data.get("checks_passed").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let duration_ms = data.get("duration_ms").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let details = data.get("details").cloned().unwrap_or(serde_json::json!([]));
            let status = if passed { "passed" } else { "failed" };

            let _ = sqlx::query(
                "UPDATE backup_verifications SET \
                 status = $2, checks_run = $3, checks_passed = $4, \
                 details = $5, duration_ms = $6, completed_at = NOW() \
                 WHERE id = $1"
            )
            .bind(verif_id).bind(status)
            .bind(checks_run).bind(checks_passed)
            .bind(details).bind(duration_ms)
            .execute(db).await;

            if !passed {
                // Fire alert for failed verification
                if let Ok(Some((user_id,))) = sqlx::query_as::<_, (Uuid,)>(
                    "SELECT id FROM users WHERE role = 'admin' LIMIT 1"
                ).fetch_optional(db).await {
                    notifications::fire_alert(
                        db, user_id, None, None,
                        "backup_verification_failed", "warning",
                        &format!("Backup verification failed: {name}"),
                        &format!("The {backup_type} backup '{filename}' for {name} failed verification ({checks_passed}/{checks_run} checks passed)."),
                    ).await;
                }
            }

            tracing::info!("Verification {status}: {backup_type} backup {filename} for {name}");
        }
        Err(e) => {
            let err_msg = e.to_string();
            let _ = sqlx::query(
                "UPDATE backup_verifications SET status = 'failed', error_message = $2, completed_at = NOW() WHERE id = $1"
            ).bind(verif_id).bind(&err_msg).execute(db).await;

            tracing::error!("Verification failed for {backup_type} {filename}: {err_msg}");
        }
    }
}
