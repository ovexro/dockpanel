use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::auth::{AuthUser, ServerScope};
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
}

/// Verify site ownership, return domain.
async fn get_site_domain(state: &AppState, site_id: Uuid, user_id: Uuid) -> Result<String, ApiError> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT domain FROM sites WHERE id = $1 AND user_id = $2")
            .bind(site_id)
            .bind(user_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("unknown", e))?;

    row.map(|(d,)| d)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))
}

/// POST /api/sites/{id}/backups — Create a backup (async with SSE).
pub async fn create(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let domain = get_site_domain(&state, id, claims.sub).await?;

    let backup_id = Uuid::new_v4();

    let (tx, _) = broadcast::channel::<ProvisionStep>(32);
    {
        let mut logs = state.provision_logs.lock().unwrap_or_else(|e| e.into_inner());
        logs.insert(backup_id, (Vec::new(), tx, Instant::now()));
    }

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
        match agent.post(&agent_path, None).await {
            Ok(result) => {
                let filename = result.get("filename").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let size_bytes = result.get("size_bytes").and_then(|v| v.as_u64()).unwrap_or(0) as i64;

                // Feature 13: Backup integrity chain — compute hash and link to previous
                let sha256_hash = result.get("sha256").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let previous_hash: Option<String> = match sqlx::query_scalar(
                    "SELECT sha256_hash FROM backups WHERE site_id = $1 ORDER BY created_at DESC LIMIT 1"
                ).bind(id).fetch_optional(&db).await {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!("DB error fetching previous backup hash: {e}");
                        None
                    }
                };

                let _ = sqlx::query(
                    "INSERT INTO backups (site_id, filename, size_bytes, sha256_hash, previous_hash, chain_valid) VALUES ($1, $2, $3, $4, $5, TRUE)",
                )
                .bind(id)
                .bind(&filename)
                .bind(size_bytes)
                .bind(if sha256_hash.is_empty() { None } else { Some(&sha256_hash) })
                .bind(previous_hash.as_deref())
                .execute(&db)
                .await;

                emit("backup", "Creating backup", "done", None);
                emit("complete", "Backup created", "done", Some(filename.clone()));
                tracing::info!("Backup created: {filename} for {domain_clone}");
                activity::log_activity(
                    &db, user_id, &email, "backup.create",
                    Some("backup"), Some(&domain_clone), Some(&filename), None,
                ).await;

                fire_event(&db, "backup.created", serde_json::json!({
                    "site_id": id, "filename": &filename,
                }));
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
    get_site_domain(&state, id, claims.sub).await?;

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
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let domain = get_site_domain(&state, id, claims.sub).await?;

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

    let (tx, _) = broadcast::channel::<ProvisionStep>(32);
    {
        let mut logs = state.provision_logs.lock().unwrap_or_else(|e| e.into_inner());
        logs.insert(restore_id, (Vec::new(), tx, Instant::now()));
    }

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

        emit("restore", "Restoring backup", "in_progress", None);

        let agent_path = format!("/backups/{}/restore/{}", domain_clone, filename);
        match agent.post(&agent_path, None).await {
            Ok(_) => {
                emit("restore", "Restoring backup", "done", None);

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
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = get_site_domain(&state, id, claims.sub).await?;

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
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = get_site_domain(&state, id, claims.sub).await?;

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
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = get_site_domain(&state, id, claims.sub).await?;

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
    ServerScope(_server_id, agent): ServerScope,
    Path((id, snapshot_id)): Path<(Uuid, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = get_site_domain(&state, id, claims.sub).await?;

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
