use sqlx::PgPool;
use uuid::Uuid;

use crate::services::agent::AgentRegistry;
use crate::services::notifications;

/// Run the backup verifier — every 6 hours, picks unverified backups and verifies them.
///
/// Every driving query is fleet-wide, so the verifier gets the REGISTRY: an archive is
/// read on the host that wrote it. Verifying a member's backup against the panel's disk
/// finds nothing, marks a good archive `failed`, and — because the driving queries skip
/// any backup that already has a verification row — never looks at it again.
pub async fn run(db: PgPool, agents: AgentRegistry, mut shutdown_rx: tokio::sync::broadcast::Receiver<()>) {
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
                if let Err(e) = tick(&db, &agents).await {
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

async fn tick(db: &PgPool, agents: &AgentRegistry) -> Result<(), String> {
    // Find policies that have verify_after_backup enabled
    let policies_exist: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM backup_policies WHERE verify_after_backup = TRUE AND enabled = TRUE"
    )
    .fetch_one(db).await.map_err(|e| e.to_string())?;

    if policies_exist.0 == 0 {
        return Ok(());
    }

    // Pick up to 3 unverified site backups (created in last 7 days, not yet verified).
    // `s.server_id` rides the JOIN this query already makes — the same shape
    // `preview_cleanup` uses, and the reason no migration is needed for `backups`.
    // It is NOT NULL (20260319000000_multi_server.sql:76) and a site never moves
    // hosts: nothing in the codebase issues `UPDATE sites SET server_id`.
    let site_backups: Vec<(Uuid, String, String, Uuid)> = sqlx::query_as(
        "SELECT b.id, s.domain, b.filename, s.server_id FROM backups b \
         JOIN sites s ON s.id = b.site_id \
         LEFT JOIN backup_verifications bv ON bv.backup_type = 'site' AND bv.backup_id = b.id \
         WHERE bv.id IS NULL AND b.created_at > NOW() - INTERVAL '7 days' \
         ORDER BY b.created_at DESC LIMIT 3"
    )
    .fetch_all(db).await.map_err(|e| e.to_string())?;

    for (backup_id, domain, filename, server_id) in &site_backups {
        verify_one(db, agents, *server_id, "site", *backup_id, domain, filename, None, None).await;
    }

    // Database backups resolve their server through the site that owns the database,
    // not through `database_backups.server_id`: that column is nullable and a policy
    // with no server writes NULL into it. `sites.server_id` is NOT NULL, so this join
    // cannot produce an ambiguous host. Same join `create_db_backup` already makes.
    let db_backups: Vec<(Uuid, String, String, String, Uuid)> = sqlx::query_as(
        "SELECT db.id, db.db_type, db.db_name, db.filename, s.server_id FROM database_backups db \
         JOIN databases d ON d.id = db.database_id \
         JOIN sites s ON s.id = d.site_id \
         LEFT JOIN backup_verifications bv ON bv.backup_type = 'database' AND bv.backup_id = db.id \
         WHERE bv.id IS NULL AND db.created_at > NOW() - INTERVAL '7 days' \
         ORDER BY db.created_at DESC LIMIT 3"
    )
    .fetch_all(db).await.map_err(|e| e.to_string())?;

    for (backup_id, db_type, db_name, filename, server_id) in &db_backups {
        verify_one(db, agents, *server_id, "database", *backup_id, db_name, filename, Some(db_type), None).await;
    }

    // A volume backup has no site to join through, so `vb.server_id` is the only
    // handle — which is why `create_volume_backup` had to stop stamping it from
    // "some online server". Rows written before that fix may still carry a wrong or
    // NULL id; NULL ones are skipped below rather than silently verified here.
    let vol_backups: Vec<(Uuid, String, String, Option<Uuid>)> = sqlx::query_as(
        "SELECT vb.id, vb.container_name, vb.filename, vb.server_id FROM volume_backups vb \
         LEFT JOIN backup_verifications bv ON bv.backup_type = 'volume' AND bv.backup_id = vb.id \
         WHERE bv.id IS NULL AND vb.created_at > NOW() - INTERVAL '7 days' \
         ORDER BY vb.created_at DESC LIMIT 2"
    )
    .fetch_all(db).await.map_err(|e| e.to_string())?;

    for (backup_id, container_name, filename, server_id) in &vol_backups {
        let Some(sid) = server_id else {
            // Deliberately NOT `for_server_or_local`. An unscoped volume backup is a
            // row we cannot place, and defaulting it to this host is exactly the
            // wrong-host read being removed. Leaving it unverified keeps it eligible.
            tracing::warn!(
                "Volume backup {backup_id} ({filename}) has no server_id — leaving it \
                 unverified rather than reading this host's disk for it."
            );
            continue;
        };
        verify_one(db, agents, *sid, "volume", *backup_id, container_name, filename, None, Some(container_name.as_str())).await;
    }

    // "considered", not "verified": a row whose server is unreachable is skipped
    // above, and the old wording printed the same line whether every backup passed
    // or every one errored.
    let total = site_backups.len() + db_backups.len() + vol_backups.len();
    if total > 0 {
        tracing::info!("Backup verifier: considered {total} backups");
    }

    Ok(())
}

async fn verify_one(
    db: &PgPool,
    agents: &AgentRegistry,
    server_id: Uuid,
    backup_type: &str,
    backup_id: Uuid,
    name: &str,
    filename: &str,
    db_type: Option<&str>,
    container_name: Option<&str>,
) {
    // Resolve BEFORE inserting the record. A verification row is written 'running'
    // and the driving queries skip any backup that already has one, so a row created
    // for a server we then decline to reach would burn this backup's only chance of
    // ever being verified.
    let agent = match agents.for_server(server_id).await {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(
                "Not verifying {backup_type} backup {filename} ({backup_id}): its server \
                 {server_id} is unreachable ({e}). Leaving it unverified so a later tick \
                 can retry, rather than reading this host's disk for another host's archive."
            );
            return;
        }
    };

    // `server_id` is bound: the row records which host actually read the archive.
    let verif_id: Uuid = match sqlx::query_scalar(
        "INSERT INTO backup_verifications (backup_type, backup_id, server_id, status, started_at) \
         VALUES ($1, $2, $3, 'running', NOW()) RETURNING id"
    )
    .bind(backup_type).bind(backup_id).bind(server_id)
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
