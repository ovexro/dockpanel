use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::AdminUser;
use crate::error::{internal_error, err, agent_error, ApiError};
use crate::AppState;

#[derive(serde::Serialize, serde::Deserialize, sqlx::FromRow, Clone)]
pub struct BackupDestination {
    pub id: Uuid,
    pub name: String,
    pub dtype: String,
    pub config: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Deserialize)]
pub struct CreateDestinationRequest {
    pub name: String,
    pub dtype: String,
    pub config: serde_json::Value,
}

#[derive(serde::Deserialize)]
pub struct UpdateDestinationRequest {
    pub name: Option<String>,
    pub config: Option<serde_json::Value>,
}

/// GET /api/backup-destinations — List all backup destinations (admin).
pub async fn list(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<Vec<BackupDestination>>, ApiError> {

    let dests: Vec<BackupDestination> = sqlx::query_as(
        "SELECT * FROM backup_destinations ORDER BY created_at DESC LIMIT 200",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("list backup_destinations", e))?;

    // Mask secret keys in response
    let masked: Vec<BackupDestination> = dests
        .into_iter()
        .map(|mut d| {
            if let Some(obj) = d.config.as_object_mut() {
                for key in ["secret_key", "password"] {
                    if let Some(v) = obj.get(key) {
                        if v.as_str().map(|s| !s.is_empty()).unwrap_or(false) {
                            obj.insert(key.to_string(), serde_json::json!("********"));
                        }
                    }
                }
            }
            d
        })
        .collect();

    Ok(Json(masked))
}

/// POST /api/backup-destinations — Create a new backup destination.
pub async fn create(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Json(body): Json<CreateDestinationRequest>,
) -> Result<(StatusCode, Json<BackupDestination>), ApiError> {


    if !["s3", "sftp"].contains(&body.dtype.as_str()) {
        return Err(err(StatusCode::BAD_REQUEST, "Type must be s3 or sftp"));
    }
    if body.name.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Name is required"));
    }

    // Encrypt sensitive fields in config before storing
    let encrypted_config = encrypt_config_secrets(&body.config, &state.config.jwt_secret)?;

    let dest: BackupDestination = sqlx::query_as(
        "INSERT INTO backup_destinations (name, dtype, config) VALUES ($1, $2, $3) RETURNING *",
    )
    .bind(body.name.trim())
    .bind(&body.dtype)
    .bind(&encrypted_config)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("create backup_destinations", e))?;

    tracing::info!("Backup destination created: {} ({})", dest.name, dest.dtype);
    Ok((StatusCode::CREATED, Json(dest)))
}

