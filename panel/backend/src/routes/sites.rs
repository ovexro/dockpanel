use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{err, agent_error, paginate, ApiError};
use crate::models::Site;
use crate::routes::is_valid_domain;
use crate::services::activity;
use crate::AppState;

#[derive(serde::Deserialize)]
pub struct ListQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(serde::Deserialize)]
pub struct CreateSiteRequest {
    pub domain: String,
    pub runtime: Option<String>,
    pub proxy_port: Option<i32>,
    pub php_version: Option<String>,
}

/// GET /api/sites — List all sites for the current user.
pub async fn list(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Query(params): Query<ListQuery>,
) -> Result<Json<Vec<Site>>, ApiError> {
    let (limit, offset) = paginate(params.limit, params.offset);

    let sites: Vec<Site> = sqlx::query_as(
        "SELECT * FROM sites WHERE user_id = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3",
    )
    .bind(claims.sub)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(sites))
}

/// POST /api/sites — Create a new site.
///
/// 1. Insert into DB with status "creating"
/// 2. Call agent to configure nginx
/// 3. Update status to "active" or "error"
pub async fn create(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<CreateSiteRequest>,
) -> Result<(StatusCode, Json<Site>), ApiError> {
    // Validate domain format
    if !is_valid_domain(&body.domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }

    let runtime = body.runtime.as_deref().unwrap_or("static");
    if !["static", "php", "proxy"].contains(&runtime) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Runtime must be static, php, or proxy",
        ));
    }

    if runtime == "proxy" && body.proxy_port.is_none() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "proxy_port is required for proxy runtime",
        ));
    }

    // Check domain uniqueness
    let existing: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM sites WHERE domain = $1")
            .bind(&body.domain)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if existing.is_some() {
        return Err(err(StatusCode::CONFLICT, "Domain already exists"));
    }

    // Insert site with status "creating"
    let site: Site = sqlx::query_as(
        "INSERT INTO sites (user_id, domain, runtime, status, proxy_port, php_version) \
         VALUES ($1, $2, $3, 'creating', $4, $5) RETURNING *",
    )
    .bind(claims.sub)
    .bind(&body.domain)
    .bind(runtime)
    .bind(body.proxy_port)
    .bind(&body.php_version)
    .fetch_one(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    // Build agent request body
    let mut agent_body = serde_json::json!({
        "runtime": runtime,
    });

    if let Some(port) = body.proxy_port {
        agent_body["proxy_port"] = serde_json::json!(port);
    }
    if let Some(ref php) = body.php_version {
        agent_body["php_socket"] = serde_json::json!(format!("unix:/run/php/php{php}-fpm.sock"));
    }

    // Call agent to create nginx config
    let agent_path = format!("/nginx/sites/{}", body.domain);
    match state.agent.put(&agent_path, agent_body).await {
        Ok(_) => {
            // Update status to active (only if still in 'creating' state)
            sqlx::query(
                "UPDATE sites SET status = 'active', updated_at = NOW() \
                 WHERE id = $1 AND status = 'creating'"
            )
                .bind(site.id)
                .execute(&state.db)
                .await
                .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

            let updated: Site = sqlx::query_as("SELECT * FROM sites WHERE id = $1")
                .bind(site.id)
                .fetch_one(&state.db)
                .await
                .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

            tracing::info!("Site created: {} ({})", body.domain, runtime);
            activity::log_activity(
                &state.db, claims.sub, &claims.email, "site.create",
                Some("site"), Some(&body.domain), Some(runtime), None,
            ).await;
            Ok((StatusCode::CREATED, Json(updated)))
        }
        Err(e) => {
            // Update status to error
            tracing::error!("Agent error creating site {}: {e}", body.domain);
            sqlx::query("UPDATE sites SET status = 'error', updated_at = NOW() WHERE id = $1")
                .bind(site.id)
                .execute(&state.db)
                .await
                .ok();

            Err(agent_error("Site configuration", e))
        }
    }
}

