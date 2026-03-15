use axum::{extract::State, Json};

use crate::auth::{AdminUser, AuthUser};
use crate::error::{agent_error, ApiError};
use crate::services::activity;
use crate::AppState;

/// GET /api/health — Public health check.
pub async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "service": "dockpanel-api",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

/// GET /api/system/info — Proxy to agent's system info (authenticated).
pub async fn info(
    State(state): State<AppState>,
    AuthUser(_claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = state
        .agent
        .get("/system/info")
        .await
        .map_err(|e| agent_error("System info", e))?;
    Ok(Json(data))
}

/// GET /api/agent/diagnostics — Proxy to agent's diagnostics (authenticated).
pub async fn diagnostics(
    State(state): State<AppState>,
    AuthUser(_claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = state
        .agent
        .get("/diagnostics")
        .await
        .map_err(|e| agent_error("Diagnostics", e))?;
    Ok(Json(data))
}

/// POST /api/agent/diagnostics/fix — Proxy to agent's diagnostics fix (admin).
pub async fn diagnostics_fix(
    State(state): State<AppState>,
    crate::auth::AdminUser(_claims): crate::auth::AdminUser,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = state
        .agent
        .post("/diagnostics/fix", Some(body))
        .await
        .map_err(|e| agent_error("Diagnostics fix", e))?;
    Ok(Json(data))
}

/// GET /api/system/updates — List available package updates (admin only).
pub async fn updates_list(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = state
        .agent
        .get("/system/updates")
        .await
        .map_err(|e| agent_error("System updates", e))?;
    Ok(Json(data))
}

/// POST /api/system/updates/apply — Apply package updates (admin only).
pub async fn updates_apply(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = state
        .agent
        .post("/system/updates/apply", Some(body))
        .await
        .map_err(|e| agent_error("Apply updates", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "system.updates.apply",
        Some("system"), Some("packages"), None, None,
    ).await;

    Ok(Json(data))
}

/// GET /api/system/updates/count — Get count of available updates (any authenticated user).
pub async fn updates_count(
    State(state): State<AppState>,
    AuthUser(_claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = state
        .agent
        .get("/system/updates/count")
        .await
        .map_err(|e| agent_error("Update count", e))?;
    Ok(Json(data))
}

/// POST /api/system/reboot — Reboot the system (admin only).
pub async fn system_reboot(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = state
        .agent
        .post("/system/reboot", None::<serde_json::Value>)
        .await
        .map_err(|e| agent_error("System reboot", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "system.reboot",
        Some("system"), Some("server"), None, None,
    ).await;

    Ok(Json(data))
}

// ── Service installers (proxy to agent) ─────────────────────────────────

pub async fn install_status(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = state.agent.get("/services/install-status").await
        .map_err(|e| agent_error("Install status", e))?;
    Ok(Json(result))
}

pub async fn install_php(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = state.agent.post("/services/install/php", None).await
        .map_err(|e| agent_error("PHP install", e))?;
    activity::log_activity(&state.db, claims.sub, &claims.email, "service.install", Some("system"), Some("php"), None, None).await;
    Ok(Json(result))
}

pub async fn install_certbot(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = state.agent.post("/services/install/certbot", None).await
        .map_err(|e| agent_error("Certbot install", e))?;
    activity::log_activity(&state.db, claims.sub, &claims.email, "service.install", Some("system"), Some("certbot"), None, None).await;
    Ok(Json(result))
}

pub async fn install_ufw(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = state.agent.post("/services/install/ufw", None).await
        .map_err(|e| agent_error("UFW install", e))?;
    activity::log_activity(&state.db, claims.sub, &claims.email, "service.install", Some("system"), Some("ufw"), None, None).await;
    Ok(Json(result))
}

// ── SSH Keys ────────────────────────────────────────────────────────────

pub async fn list_ssh_keys(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = state.agent.get("/ssh-keys").await.map_err(|e| agent_error("SSH keys", e))?;
    Ok(Json(result))
}

pub async fn add_ssh_key(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = state.agent.post("/ssh-keys", Some(body)).await.map_err(|e| agent_error("Add SSH key", e))?;
    activity::log_activity(&state.db, claims.sub, &claims.email, "ssh.key.add", Some("system"), None, None, None).await;
    Ok(Json(result))
}

pub async fn remove_ssh_key(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    axum::extract::Path(fingerprint): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = state.agent.delete(&format!("/ssh-keys/{fingerprint}")).await.map_err(|e| agent_error("Remove SSH key", e))?;
    activity::log_activity(&state.db, claims.sub, &claims.email, "ssh.key.remove", Some("system"), None, None, None).await;
    Ok(Json(result))
}

// ── Auto-Updates ────────────────────────────────────────────────────────

pub async fn auto_updates_status(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = state.agent.get("/auto-updates/status").await.map_err(|e| agent_error("Auto-updates", e))?;
    Ok(Json(result))
}

pub async fn enable_auto_updates(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = state.agent.post("/auto-updates/enable", None).await.map_err(|e| agent_error("Enable auto-updates", e))?;
    activity::log_activity(&state.db, claims.sub, &claims.email, "auto-updates.enable", Some("system"), None, None, None).await;
    Ok(Json(result))
}

pub async fn disable_auto_updates(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = state.agent.post("/auto-updates/disable", None).await.map_err(|e| agent_error("Disable auto-updates", e))?;
    activity::log_activity(&state.db, claims.sub, &claims.email, "auto-updates.disable", Some("system"), None, None, None).await;
    Ok(Json(result))
}

// ── Panel IP Whitelist ──────────────────────────────────────────────────

pub async fn get_panel_whitelist(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = state.agent.get("/panel-whitelist").await.map_err(|e| agent_error("Whitelist", e))?;
    Ok(Json(result))
}

pub async fn set_panel_whitelist(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = state.agent.post("/panel-whitelist", Some(body)).await.map_err(|e| agent_error("Set whitelist", e))?;
    activity::log_activity(&state.db, claims.sub, &claims.email, "panel.whitelist.update", Some("system"), None, None, None).await;
    Ok(Json(result))
}

pub async fn install_powerdns(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = state.agent.post("/services/install/powerdns", None).await
        .map_err(|e| agent_error("PowerDNS install", e))?;

    // Auto-save API URL and key to settings
    if let (Some(url), Some(key)) = (
        result.get("api_url").and_then(|v| v.as_str()),
        result.get("api_key").and_then(|v| v.as_str()),
    ) {
        let _ = sqlx::query("INSERT INTO settings (key, value, updated_at) VALUES ('pdns_api_url', $1, NOW()) ON CONFLICT (key) DO UPDATE SET value = $1, updated_at = NOW()")
            .bind(url)
            .execute(&state.db)
            .await;
        let _ = sqlx::query("INSERT INTO settings (key, value, updated_at) VALUES ('pdns_api_key', $1, NOW()) ON CONFLICT (key) DO UPDATE SET value = $1, updated_at = NOW()")
            .bind(key)
            .execute(&state.db)
            .await;
    }

    activity::log_activity(&state.db, claims.sub, &claims.email, "service.install", Some("system"), Some("powerdns"), None, None).await;
    Ok(Json(result))
}

pub async fn install_fail2ban(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = state.agent.post("/services/install/fail2ban", None).await
        .map_err(|e| agent_error("Fail2Ban install", e))?;
    activity::log_activity(&state.db, claims.sub, &claims.email, "service.install", Some("system"), Some("fail2ban"), None, None).await;
    Ok(Json(result))
}
