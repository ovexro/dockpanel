use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::{AdminUser, AuthUser, ServerScope};
use crate::error::{err, agent_error, paginate, ApiError};
use crate::services::activity;
use crate::services::extensions::fire_event;
use crate::AppState;

// ── Types ───────────────────────────────────────────────────────────────────

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct BackupPolicy {
    pub id: Uuid,
    pub user_id: Uuid,
    pub server_id: Option<Uuid>,
    pub name: String,
    pub backup_sites: bool,
    pub backup_databases: bool,
    pub backup_volumes: bool,
    pub schedule: String,
    pub destination_id: Option<Uuid>,
    pub retention_count: i32,
    pub encrypt: bool,
    pub verify_after_backup: bool,
    pub enabled: bool,
    pub last_run: Option<chrono::DateTime<chrono::Utc>>,
    pub last_status: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Deserialize)]
pub struct CreatePolicyRequest {
    pub name: String,
    pub server_id: Option<Uuid>,
    pub backup_sites: Option<bool>,
    pub backup_databases: Option<bool>,
    pub backup_volumes: Option<bool>,
    pub schedule: Option<String>,
    pub destination_id: Option<Uuid>,
    pub retention_count: Option<i32>,
    pub encrypt: Option<bool>,
    pub verify_after_backup: Option<bool>,
    pub enabled: Option<bool>,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct DatabaseBackup {
    pub id: Uuid,
    pub database_id: Uuid,
    pub server_id: Option<Uuid>,
    pub filename: String,
    pub size_bytes: i64,
    pub db_type: String,
    pub db_name: String,
    pub encrypted: bool,
    pub uploaded: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct VolumeBackup {
    pub id: Uuid,
    pub container_id: String,
    pub container_name: String,
    pub server_id: Option<Uuid>,
    pub volume_name: String,
    pub filename: String,
    pub size_bytes: i64,
    pub encrypted: bool,
    pub uploaded: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct BackupVerification {
    pub id: Uuid,
    pub backup_type: String,
    pub backup_id: Uuid,
    pub server_id: Option<Uuid>,
    pub status: String,
    pub checks_run: i32,
    pub checks_passed: i32,
    pub details: serde_json::Value,
    pub error_message: Option<String>,
    pub duration_ms: Option<i32>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Deserialize)]
pub struct PaginationQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

// ── Health Dashboard ────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct BackupHealth {
    pub total_site_backups: i64,
    pub total_db_backups: i64,
    pub total_volume_backups: i64,
    pub total_storage_bytes: i64,
    pub last_24h_success: i64,
    pub last_24h_failed: i64,
    pub policies_active: i64,
    pub policies_total: i64,
    pub verifications_passed: i64,
    pub verifications_failed: i64,
    pub oldest_unverified_days: Option<i64>,
    pub stale_backups: Vec<StaleBackup>,
}

#[derive(serde::Serialize)]
pub struct StaleBackup {
    pub resource_type: String,
    pub resource_name: String,
    pub last_backup: chrono::DateTime<chrono::Utc>,
    pub days_since: i64,
}

/// GET /api/backup-orchestrator/health — Global backup health dashboard.
pub async fn health(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<BackupHealth>, ApiError> {
    let db = &state.db;

    let (total_site,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM backups")
        .fetch_one(db).await.map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    let (total_db,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM database_backups")
        .fetch_one(db).await.map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    let (total_vol,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM volume_backups")
        .fetch_one(db).await.map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let (site_storage,): (Option<i64>,) = sqlx::query_as("SELECT SUM(size_bytes) FROM backups")
        .fetch_one(db).await.map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    let (db_storage,): (Option<i64>,) = sqlx::query_as("SELECT SUM(size_bytes) FROM database_backups")
        .fetch_one(db).await.map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    let (vol_storage,): (Option<i64>,) = sqlx::query_as("SELECT SUM(size_bytes) FROM volume_backups")
        .fetch_one(db).await.map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let total_storage = site_storage.unwrap_or(0) + db_storage.unwrap_or(0) + vol_storage.unwrap_or(0);

    // Count successful schedules in last 24h
    let (success_24h,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM backup_schedules WHERE last_status = 'success' AND last_run > NOW() - INTERVAL '24 hours'"
    ).fetch_one(db).await.map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    let (failed_24h,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM backup_schedules WHERE last_status = 'failed' AND last_run > NOW() - INTERVAL '24 hours'"
    ).fetch_one(db).await.map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let (policies_active,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM backup_policies WHERE enabled = TRUE")
        .fetch_one(db).await.map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    let (policies_total,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM backup_policies")
        .fetch_one(db).await.map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let (verif_passed,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM backup_verifications WHERE status = 'passed'")
        .fetch_one(db).await.map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    let (verif_failed,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM backup_verifications WHERE status = 'failed'")
        .fetch_one(db).await.map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    // Find stale sites (no backup in > 7 days)
    let stale_sites: Vec<(String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT s.domain, MAX(b.created_at) as last_backup \
         FROM sites s LEFT JOIN backups b ON b.site_id = s.id \
         GROUP BY s.id, s.domain \
         HAVING MAX(b.created_at) IS NULL OR MAX(b.created_at) < NOW() - INTERVAL '7 days' \
         ORDER BY MAX(b.created_at) NULLS FIRST LIMIT 10"
    ).fetch_all(db).await.unwrap_or_default();

    let now = chrono::Utc::now();
    let stale_backups: Vec<StaleBackup> = stale_sites.into_iter().map(|(domain, last)| {
        let days = (now - last).num_days();
        StaleBackup {
            resource_type: "site".into(),
            resource_name: domain,
            last_backup: last,
            days_since: days,
        }
    }).collect();

    Ok(Json(BackupHealth {
        total_site_backups: total_site,
        total_db_backups: total_db,
        total_volume_backups: total_vol,
        total_storage_bytes: total_storage,
        last_24h_success: success_24h,
        last_24h_failed: failed_24h,
        policies_active,
        policies_total,
        verifications_passed: verif_passed,
        verifications_failed: verif_failed,
        oldest_unverified_days: None,
        stale_backups,
    }))
}

// ── Policies CRUD ───────────────────────────────────────────────────────────

/// GET /api/backup-orchestrator/policies — List policies.
pub async fn list_policies(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<Vec<BackupPolicy>>, ApiError> {
    let policies: Vec<BackupPolicy> = sqlx::query_as(
        "SELECT * FROM backup_policies WHERE user_id = $1 ORDER BY created_at DESC"
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(policies))
}

/// POST /api/backup-orchestrator/policies — Create a policy.
pub async fn create_policy(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(req): Json<CreatePolicyRequest>,
) -> Result<(StatusCode, Json<BackupPolicy>), ApiError> {
    if req.name.is_empty() || req.name.len() > 100 {
        return Err(err(StatusCode::BAD_REQUEST, "Name must be 1-100 characters"));
    }

    let schedule = req.schedule.unwrap_or_else(|| "0 2 * * *".into());
    let retention = req.retention_count.unwrap_or(7).max(1).min(365);

    let policy: BackupPolicy = sqlx::query_as(
        "INSERT INTO backup_policies (user_id, server_id, name, backup_sites, backup_databases, backup_volumes, \
         schedule, destination_id, retention_count, encrypt, verify_after_backup, enabled) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) \
         RETURNING *"
    )
    .bind(claims.sub)
    .bind(req.server_id)
    .bind(&req.name)
    .bind(req.backup_sites.unwrap_or(true))
    .bind(req.backup_databases.unwrap_or(true))
    .bind(req.backup_volumes.unwrap_or(false))
    .bind(&schedule)
    .bind(req.destination_id)
    .bind(retention)
    .bind(req.encrypt.unwrap_or(false))
    .bind(req.verify_after_backup.unwrap_or(false))
    .bind(req.enabled.unwrap_or(true))
    .fetch_one(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "backup_policy.create",
        Some("backup_policy"), Some(&req.name), None, None,
    ).await;

    fire_event(&state.db, "backup_policy.created", serde_json::json!({
        "policy_id": policy.id, "name": &req.name,
    }));

    Ok((StatusCode::CREATED, Json(policy)))
}

/// PUT /api/backup-orchestrator/policies/{id} — Update a policy.
pub async fn update_policy(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<CreatePolicyRequest>,
) -> Result<Json<BackupPolicy>, ApiError> {
    // Verify ownership
    let existing: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM backup_policies WHERE id = $1 AND user_id = $2"
    )
    .bind(id).bind(claims.sub)
    .fetch_optional(&state.db).await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if existing.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Policy not found"));
    }

    let retention = req.retention_count.unwrap_or(7).max(1).min(365);

    let policy: BackupPolicy = sqlx::query_as(
        "UPDATE backup_policies SET \
         name = COALESCE(NULLIF($2, ''), name), \
         server_id = $3, \
         backup_sites = COALESCE($4, backup_sites), \
         backup_databases = COALESCE($5, backup_databases), \
         backup_volumes = COALESCE($6, backup_volumes), \
         schedule = COALESCE(NULLIF($7, ''), schedule), \
         destination_id = $8, \
         retention_count = $9, \
         encrypt = COALESCE($10, encrypt), \
         verify_after_backup = COALESCE($11, verify_after_backup), \
         enabled = COALESCE($12, enabled), \
         updated_at = NOW() \
         WHERE id = $1 RETURNING *"
    )
    .bind(id)
    .bind(&req.name)
    .bind(req.server_id)
    .bind(req.backup_sites)
    .bind(req.backup_databases)
    .bind(req.backup_volumes)
    .bind(req.schedule.as_deref().unwrap_or(""))
    .bind(req.destination_id)
    .bind(retention)
    .bind(req.encrypt)
    .bind(req.verify_after_backup)
    .bind(req.enabled)
    .fetch_one(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(policy))
}

/// DELETE /api/backup-orchestrator/policies/{id} — Delete a policy.
pub async fn delete_policy(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = sqlx::query("DELETE FROM backup_policies WHERE id = $1 AND user_id = $2")
        .bind(id).bind(claims.sub)
        .execute(&state.db).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Policy not found"));
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/backup-orchestrator/policies/protect-all — Create a backup-everything policy.
pub async fn protect_all(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let db = &state.db;
    let policy_name = "Protect Everything";

    // Check if already exists
    let existing: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM backup_policies WHERE user_id = $1 AND name = $2"
    )
    .bind(claims.sub).bind(policy_name)
    .fetch_optional(db).await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if let Some((existing_id,)) = existing {
        return Err(err(StatusCode::CONFLICT,
            &format!("Policy '{}' already exists (id: {})", policy_name, existing_id)));
    }

    let policy: BackupPolicy = sqlx::query_as(
        "INSERT INTO backup_policies (user_id, name, backup_sites, backup_databases, backup_volumes, \
         schedule, retention_count, encrypt, verify_after_backup, enabled) \
         VALUES ($1, $2, TRUE, TRUE, TRUE, '0 2 * * *', 7, FALSE, TRUE, TRUE) \
         RETURNING *"
    )
    .bind(claims.sub)
    .bind(policy_name)
    .fetch_one(db).await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    activity::log_activity(
        db, claims.sub, &claims.email, "backup_policy.protect_all",
        Some("backup_policy"), Some(policy_name), None, None,
    ).await;

    fire_event(db, "backup_policy.created", serde_json::json!({
        "policy_id": policy.id, "name": policy_name, "preset": "protect-all",
    }));

    Ok((StatusCode::CREATED, Json(serde_json::json!({
        "id": policy.id,
        "name": policy_name,
        "schedule": "0 2 * * *",
        "backup_sites": true,
        "backup_databases": true,
        "backup_volumes": true,
        "retention_count": 7,
        "verify_after_backup": true,
    }))))
}

// ── Database Backups ────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct CreateDbBackupRequest {
    pub database_id: Uuid,
}

/// POST /api/backup-orchestrator/db-backup — Create a database backup.
pub async fn create_db_backup(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(req): Json<CreateDbBackupRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    // Fetch database details (join with sites for user ownership and server_id)
    let row: Option<(Uuid, String, String, String, String, Option<Uuid>)> = sqlx::query_as(
        "SELECT d.id, d.name, d.engine, d.db_user, d.db_password_enc, s.server_id \
         FROM databases d JOIN sites s ON d.site_id = s.id \
         WHERE d.id = $1 AND s.user_id = $2"
    )
    .bind(req.database_id).bind(claims.sub)
    .fetch_optional(&state.db).await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let (db_id, db_name, engine, user, password, server_id) =
        row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Database not found"))?;

    // Container name follows convention: dockpanel-db-{name}
    let container_name = format!("dockpanel-db-{db_name}");

    // Get encryption key from destination if configured
    let encryption_key: Option<String> = sqlx::query_scalar(
        "SELECT bd.encryption_key FROM backup_destinations bd \
         WHERE bd.encryption_enabled = TRUE \
         LIMIT 1"
    ).fetch_optional(&state.db).await.unwrap_or(None);

    // Call agent to dump database
    let body = serde_json::json!({
        "container_name": container_name,
        "db_name": db_name,
        "db_type": engine,
        "user": user,
        "password": password,
        "encryption_key": encryption_key,
    });

    let result = agent.post("/db-backups/dump", Some(body)).await
        .map_err(|e| agent_error("Database backup", e))?;

    let filename = result.get("filename").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let size_bytes = result.get("size_bytes").and_then(|v| v.as_u64()).unwrap_or(0) as i64;
    let encrypted = encryption_key.is_some();

    let backup: DatabaseBackup = sqlx::query_as(
        "INSERT INTO database_backups (database_id, server_id, filename, size_bytes, db_type, db_name, encrypted) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING *"
    )
    .bind(db_id).bind(server_id).bind(&filename).bind(size_bytes)
    .bind(&engine).bind(&db_name).bind(encrypted)
    .fetch_one(&state.db).await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "db_backup.create",
        Some("database"), Some(&db_name), Some(&filename), None,
    ).await;

    fire_event(&state.db, "db_backup.created", serde_json::json!({
        "database": &db_name, "filename": &filename, "size_bytes": size_bytes, "encrypted": encrypted,
    }));

    Ok((StatusCode::CREATED, Json(serde_json::json!({
        "id": backup.id,
        "filename": filename,
        "size_bytes": size_bytes,
        "encrypted": encrypted,
    }))))
}

/// GET /api/backup-orchestrator/db-backups — List database backups.
pub async fn list_db_backups(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Query(params): Query<PaginationQuery>,
) -> Result<Json<Vec<DatabaseBackup>>, ApiError> {
    let (limit, offset) = paginate(params.limit, params.offset);

    let backups: Vec<DatabaseBackup> = sqlx::query_as(
        "SELECT db.* FROM database_backups db \
         JOIN databases d ON d.id = db.database_id JOIN sites s ON d.site_id = s.id AND s.user_id = $1 \
         ORDER BY db.created_at DESC LIMIT $2 OFFSET $3"
    )
    .bind(claims.sub).bind(limit).bind(offset)
    .fetch_all(&state.db).await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(backups))
}

/// DELETE /api/backup-orchestrator/db-backups/{id} — Delete a database backup.
pub async fn delete_db_backup(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let backup: Option<DatabaseBackup> = sqlx::query_as(
        "SELECT db.* FROM database_backups db \
         JOIN databases d ON d.id = db.database_id JOIN sites s ON d.site_id = s.id AND s.user_id = $1 \
         WHERE db.id = $2"
    )
    .bind(claims.sub).bind(id)
    .fetch_optional(&state.db).await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let backup = backup.ok_or_else(|| err(StatusCode::NOT_FOUND, "Backup not found"))?;

    // Delete from agent
    let agent_path = format!("/db-backups/{}/{}", backup.db_name, backup.filename);
    agent.delete(&agent_path).await
        .map_err(|e| agent_error("Delete backup", e))?;

    sqlx::query("DELETE FROM database_backups WHERE id = $1")
        .bind(id).execute(&state.db).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/backup-orchestrator/db-backups/{id}/restore — Restore a database from backup.
pub async fn restore_db_backup(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Look up the backup record, verify ownership
    let backup: Option<DatabaseBackup> = sqlx::query_as(
        "SELECT db.* FROM database_backups db \
         JOIN databases d ON d.id = db.database_id JOIN sites s ON d.site_id = s.id AND s.user_id = $1 \
         WHERE db.id = $2"
    )
    .bind(claims.sub).bind(id)
    .fetch_optional(&state.db).await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let backup = backup.ok_or_else(|| err(StatusCode::NOT_FOUND, "Backup not found"))?;

    // Fetch database credentials (join with sites for user/password)
    let creds: Option<(String, String, String)> = sqlx::query_as(
        "SELECT d.engine, d.db_user, d.db_password_enc FROM databases d WHERE d.id = $1"
    )
    .bind(backup.database_id)
    .fetch_optional(&state.db).await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let (_engine, user, password) =
        creds.ok_or_else(|| err(StatusCode::NOT_FOUND, "Database not found"))?;

    let container_name = format!("dockpanel-db-{}", backup.db_name);

    // Get encryption key if backup is encrypted
    let encryption_key: Option<String> = if backup.encrypted {
        let key: Option<String> = sqlx::query_scalar(
            "SELECT bd.encryption_key FROM backup_destinations bd \
             WHERE bd.encryption_enabled = TRUE \
             LIMIT 1"
        ).fetch_optional(&state.db).await.unwrap_or(None);
        Some(key.ok_or_else(|| err(StatusCode::BAD_REQUEST, "Encrypted backup but no encryption key found"))?)
    } else {
        None
    };

    // Call agent to restore database
    let body = serde_json::json!({
        "container_name": container_name,
        "db_type": backup.db_type,
        "user": user,
        "password": password,
        "encryption_key": encryption_key,
    });

    let agent_path = format!("/db-backups/{}/restore/{}", backup.db_name, backup.filename);
    let result = agent.post(&agent_path, Some(body)).await
        .map_err(|e| agent_error("Database restore", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "db_backup.restore",
        Some("database"), Some(&backup.db_name), Some(&backup.filename), None,
    ).await;

    fire_event(&state.db, "db_backup.restored", serde_json::json!({
        "database": &backup.db_name, "filename": &backup.filename, "backup_id": id.to_string(),
    }));

    Ok(Json(result))
}

// ── Volume Backups ──────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct CreateVolumeBackupRequest {
    pub container_id: String,
    pub container_name: String,
    pub volume_name: String,
}

/// POST /api/backup-orchestrator/volume-backup — Create a volume backup.
pub async fn create_volume_backup(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(req): Json<CreateVolumeBackupRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let body = serde_json::json!({
        "volume_name": req.volume_name,
        "container_name": req.container_name,
    });

    let result = agent.post("/volume-backups/create", Some(body)).await
        .map_err(|e| agent_error("Volume backup", e))?;

    let filename = result.get("filename").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let size_bytes = result.get("size_bytes").and_then(|v| v.as_u64()).unwrap_or(0) as i64;

    let server_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM servers WHERE status = 'online' LIMIT 1"
    ).fetch_optional(&state.db).await.unwrap_or(None);

    let backup: VolumeBackup = sqlx::query_as(
        "INSERT INTO volume_backups (container_id, container_name, server_id, volume_name, filename, size_bytes) \
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING *"
    )
    .bind(&req.container_id).bind(&req.container_name).bind(server_id)
    .bind(&req.volume_name).bind(&filename).bind(size_bytes)
    .fetch_one(&state.db).await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "volume_backup.create",
        Some("volume"), Some(&req.container_name), Some(&filename), None,
    ).await;

    Ok((StatusCode::CREATED, Json(serde_json::json!({
        "id": backup.id,
        "filename": filename,
        "size_bytes": size_bytes,
    }))))
}

/// GET /api/backup-orchestrator/volume-backups — List volume backups.
pub async fn list_volume_backups(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Query(params): Query<PaginationQuery>,
) -> Result<Json<Vec<VolumeBackup>>, ApiError> {
    let (limit, offset) = paginate(params.limit, params.offset);

    let backups: Vec<VolumeBackup> = sqlx::query_as(
        "SELECT * FROM volume_backups ORDER BY created_at DESC LIMIT $1 OFFSET $2"
    )
    .bind(limit).bind(offset)
    .fetch_all(&state.db).await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(backups))
}

/// POST /api/backup-orchestrator/volume-backups/{id}/restore — Restore a volume from backup.
pub async fn restore_volume_backup(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Look up the volume backup record
    let backup: Option<VolumeBackup> = sqlx::query_as(
        "SELECT * FROM volume_backups WHERE id = $1"
    )
    .bind(id)
    .fetch_optional(&state.db).await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let backup = backup.ok_or_else(|| err(StatusCode::NOT_FOUND, "Volume backup not found"))?;

    // Call agent to restore volume
    let agent_path = format!("/volume-backups/{}/restore/{}", backup.container_name, backup.filename);
    let result = agent.post(&agent_path, None).await
        .map_err(|e| agent_error("Volume restore", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "volume_backup.restore",
        Some("volume"), Some(&backup.container_name), Some(&backup.filename), None,
    ).await;

    fire_event(&state.db, "volume_backup.restored", serde_json::json!({
        "container_name": &backup.container_name, "volume_name": &backup.volume_name,
        "filename": &backup.filename, "backup_id": id.to_string(),
    }));

    Ok(Json(result))
}

// ── Verification ────────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct VerifyRequest {
    pub backup_type: String, // site, database, volume
    pub backup_id: Uuid,
}

/// POST /api/backup-orchestrator/verify — Trigger backup verification.
pub async fn trigger_verify(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(req): Json<VerifyRequest>,
) -> Result<(StatusCode, Json<BackupVerification>), ApiError> {
    // Create pending verification record
    let verification: BackupVerification = sqlx::query_as(
        "INSERT INTO backup_verifications (backup_type, backup_id, status, started_at) \
         VALUES ($1, $2, 'running', NOW()) RETURNING *"
    )
    .bind(&req.backup_type).bind(req.backup_id)
    .fetch_one(&state.db).await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let verif_id = verification.id;
    let db = state.db.clone();
    let backup_type = req.backup_type.clone();
    let backup_id = req.backup_id;

    // Run verification async
    tokio::spawn(async move {
        let result: Result<serde_json::Value, String> = match backup_type.as_str() {
            "site" => {
                let row: Option<(String, String)> = sqlx::query_as(
                    "SELECT s.domain, b.filename FROM backups b JOIN sites s ON s.id = b.site_id WHERE b.id = $1"
                ).bind(backup_id).fetch_optional(&db).await.ok().flatten();

                if let Some((domain, filename)) = row {
                    let body = serde_json::json!({ "domain": domain, "filename": filename });
                    agent.post("/backups/verify/site", Some(body)).await.map_err(|e| e.to_string())
                } else {
                    Err("Backup not found".to_string())
                }
            }
            "database" => {
                let row: Option<(String, String, String)> = sqlx::query_as(
                    "SELECT db_type, db_name, filename FROM database_backups WHERE id = $1"
                ).bind(backup_id).fetch_optional(&db).await.ok().flatten();

                if let Some((db_type, db_name, filename)) = row {
                    let body = serde_json::json!({ "db_type": db_type, "db_name": db_name, "filename": filename });
                    agent.post("/backups/verify/database", Some(body)).await.map_err(|e| e.to_string())
                } else {
                    Err("Database backup not found".to_string())
                }
            }
            "volume" => {
                let row: Option<(String, String)> = sqlx::query_as(
                    "SELECT container_name, filename FROM volume_backups WHERE id = $1"
                ).bind(backup_id).fetch_optional(&db).await.ok().flatten();

                if let Some((container_name, filename)) = row {
                    let body = serde_json::json!({ "container_name": container_name, "filename": filename });
                    agent.post("/backups/verify/volume", Some(body)).await.map_err(|e| e.to_string())
                } else {
                    Err("Volume backup not found".to_string())
                }
            }
            _ => Err("Invalid backup type".to_string()),
        };

        match result {
            Ok(data) => {
                let passed = data.get("passed").and_then(|v| v.as_bool()).unwrap_or(false);
                let checks_run = data.get("checks_run").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let checks_passed = data.get("checks_passed").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let duration_ms = data.get("duration_ms").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let details = data.get("details").cloned().unwrap_or(serde_json::json!([]));

                let _ = sqlx::query(
                    "UPDATE backup_verifications SET \
                     status = $2, checks_run = $3, checks_passed = $4, \
                     details = $5, duration_ms = $6, completed_at = NOW() \
                     WHERE id = $1"
                )
                .bind(verif_id)
                .bind(if passed { "passed" } else { "failed" })
                .bind(checks_run).bind(checks_passed)
                .bind(details).bind(duration_ms)
                .execute(&db).await;
            }
            Err(e) => {
                let _ = sqlx::query(
                    "UPDATE backup_verifications SET status = 'failed', error_message = $2, completed_at = NOW() WHERE id = $1"
                ).bind(verif_id).bind(&e).execute(&db).await;
            }
        }
    });

    Ok((StatusCode::ACCEPTED, Json(verification)))
}

/// GET /api/backup-orchestrator/storage-history — Backup storage growth over time (last 30 days).
pub async fn storage_history(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    // Query system_logs for 'backup_storage' entries, aggregate daily totals over last 30 days
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT DATE(created_at)::TEXT as day, message \
         FROM system_logs \
         WHERE source = 'backup_storage' AND created_at > NOW() - INTERVAL '30 days' \
         ORDER BY created_at ASC"
    )
    .fetch_all(&state.db).await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    // Group by day, keep the last reading per day
    let mut daily: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
    for (day, message) in &rows {
        if let Ok(bytes) = message.parse::<i64>() {
            daily.insert(day.clone(), bytes);
        }
    }

    let result: Vec<serde_json::Value> = daily.into_iter()
        .map(|(day, bytes)| serde_json::json!({
            "date": day,
            "total_bytes": bytes,
            "total_mb": (bytes as f64 / 1_048_576.0).round() as i64,
        }))
        .collect();

    Ok(Json(result))
}

/// GET /api/backup-orchestrator/verifications — List verifications.
pub async fn list_verifications(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Query(params): Query<PaginationQuery>,
) -> Result<Json<Vec<BackupVerification>>, ApiError> {
    let (limit, offset) = paginate(params.limit, params.offset);

    let verifications: Vec<BackupVerification> = sqlx::query_as(
        "SELECT * FROM backup_verifications ORDER BY created_at DESC LIMIT $1 OFFSET $2"
    )
    .bind(limit).bind(offset)
    .fetch_all(&state.db).await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(verifications))
}
