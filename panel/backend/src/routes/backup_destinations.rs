use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{err, ApiError};
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
    AuthUser(claims): AuthUser,
) -> Result<Json<Vec<BackupDestination>>, ApiError> {
    if claims.role != "admin" {
        return Err(err(StatusCode::FORBIDDEN, "Admin only"));
    }

    let dests: Vec<BackupDestination> = sqlx::query_as(
        "SELECT * FROM backup_destinations ORDER BY created_at DESC",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

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
    AuthUser(claims): AuthUser,
    Json(body): Json<CreateDestinationRequest>,
) -> Result<(StatusCode, Json<BackupDestination>), ApiError> {
    if claims.role != "admin" {
        return Err(err(StatusCode::FORBIDDEN, "Admin only"));
    }

    if !["s3", "sftp"].contains(&body.dtype.as_str()) {
        return Err(err(StatusCode::BAD_REQUEST, "Type must be s3 or sftp"));
    }
    if body.name.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Name is required"));
    }

    let dest: BackupDestination = sqlx::query_as(
        "INSERT INTO backup_destinations (name, dtype, config) VALUES ($1, $2, $3) RETURNING *",
    )
    .bind(body.name.trim())
    .bind(&body.dtype)
    .bind(&body.config)
    .fetch_one(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    tracing::info!("Backup destination created: {} ({})", dest.name, dest.dtype);
    Ok((StatusCode::CREATED, Json(dest)))
}

/// PUT /api/backup-destinations/{id} — Update a destination.
pub async fn update(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateDestinationRequest>,
) -> Result<Json<BackupDestination>, ApiError> {
    if claims.role != "admin" {
        return Err(err(StatusCode::FORBIDDEN, "Admin only"));
    }

    // If config has masked secrets, merge with existing
    let mut new_config = body.config.clone();
    if let Some(ref cfg) = new_config {
        if let Some(obj) = cfg.as_object() {
            let has_masked = obj.values().any(|v| v.as_str() == Some("********"));
            if has_masked {
                // Load existing config and merge
                let existing: Option<(serde_json::Value,)> =
                    sqlx::query_as("SELECT config FROM backup_destinations WHERE id = $1")
                        .bind(id)
                        .fetch_optional(&state.db)
                        .await
                        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
                if let Some((existing_cfg,)) = existing {
                    let mut merged = existing_cfg;
                    if let Some(merged_obj) = merged.as_object_mut() {
                        for (k, v) in obj {
                            if v.as_str() != Some("********") {
                                merged_obj.insert(k.clone(), v.clone());
                            }
                        }
                    }
                    new_config = Some(merged);
                }
            }
        }
    }

    let dest: BackupDestination = sqlx::query_as(
        "UPDATE backup_destinations SET \
         name = COALESCE($1, name), \
         config = COALESCE($2, config), \
         updated_at = NOW() \
         WHERE id = $3 RETURNING *",
    )
    .bind(body.name.as_deref())
    .bind(&new_config)
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Destination not found"))?;

    Ok(Json(dest))
}

/// DELETE /api/backup-destinations/{id} — Delete a destination.
pub async fn remove(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if claims.role != "admin" {
        return Err(err(StatusCode::FORBIDDEN, "Admin only"));
    }

    let deleted = sqlx::query("DELETE FROM backup_destinations WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if deleted.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Destination not found"));
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/backup-destinations/{id}/test — Test connection.
pub async fn test_connection(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if claims.role != "admin" {
        return Err(err(StatusCode::FORBIDDEN, "Admin only"));
    }

    let dest: BackupDestination = sqlx::query_as(
        "SELECT * FROM backup_destinations WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Destination not found"))?;

    // Build agent request
    let agent_body = serde_json::json!({
        "destination": build_agent_destination(&dest),
    });

    let result = state
        .agent
        .post("/backups/test-destination", Some(agent_body))
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Test failed: {e}")))?;

    Ok(Json(result))
}

/// Build the agent destination config from a DB record.
pub fn build_agent_destination(dest: &BackupDestination) -> serde_json::Value {
    let mut d = dest.config.clone();
    if let Some(obj) = d.as_object_mut() {
        obj.insert("type".to_string(), serde_json::json!(&dest.dtype));
    } else {
        d = serde_json::json!({ "type": &dest.dtype });
    }
    d
}
