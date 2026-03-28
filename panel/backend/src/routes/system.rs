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

use crate::auth::{AdminUser, ServerScope};
use crate::error::{err, agent_error, ApiError};
use crate::routes::sites::ProvisionStep;
use crate::services::activity;
use crate::services::agent::AgentHandle;
use crate::AppState;

/// GET /api/health — Public health check (includes DB connectivity).
pub async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    let db_ok = sqlx::query("SELECT 1").execute(&state.db).await.is_ok();

    if db_ok {
        Json(serde_json::json!({
            "status": "ok",
            "service": "dockpanel-api",
            "version": env!("CARGO_PKG_VERSION"),
        }))
    } else {
        Json(serde_json::json!({
            "status": "degraded",
            "db": "unreachable",
            "service": "dockpanel-api",
            "version": env!("CARGO_PKG_VERSION"),
        }))
    }
}

/// GET /api/system/info — Proxy to agent's system info (admin only).
pub async fn info(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = agent
        .get("/system/info")
        .await
        .map_err(|e| agent_error("System info", e))?;
    Ok(Json(data))
}

/// GET /api/agent/diagnostics — Proxy to agent's diagnostics (admin only).
pub async fn diagnostics(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = agent
        .get("/diagnostics")
        .await
        .map_err(|e| agent_error("Diagnostics", e))?;
    Ok(Json(data))
}

/// POST /api/agent/diagnostics/fix — Proxy to agent's diagnostics fix (admin).
pub async fn diagnostics_fix(
    State(state): State<AppState>,
    crate::auth::AdminUser(_claims): crate::auth::AdminUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = agent
        .post("/diagnostics/fix", Some(body))
        .await
        .map_err(|e| agent_error("Diagnostics fix", e))?;
    Ok(Json(data))
}

/// GET /api/agent/recommendations — Auto-optimization recommendations (admin only).
pub async fn recommendations(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = agent
        .get("/diagnostics/recommendations")
        .await
        .map_err(|e| agent_error("Recommendations", e))?;
    Ok(Json(data))
}

/// POST /api/system/cleanup — Proxy to agent's disk cleanup (admin only).
pub async fn disk_cleanup(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = agent
        .post("/system/cleanup", None)
        .await
        .map_err(|e| agent_error("Disk cleanup", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "system.cleanup",
        None, None, None, None,
    ).await;

    Ok(Json(data))
}

/// POST /api/system/hostname — Proxy to agent's hostname change (admin only).
pub async fn change_hostname(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = agent
        .post("/system/hostname", Some(body))
        .await
        .map_err(|e| agent_error("Hostname change", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "system.hostname_change",
        None, None, None, None,
    ).await;

    Ok(Json(data))
}

/// GET /api/system/updates — List available package updates (admin only).
pub async fn updates_list(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = agent
        .get("/system/updates")
        .await
        .map_err(|e| agent_error("System updates", e))?;
    Ok(Json(data))
}

/// POST /api/system/updates/apply — Apply package updates (admin only).
/// Returns install_id for SSE progress tracking via /api/services/install/{id}/log.
/// Proxies to agent which runs apt with proper filesystem access, then streams
/// the output line-by-line for live terminal experience.
pub async fn updates_apply(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let install_id = uuid::Uuid::new_v4();

    let (tx, _) = tokio::sync::broadcast::channel::<ProvisionStep>(256);
    {
        let mut logs = state.provision_logs.lock().unwrap_or_else(|e| e.into_inner());
        logs.insert(install_id, (Vec::new(), tx, Instant::now()));
    }

    let logs = state.provision_logs.clone();
    let db = state.db.clone();
    let email = claims.email.clone();
    let user_id = claims.sub;

    tokio::spawn(async move {
        let emit = |step: &str, label: &str, status: &str, msg: Option<String>| {
            let ev = ProvisionStep {
                step: step.into(), label: label.into(), status: status.into(), message: msg,
            };
            if let Ok(mut map) = logs.lock() {
                if let Some((history, tx, _)) = map.get_mut(&install_id) {
                    history.push(ev.clone());
                    let _ = tx.send(ev);
                }
            }
        };

        emit("update", "Applying system updates", "in_progress", None);

        // Proxy to agent (which has proper filesystem access for apt)
        match agent.post_long("/system/updates/apply", Some(body), 300).await {
            Ok(result) => {
                let success = result.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
                let output = result.get("output").and_then(|v| v.as_str()).unwrap_or("");

                // Stream output line-by-line for live terminal experience
                for line in output.lines() {
                    if !line.is_empty() {
                        emit("line", line, "in_progress", None);
                    }
                }

                if success {
                    emit("update", "Applying system updates", "done", None);
                    emit("complete", "Updates applied", "done", Some(output.to_string()));
                } else {
                    emit("update", "Applying system updates", "error", None);
                    emit("complete", "Updates finished with errors", "error", Some(output.to_string()));
                }

                activity::log_activity(&db, user_id, &email, "system.updates.apply",
                    Some("system"), Some("packages"), None, None).await;
            }
            Err(e) => {
                emit("update", "Failed to apply updates", "error", Some(format!("{e}")));
                emit("complete", "Update failed", "error", None);
            }
        }

        tokio::time::sleep(Duration::from_secs(60)).await;
        logs.lock().unwrap_or_else(|e| e.into_inner()).remove(&install_id);
    });

    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({
        "install_id": install_id,
        "message": "Updates started",
    }))))
}

