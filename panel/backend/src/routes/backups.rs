use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use std::time::Duration;
use uuid::Uuid;

use crate::auth::{AuthUser, Claims};
use crate::error::{internal_error, err, agent_error, paginate, ApiError};
use crate::routes::sites::ProvisionStep;
use crate::services::activity;
use crate::services::extensions::fire_event;
use crate::AppState;

#[derive(serde::Deserialize)]
pub struct BackupListQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct Backup {
    pub id: Uuid,
    pub site_id: Uuid,
    pub filename: String,
    pub size_bytes: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[serde(default)]
    pub databases_included: i32,
    #[serde(default)]
    pub databases_expected: i32,
}

/// Helper: resolve a site this caller may act on, and return its domain.
///
/// One line over [`crate::helpers::site_domain_for_caller`], which is shared with
/// the five other modules that each carried their own copy of this query. The
/// rules — including why only the admin arm is scoped by server — live there.
async fn get_site_domain(state: &AppState, site_id: Uuid, claims: &Claims) -> Result<String, ApiError> {
    crate::helpers::site_domain_for_caller(state, site_id, claims).await
}

/// The databases attached to a site, ready to hand to the agent.
pub struct SiteDatabases {
    /// Specs the agent can act on, with decrypted credentials.
    pub specs: Vec<serde_json::Value>,
    /// Databases that exist as rows but cannot safely be touched, and why.
    /// Counted as expected so a backup missing them never reads as complete.
    pub unavailable: Vec<(String, String)>,
}

impl SiteDatabases {
    pub fn expected(&self) -> usize {
        self.specs.len() + self.unavailable.len()
    }
}

/// Resolve a site's databases and decrypt their credentials for the agent.
///
/// The agent has no access to the `databases` table, so this is the only place
/// that can answer "what does backing up this site actually involve?".
pub(crate) async fn site_databases(state: &AppState, site_id: Uuid) -> SiteDatabases {
    site_database_specs(&state.db, &state.config.jwt_secret, site_id).await
}

/// The same resolution, reachable from the schedulers — which hold a pool and a
/// secret rather than an `AppState`.
///
/// EVERY path that creates a site backup must go through this. A manual backup
/// that includes the database while the SCHEDULED one silently does not would
/// be the same defect this exists to close, hiding in the automated path that
/// people rely on most.
pub async fn site_database_specs(
    db: &sqlx::PgPool,
    jwt_secret: &str,
    site_id: Uuid,
) -> SiteDatabases {
    let rows: Vec<(String, String, String, String, Option<String>)> = sqlx::query_as(
        "SELECT name, engine, db_user, db_password_enc, container_id \
         FROM databases WHERE site_id = $1 ORDER BY name",
    )
    .bind(site_id)
    .fetch_all(db)
    .await
    .unwrap_or_else(|e| {
        tracing::warn!("Could not resolve databases for site {site_id}: {e}");
        Vec::new()
    });

    let mut specs = Vec::new();
    let mut unavailable = Vec::new();

    for (name, engine, db_user, password_enc, container_id) in rows {
        // Mirror the get_db_info guard (v2.19.0 H1): a row with an EMPTY
        // container_id has no confirmed container of its own, and resolving it
        // by the non-unique name `dockpanel-db-{name}` could reach a container
        // that is not this tenant's. Never dump one — but never drop it
        // silently either; it is counted as expected and reported.
        if container_id.as_deref().unwrap_or("").is_empty() {
            unavailable.push((
                name,
                "database is not fully provisioned yet (no container), so it could not be backed up".to_string(),
            ));
            continue;
        }

        let password = crate::services::secrets_crypto::decrypt_credential_or_legacy(
            &password_enc,
            jwt_secret,
        );

        specs.push(serde_json::json!({
            "container_name": format!("dockpanel-db-{name}"),
            "db_name": name,
            "db_type": engine,
            "user": db_user,
            "password": password,
        }));
    }

    SiteDatabases { specs, unavailable }
}