/// GET /api/sites/{id} — Get site details.
pub async fn get_one(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Site>, ApiError> {
    let site: Site = sqlx::query_as(
        "SELECT * FROM sites WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    Ok(Json(site))
}

/// PUT /api/sites/{id}/php — Switch PHP version for a site.
#[derive(serde::Deserialize)]
pub struct SwitchPhpRequest {
    pub version: String,
}

pub async fn switch_php(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<SwitchPhpRequest>,
) -> Result<Json<Site>, ApiError> {
    let version = body.version.trim();

    // Validate version format (e.g. "8.1", "8.2", "8.3", "8.4")
    if !["8.1", "8.2", "8.3", "8.4"].contains(&version) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Invalid PHP version. Allowed: 8.1, 8.2, 8.3, 8.4",
        ));
    }

    // Fetch site and verify ownership
    let site: Site = sqlx::query_as(
        "SELECT * FROM sites WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    if site.runtime != "php" {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "PHP version can only be changed on PHP sites",
        ));
    }

    // Build agent request to re-render nginx config with the new PHP socket
    let mut agent_body = serde_json::json!({
        "runtime": "php",
        "php_socket": format!("unix:/run/php/php{version}-fpm.sock"),
    });

    // Preserve custom nginx directives
    if let Some(ref custom) = site.custom_nginx {
        agent_body["custom_nginx"] = serde_json::json!(custom);
    }

    // Preserve SSL state
    if site.ssl_enabled {
        agent_body["ssl"] = serde_json::json!(true);
        if let Some(ref cert) = site.ssl_cert_path {
            agent_body["ssl_cert"] = serde_json::json!(cert);
        }
        if let Some(ref key) = site.ssl_key_path {
            agent_body["ssl_key"] = serde_json::json!(key);
        }
    }

    // Call agent to update nginx config
    let agent_path = format!("/nginx/sites/{}", site.domain);
    state
        .agent
        .put(&agent_path, agent_body)
        .await
        .map_err(|e| agent_error("Nginx update", e))?;

    // Update DB
    let updated: Site = sqlx::query_as(
        "UPDATE sites SET php_version = $1, updated_at = NOW() WHERE id = $2 RETURNING *",
    )
    .bind(version)
    .bind(id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    tracing::info!("PHP version switched to {} for {}", version, site.domain);
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "site.php_switch",
        Some("site"), Some(&site.domain), Some(version), None,
    ).await;

    Ok(Json(updated))
}

