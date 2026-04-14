use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::{AuthUser, ServerScope};
use crate::error::{internal_error, err, agent_error, ApiError};
use crate::services::activity;
use crate::services::extensions::fire_event;
use crate::services::secrets_crypto;
use crate::AppState;

// ── Types ───────────────────────────────────────────────────────────────────

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct SecretVault {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub site_id: Option<Uuid>,
    pub server_id: Option<Uuid>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Serialize)]
pub struct SecretEntry {
    pub id: Uuid,
    pub vault_id: Uuid,
    pub key: String,
    pub value: String, // Decrypted on read, masked by default
    pub description: Option<String>,
    pub secret_type: String,
    pub auto_inject: bool,
    pub version: i32,
    pub updated_by: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(sqlx::FromRow)]
struct SecretRow {
    id: Uuid,
    vault_id: Uuid,
    key: String,
    encrypted_value: String,
    description: Option<String>,
    secret_type: String,
    auto_inject: bool,
    version: i32,
    updated_by: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct SecretVersion {
    pub id: Uuid,
    pub secret_id: Uuid,
    pub version: i32,
    pub changed_by: Option<String>,
    pub change_type: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Deserialize)]
pub struct CreateVaultRequest {
    pub name: String,
    pub description: Option<String>,
    pub site_id: Option<Uuid>,
}

#[derive(serde::Deserialize)]
pub struct UpdateVaultRequest {
    pub name: Option<String>,
    pub description: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct CreateSecretRequest {
    pub key: String,
    pub value: String,
    pub description: Option<String>,
    pub secret_type: Option<String>,
    pub auto_inject: Option<bool>,
}

#[derive(serde::Deserialize)]
pub struct UpdateSecretRequest {
    pub value: Option<String>,
    pub description: Option<String>,
    pub auto_inject: Option<bool>,
}

#[derive(serde::Deserialize)]
#[allow(dead_code)]
pub struct PaginationQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub reveal: Option<bool>, // If true, show actual values (default: masked)
}

const VALID_TYPES: &[&str] = &["env", "api_key", "password", "certificate", "custom"];

/// Derive a dedicated encryption key separate from the JWT secret.
/// Uses SECRETS_ENCRYPTION_KEY env var if set, otherwise derives one via SHA-256.
fn get_encryption_key(jwt_secret: &str) -> String {
    std::env::var("SECRETS_ENCRYPTION_KEY").unwrap_or_else(|_| {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(b"dockpanel-secrets-encryption:");
        hasher.update(jwt_secret.as_bytes());
        hex::encode(hasher.finalize())
    })
}

fn mask_value(value: &str) -> String {
    if value.len() <= 4 {
        "••••••••".to_string()
    } else {
        format!("{}••••••••", &value[..4])
    }
}

// ── Vault CRUD ──────────────────────────────────────────────────────────────

