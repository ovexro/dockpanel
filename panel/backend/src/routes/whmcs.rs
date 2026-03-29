use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;

use argon2::{Argon2, PasswordHasher};

use crate::auth::AuthUser;
use crate::error::{internal_error, err, require_admin, ApiError};
use crate::services::activity;
use crate::AppState;

// ─── WHMCS Configuration ───────────────────────────────────────

#[derive(Deserialize)]
pub struct WhmcsConfigRequest {
    pub api_url: String,
    pub api_identifier: String,
    pub api_secret: String,
    pub auto_provision: Option<bool>,
    pub auto_suspend: Option<bool>,
    pub auto_terminate: Option<bool>,
}

/// GET /api/whmcs/config — Get WHMCS integration configuration.
pub async fn get_config(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let config: Option<(String, String, bool, bool, bool, Option<String>)> = sqlx::query_as(
        "SELECT api_url, api_identifier, auto_provision, auto_suspend, auto_terminate, webhook_secret \
         FROM whmcs_config LIMIT 1"
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("whmcs config", e))?;

    match config {
        Some((url, ident, prov, susp, term, webhook)) => {
            // Mask API identifier (show only first 4 + last 4 chars)
            let masked_ident = if ident.len() > 8 {
                format!("{}...{}", &ident[..4], &ident[ident.len()-4..])
            } else {
                "*".repeat(ident.len())
            };
            Ok(Json(serde_json::json!({
                "configured": true,
                "api_url": url,
                "api_identifier": masked_ident,
                "auto_provision": prov,
                "auto_suspend": susp,
                "auto_terminate": term,
                "webhook_secret": webhook,
            })))
        }
        None => Ok(Json(serde_json::json!({ "configured": false }))),
    }
}

/// PUT /api/whmcs/config — Configure WHMCS integration.
pub async fn update_config(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<WhmcsConfigRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    if body.api_url.is_empty() || body.api_url.len() > 512 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid API URL"));
    }
    if body.api_identifier.is_empty() || body.api_identifier.len() > 255 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid API identifier"));
    }
    if body.api_secret.is_empty() || body.api_secret.len() > 1024 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid API secret"));
    }

    // Encrypt the API secret
    let encrypted_secret = crate::services::secrets_crypto::encrypt_credential(&body.api_secret, &state.config.jwt_secret)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Encryption failed: {e}")))?;

    // Generate webhook secret for incoming hooks
    let webhook_secret = uuid::Uuid::new_v4().to_string().replace('-', "");

    sqlx::query(
        "INSERT INTO whmcs_config (id, api_url, api_identifier, api_secret_encrypted, auto_provision, auto_suspend, auto_terminate, webhook_secret) \
         VALUES (gen_random_uuid(), $1, $2, $3, $4, $5, $6, $7) \
         ON CONFLICT ((true)) DO UPDATE SET \
         api_url = $1, api_identifier = $2, api_secret_encrypted = $3, \
         auto_provision = $4, auto_suspend = $5, auto_terminate = $6, updated_at = NOW()"
    )
    .bind(&body.api_url)
    .bind(&body.api_identifier)
    .bind(&encrypted_secret)
    .bind(body.auto_provision.unwrap_or(true))
    .bind(body.auto_suspend.unwrap_or(true))
    .bind(body.auto_terminate.unwrap_or(false))
    .bind(&webhook_secret)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("save whmcs config", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "whmcs.configured",
        Some("settings"), None, None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true, "webhook_secret": webhook_secret })))
}