/// GET /api/php/versions — List available PHP versions (proxy to agent).
pub async fn php_versions(
    State(state): State<AppState>,
    AuthUser(_claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = state
        .agent
        .get("/php/versions")
        .await
        .map_err(|e| agent_error("Site agent operation", e))?;

    Ok(Json(result))
}

/// POST /api/php/install — Install a PHP version (proxy to agent, admin only).
#[derive(serde::Deserialize)]
pub struct InstallPhpRequest {
    pub version: String,
}

pub async fn php_install(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<InstallPhpRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if claims.role != "admin" {
        return Err(err(StatusCode::FORBIDDEN, "Admin only"));
    }

    let result = state
        .agent
        .post(
            "/php/install",
            Some(serde_json::json!({ "version": body.version })),
        )
        .await
        .map_err(|e| agent_error("PHP install", e))?;

    Ok(Json(result))
}

/// PUT /api/sites/{id}/limits — Update per-site resource limits.
#[derive(serde::Deserialize)]
pub struct UpdateLimitsRequest {
    pub rate_limit: Option<i32>,       // requests/sec per IP, null = unlimited
    pub max_upload_mb: Option<i32>,    // client_max_body_size
    pub php_memory_mb: Option<i32>,    // PHP memory_limit
    pub php_max_workers: Option<i32>,  // PHP-FPM pm.max_children
    pub custom_nginx: Option<String>,  // custom nginx directives
}

pub async fn update_limits(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateLimitsRequest>,
) -> Result<Json<Site>, ApiError> {
    let site: Site = sqlx::query_as(
        "SELECT * FROM sites WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    // Validate limits
    if let Some(rl) = body.rate_limit {
        if rl < 1 || rl > 10000 {
            return Err(err(StatusCode::BAD_REQUEST, "Rate limit must be between 1 and 10000"));
        }
    }
    let max_upload = body.max_upload_mb.unwrap_or(site.max_upload_mb);
    if max_upload < 1 || max_upload > 10240 {
        return Err(err(StatusCode::BAD_REQUEST, "Max upload must be between 1 and 10240 MB"));
    }
    let php_memory = body.php_memory_mb.unwrap_or(site.php_memory_mb);
    if php_memory < 32 || php_memory > 4096 {
        return Err(err(StatusCode::BAD_REQUEST, "PHP memory must be between 32 and 4096 MB"));
    }
    let php_workers = body.php_max_workers.unwrap_or(site.php_max_workers);
    if php_workers < 1 || php_workers > 100 {
        return Err(err(StatusCode::BAD_REQUEST, "PHP workers must be between 1 and 100"));
    }

    // Validate custom_nginx
    if let Some(ref custom) = body.custom_nginx {
        if custom.len() > 10240 {
            return Err(err(StatusCode::BAD_REQUEST, "Custom nginx directives must be under 10KB"));
        }
        if custom.contains('\0') {
            return Err(err(StatusCode::BAD_REQUEST, "Custom nginx directives contain invalid characters"));
        }
    }

    // Update DB
    let custom_nginx = body.custom_nginx.as_deref();
    let updated: Site = sqlx::query_as(
        "UPDATE sites SET rate_limit = $1, max_upload_mb = $2, php_memory_mb = $3, php_max_workers = $4, \
         custom_nginx = $5, updated_at = NOW() WHERE id = $6 RETURNING *",
    )
    .bind(body.rate_limit)
    .bind(max_upload)
    .bind(php_memory)
    .bind(php_workers)
    .bind(custom_nginx)
    .bind(id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    // Re-render nginx config with new limits
    let mut agent_body = serde_json::json!({
        "runtime": site.runtime,
        "rate_limit": body.rate_limit,
        "max_upload_mb": max_upload,
        "php_memory_mb": php_memory,
        "php_max_workers": php_workers,
    });
    if let Some(ref custom) = body.custom_nginx {
        agent_body["custom_nginx"] = serde_json::json!(custom);
    } else if let Some(ref existing) = site.custom_nginx {
        agent_body["custom_nginx"] = serde_json::json!(existing);
    }

    if let Some(port) = site.proxy_port {
        agent_body["proxy_port"] = serde_json::json!(port);
    }
    if let Some(ref php) = site.php_version {
        agent_body["php_socket"] = serde_json::json!(format!("unix:/run/php/php{php}-fpm.sock"));
    }
    if site.ssl_enabled {
        agent_body["ssl"] = serde_json::json!(true);
        if let Some(ref cert) = site.ssl_cert_path {
            agent_body["ssl_cert"] = serde_json::json!(cert);
        }
        if let Some(ref key) = site.ssl_key_path {
            agent_body["ssl_key"] = serde_json::json!(key);
        }
    }

    let agent_path = format!("/nginx/sites/{}", site.domain);
    state
        .agent
        .put(&agent_path, agent_body)
        .await
        .map_err(|e| agent_error("Resource limits", e))?;

    tracing::info!("Resource limits updated for {}", site.domain);
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "site.limits",
        Some("site"), Some(&site.domain), None, None,
    ).await;

    Ok(Json(updated))
}

/// DELETE /api/sites/{id} — Delete a site and its nginx config.
pub async fn remove(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: Site = sqlx::query_as(
        "SELECT * FROM sites WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    // Call agent to remove nginx config (must succeed before DB deletion)
    let agent_path = format!("/nginx/sites/{}", site.domain);
    state.agent.delete(&agent_path).await
        .map_err(|e| agent_error("Site removal", e))?;

    // Delete from DB
    sqlx::query("DELETE FROM sites WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    tracing::info!("Site deleted: {}", site.domain);
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "site.delete",
        Some("site"), Some(&site.domain), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true, "domain": site.domain })))
}
