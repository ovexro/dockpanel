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


    // ENCRYPT FIRST, THEN carry the masked values across.
    //
    // The order matters and the other one is silently destructive. This used to
    // merge first — copying the stored, ALREADY-ENCRYPTED secret into the config
    // and then handing the whole object to `encrypt_config_secrets`, which skips
    // only "" and the mask sentinel. A stored ciphertext is neither, so it was
    // encrypted a SECOND time. One decrypt on the way out then yields ciphertext,
    // and the destination authenticates with gibberish while the row looks fine.
    //
    // Nothing had ever exercised it: this endpoint had no caller in the panel at
    // all until the Destinations tab gained an Edit button (GH #93), so the very
    // first edit of a destination would have corrupted its credential.
    let encrypted_config = match body.config {
        None => None,
        Some(ref incoming) => {
            // Whatever arrived as plaintext gets encrypted exactly once. Masked
            // fields are left as the sentinel by design.
            let mut cfg = encrypt_config_secrets(incoming, &state.config.jwt_secret)?;

            let still_masked: Vec<String> = cfg
                .as_object()
                .map(|o| {
                    o.iter()
                        .filter(|(_, v)| v.as_str() == Some("********"))
                        .map(|(k, _)| k.clone())
                        .collect()
                })
                .unwrap_or_default();

            if !still_masked.is_empty() {
                let existing: Option<(serde_json::Value,)> =
                    sqlx::query_as("SELECT config FROM backup_destinations WHERE id = $1")
                        .bind(id)
                        .fetch_optional(&state.db)
                        .await
                        .map_err(|e| internal_error("update backup_destinations", e))?;
                if let Some((existing_cfg,)) = existing {
                    carry_masked_secrets(&mut cfg, &existing_cfg, &still_masked);
                }
            }
            Some(cfg)
        }
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
    agent_destination_payload(&dest.dtype, &dest.config)
}

/// The `destination` object the agent's `/backups/*` handlers expect: the stored
/// config with its secrets decrypted and `type` folded in.
///
/// Everything that hands a destination to the agent must come through here.
/// Three places built this object and only one of them — `test_connection` —
/// decrypted first. The other two, the per-site scheduler and the policy
/// executor, cloned the row's `config` straight out of the database and posted
/// the CIPHERTEXT as the S3 secret key or the SFTP password. The failure is
/// shaped to survive review: Test Connection uses the decrypting path and
/// reports success, so a destination verifies green and then every actual upload
/// authenticates with an encrypted blob. Taking `dtype` and `config` rather than
/// a `BackupDestination` is deliberate — both callers hold the two columns from a
/// joined query and would otherwise have kept hand-rolling this.
pub fn agent_destination_payload(dtype: &str, config: &serde_json::Value) -> serde_json::Value {
    let mut d = decrypt_config_secrets(config);
    if let Some(obj) = d.as_object_mut() {
        obj.insert("type".to_string(), serde_json::json!(dtype));
    } else {
        d = serde_json::json!({ "type": dtype });
    }
    d
}

/// Replace each still-masked key in `cfg` with the value already stored for it,
/// verbatim.
///
/// "Verbatim" is the whole point: the stored value is ciphertext, and putting it
/// through the encryptor again is exactly the bug this function exists to make
/// impossible to write by accident. A key the caller masked but which the stored
/// config does not have is dropped rather than left as the sentinel, so the mask
/// can never reach the agent as if it were a credential.
fn carry_masked_secrets(
    cfg: &mut serde_json::Value,
    existing: &serde_json::Value,
    masked_keys: &[String],
) {
    let (Some(obj), Some(stored)) = (cfg.as_object_mut(), existing.as_object()) else {
        return;
    };
    for k in masked_keys {
        match stored.get(k) {
            Some(v) => { obj.insert(k.clone(), v.clone()); }
            None => { obj.remove(k); }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    const JWT: &str = "test-jwt-secret-at-least-32-bytes-long-for-hkdf";

    /// The shape an edit takes when the operator changed the bucket but left the
    /// secret masked — which is what the Destinations tab sends for every edit
    /// that does not rotate the credential.
    #[test]
    fn a_masked_secret_is_carried_across_not_re_encrypted() {
        let stored = encrypt_config_secrets(
            &serde_json::json!({ "bucket": "old", "secret_key": "s3cr3t" }),
            JWT,
        )
        .unwrap();
        let stored_cipher = stored["secret_key"].as_str().unwrap().to_string();
        assert_ne!(stored_cipher, "s3cr3t", "precondition: it really was encrypted");

        // What the UI sends back: new bucket, masked secret.
        let incoming = serde_json::json!({ "bucket": "new", "secret_key": "********" });
        let mut cfg = encrypt_config_secrets(&incoming, JWT).unwrap();
        assert_eq!(cfg["secret_key"], "********", "the sentinel is not encrypted");

        carry_masked_secrets(&mut cfg, &stored, &["secret_key".to_string()]);

        assert_eq!(cfg["bucket"], "new");
        // The bug: merging BEFORE encrypting fed this ciphertext back through the
        // encryptor, so one decrypt returned ciphertext and the destination
        // authenticated with gibberish. It must come out byte-identical.
        assert_eq!(cfg["secret_key"].as_str().unwrap(), stored_cipher);
        assert_eq!(
            crate::services::secrets_crypto::decrypt_credential(&stored_cipher, JWT).unwrap(),
            "s3cr3t",
            "and one decrypt must still recover the original"
        );
    }

    #[test]
    fn a_rotated_secret_replaces_the_stored_one() {
        let stored = encrypt_config_secrets(&serde_json::json!({ "secret_key": "old" }), JWT).unwrap();
        let mut cfg =
            encrypt_config_secrets(&serde_json::json!({ "secret_key": "new" }), JWT).unwrap();
        // Nothing is masked, so nothing is carried.
        carry_masked_secrets(&mut cfg, &stored, &[]);
        let got = crate::services::secrets_crypto::decrypt_credential(
            cfg["secret_key"].as_str().unwrap(),
            JWT,
        )
        .unwrap();
        assert_eq!(got, "new");
    }

    #[test]
    fn a_mask_with_nothing_stored_is_dropped_never_passed_through() {
        // Otherwise "********" would reach the agent as the literal credential.
        let mut cfg = serde_json::json!({ "host": "h", "password": "********" });
        carry_masked_secrets(&mut cfg, &serde_json::json!({ "host": "h" }), &["password".to_string()]);
        assert!(cfg.get("password").is_none());
    }

    #[test]
    fn the_agent_payload_carries_its_type() {
        // Only the `type` fold is asserted here. The decrypt half reads JWT_SECRET
        // from the process environment, and setting that from a test would race
        // every other test in the binary — it is covered by the round-trip above
        // and by the live drive on a real box.
        let payload = agent_destination_payload("sftp", &serde_json::json!({ "host": "h" }));
        assert_eq!(payload["type"], "sftp");
        assert_eq!(payload["host"], "h");
    }
}