/// GET /api/system/updates/count — Get count of available updates (admin only).
pub async fn updates_count(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = agent
        .get("/system/updates/count")
        .await
        .map_err(|e| agent_error("Update count", e))?;
    Ok(Json(data))
}

/// POST /api/system/reboot — Reboot the system (admin only).
pub async fn system_reboot(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = agent
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
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent.get("/services/install-status").await
        .map_err(|e| agent_error("Install status", e))?;
    Ok(Json(result))
}

/// Generic service install with provisioning log (async SSE).
async fn install_service_with_log(
    state: &AppState,
    agent: AgentHandle,
    claims_sub: Uuid,
    claims_email: &str,
    service_name: &str,
    agent_path: &str,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let install_id = Uuid::new_v4();

    let (tx, _) = broadcast::channel::<ProvisionStep>(32);
    {
        let mut logs = state.provision_logs.lock().unwrap_or_else(|e| e.into_inner());
        logs.insert(install_id, (Vec::new(), tx, Instant::now()));
    }

    let logs = state.provision_logs.clone();
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
        logs.lock().unwrap_or_else(|e| e.into_inner()).remove(&install_id);
    });

    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({
        "install_id": install_id,
        "message": format!("{service_name} installation started"),
    }))))
}

pub async fn install_php(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, agent, claims.sub, &claims.email, "PHP", "/services/install/php").await
}

pub async fn install_certbot(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, agent, claims.sub, &claims.email, "Certbot", "/services/install/certbot").await
}

pub async fn install_ufw(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, agent, claims.sub, &claims.email, "UFW Firewall", "/services/install/ufw").await
}

/// GET /api/services/install/{install_id}/log — SSE stream of install progress.
pub async fn install_log(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Path(install_id): Path<Uuid>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, axum::BoxError>>>, ApiError> {
    let (snapshot, rx) = {
        let logs = state.provision_logs.lock().unwrap_or_else(|e| e.into_inner());
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
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent.get("/ssh-keys").await.map_err(|e| agent_error("SSH keys", e))?;
    Ok(Json(result))
}

pub async fn add_ssh_key(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent.post("/ssh-keys", Some(body)).await.map_err(|e| agent_error("Add SSH key", e))?;
    activity::log_activity(&state.db, claims.sub, &claims.email, "ssh.key.add", Some("system"), None, None, None).await;
    Ok(Json(result))
}

pub async fn remove_ssh_key(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    axum::extract::Path(fingerprint): axum::extract::Path<String>,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent.delete(&format!("/ssh-keys/{fingerprint}")).await.map_err(|e| agent_error("Remove SSH key", e))?;
    activity::log_activity(&state.db, claims.sub, &claims.email, "ssh.key.remove", Some("system"), None, None, None).await;
    Ok(Json(result))
}

// ── Auto-Updates ────────────────────────────────────────────────────────

pub async fn auto_updates_status(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent.get("/auto-updates/status").await.map_err(|e| agent_error("Auto-updates", e))?;
    Ok(Json(result))
}

pub async fn enable_auto_updates(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent.post("/auto-updates/enable", None).await.map_err(|e| agent_error("Enable auto-updates", e))?;
    activity::log_activity(&state.db, claims.sub, &claims.email, "auto-updates.enable", Some("system"), None, None, None).await;
    Ok(Json(result))
}

pub async fn disable_auto_updates(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent.post("/auto-updates/disable", None).await.map_err(|e| agent_error("Disable auto-updates", e))?;
    activity::log_activity(&state.db, claims.sub, &claims.email, "auto-updates.disable", Some("system"), None, None, None).await;
    Ok(Json(result))
}

// ── Panel IP Whitelist ──────────────────────────────────────────────────

pub async fn get_panel_whitelist(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent.get("/panel-whitelist").await.map_err(|e| agent_error("Whitelist", e))?;
    Ok(Json(result))
}

pub async fn set_panel_whitelist(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent.post("/panel-whitelist", Some(body)).await.map_err(|e| agent_error("Set whitelist", e))?;
    activity::log_activity(&state.db, claims.sub, &claims.email, "panel.whitelist.update", Some("system"), None, None, None).await;
    Ok(Json(result))
}

pub async fn install_powerdns(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let install_id = Uuid::new_v4();

    let (tx, _) = broadcast::channel::<ProvisionStep>(32);
    {
        let mut logs = state.provision_logs.lock().unwrap_or_else(|e| e.into_inner());
        logs.insert(install_id, (Vec::new(), tx, Instant::now()));
    }

    let logs = state.provision_logs.clone();
    let db = state.db.clone();
    let jwt_secret = state.config.jwt_secret.clone();
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
                    let encrypted_key = crate::services::secrets_crypto::encrypt_credential(key, &jwt_secret)
                        .unwrap_or_else(|_| key.to_string());
                    let _ = sqlx::query("INSERT INTO settings (key, value, updated_at) VALUES ('pdns_api_key', $1, NOW()) ON CONFLICT (key) DO UPDATE SET value = $1, updated_at = NOW()")
                        .bind(&encrypted_key)
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
        logs.lock().unwrap_or_else(|e| e.into_inner()).remove(&install_id);
    });

    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({
        "install_id": install_id,
        "message": "PowerDNS installation started",
    }))))
}

