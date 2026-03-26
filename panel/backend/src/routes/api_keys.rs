use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use sha2::{Sha256, Digest};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{internal_error, err, ApiError};
use crate::services::activity;
use crate::AppState;

#[derive(serde::Deserialize)]
pub struct CreateKeyRequest {
    pub name: String,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct ApiKeyInfo {
    pub id: Uuid,
    pub name: String,
    pub key_prefix: String,
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// GET /api/api-keys — List current user's API keys.
pub async fn list(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<Vec<ApiKeyInfo>>, ApiError> {
    let keys: Vec<ApiKeyInfo> = sqlx::query_as(
        "SELECT id, name, key_prefix, last_used_at, created_at FROM api_keys \
         WHERE user_id = $1 ORDER BY created_at DESC",
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("list api_keys", e))?;

    Ok(Json(keys))
}

/// POST /api/api-keys — Create a new API key. Returns the full key ONCE.
pub async fn create(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<CreateKeyRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let name = body.name.trim();
    if name.is_empty() || name.len() > 100 {
        return Err(err(StatusCode::BAD_REQUEST, "Name must be 1-100 characters"));
    }

    // Count existing keys (limit 10 per user)
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM api_keys WHERE user_id = $1")
        .bind(claims.sub)
        .fetch_one(&state.db)
        .await
        .map_err(|e| internal_error("create api_keys", e))?;

    if count.0 >= 10 {
        return Err(err(StatusCode::BAD_REQUEST, "Maximum 10 API keys per account"));
    }

    // Generate key: dp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
    let raw = Uuid::new_v4().to_string().replace('-', "")
        + &Uuid::new_v4().to_string().replace('-', "");
    let key = format!("dp_{raw}");
    let prefix = &key[..12]; // "dp_xxxxxxx"

    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    let key_hash = hex::encode(hasher.finalize());

    sqlx::query(
        "INSERT INTO api_keys (user_id, name, key_hash, key_prefix) VALUES ($1, $2, $3, $4)",
    )
    .bind(claims.sub)
    .bind(name)
    .bind(&key_hash)
    .bind(prefix)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("create api_keys", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "api_key.created",
        Some("api_key"), Some(name), None, None,
    ).await;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "key": key,
            "prefix": prefix,
            "name": name,
            "message": "Save this key — it won't be shown again.",
        })),
    ))
}

/// DELETE /api/api-keys/{id} — Revoke an API key.
pub async fn revoke(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Fetch key name before deleting for activity log
    let key_info: Option<(String,)> =
        sqlx::query_as("SELECT name FROM api_keys WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(claims.sub)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("revoke", e))?;

    let key_name = key_info
        .map(|(n,)| n)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "API key not found"))?;

    let result = sqlx::query("DELETE FROM api_keys WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(claims.sub)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("revoke", e))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "API key not found"));
    }

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "api_key.revoked",
        Some("api_key"), Some(&key_name), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/api-keys/{id}/rotate — Rotate an API key (atomically replace old key with new one).
pub async fn rotate(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    // 1. Verify key exists and belongs to user, get name
    let key_info: Option<(String,)> =
        sqlx::query_as("SELECT name FROM api_keys WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(claims.sub)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("rotate", e))?;

    let name = key_info
        .map(|(n,)| n)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "API key not found"))?;

    // 2. Generate new key
    let raw = Uuid::new_v4().to_string().replace('-', "")
        + &Uuid::new_v4().to_string().replace('-', "");
    let key = format!("dp_{raw}");
    let prefix = &key[..12];

    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    let key_hash = hex::encode(hasher.finalize());

    // 3. Update in-place (same ID, same name, new hash + prefix + timestamp)
    sqlx::query(
        "UPDATE api_keys SET key_hash = $1, key_prefix = $2, created_at = NOW() WHERE id = $3 AND user_id = $4",
    )
    .bind(&key_hash)
    .bind(prefix)
    .bind(id)
    .bind(claims.sub)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("rotate", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "api_key.rotated",
        Some("api_key"), Some(&name), None, None,
    ).await;

    Ok((
        StatusCode::OK,
        Json(serde_json::json!({
            "key": key,
            "prefix": prefix,
            "name": name,
            "message": "Key rotated — save the new key, the old one is now invalid.",
        })),
    ))
}
