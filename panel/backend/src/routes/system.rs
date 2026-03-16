use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
    Json,
};
use futures::stream::StreamExt;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use uuid::Uuid;

use crate::auth::{AdminUser, AuthUser};
use crate::error::{err, agent_error, ApiError};
use crate::routes::sites::ProvisionStep;
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

// ── Service installers (proxy to agent, async with SSE progress) ─────────

pub async fn install_status(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = state.agent.get("/services/install-status").await
        .map_err(|e| agent_error("Install status", e))?;
    Ok(Json(result))
}

/// Generic service install with provisioning log (async SSE).
async fn install_service_with_log(
    state: &AppState,
    claims_sub: Uuid,
    claims_email: &str,
    service_name: &str,
    agent_path: &str,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let install_id = Uuid::new_v4();

    let (tx, _) = broadcast::channel::<ProvisionStep>(32);
    {
        let mut logs = state.provision_logs.lock().unwrap();
        logs.insert(install_id, (Vec::new(), tx, Instant::now()));
    }

    let logs = state.provision_logs.clone();
    let agent = state.agent.clone();
    let db = state.db.clone();
    let svc = service_name.to_string();
    let path = agent_path.to_string();
    let email = claims_email.to_string();

    tokio::spawn(async move {
        let emit = |step: &str, lbl: &str, status: &str, msg: Option<String>| {
            let ev = ProvisionStep {
                step: step.into(),
                label: lbl.into(),
                status: status.into(),
                message: msg,
            };
            if let Ok(mut map) = logs.lock() {
                if let Some((history, tx, _)) = map.get_mut(&install_id) {
                    history.push(ev.clone());
                    let _ = tx.send(ev);
                }
            }
        };

        emit("install", &format!("Installing {svc}"), "in_progress", None);

        match agent.post(&path, None).await {
            Ok(_) => {
                emit("install", &format!("Installing {svc}"), "done", None);
                emit("complete", &format!("{svc} installed"), "done", None);
                activity::log_activity(
                    &db, claims_sub, &email, "service.install",
                    Some("system"), Some(&svc), None, None,
                ).await;
                tracing::info!("Service installed: {svc}");
            }
            Err(e) => {
                emit("install", &format!("Installing {svc}"), "error", Some(format!("{e}")));
                emit("complete", "Install failed", "error", None);
                tracing::error!("Service install failed: {svc}: {e}");
            }
        }

        tokio::time::sleep(Duration::from_secs(30)).await;
        logs.lock().unwrap().remove(&install_id);
    });

    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({
        "install_id": install_id,
        "message": format!("{service_name} installation started"),
    }))))
}

pub async fn install_php(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, claims.sub, &claims.email, "PHP", "/services/install/php").await
}

pub async fn install_certbot(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, claims.sub, &claims.email, "Certbot", "/services/install/certbot").await
}

pub async fn install_ufw(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, claims.sub, &claims.email, "UFW Firewall", "/services/install/ufw").await
}

/// GET /api/services/install/{install_id}/log — SSE stream of install progress.
pub async fn install_log(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Path(install_id): Path<Uuid>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, axum::BoxError>>>, ApiError> {
    let (snapshot, rx) = {
        let logs = state.provision_logs.lock().unwrap();
        match logs.get(&install_id) {
            Some((history, tx, _)) => (history.clone(), Some(tx.subscribe())),
            None => (Vec::new(), None),
        }
    };

    let rx = rx.ok_or_else(|| err(StatusCode::NOT_FOUND, "No active install"))?;

    let snapshot_stream = futures::stream::iter(
        snapshot.into_iter().map(|step| {
            let data = serde_json::to_string(&step).unwrap_or_default();
            Ok(Event::default().data(data))
        }),
    );

    let live_stream = BroadcastStream::new(rx).filter_map(|result| async {
        match result {
            Ok(step) => {
                let data = serde_json::to_string(&step).ok()?;
                Some(Ok(Event::default().data(data)))
            }
            Err(_) => None,
        }
    });

    Ok(
        Sse::new(snapshot_stream.chain(live_stream))
            .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("ping")),
    )
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
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let install_id = Uuid::new_v4();

    let (tx, _) = broadcast::channel::<ProvisionStep>(32);
    {
        let mut logs = state.provision_logs.lock().unwrap();
        logs.insert(install_id, (Vec::new(), tx, Instant::now()));
    }

    let logs = state.provision_logs.clone();
    let agent = state.agent.clone();
    let db = state.db.clone();
    let user_id = claims.sub;
    let email = claims.email.clone();

    tokio::spawn(async move {
        let emit = |step: &str, lbl: &str, status: &str, msg: Option<String>| {
            let ev = ProvisionStep {
                step: step.into(),
                label: lbl.into(),
                status: status.into(),
                message: msg,
            };
            if let Ok(mut map) = logs.lock() {
                if let Some((history, tx, _)) = map.get_mut(&install_id) {
                    history.push(ev.clone());
                    let _ = tx.send(ev);
                }
            }
        };

        emit("install", "Installing PowerDNS", "in_progress", None);

        match agent.post("/services/install/powerdns", None).await {
            Ok(result) => {
                // Auto-save API URL and key to settings
                if let (Some(url), Some(key)) = (
                    result.get("api_url").and_then(|v| v.as_str()),
                    result.get("api_key").and_then(|v| v.as_str()),
                ) {
                    let _ = sqlx::query("INSERT INTO settings (key, value, updated_at) VALUES ('pdns_api_url', $1, NOW()) ON CONFLICT (key) DO UPDATE SET value = $1, updated_at = NOW()")
                        .bind(url)
                        .execute(&db)
                        .await;
                    let _ = sqlx::query("INSERT INTO settings (key, value, updated_at) VALUES ('pdns_api_key', $1, NOW()) ON CONFLICT (key) DO UPDATE SET value = $1, updated_at = NOW()")
                        .bind(key)
                        .execute(&db)
                        .await;
                }

                emit("install", "Installing PowerDNS", "done", None);
                emit("complete", "PowerDNS installed", "done", None);
                activity::log_activity(
                    &db, user_id, &email, "service.install",
                    Some("system"), Some("powerdns"), None, None,
                ).await;
                tracing::info!("Service installed: PowerDNS");
            }
            Err(e) => {
                emit("install", "Installing PowerDNS", "error", Some(format!("{e}")));
                emit("complete", "Install failed", "error", None);
                tracing::error!("Service install failed: PowerDNS: {e}");
            }
        }

        tokio::time::sleep(Duration::from_secs(30)).await;
        logs.lock().unwrap().remove(&install_id);
    });

    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({
        "install_id": install_id,
        "message": "PowerDNS installation started",
    }))))
}

pub async fn install_fail2ban(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, claims.sub, &claims.email, "Fail2Ban", "/services/install/fail2ban").await
}