/// POST /api/sites/{id}/backups — Create a backup (async with SSE).
pub async fn create(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    let backup_id = Uuid::new_v4();

    crate::helpers::register_provision_log(
        &state.provision_logs,
        &state.deploy_owners,
        backup_id,
        claims.sub,
        32,
    );

    let site_dbs = site_databases(&state, id).await;
    let db_expected = site_dbs.expected() as i32;
    let db_unavailable = site_dbs.unavailable.clone();
    let agent_body = serde_json::json!({ "databases": site_dbs.specs });

    let logs = state.provision_logs.clone();
    let db = state.db.clone();
    let user_id = claims.sub;
    let email = claims.email.clone();
    let domain_clone = domain.clone();

    tokio::spawn(async move {
        let emit = |step: &str, label: &str, status: &str, msg: Option<String>| {
            let ev = ProvisionStep {
                step: step.into(), label: label.into(), status: status.into(), message: msg,
            };
            if let Ok(mut map) = logs.lock() {
                if let Some((history, tx, _)) = map.get_mut(&backup_id) {
                    history.push(ev.clone());
                    let _ = tx.send(ev);
                }
            }
        };

        emit("backup", "Creating backup", "in_progress", None);

        let agent_path = format!("/backups/{}/create", domain_clone);
        match agent.post(&agent_path, Some(agent_body)).await {
            Ok(result) => {
                let filename = result.get("filename").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let size_bytes = result.get("size_bytes").and_then(|v| v.as_u64()).unwrap_or(0) as i64;

                let included: Vec<String> = result.get("databases_included")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                let mut failed: Vec<String> = result.get("databases_failed")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|f| {
                        let name = f.get("db_name")?.as_str()?;
                        let why = f.get("error").and_then(|v| v.as_str()).unwrap_or("unknown error");
                        Some(format!("{name}: {why}"))
                    }).collect())
                    .unwrap_or_default();
                // Databases we refused to hand to the agent are missing from the
                // archive just the same, so they belong in the same report.
                failed.extend(db_unavailable.iter().map(|(n, why)| format!("{n}: {why}")));

                // Feature 13: Backup integrity chain — compute hash and link to previous
                let sha256_hash = result.get("sha256").and_then(|v| v.as_str()).unwrap_or("").to_string();
                // `::<_, Option<String>>` is load-bearing: sha256_hash is NULLABLE, so
                // decoding it as a bare String turns the ordinary "predecessor has no
                // hash" case into a decode ERROR — which this arm then reported as
                // `DB error fetching previous backup hash`, blaming the database for a
                // column that was simply empty. Same annotation at the six sibling sites.
                //
                // ⚠ Do not drop it now that the unattended writers fill the column in
                // (#114). The reason this comment used to give — "the scheduled writers
                // leave it NULL" — is gone; the annotation is not. Every row written
                // before v2.118.0 still has a NULL hash and it is the NEWEST of those
                // this query hits first, and a failed `sha256sum` on the agent still
                // writes NULL today.
                let previous_hash: Option<String> = match sqlx::query_scalar::<_, Option<String>>(
                    "SELECT sha256_hash FROM backups WHERE site_id = $1 ORDER BY created_at DESC LIMIT 1"
                ).bind(id).fetch_optional(&db).await {
                    Ok(v) => v.flatten(),
                    Err(e) => {
                        tracing::warn!("DB error fetching previous backup hash: {e}");
                        None
                    }
                };

                let recorded = sqlx::query(
                    "INSERT INTO backups (site_id, filename, size_bytes, sha256_hash, previous_hash, chain_valid, databases_included, databases_expected) \
                     VALUES ($1, $2, $3, $4, $5, TRUE, $6, $7)",
                )
                .bind(id)
                .bind(&filename)
                .bind(size_bytes)
                .bind(if sha256_hash.is_empty() { None } else { Some(&sha256_hash) })
                .bind(previous_hash.as_deref())
                .bind(included.len() as i32)
                .bind(db_expected)
                .execute(&db)
                .await;

                // An archive with no row is unreachable through the product: the
                // list and both restore paths look it up by row. Reporting the
                // step green over that failure is the one thing this live log
                // must not do.
                //
                // Deliberately NOT an early return — the tail of this task removes
                // the log from the shared map, and returning here would leak both
                // it and its owner row until the hourly prune.
                if let Err(e) = recorded {
                    tracing::error!("Backup created for site {id} but could not be recorded: {e}");
                    emit("backup", "Backup created but could not be recorded", "error",
                        Some("The archive exists on the server but is not listed in the panel, so it cannot be restored from here.".to_string()));
                    emit("complete", "Backup not recorded", "error", Some(filename.clone()));
                } else {

                emit("backup", "Creating backup", "done", None);

                // Say what is actually in the archive. A backup missing a
                // database the site has is NOT a clean success — the whole point
                // of this path is that the user can tell before they need it.
                if db_expected == 0 {
                    emit("databases", "No databases attached to this site", "done", None);
                } else if failed.is_empty() {
                    emit("databases", &format!(
                        "Databases included: {}", included.join(", ")
                    ), "done", None);
                } else {
                    emit("databases", "Databases NOT included in this backup", "error",
                        Some(failed.join("; ")));
                }

                if failed.is_empty() {
                    emit("complete", "Backup created", "done", Some(filename.clone()));
                } else {
                    emit("complete", &format!(
                        "Backup created WITHOUT {} of {} database(s) — restoring it will not bring their content back",
                        failed.len(), db_expected
                    ), "error", Some(filename.clone()));
                }

                tracing::info!(
                    "Backup created: {filename} for {domain_clone} ({} of {db_expected} databases included)",
                    included.len()
                );
                activity::log_activity(
                    &db, user_id, &email, "backup.create",
                    Some("backup"), Some(&domain_clone), Some(&filename), None,
                ).await;

                fire_event(&db, "backup.created", serde_json::json!({
                    "site_id": id, "filename": &filename,
                    "databases_included": included.len(),
                    "databases_expected": db_expected,
                }));

                } // end: the row was recorded
            }
            Err(e) => {
                emit("backup", "Creating backup", "error", Some(format!("{e}")));
                emit("complete", "Backup failed", "error", None);
                tracing::error!("Backup creation failed for {domain_clone}: {e}");
            }
        }

        tokio::time::sleep(Duration::from_secs(60)).await;
        logs.lock().unwrap_or_else(|e| e.into_inner()).remove(&backup_id);
    });

    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({
        "backup_id": backup_id,
        "message": "Backup creation started",
    }))))
}

