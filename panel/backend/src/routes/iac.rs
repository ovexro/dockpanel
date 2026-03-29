use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use sha2::{Sha256, Digest};

use crate::auth::AuthUser;
use crate::error::{internal_error, err, require_admin, ApiError};
use crate::services::activity;
use crate::AppState;

// ─── Terraform / Pulumi IaC Provider API ───────────────────────
//
// These endpoints provide a stable API surface for Terraform/Pulumi providers.
// Authentication via IaC tokens (Bearer token) or regular JWT.
// Resources: sites, databases, dns_records

/// POST /api/iac/tokens — Create a new IaC token.
pub async fn create_token(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<serde_json::Value>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    require_admin(&claims.role)?;

    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("default");
    if name.is_empty() || name.len() > 255 {
        return Err(err(StatusCode::BAD_REQUEST, "Name must be 1-255 characters"));
    }

    let scopes = body.get("scopes").and_then(|v| v.as_str()).unwrap_or("sites,databases,dns");

    // Generate a secure random token
    let raw_token = format!("dpiac_{}", uuid::Uuid::new_v4().to_string().replace('-', ""));
    let mut hasher = Sha256::new();
    hasher.update(raw_token.as_bytes());
    let token_hash = hex::encode(hasher.finalize());

    // Limit: max 10 tokens per user
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM iac_tokens WHERE user_id = $1")
        .bind(claims.sub).fetch_one(&state.db).await
        .map_err(|e| internal_error("iac token count", e))?;
    if count.0 >= 10 {
        return Err(err(StatusCode::BAD_REQUEST, "Maximum 10 IaC tokens per user"));
    }

    let id: (uuid::Uuid,) = sqlx::query_as(
        "INSERT INTO iac_tokens (user_id, name, token_hash, scopes) VALUES ($1, $2, $3, $4) RETURNING id"
    )
    .bind(claims.sub)
    .bind(name)
    .bind(&token_hash)
    .bind(scopes)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("create iac token", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "iac.token_created",
        Some("iac_token"), Some(name), None, None,
    ).await;

    // Return the raw token ONCE — it cannot be retrieved after this
    Ok((StatusCode::CREATED, Json(serde_json::json!({
        "id": id.0,
        "name": name,
        "token": raw_token,
        "scopes": scopes,
    }))))
}

/// GET /api/iac/tokens — List IaC tokens (without revealing the token value).
pub async fn list_tokens(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let tokens: Vec<(uuid::Uuid, String, String, Option<chrono::DateTime<chrono::Utc>>, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as(
            "SELECT id, name, scopes, last_used_at, created_at FROM iac_tokens WHERE user_id = $1 ORDER BY created_at"
        )
        .bind(claims.sub)
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("list iac tokens", e))?;

    let items: Vec<serde_json::Value> = tokens.iter().map(|(id, name, scopes, last_used, created)| {
        serde_json::json!({
            "id": id, "name": name, "scopes": scopes,
            "last_used_at": last_used, "created_at": created,
        })
    }).collect();

    Ok(Json(serde_json::json!({ "tokens": items })))
}