/// DELETE /api/whmcs/config — Remove WHMCS integration.
pub async fn delete_config(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    sqlx::query("DELETE FROM whmcs_config").execute(&state.db).await.ok();

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "whmcs.removed",
        Some("settings"), None, None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

// ─── WHMCS Webhook Receiver ───────────────────────────────────

#[derive(Deserialize)]
pub struct WhmcsWebhookPayload {
    pub action: String,
    pub service_id: Option<i32>,
    pub client_email: Option<String>,
    pub domain: Option<String>,
    pub plan: Option<String>,
    pub secret: Option<String>,
}

/// POST /api/whmcs/webhook — Receive provisioning hooks from WHMCS.
pub async fn webhook(
    State(state): State<AppState>,
    Json(body): Json<WhmcsWebhookPayload>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Verify webhook secret
    let config: Option<(Option<String>, bool, bool, bool)> = sqlx::query_as(
        "SELECT webhook_secret, auto_provision, auto_suspend, auto_terminate FROM whmcs_config LIMIT 1"
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("whmcs webhook", e))?;

    let (secret, auto_provision, auto_suspend, auto_terminate) = config
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "WHMCS not configured"))?;

    match secret {
        Some(ref expected) => {
            let provided = body.secret.as_deref().unwrap_or("");
            // Constant-time comparison to prevent timing attacks
            use subtle::ConstantTimeEq;
            if provided.as_bytes().ct_eq(expected.as_bytes()).unwrap_u8() != 1 {
                return Err(err(StatusCode::UNAUTHORIZED, "Invalid webhook secret"));
            }
        }
        None => {
            // Reject webhooks when no secret is configured (prevents open-access window)
            return Err(err(StatusCode::UNAUTHORIZED, "Webhook secret not configured"));
        }
    }

    let service_id = body.service_id.unwrap_or(0);

    match body.action.as_str() {
        "provision" | "CreateAccount" if auto_provision => {
            // Create user + site for new WHMCS service
            let email = body.client_email.as_deref().unwrap_or("user@example.com");
            let domain = body.domain.as_deref().unwrap_or("pending.dockpanel.dev");
            let plan = body.plan.as_deref().unwrap_or("basic");

            // Check if user already exists
            let existing: Option<(uuid::Uuid,)> = sqlx::query_as(
                "SELECT id FROM users WHERE email = $1"
            ).bind(email).fetch_optional(&state.db).await.ok().flatten();

            let user_id = if let Some((uid,)) = existing {
                uid
            } else {
                // Create user with random password (they'll use password reset)
                let temp_pass = uuid::Uuid::new_v4().to_string();
                let salt = argon2::password_hash::SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);
                let hash = Argon2::default()
                    .hash_password(temp_pass.as_bytes(), &salt)
                    .map_err(|e| internal_error("whmcs provision", e))?
                    .to_string();

                let row: (uuid::Uuid,) = sqlx::query_as(
                    "INSERT INTO users (email, password_hash, role, email_verified, approved, plan) \
                     VALUES ($1, $2, 'user', true, true, $3) RETURNING id"
                )
                .bind(email)
                .bind(&hash)
                .bind(plan)
                .fetch_one(&state.db)
                .await
                .map_err(|e| internal_error("whmcs create user", e))?;
                row.0
            };

            // Map service
            sqlx::query(
                "INSERT INTO whmcs_service_map (whmcs_service_id, user_id, plan, status) \
                 VALUES ($1, $2, $3, 'active') ON CONFLICT (whmcs_service_id) DO UPDATE SET status = 'active'"
            )
            .bind(service_id)
            .bind(user_id)
            .bind(body.plan.as_deref().unwrap_or("basic"))
            .execute(&state.db)
            .await
            .ok();

            tracing::info!("WHMCS provisioned service {service_id} for {email}");
            Ok(Json(serde_json::json!({ "ok": true, "action": "provisioned", "user_id": user_id })))
        }

        "suspend" | "SuspendAccount" if auto_suspend => {
            // Suspend user account
            let mapping: Option<(Option<uuid::Uuid>,)> = sqlx::query_as(
                "SELECT user_id FROM whmcs_service_map WHERE whmcs_service_id = $1"
            ).bind(service_id).fetch_optional(&state.db).await.ok().flatten();

            if let Some((Some(user_id),)) = mapping {
                sqlx::query("UPDATE users SET role = 'suspended' WHERE id = $1")
                    .bind(user_id).execute(&state.db).await.ok();

                sqlx::query("UPDATE whmcs_service_map SET status = 'suspended' WHERE whmcs_service_id = $1")
                    .bind(service_id).execute(&state.db).await.ok();

                tracing::info!("WHMCS suspended service {service_id}");
            }
            Ok(Json(serde_json::json!({ "ok": true, "action": "suspended" })))
        }

        "unsuspend" | "UnsuspendAccount" => {
            let mapping: Option<(Option<uuid::Uuid>,)> = sqlx::query_as(
                "SELECT user_id FROM whmcs_service_map WHERE whmcs_service_id = $1"
            ).bind(service_id).fetch_optional(&state.db).await.ok().flatten();

            if let Some((Some(user_id),)) = mapping {
                sqlx::query("UPDATE users SET role = 'user' WHERE id = $1 AND role = 'suspended'")
                    .bind(user_id).execute(&state.db).await.ok();

                sqlx::query("UPDATE whmcs_service_map SET status = 'active' WHERE whmcs_service_id = $1")
                    .bind(service_id).execute(&state.db).await.ok();
            }
            Ok(Json(serde_json::json!({ "ok": true, "action": "unsuspended" })))
        }

        "terminate" | "TerminateAccount" if auto_terminate => {
            let mapping: Option<(Option<uuid::Uuid>,)> = sqlx::query_as(
                "SELECT user_id FROM whmcs_service_map WHERE whmcs_service_id = $1"
            ).bind(service_id).fetch_optional(&state.db).await.ok().flatten();

            if let Some((Some(user_id),)) = mapping {
                // Don't delete user — just mark terminated
                sqlx::query("UPDATE whmcs_service_map SET status = 'terminated' WHERE whmcs_service_id = $1")
                    .bind(service_id).execute(&state.db).await.ok();

                tracing::info!("WHMCS terminated service {service_id}");
            }
            Ok(Json(serde_json::json!({ "ok": true, "action": "terminated" })))
        }

        _ => Ok(Json(serde_json::json!({ "ok": true, "action": "ignored" }))),
    }
}