/// GET /api/system/disk-io — Proxy to agent's disk I/O stats (admin only).
pub async fn disk_io(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = agent
        .get("/system/disk-io")
        .await
        .map_err(|e| agent_error("Disk I/O", e))?;
    Ok(Json(data))
}

pub async fn install_fail2ban(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, agent, claims.sub, &claims.email, "Fail2Ban", "/services/install/fail2ban").await
}

// ── Service uninstallers (proxy to agent, async with SSE progress) ───────

pub async fn uninstall_php(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, agent, claims.sub, &claims.email, "PHP (uninstall)", "/services/uninstall/php").await
}

pub async fn uninstall_certbot(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, agent, claims.sub, &claims.email, "Certbot (uninstall)", "/services/uninstall/certbot").await
}

pub async fn uninstall_ufw(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, agent, claims.sub, &claims.email, "UFW Firewall (uninstall)", "/services/uninstall/ufw").await
}

pub async fn uninstall_fail2ban(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, agent, claims.sub, &claims.email, "Fail2Ban (uninstall)", "/services/uninstall/fail2ban").await
}

pub async fn uninstall_powerdns(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, agent, claims.sub, &claims.email, "PowerDNS (uninstall)", "/services/uninstall/powerdns").await
}

pub async fn uninstall_redis(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, agent, claims.sub, &claims.email, "Redis (uninstall)", "/services/uninstall/redis").await
}

pub async fn uninstall_nodejs(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, agent, claims.sub, &claims.email, "Node.js (uninstall)", "/services/uninstall/nodejs").await
}

pub async fn uninstall_composer(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    install_service_with_log(&state, agent, claims.sub, &claims.email, "Composer (uninstall)", "/services/uninstall/composer").await
}

/// POST /api/traefik/install — Install Traefik reverse proxy.
pub async fn traefik_install(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let acme_email = body.get("acme_email").and_then(|v| v.as_str()).unwrap_or("admin@localhost");

    let result = agent
        .post("/traefik/install", Some(serde_json::json!({ "acme_email": acme_email })))
        .await
        .map_err(|e| agent_error("Traefik install", e))?;

    // Save reverse_proxy setting
    sqlx::query("INSERT INTO settings (key, value, updated_at) VALUES ('reverse_proxy', 'traefik', NOW()) ON CONFLICT (key) DO UPDATE SET value = 'traefik', updated_at = NOW()")
        .execute(&state.db).await.ok();

    activity::log_activity(&state.db, claims.sub, &claims.email, "traefik.install", Some("system"), None, None, None).await;

    Ok(Json(result))
}

/// POST /api/traefik/uninstall — Remove Traefik.
pub async fn traefik_uninstall(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent
        .post("/traefik/uninstall", None)
        .await
        .map_err(|e| agent_error("Traefik uninstall", e))?;

    // Revert to nginx
    sqlx::query("INSERT INTO settings (key, value, updated_at) VALUES ('reverse_proxy', 'nginx', NOW()) ON CONFLICT (key) DO UPDATE SET value = 'nginx', updated_at = NOW()")
        .execute(&state.db).await.ok();

    activity::log_activity(&state.db, claims.sub, &claims.email, "traefik.uninstall", Some("system"), None, None, None).await;

    Ok(Json(result))
}

/// GET /api/traefik/status — Get Traefik status.
pub async fn traefik_status(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent
        .get("/traefik/status")
        .await
        .map_err(|e| agent_error("Traefik status", e))?;

    Ok(Json(result))
}