/// GET /api/secrets/vaults — List vaults.
pub async fn list_vaults(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<Vec<SecretVault>>, ApiError> {
    let vaults: Vec<SecretVault> = sqlx::query_as(
        "SELECT * FROM secret_vaults WHERE user_id = $1 ORDER BY created_at DESC LIMIT 200"
    )
    .bind(claims.sub)
    .fetch_all(&state.db).await
    .map_err(|e| internal_error("list vaults", e))?;

    Ok(Json(vaults))
}

/// POST /api/secrets/vaults — Create a vault.
pub async fn create_vault(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(req): Json<CreateVaultRequest>,
) -> Result<(StatusCode, Json<SecretVault>), ApiError> {
    if req.name.is_empty() || req.name.len() > 100 {
        return Err(err(StatusCode::BAD_REQUEST, "Name must be 1-100 characters"));
    }

    let vault: SecretVault = sqlx::query_as(
        "INSERT INTO secret_vaults (user_id, name, description, site_id) VALUES ($1, $2, $3, $4) RETURNING *"
    )
    .bind(claims.sub)
    .bind(&req.name)
    .bind(&req.description)
    .bind(req.site_id)
    .fetch_one(&state.db).await
    .map_err(|e| internal_error("create vault", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "vault.create",
        Some("vault"), Some(&req.name), None, None,
    ).await;

    Ok((StatusCode::CREATED, Json(vault)))
}

/// DELETE /api/secrets/vaults/{id} — Delete a vault and all its secrets.
pub async fn delete_vault(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = sqlx::query("DELETE FROM secret_vaults WHERE id = $1 AND user_id = $2")
        .bind(id).bind(claims.sub)
        .execute(&state.db).await
        .map_err(|e| internal_error("delete vault", e))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Vault not found"));
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// PUT /api/secrets/vaults/{id} — Update vault name/description.
pub async fn update_vault(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateVaultRequest>,
) -> Result<Json<SecretVault>, ApiError> {
    if let Some(ref name) = req.name {
        if name.is_empty() || name.len() > 100 {
            return Err(err(StatusCode::BAD_REQUEST, "Name must be 1-100 characters"));
        }
    }

    let vault: Option<SecretVault> = sqlx::query_as(
        "UPDATE secret_vaults SET name = COALESCE($1, name), description = COALESCE($2, description), \
         updated_at = NOW() WHERE id = $3 AND user_id = $4 RETURNING *"
    )
    .bind(&req.name)
    .bind(&req.description)
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db).await
    .map_err(|e| internal_error("update vault", e))?;

    let vault = vault.ok_or_else(|| err(StatusCode::NOT_FOUND, "Vault not found"))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "vault.update",
        Some("vault"), Some(&vault.name), None, None,
    ).await;

    Ok(Json(vault))
}

// ── Secret CRUD ─────────────────────────────────────────────────────────────

/// Verify vault ownership, return vault_id.
async fn verify_vault(state: &AppState, vault_id: Uuid, user_id: Uuid) -> Result<(), ApiError> {
    let exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM secret_vaults WHERE id = $1 AND user_id = $2"
    )
    .bind(vault_id).bind(user_id)
    .fetch_optional(&state.db).await
    .map_err(|e| internal_error("delete vault", e))?;

    exists.map(|_| ()).ok_or_else(|| err(StatusCode::NOT_FOUND, "Vault not found"))
}