/// GET /api/whmcs/services — List WHMCS service mappings.
pub async fn list_services(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let services: Vec<(i32, Option<uuid::Uuid>, Option<uuid::Uuid>, String, String, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as(
            "SELECT whmcs_service_id, user_id, site_id, plan, status, created_at \
             FROM whmcs_service_map ORDER BY created_at DESC LIMIT 100"
        )
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("list whmcs services", e))?;

    let items: Vec<serde_json::Value> = services.iter().map(|(sid, uid, site_id, plan, status, created)| {
        serde_json::json!({
            "whmcs_service_id": sid,
            "user_id": uid,
            "site_id": site_id,
            "plan": plan,
            "status": status,
            "created_at": created,
        })
    }).collect();

    Ok(Json(serde_json::json!({ "services": items })))
}

// ─── App Migration Between Servers ─────────────────────────────

#[derive(Deserialize)]
pub struct MigrateRequest {
    pub container_id: String,
    pub container_name: String,
    pub target_server_id: uuid::Uuid,
}

/// POST /api/migrations/apps — Start migrating a container to another server.
pub async fn start_migration(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<MigrateRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    require_admin(&claims.role)?;

    if body.container_id.is_empty() || body.container_id.len() > 64 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }

    // Verify target server exists and user owns it
    let target: Option<(uuid::Uuid,)> = sqlx::query_as(
        "SELECT id FROM servers WHERE id = $1"
    ).bind(body.target_server_id).fetch_optional(&state.db).await
    .map_err(|e| internal_error("verify target server", e))?;

    if target.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Target server not found"));
    }

    // Get source server (local)
    let source_id: Option<(uuid::Uuid,)> = sqlx::query_as(
        "SELECT id FROM servers WHERE is_local = true LIMIT 1"
    ).fetch_optional(&state.db).await
    .map_err(|e| internal_error("get local server", e))?;

    let source_server_id = source_id
        .map(|(id,)| id)
        .unwrap_or_else(uuid::Uuid::nil);

    if source_server_id == body.target_server_id {
        return Err(err(StatusCode::BAD_REQUEST, "Source and target servers are the same"));
    }

    let mig_id: (uuid::Uuid,) = sqlx::query_as(
        "INSERT INTO app_migrations (container_id, container_name, source_server_id, target_server_id, status, started_at) \
         VALUES ($1, $2, $3, $4, 'in_progress', NOW()) RETURNING id"
    )
    .bind(&body.container_id)
    .bind(&body.container_name)
    .bind(source_server_id)
    .bind(body.target_server_id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("create migration", e))?;

    // The actual migration would be handled by a background task that:
    // 1. Exports container (docker export) on source
    // 2. Transfers image to target server
    // 3. Imports and starts on target
    // 4. Updates DNS if needed
    // For now, we create the migration record and the background task processes it.

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "migration.started",
        Some("container"), Some(&body.container_name),
        Some(&format!("to server {}", body.target_server_id)), None,
    ).await;

    Ok((StatusCode::CREATED, Json(serde_json::json!({
        "ok": true,
        "migration_id": mig_id.0,
        "status": "in_progress",
    }))))
}

/// GET /api/migrations/apps — List all app migrations.
pub async fn list_migrations(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let migrations: Vec<(uuid::Uuid, String, String, uuid::Uuid, uuid::Uuid, String, i32, Option<String>, Option<chrono::DateTime<chrono::Utc>>, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as(
            "SELECT id, container_id, container_name, source_server_id, target_server_id, \
             status, progress_pct, error_message, started_at, completed_at \
             FROM app_migrations ORDER BY created_at DESC LIMIT 50"
        )
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("list migrations", e))?;

    let items: Vec<serde_json::Value> = migrations.iter().map(|(id, cid, name, src, tgt, status, pct, err_msg, started, completed)| {
        serde_json::json!({
            "id": id,
            "container_id": cid,
            "container_name": name,
            "source_server_id": src,
            "target_server_id": tgt,
            "status": status,
            "progress_pct": pct,
            "error_message": err_msg,
            "started_at": started,
            "completed_at": completed,
        })
    }).collect();

    Ok(Json(serde_json::json!({ "migrations": items })))
}

/// GET /api/migrations/apps/{id} — Get migration status.
pub async fn migration_status(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let mig: Option<(String, String, String, i32, Option<String>)> = sqlx::query_as(
        "SELECT container_name, status, COALESCE(error_message, ''), progress_pct, \
         CASE WHEN completed_at IS NOT NULL THEN to_char(completed_at, 'YYYY-MM-DD HH24:MI:SS') END \
         FROM app_migrations WHERE id = $1"
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("migration status", e))?;

    match mig {
        Some((name, status, err_msg, pct, completed)) => {
            Ok(Json(serde_json::json!({
                "container_name": name,
                "status": status,
                "error_message": if err_msg.is_empty() { None } else { Some(err_msg) },
                "progress_pct": pct,
                "completed_at": completed,
            })))
        }
        None => Err(err(StatusCode::NOT_FOUND, "Migration not found")),
    }
}
