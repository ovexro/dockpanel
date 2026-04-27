use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::{AdminUser, AuthUser};
use crate::error::{internal_error, err, ApiError};
use crate::services::activity;
use crate::AppState;

#[derive(serde::Deserialize)]
pub struct CreateServerRequest {
    pub name: String,
    pub ip_address: Option<String>,
}

#[derive(serde::Serialize, sqlx::FromRow)]
#[allow(dead_code)]
pub struct Server {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub ip_address: Option<String>,
    #[serde(skip_serializing)]
    pub agent_token: String,
    pub agent_url: Option<String>,
    pub status: String,
    pub is_local: bool,
    pub last_seen_at: Option<chrono::DateTime<chrono::Utc>>,
    pub os_info: Option<String>,
    pub cpu_cores: Option<i32>,
    pub ram_mb: Option<i32>,
    pub disk_gb: Option<i32>,
    pub agent_version: Option<String>,
    pub cpu_usage: Option<f32>,
    pub mem_used_mb: Option<i64>,
    pub uptime_secs: Option<i64>,
    pub cert_fingerprint: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// GET /api/servers — List current user's servers.
pub async fn list(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<Vec<Server>>, ApiError> {
    let servers: Vec<Server> =
        sqlx::query_as("SELECT * FROM servers WHERE user_id = $1 ORDER BY is_local DESC, created_at DESC LIMIT 500")
            .bind(claims.sub)
            .fetch_all(&state.db)
            .await
            .map_err(|e| internal_error("list servers", e))?;

    Ok(Json(servers))
}

/// POST /api/servers — Register a new remote server. Returns agent token and install script.
pub async fn create(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Json(body): Json<CreateServerRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let name = body.name.trim();
    if name.is_empty() || name.len() > 100 {
        return Err(err(StatusCode::BAD_REQUEST, "Name must be 1-100 characters"));
    }

    let ip = body.ip_address.as_deref().unwrap_or("").trim().to_string();

    let agent_token = format!(
        "{}{}",
        Uuid::new_v4().to_string().replace('-', ""),
        Uuid::new_v4().to_string().replace('-', ""),
    );

    // Default agent URL from IP (port 9443 for HTTPS agent)
    let agent_url = if !ip.is_empty() {
        format!("https://{}:9443", ip)
    } else {
        String::new()
    };

    let agent_token_hash = crate::helpers::hash_agent_token(&agent_token);

    let server: Server = sqlx::query_as(
        "INSERT INTO servers (user_id, name, ip_address, agent_token, agent_token_hash, agent_url, status, is_local) \
         VALUES ($1, $2, $3, $4, $5, $6, 'pending', false) RETURNING *",
    )
    .bind(claims.sub)
    .bind(name)
    .bind(if ip.is_empty() { None } else { Some(&ip) })
    .bind(&agent_token)
    .bind(&agent_token_hash)
    .bind(if agent_url.is_empty() { None } else { Some(&agent_url) })
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("create servers", e))?;

    // Generate install script with panel URL and token
    let panel_url = &state.config.base_url;
    let install_script = format!(
        "curl -sSL {panel_url}/install-agent.sh | sudo bash -s -- \\\n  --panel-url {panel_url} \\\n  --token {agent_token} \\\n  --server-id {}",
        server.id
    );

    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "server.create",
        Some("server"),
        Some(name),
        None,
        None,
    )
    .await;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": server.id,
            "name": server.name,
            "status": server.status,
            "agent_token": agent_token,
            "install_script": install_script,
        })),
    ))
}