/// GET /api/secrets/vaults/{vault_id}/secrets — List secrets in a vault.
pub async fn list_secrets(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(vault_id): Path<Uuid>,
    Query(params): Query<PaginationQuery>,
) -> Result<Json<Vec<SecretEntry>>, ApiError> {
    verify_vault(&state, vault_id, claims.sub).await?;

    let encryption_key = get_encryption_key(&state.config.jwt_secret);
    let reveal = params.reveal.unwrap_or(false);

    let rows: Vec<SecretRow> = sqlx::query_as(
        "SELECT * FROM secrets WHERE vault_id = $1 ORDER BY key ASC"
    )
    .bind(vault_id)
    .fetch_all(&state.db).await
    .map_err(|e| internal_error("list secrets", e))?;

    let entries: Vec<SecretEntry> = rows.into_iter().map(|r| {
        let value = if reveal {
            secrets_crypto::decrypt(&r.encrypted_value, &encryption_key).unwrap_or_else(|_| "••••••••".into())
        } else {
            let decrypted = secrets_crypto::decrypt(&r.encrypted_value, &encryption_key).unwrap_or_default();
            mask_value(&decrypted)
        };

        SecretEntry {
            id: r.id,
            vault_id: r.vault_id,
            key: r.key,
            value,
            description: r.description,
            secret_type: r.secret_type,
            auto_inject: r.auto_inject,
            version: r.version,
            updated_by: r.updated_by,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }).collect();

    Ok(Json(entries))
}

/// POST /api/secrets/vaults/{vault_id}/secrets — Create a secret.
pub async fn create_secret(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(vault_id): Path<Uuid>,
    Json(req): Json<CreateSecretRequest>,
) -> Result<(StatusCode, Json<SecretEntry>), ApiError> {
    verify_vault(&state, vault_id, claims.sub).await?;

    if req.key.is_empty() || req.key.len() > 200 {
        return Err(err(StatusCode::BAD_REQUEST, "Key must be 1-200 characters"));
    }
    if req.value.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Value cannot be empty"));
    }

    let secret_type = req.secret_type.as_deref().unwrap_or("env");
    if !VALID_TYPES.contains(&secret_type) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid secret_type"));
    }

    let encryption_key = get_encryption_key(&state.config.jwt_secret);
    let encrypted = secrets_crypto::encrypt(&req.value, &encryption_key)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    let row: SecretRow = sqlx::query_as(
        "INSERT INTO secrets (vault_id, key, encrypted_value, description, secret_type, auto_inject, updated_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING *"
    )
    .bind(vault_id)
    .bind(&req.key)
    .bind(&encrypted)
    .bind(&req.description)
    .bind(secret_type)
    .bind(req.auto_inject.unwrap_or(false))
    .bind(&claims.email)
    .fetch_one(&state.db).await
    .map_err(|e| internal_error("create secret", e))?;

    // Record initial version
    let _ = sqlx::query(
        "INSERT INTO secret_versions (secret_id, version, encrypted_value, changed_by, change_type) VALUES ($1, 1, $2, $3, 'create')"
    )
    .bind(row.id).bind(&encrypted).bind(&claims.email)
    .execute(&state.db).await;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "secret.create",
        Some("secret"), Some(&req.key), None, None,
    ).await;

    Ok((StatusCode::CREATED, Json(SecretEntry {
        id: row.id,
        vault_id: row.vault_id,
        key: row.key,
        value: mask_value(&req.value),
        description: row.description,
        secret_type: row.secret_type,
        auto_inject: row.auto_inject,
        version: row.version,
        updated_by: row.updated_by,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })))
}