/// PUT /api/backup-destinations/{id} — Update a destination.
pub async fn update(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateDestinationRequest>,
) -> Result<Json<BackupDestination>, ApiError> {


    // If config has masked secrets, merge with existing (already-encrypted) values
    let mut new_config = body.config.clone();
    if let Some(ref cfg) = new_config {
        if let Some(obj) = cfg.as_object() {
            let has_masked = obj.values().any(|v| v.as_str() == Some("********"));
            if has_masked {
                // Load existing config (already encrypted) and merge
                let existing: Option<(serde_json::Value,)> =
                    sqlx::query_as("SELECT config FROM backup_destinations WHERE id = $1")
                        .bind(id)
                        .fetch_optional(&state.db)
                        .await
                        .map_err(|e| internal_error("update backup_destinations", e))?;
                if let Some((existing_cfg,)) = existing {
                    let mut merged = existing_cfg;
                    if let Some(merged_obj) = merged.as_object_mut() {
                        for (k, v) in obj {
                            if v.as_str() != Some("********") {
                                merged_obj.insert(k.clone(), v.clone());
                            }
                            // If masked ("********"), keep the existing encrypted value
                        }
                    }
                    new_config = Some(merged);
                }
            }
        }
    }

    // Encrypt sensitive fields in the new config before storing
    let encrypted_config = if let Some(cfg) = new_config {
        Some(encrypt_config_secrets(&cfg, &state.config.jwt_secret)?)
    } else {
        None
    };

    let dest: BackupDestination = sqlx::query_as(
        "UPDATE backup_destinations SET \
         name = COALESCE($1, name), \
         config = COALESCE($2, config), \
         updated_at = NOW() \
         WHERE id = $3 RETURNING *",
    )
    .bind(body.name.as_deref())
    .bind(&encrypted_config)
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("update backup_destinations", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Destination not found"))?;

    Ok(Json(dest))
}

/// DELETE /api/backup-destinations/{id} — Delete a destination.
pub async fn remove(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {


    // Check for dependent backup schedules
    let dependent_count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM backup_schedules WHERE destination_id = $1"
    )
    .bind(id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("remove backup_destinations", e))?;

    let deleted = sqlx::query("DELETE FROM backup_destinations WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("remove backup_destinations", e))?;

    if deleted.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Destination not found"));
    }

    // Nullify destination_id on dependent schedules (FK is SET NULL)
    let mut resp = serde_json::json!({ "ok": true });
    if dependent_count.0 > 0 {
        resp["warning"] = serde_json::json!(format!(
            "{} backup schedule(s) were using this destination and are now unassigned",
            dependent_count.0
        ));
    }

    Ok(Json(resp))
}

/// POST /api/backup-destinations/{id}/test — Test connection.
pub async fn test_connection(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {


    let dest: BackupDestination = sqlx::query_as(
        "SELECT * FROM backup_destinations WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("test connection", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Destination not found"))?;

    // Build agent request
    let agent_body = serde_json::json!({
        "destination": build_agent_destination(&dest),
    });

    let result = state
        .agent
        .post("/backups/test-destination", Some(agent_body))
        .await
        .map_err(|e| agent_error("Backup destination test", e))?;

    Ok(Json(result))
}

/// Build the agent destination config from a DB record.
/// Decrypts sensitive fields before sending to the agent.
pub fn build_agent_destination(dest: &BackupDestination) -> serde_json::Value {
    let mut d = decrypt_config_secrets(&dest.config);
    if let Some(obj) = d.as_object_mut() {
        obj.insert("type".to_string(), serde_json::json!(&dest.dtype));
    } else {
        d = serde_json::json!({ "type": &dest.dtype });
    }
    d
}

/// Sensitive keys within the backup destination config JSON.
const CONFIG_SENSITIVE_KEYS: &[&str] = &["secret_key", "password"];

/// Encrypt sensitive fields within a backup destination config JSON.
fn encrypt_config_secrets(config: &serde_json::Value, jwt_secret: &str) -> Result<serde_json::Value, ApiError> {
    let mut cfg = config.clone();
    if let Some(obj) = cfg.as_object_mut() {
        for key in CONFIG_SENSITIVE_KEYS {
            if let Some(v) = obj.get(*key) {
                if let Some(s) = v.as_str() {
                    if !s.is_empty() && s != "********" {
                        let encrypted = crate::services::secrets_crypto::encrypt_credential(s, jwt_secret)
                            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Encryption failed: {e}")))?;
                        obj.insert(key.to_string(), serde_json::json!(encrypted));
                    }
                }
            }
        }
    }
    Ok(cfg)
}

/// Decrypt sensitive fields within a backup destination config JSON.
/// Falls back to plaintext for legacy unencrypted values.
fn decrypt_config_secrets(config: &serde_json::Value) -> serde_json::Value {
    let mut cfg = config.clone();
    if let Some(obj) = cfg.as_object_mut() {
        for key in CONFIG_SENSITIVE_KEYS {
            if let Some(v) = obj.get(*key) {
                if let Some(s) = v.as_str() {
                    if !s.is_empty() {
                        let decrypted = crate::services::secrets_crypto::decrypt_credential_from_env(s);
                        obj.insert(key.to_string(), serde_json::json!(decrypted));
                    }
                }
            }
        }
    }
    cfg
}