/// GET /api/sites/{id}/backups — List backups.
pub async fn list(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Query(params): Query<BackupListQuery>,
) -> Result<Json<Vec<Backup>>, ApiError> {
    // Verify ownership
    get_site_domain(&state, id, &claims).await?;

    let (limit, offset) = paginate(params.limit, params.offset);

    let backups: Vec<Backup> = sqlx::query_as(
        "SELECT * FROM backups WHERE site_id = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3",
    )
    .bind(id)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("list backups", e))?;

    Ok(Json(backups))
}

/// POST /api/sites/{id}/backups/{backup_id}/restore — Restore a backup (async with SSE).
pub async fn restore(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path((id, backup_id)): Path<(Uuid, Uuid)>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    let backup: Backup = sqlx::query_as(
        "SELECT * FROM backups WHERE id = $1 AND site_id = $2",
    )
    .bind(backup_id)
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("restore", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Backup not found"))?;

    let restore_id = Uuid::new_v4();

    crate::helpers::register_provision_log(
        &state.provision_logs,
        &state.deploy_owners,
        restore_id,
        claims.sub,
        32,
    );

    let site_dbs = site_databases(&state, id).await;
    let site_has_dbs = site_dbs.expected() > 0;
    let agent_body = serde_json::json!({ "databases": site_dbs.specs });
    let archive_has_dbs = backup.databases_included > 0;

    let logs = state.provision_logs.clone();
    let db = state.db.clone();
    let user_id = claims.sub;
    let email = claims.email.clone();
    let domain_clone = domain.clone();
    let filename = backup.filename.clone();

    tokio::spawn(async move {
        let emit = |step: &str, label: &str, status: &str, msg: Option<String>| {
            let ev = ProvisionStep {
                step: step.into(), label: label.into(), status: status.into(), message: msg,
            };
            if let Ok(mut map) = logs.lock() {
                if let Some((history, tx, _)) = map.get_mut(&restore_id) {
                    history.push(ev.clone());
                    let _ = tx.send(ev);
                }
            }
        };

        // Tell the operator BEFORE the destructive step what this archive can
        // and cannot give back. An archive made before v2.34.0 has no database
        // in it, and a site restored from one comes back with its old files and
        // its current content — which is exactly the surprise this ship exists
        // to remove.
        if site_has_dbs && !archive_has_dbs {
            emit("databases", "This backup contains files only", "error", Some(
                "It was made before DockPanel included databases in site backups, or its database dump failed. \
                 Your site's database will NOT be restored and its content will stay as it is now. \
                 Restore the database separately from the Databases page if that is what you need."
                    .to_string(),
            ));
        }

        emit("restore", "Restoring backup", "in_progress", None);

        let agent_path = format!("/backups/{}/restore/{}", domain_clone, filename);

        // The agent answers 500 with a structured report when it restored the
        // files but could not load a database — the body is the useful part.
        let outcome = match agent.post(&agent_path, Some(agent_body)).await {
            Ok(v) => Ok(v),
            Err(crate::services::agent::AgentError::Status(_, body)) => {
                match serde_json::from_str::<serde_json::Value>(&body) {
                    Ok(report) if report.get("files_restored").is_some() => Ok(report),
                    _ => Err(crate::services::agent::AgentError::Status(500, body)),
                }
            }
            Err(e) => Err(e),
        };

        match outcome {
            Ok(report) => {
                let files_restored = report.get("files_restored").and_then(|v| v.as_bool()).unwrap_or(true);
                let restored: Vec<String> = report.get("databases_restored")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                let failed: Vec<String> = report.get("databases_failed")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|f| {
                        let name = f.get("db_name")?.as_str()?;
                        let why = f.get("error").and_then(|v| v.as_str()).unwrap_or("unknown error");
                        Some(format!("{name}: {why}"))
                    }).collect())
                    .unwrap_or_default();

                if !failed.is_empty() {
                    // The dangerous state: files are already replaced.
                    emit("restore", "Restore incomplete", "error", Some(format!(
                        "The site's FILES have been restored, but {} database(s) could not be: {}. \
                         The site is now running restored files against its previous database content.",
                        failed.len(), failed.join("; ")
                    )));
                    emit("complete", "Restore failed — files restored, database not", "error", None);
                    tracing::error!(
                        "Restore incomplete for {domain_clone}: files_restored={files_restored}, database failures: {}",
                        failed.join("; ")
                    );
                    activity::log_activity(
                        &db, user_id, &email, "backup.restore_partial",
                        Some("backup"), Some(&domain_clone), Some(&filename), None,
                    ).await;
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    logs.lock().unwrap_or_else(|e| e.into_inner()).remove(&restore_id);
                    return;
                }

                if restored.is_empty() {
                    emit("restore", "Restoring backup", "done", None);
                } else {
                    emit("restore", &format!(
                        "Restored files and {} database(s): {}", restored.len(), restored.join(", ")
                    ), "done", None);
                }

                // Post-restore: reload services to pick up restored files
                emit("services", "Reloading services", "in_progress", None);

                // Reload nginx to pick up any config changes
                let _ = agent.post("/diagnostics/fix", Some(serde_json::json!({
                    "fix_id": "restart-service:nginx"
                }))).await;

                // Restart PHP-FPM if the site uses PHP (clear OPcache)
                let site_info: Option<(String, Option<String>)> = match sqlx::query_as(
                    "SELECT runtime, php_version FROM sites WHERE id = $1"
                ).bind(id).fetch_optional(&db).await {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!("DB error fetching site info for post-restore reload: {e}");
                        None
                    }
                };

                if let Some((runtime, php_version)) = &site_info {
                    if runtime == "php" {
                        if let Some(ver) = php_version {
                            let _ = agent.post("/diagnostics/fix", Some(serde_json::json!({
                                "fix_id": format!("restart-service:php{}-fpm", ver)
                            }))).await;
                        }
                    }
                }

                // Invalidate nginx cache for this domain
                let _ = agent.post("/diagnostics/fix", Some(serde_json::json!({
                    "fix_id": format!("clean-cache:{}", domain_clone)
                }))).await;

                emit("services", "Services reloaded", "done", None);
                tracing::info!("Post-restore service reload completed for {domain_clone}");

                emit("complete", "Backup restored", "done", None);
                tracing::info!("Backup restored: {filename} for {domain_clone}");
                activity::log_activity(
                    &db, user_id, &email, "backup.restore",
                    Some("backup"), Some(&domain_clone), Some(&filename), None,
                ).await;
            }
            Err(e) => {
                emit("restore", "Restoring backup", "error", Some(format!("{e}")));
                emit("complete", "Restore failed", "error", None);
                tracing::error!("Backup restore failed for {domain_clone}: {e}");
            }
        }

        tokio::time::sleep(Duration::from_secs(60)).await;
        logs.lock().unwrap_or_else(|e| e.into_inner()).remove(&restore_id);
    });

    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({
        "restore_id": restore_id,
        "message": "Restore started",
    }))))
}