/// DELETE /api/iac/tokens/{id} — Revoke an IaC token.
pub async fn delete_token(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let result = sqlx::query("DELETE FROM iac_tokens WHERE id = $1 AND user_id = $2")
        .bind(id).bind(claims.sub)
        .execute(&state.db).await
        .map_err(|e| internal_error("delete iac token", e))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Token not found"));
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /api/iac/resources/sites — Terraform-compatible site listing.
pub async fn tf_list_sites(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let sites: Vec<(uuid::Uuid, String, String, String, bool, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as(
            "SELECT id, domain, runtime, status, ssl_enabled, created_at \
             FROM sites WHERE user_id = $1 ORDER BY domain"
        )
        .bind(claims.sub)
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("tf list sites", e))?;

    let items: Vec<serde_json::Value> = sites.iter().map(|(id, domain, runtime, status, ssl, created)| {
        serde_json::json!({
            "id": id, "domain": domain, "runtime": runtime,
            "status": status, "ssl_enabled": ssl, "created_at": created,
        })
    }).collect();

    Ok(Json(serde_json::json!({ "resources": items })))
}

/// GET /api/iac/resources/databases — Terraform-compatible database listing.
pub async fn tf_list_databases(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let dbs: Vec<(uuid::Uuid, String, String, Option<i32>, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as(
            "SELECT d.id, d.name, d.engine, d.port, d.created_at \
             FROM databases d \
             JOIN sites s ON d.site_id = s.id \
             WHERE s.user_id = $1 ORDER BY d.name"
        )
        .bind(claims.sub)
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("tf list databases", e))?;

    let items: Vec<serde_json::Value> = dbs.iter().map(|(id, name, engine, port, created)| {
        serde_json::json!({
            "id": id, "name": name, "engine": engine,
            "port": port, "created_at": created,
        })
    }).collect();

    Ok(Json(serde_json::json!({ "resources": items })))
}

// ─── Horizontal Auto-Scaling ───────────────────────────────────

#[derive(serde::Deserialize)]
pub struct AutoscaleRuleRequest {
    pub container_id: Option<String>,
    pub container_name: Option<String>,
    pub enabled: Option<bool>,
    pub min_replicas: Option<i32>,
    pub max_replicas: Option<i32>,
    pub cpu_threshold_up: Option<i32>,
    pub cpu_threshold_down: Option<i32>,
    pub cooldown_seconds: Option<i32>,
}

/// GET /api/autoscale — List all auto-scaling rules.
pub async fn list_autoscale(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let rules: Vec<(uuid::Uuid, String, String, bool, i32, i32, i32, i32, i32, i32, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as(
            "SELECT id, container_id, container_name, enabled, min_replicas, max_replicas, \
             cpu_threshold_up, cpu_threshold_down, cooldown_seconds, current_replicas, last_scale_at \
             FROM autoscale_rules ORDER BY container_name"
        )
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("list autoscale rules", e))?;

    let items: Vec<serde_json::Value> = rules.iter().map(|(id, cid, name, enabled, min_r, max_r, up, down, cool, current, last_scale)| {
        serde_json::json!({
            "id": id, "container_id": cid, "container_name": name,
            "enabled": enabled, "min_replicas": min_r, "max_replicas": max_r,
            "cpu_threshold_up": up, "cpu_threshold_down": down,
            "cooldown_seconds": cool, "current_replicas": current,
            "last_scale_at": last_scale,
        })
    }).collect();

    Ok(Json(serde_json::json!({ "rules": items })))
}

/// POST /api/autoscale — Create an auto-scaling rule.
pub async fn create_autoscale(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<AutoscaleRuleRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    require_admin(&claims.role)?;

    let cid = body.container_id.as_deref().ok_or_else(|| err(StatusCode::BAD_REQUEST, "container_id required"))?;
    let name = body.container_name.as_deref().unwrap_or(cid);

    let min_r = body.min_replicas.unwrap_or(1).max(1).min(20);
    let max_r = body.max_replicas.unwrap_or(5).max(min_r).min(50);
    let up = body.cpu_threshold_up.unwrap_or(80).max(10).min(100);
    let down = body.cpu_threshold_down.unwrap_or(20).max(5).min(up - 5);
    let cool = body.cooldown_seconds.unwrap_or(300).max(60).min(3600);

    let id: (uuid::Uuid,) = sqlx::query_as(
        "INSERT INTO autoscale_rules (container_id, container_name, enabled, min_replicas, max_replicas, \
         cpu_threshold_up, cpu_threshold_down, cooldown_seconds) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
         ON CONFLICT (container_id) DO UPDATE SET \
         enabled = $3, min_replicas = $4, max_replicas = $5, \
         cpu_threshold_up = $6, cpu_threshold_down = $7, cooldown_seconds = $8, updated_at = NOW() \
         RETURNING id"
    )
    .bind(cid).bind(name)
    .bind(body.enabled.unwrap_or(true))
    .bind(min_r).bind(max_r)
    .bind(up).bind(down).bind(cool)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("create autoscale rule", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "autoscale.rule_created",
        Some("container"), Some(name), None, None,
    ).await;

    Ok((StatusCode::CREATED, Json(serde_json::json!({ "ok": true, "id": id.0 }))))
}

/// PUT /api/autoscale/{id} — Update an auto-scaling rule.
pub async fn update_autoscale(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<uuid::Uuid>,
    Json(body): Json<AutoscaleRuleRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let result = sqlx::query(
        "UPDATE autoscale_rules SET \
         enabled = COALESCE($1, enabled), \
         min_replicas = COALESCE($2, min_replicas), \
         max_replicas = COALESCE($3, max_replicas), \
         cpu_threshold_up = COALESCE($4, cpu_threshold_up), \
         cpu_threshold_down = COALESCE($5, cpu_threshold_down), \
         cooldown_seconds = COALESCE($6, cooldown_seconds), \
         updated_at = NOW() \
         WHERE id = $7"
    )
    .bind(body.enabled)
    .bind(body.min_replicas.map(|v| v.max(1).min(20)))
    .bind(body.max_replicas.map(|v| v.max(1).min(50)))
    .bind(body.cpu_threshold_up.map(|v| v.max(10).min(100)))
    .bind(body.cpu_threshold_down.map(|v| v.max(5).min(95)))
    .bind(body.cooldown_seconds.map(|v| v.max(60).min(3600)))
    .bind(id)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("update autoscale rule", e))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Rule not found"));
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// DELETE /api/autoscale/{id} — Remove an auto-scaling rule.
pub async fn delete_autoscale(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let result = sqlx::query("DELETE FROM autoscale_rules WHERE id = $1")
        .bind(id).execute(&state.db).await
        .map_err(|e| internal_error("delete autoscale rule", e))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Rule not found"));
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}