/// GET /api/servers/{id} — Get server details.
pub async fn get_one(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Server>, ApiError> {
    let server: Server =
        sqlx::query_as("SELECT * FROM servers WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(claims.sub)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("get_one servers", e))?
            .ok_or_else(|| err(StatusCode::NOT_FOUND, "Server not found"))?;

    Ok(Json(server))
}

/// DELETE /api/servers/{id} — Remove a server (cannot delete local server).
pub async fn remove(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let server: Server =
        sqlx::query_as("SELECT * FROM servers WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(claims.sub)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("remove servers", e))?
            .ok_or_else(|| err(StatusCode::NOT_FOUND, "Server not found"))?;

    if server.is_local {
        return Err(err(StatusCode::BAD_REQUEST, "Cannot delete the local server"));
    }

    // Cascade delete removes sites, DBs, stacks, etc.
    sqlx::query("DELETE FROM servers WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("remove servers", e))?;

    // Invalidate remote agent cache
    state.agents.invalidate(id).await;

    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "server.delete",
        Some("server"),
        Some(&server.name),
        None,
        None,
    )
    .await;

    Ok(Json(serde_json::json!({ "ok": true, "name": server.name })))
}

/// POST /api/servers/{id}/rotate-cert-pin — Clear the stored TLS fingerprint
/// so the next agent checkin re-captures it (TOFU). Use when an agent's cert
/// has legitimately changed (rotation, reinstall) — otherwise an unexpected
/// fingerprint change signals MITM and is refused at checkin time.
pub async fn rotate_cert_pin(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let server: Server =
        sqlx::query_as("SELECT * FROM servers WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(claims.sub)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("rotate cert pin", e))?
            .ok_or_else(|| err(StatusCode::NOT_FOUND, "Server not found"))?;

    sqlx::query("UPDATE servers SET cert_fingerprint = NULL WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("rotate cert pin", e))?;

    // Invalidate remote agent cache so a refreshed RemoteAgentClient is built
    // the next time a route needs the agent handle.
    state.agents.invalidate(id).await;

    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "server.rotate_cert_pin",
        Some("server"),
        Some(&server.name),
        None,
        None,
    )
    .await;

    Ok(Json(serde_json::json!({
        "ok": true,
        "message": "Cert pin cleared — next agent checkin will re-capture the fingerprint (TOFU).",
    })))
}

/// POST /api/servers/{id}/test — Test connection to a remote server's agent.
pub async fn test_connection(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let server: Server =
        sqlx::query_as("SELECT * FROM servers WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(claims.sub)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("test connection", e))?
            .ok_or_else(|| err(StatusCode::NOT_FOUND, "Server not found"))?;

    // Try to reach the agent
    let agent = state
        .agents
        .for_server(id)
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &e.to_string()))?;

    match agent.get("/health").await {
        Ok(resp) => {
            // Update server status to online
            let version = resp.get("version").and_then(|v| v.as_str()).unwrap_or("unknown");
            sqlx::query(
                "UPDATE servers SET status = 'online', last_seen_at = NOW(), agent_version = $1 WHERE id = $2",
            )
            .bind(version)
            .bind(id)
            .execute(&state.db)
            .await
            .ok();

            Ok(Json(serde_json::json!({
                "ok": true,
                "status": "online",
                "version": version,
                "name": server.name,
            })))
        }
        Err(e) => {
            // Update status to offline
            sqlx::query("UPDATE servers SET status = 'offline' WHERE id = $1")
                .bind(id)
                .execute(&state.db)
                .await
                .ok();

            Err(err(
                StatusCode::BAD_GATEWAY,
                &format!("Agent unreachable: {e}"),
            ))
        }
    }
}

/// PUT /api/servers/{id} — Update server name/IP/URL.
pub async fn update(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<Server>, ApiError> {
    let server: Server =
        sqlx::query_as("SELECT * FROM servers WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(claims.sub)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("update servers", e))?
            .ok_or_else(|| err(StatusCode::NOT_FOUND, "Server not found"))?;

    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or(&server.name);
    let ip = body
        .get("ip_address")
        .and_then(|v| v.as_str())
        .or(server.ip_address.as_deref());
    let url = body
        .get("agent_url")
        .and_then(|v| v.as_str())
        .or(server.agent_url.as_deref());

    let updated: Server = sqlx::query_as(
        "UPDATE servers SET name = $1, ip_address = $2, agent_url = $3 WHERE id = $4 RETURNING *",
    )
    .bind(name)
    .bind(ip)
    .bind(url)
    .bind(id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("update servers", e))?;

    // Invalidate remote agent cache so it picks up new URL/token
    state.agents.invalidate(id).await;

    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "server.update",
        Some("server"),
        Some(name),
        None,
        None,
    )
    .await;

    Ok(Json(updated))
}

/// POST /api/servers/{id}/rotate-token — Rotate agent token.
/// Calls the agent's rotation endpoint, then updates the hash in the database.
pub async fn rotate_token(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Verify server belongs to this admin
    let _server: Server = sqlx::query_as("SELECT * FROM servers WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(claims.sub)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("rotate token", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Server not found"))?;

    // Get agent handle for this server
    let agent = state
        .agents
        .for_server(id)
        .await
        .map_err(|e| crate::error::agent_error("get agent", e))?;

    // Call agent's rotation endpoint
    let resp: serde_json::Value = agent
        .post("/auth/rotate-token", Some(serde_json::json!({})))
        .await
        .map_err(|e| crate::error::agent_error("rotate token", e))?;

    let new_token = resp
        .get("new_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| err(StatusCode::INTERNAL_SERVER_ERROR, "Agent did not return new token"))?;

    // Update database with new hash (and plaintext for remote agent communication)
    let new_hash = crate::helpers::hash_agent_token(new_token);
    sqlx::query(
        "UPDATE servers SET agent_token = $1, agent_token_hash = $2 WHERE id = $3",
    )
    .bind(new_token)
    .bind(&new_hash)
    .bind(id)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("rotate token", e))?;

    // Invalidate cached remote agent client so it picks up the new token
    state.agents.invalidate(id).await;

    // If this is the local server, update the in-memory AgentClient token and api.env on disk
    if let Some(local_id) = state.agents.local_server_id().await {
        if local_id == id {
            // Update in-memory token so the API can immediately communicate with the agent
            state.agents.local().update_token(new_token.to_string()).await;

            // Update api.env on disk for persistence across API restarts
            let env_path = "/etc/dockpanel/api.env";
            if let Ok(contents) = std::fs::read_to_string(env_path) {
                let updated = contents
                    .lines()
                    .map(|line| {
                        if line.starts_with("AGENT_TOKEN=") {
                            format!("AGENT_TOKEN={new_token}")
                        } else {
                            line.to_string()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if let Err(e) = std::fs::write(env_path, &updated) {
                    tracing::warn!("Failed to update api.env with new token: {e}");
                } else {
                    tracing::info!("Updated AGENT_TOKEN in {env_path}");
                }
            }
        }
    }

    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "server.rotate_token",
        Some("server"),
        None,
        None,
        None,
    )
    .await;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Agent token rotated successfully"
    })))
}

#[derive(serde::Serialize)]
pub struct UptimeResponse {
    pub buckets: Vec<bool>,
    pub window_hours: i32,
    pub bucket_minutes: i32,
}

/// GET /api/servers/{id}/uptime — 24h × 10min uptime sparkline derived from
/// `metrics_history` row presence. A bucket is "online" if at least one
/// metrics_collector row landed in that 10-minute window for the server.
/// 144 buckets total, oldest first (index 0 = ~24h ago, last = now).
pub async fn uptime(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    axum::extract::Path(id): axum::extract::Path<Uuid>,
) -> Result<Json<UptimeResponse>, ApiError> {
    let owned: Option<bool> =
        sqlx::query_scalar("SELECT TRUE FROM servers WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(claims.sub)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("check server ownership", e))?;
    if owned.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Server not found"));
    }

    const BUCKET_SECS: i64 = 600;
    const TOTAL_BUCKETS: i64 = 144;

    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT DISTINCT FLOOR(EXTRACT(EPOCH FROM created_at) / 600)::bigint AS bucket \
         FROM metrics_history \
         WHERE server_id = $1 AND created_at >= NOW() - INTERVAL '24 hours'",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("query uptime buckets", e))?;

    let online: std::collections::HashSet<i64> = rows.into_iter().map(|(b,)| b).collect();
    let now_bucket = chrono::Utc::now().timestamp() / BUCKET_SECS;
    let start = now_bucket - TOTAL_BUCKETS + 1;
    let buckets: Vec<bool> = (start..=now_bucket).map(|b| online.contains(&b)).collect();

    Ok(Json(UptimeResponse {
        buckets,
        window_hours: 24,
        bucket_minutes: 10,
    }))
}