/// DELETE /api/sites/{id}/backups/{backup_id} — Delete a backup.
pub async fn remove(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path((id, backup_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    let backup: Backup = sqlx::query_as(
        "SELECT * FROM backups WHERE id = $1 AND site_id = $2",
    )
    .bind(backup_id)
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("remove backups", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Backup not found"))?;

    // Delete from agent (must succeed before DB deletion)
    let agent_path = format!("/backups/{}/{}", domain, backup.filename);
    agent.delete(&agent_path).await
        .map_err(|e| agent_error("Backup deletion", e))?;

    // Delete from DB
    sqlx::query("DELETE FROM backups WHERE id = $1")
        .bind(backup_id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("remove backups", e))?;

    tracing::info!("Backup deleted: {} for {domain}", backup.filename);

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/sites/{id}/restic/backup — Run incremental Restic backup.
pub async fn restic_backup(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    let result = agent
        .post_long(
            &format!("/backups/{}/restic/backup", domain),
            None,
            600,
        )
        .await
        .map_err(|e| agent_error("Restic backup", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email,
        "backup.restic.create", Some("site"), Some(&domain), None, None,
    ).await;

    Ok(Json(result))
}

/// GET /api/sites/{id}/restic/snapshots — List Restic snapshots.
pub async fn restic_snapshots(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    let result = agent
        .get(&format!("/backups/{}/restic/snapshots", domain))
        .await
        .map_err(|e| agent_error("Restic snapshots", e))?;

    Ok(Json(result))
}

/// POST /api/sites/{id}/restic/restore/{snapshot_id} — Restore from Restic snapshot.
pub async fn restic_restore(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path((id, snapshot_id)): Path<(Uuid, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (domain, agent) = crate::helpers::site_agent_for_caller(&state, id, &claims).await?;

    // Validate snapshot ID
    if snapshot_id.len() < 6 || !snapshot_id.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid snapshot ID"));
    }

    let result = agent
        .post_long(
            &format!("/backups/{}/restic/restore/{}", domain, snapshot_id),
            None,
            600,
        )
        .await
        .map_err(|e| agent_error("Restic restore", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email,
        "backup.restic.restore", Some("site"), Some(&domain), Some(&snapshot_id), None,
    ).await;

    Ok(Json(result))
}