/// PUT /api/secrets/vaults/{vault_id}/secrets/{secret_id} — Update a secret value.
pub async fn update_secret(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path((vault_id, secret_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<UpdateSecretRequest>,
) -> Result<Json<SecretEntry>, ApiError> {
    verify_vault(&state, vault_id, claims.sub).await?;

    let encryption_key = get_encryption_key(&state.config.jwt_secret);

    // Get current secret
    let current: SecretRow = sqlx::query_as(
        "SELECT * FROM secrets WHERE id = $1 AND vault_id = $2"
    )
    .bind(secret_id).bind(vault_id)
    .fetch_optional(&state.db).await
    .map_err(|e| internal_error("update secret", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Secret not found"))?;

    let new_version = current.version + 1;

    if let Some(ref new_value) = req.value {
        // Encrypt new value
        let encrypted = secrets_crypto::encrypt(new_value, &encryption_key)
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

        // Save old version
        let _ = sqlx::query(
            "INSERT INTO secret_versions (secret_id, version, encrypted_value, changed_by, change_type) \
             VALUES ($1, $2, $3, $4, 'update')"
        )
        .bind(secret_id).bind(new_version).bind(&encrypted).bind(&claims.email)
        .execute(&state.db).await;

        // Update secret
        let _ = sqlx::query(
            "UPDATE secrets SET encrypted_value = $2, version = $3, updated_by = $4, \
             description = COALESCE($5, description), auto_inject = COALESCE($6, auto_inject), \
             updated_at = NOW() WHERE id = $1"
        )
        .bind(secret_id).bind(&encrypted).bind(new_version).bind(&claims.email)
        .bind(&req.description).bind(req.auto_inject)
        .execute(&state.db).await
        .map_err(|e| internal_error("update secret", e))?;
    } else {
        // Update metadata only
        let _ = sqlx::query(
            "UPDATE secrets SET description = COALESCE($2, description), \
             auto_inject = COALESCE($3, auto_inject), updated_at = NOW() WHERE id = $1"
        )
        .bind(secret_id).bind(&req.description).bind(req.auto_inject)
        .execute(&state.db).await
        .map_err(|e| internal_error("update secret", e))?;
    }

    // Re-fetch
    let row: SecretRow = sqlx::query_as("SELECT * FROM secrets WHERE id = $1")
        .bind(secret_id).fetch_one(&state.db).await
        .map_err(|e| internal_error("update secret", e))?;

    let decrypted = secrets_crypto::decrypt(&row.encrypted_value, &encryption_key).unwrap_or_default();

    Ok(Json(SecretEntry {
        id: row.id, vault_id: row.vault_id, key: row.key,
        value: mask_value(&decrypted),
        description: row.description, secret_type: row.secret_type,
        auto_inject: row.auto_inject, version: row.version,
        updated_by: row.updated_by, created_at: row.created_at, updated_at: row.updated_at,
    }))
}

/// DELETE /api/secrets/vaults/{vault_id}/secrets/{secret_id} — Delete a secret.
pub async fn delete_secret(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path((vault_id, secret_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    verify_vault(&state, vault_id, claims.sub).await?;

    let result = sqlx::query("DELETE FROM secrets WHERE id = $1 AND vault_id = $2")
        .bind(secret_id).bind(vault_id)
        .execute(&state.db).await
        .map_err(|e| internal_error("delete secret", e))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Secret not found"));
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Version History ─────────────────────────────────────────────────────────

/// GET /api/secrets/vaults/{vault_id}/secrets/{secret_id}/versions — Version history.
pub async fn list_versions(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path((vault_id, secret_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<SecretVersion>>, ApiError> {
    verify_vault(&state, vault_id, claims.sub).await?;

    let versions: Vec<SecretVersion> = sqlx::query_as(
        "SELECT id, secret_id, version, changed_by, change_type, created_at \
         FROM secret_versions WHERE secret_id = $1 ORDER BY version DESC"
    )
    .bind(secret_id)
    .fetch_all(&state.db).await
    .map_err(|e| internal_error("list versions", e))?;

    Ok(Json(versions))
}

// ── Inject into Site ────────────────────────────────────────────────────────

/// POST /api/secrets/vaults/{vault_id}/inject/{site_id} — Inject auto-inject secrets into site .env.
pub async fn inject_to_site(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path((vault_id, site_id)): Path<(Uuid, Uuid)>,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    verify_vault(&state, vault_id, claims.sub).await?;

    let encryption_key = get_encryption_key(&state.config.jwt_secret);

    // Get domain for site
    let domain: (String,) = sqlx::query_as("SELECT domain FROM sites WHERE id = $1 AND user_id = $2")
        .bind(site_id).bind(claims.sub)
        .fetch_optional(&state.db).await
        .map_err(|e| internal_error("inject to site", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    // Get all auto-inject secrets
    let rows: Vec<SecretRow> = sqlx::query_as(
        "SELECT * FROM secrets WHERE vault_id = $1 AND auto_inject = TRUE ORDER BY key"
    )
    .bind(vault_id)
    .fetch_all(&state.db).await
    .map_err(|e| internal_error("inject to site", e))?;

    if rows.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "No auto-inject secrets in this vault"));
    }

    // Decrypt and build env content
    let mut env_pairs = Vec::new();
    for row in &rows {
        let value = secrets_crypto::decrypt(&row.encrypted_value, &encryption_key)
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
        env_pairs.push(serde_json::json!({ "key": row.key, "value": value }));
    }

    // Write to site via agent
    let body = serde_json::json!({ "vars": env_pairs });
    agent.put(&format!("/nginx/env/{}", domain.0), body).await
        .map_err(|e| agent_error("Inject secrets", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "secrets.inject",
        Some("site"), Some(&domain.0), Some(&format!("{} secrets", rows.len())), None,
    ).await;

    fire_event(&state.db, "secrets.injected", serde_json::json!({
        "site_id": site_id, "domain": &domain.0, "count": rows.len(),
    }));

    Ok(Json(serde_json::json!({
        "ok": true,
        "injected": rows.len(),
        "domain": domain.0,
    })))
}

// ── Pull (get all secrets as env format) ────────────────────────────────────

/// GET /api/secrets/vaults/{vault_id}/pull — Get all secrets as KEY=VALUE (for CLI).
pub async fn pull(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(vault_id): Path<Uuid>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    verify_vault(&state, vault_id, claims.sub).await?;

    let encryption_key = get_encryption_key(&state.config.jwt_secret);

    let rows: Vec<SecretRow> = sqlx::query_as(
        "SELECT * FROM secrets WHERE vault_id = $1 ORDER BY key"
    )
    .bind(vault_id)
    .fetch_all(&state.db).await
    .map_err(|e| internal_error("pull", e))?;

    let entries: Vec<serde_json::Value> = rows.into_iter().map(|r| {
        let value = secrets_crypto::decrypt(&r.encrypted_value, &encryption_key).unwrap_or_default();
        serde_json::json!({ "key": r.key, "value": value, "type": r.secret_type })
    }).collect();

    Ok(Json(entries))
}

// ── GAP 17: Vault Export (encrypted backup) ─────────────────────────────────

/// GET /api/secrets/vaults/{vault_id}/export — Export vault as encrypted JSON (for backup/transfer).
pub async fn export_vault(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(vault_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    verify_vault(&state, vault_id, claims.sub).await?;

    let vault: SecretVault = sqlx::query_as("SELECT * FROM secret_vaults WHERE id = $1")
        .bind(vault_id).fetch_one(&state.db).await
        .map_err(|e| internal_error("export vault", e))?;

    let rows: Vec<SecretRow> = sqlx::query_as("SELECT * FROM secrets WHERE vault_id = $1 ORDER BY key")
        .bind(vault_id).fetch_all(&state.db).await
        .map_err(|e| internal_error("export vault", e))?;

    // Export with encrypted values (portable — can be imported on another DockPanel with same key)
    let secrets_export: Vec<serde_json::Value> = rows.into_iter().map(|r| {
        serde_json::json!({
            "key": r.key,
            "encrypted_value": r.encrypted_value,
            "description": r.description,
            "secret_type": r.secret_type,
            "auto_inject": r.auto_inject,
            "version": r.version,
        })
    }).collect();

    Ok(Json(serde_json::json!({
        "vault_name": vault.name,
        "vault_description": vault.description,
        "exported_at": chrono::Utc::now(),
        "secret_count": secrets_export.len(),
        "secrets": secrets_export,
    })))
}

/// POST /api/secrets/vaults/{vault_id}/import — Import secrets from exported JSON.
pub async fn import_vault(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(vault_id): Path<Uuid>,
    Json(data): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    verify_vault(&state, vault_id, claims.sub).await?;

    let secrets_arr = data.get("secrets").and_then(|v| v.as_array())
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing 'secrets' array"))?;

    let mut imported = 0;
    for secret in secrets_arr {
        let key = secret.get("key").and_then(|v| v.as_str()).unwrap_or("");
        let encrypted_value = secret.get("encrypted_value").and_then(|v| v.as_str()).unwrap_or("");
        let description = secret.get("description").and_then(|v| v.as_str());
        let secret_type = secret.get("secret_type").and_then(|v| v.as_str()).unwrap_or("env");
        let auto_inject = secret.get("auto_inject").and_then(|v| v.as_bool()).unwrap_or(false);

        if key.is_empty() || encrypted_value.is_empty() { continue; }

        let result = sqlx::query(
            "INSERT INTO secrets (vault_id, key, encrypted_value, description, secret_type, auto_inject, updated_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT (vault_id, key) DO UPDATE SET \
             encrypted_value = EXCLUDED.encrypted_value, updated_at = NOW()"
        )
        .bind(vault_id).bind(key).bind(encrypted_value)
        .bind(description).bind(secret_type).bind(auto_inject).bind(&claims.email)
        .execute(&state.db).await;

        if result.is_ok() { imported += 1; }
    }

    Ok(Json(serde_json::json!({ "ok": true, "imported": imported })))
}
