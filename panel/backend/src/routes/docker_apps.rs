use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
    Json,
};
use futures::stream::StreamExt;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::auth::ServerScope;
use crate::error::{internal_error, err, agent_error, require_admin, ApiError};
use crate::routes::{is_valid_container_id, is_valid_name};
use crate::routes::sites::ProvisionStep;
use crate::services::activity;
use crate::services::extensions::fire_event;
use crate::AppState;

#[derive(serde::Deserialize)]
pub struct DeployRequest {
    pub template_id: String,
    pub name: String,
    pub port: u16,
    pub env: Option<HashMap<String, String>>,
    pub domain: Option<String>,
    pub ssl_email: Option<String>,
    pub memory_mb: Option<u64>,
    pub cpu_percent: Option<u64>,
    #[serde(default)]
    pub gpu_enabled: bool,
}

/// GET /api/apps/templates — List available app templates.
pub async fn list_templates(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let result = agent
        .get("/apps/templates")
        .await
        .map_err(|e| agent_error("Docker apps", e))?;

    Ok(Json(result))
}

/// POST /api/apps/deploy — Deploy a Docker app from template (async with SSE progress).
pub async fn deploy(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<DeployRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    require_admin(&claims.role)?;

    if !is_valid_name(&body.name) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid app name"));
    }

    if body.port == 0 {
        return Err(err(StatusCode::BAD_REQUEST, "Port must be between 1 and 65535"));
    }

    // Validate env vars: max 50 vars, max 4KB per value
    if let Some(ref env) = body.env {
        if env.len() > 50 {
            return Err(err(StatusCode::BAD_REQUEST, "Too many environment variables (max 50)"));
        }
        for (key, value) in env {
            if key.is_empty() || key.len() > 255 {
                return Err(err(StatusCode::BAD_REQUEST, "Invalid environment variable name"));
            }
            if value.len() > 4096 {
                return Err(err(StatusCode::BAD_REQUEST, "Environment variable value too large (max 4KB)"));
            }
        }
    }

    // ── Container policy enforcement ──
    let policy: Option<(i32, i64, i32, Option<String>)> = sqlx::query_as(
        "SELECT max_containers, max_memory_mb, max_cpu_percent, allowed_images FROM container_policies WHERE user_id = $1"
    )
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("container policy check", e))?;

    if let Some((max_containers, max_memory, max_cpu, allowed_images)) = &policy {
        // Check container count via agent (count dockpanel-managed containers)
        if let Ok(apps_json) = agent.get("/apps").await {
            if let Some(apps) = apps_json.as_array() {
                if apps.len() >= *max_containers as usize {
                    return Err(err(StatusCode::FORBIDDEN, &format!("Container limit reached ({max_containers})")));
                }
            }
        }

        // Enforce memory limit
        if let Some(mem) = body.memory_mb {
            if mem as i64 > *max_memory {
                return Err(err(StatusCode::FORBIDDEN, &format!("Memory exceeds policy limit ({max_memory}MB)")));
            }
        }

        // Enforce CPU limit
        if let Some(cpu) = body.cpu_percent {
            if cpu as i32 > *max_cpu {
                return Err(err(StatusCode::FORBIDDEN, &format!("CPU exceeds policy limit ({max_cpu}%)")));
            }
        }

        // Check allowed images
        if let Some(allowed) = allowed_images {
            if !allowed.is_empty() {
                let allowed_list: Vec<&str> = allowed.split(',').map(|s| s.trim()).collect();
                if !allowed_list.iter().any(|a| body.template_id.contains(a) || a == &"*") {
                    return Err(err(StatusCode::FORBIDDEN, "Image not in allowed list"));
                }
            }
        }
    }

    // Pass user_id to agent for labeling
    let user_id_for_agent = claims.sub.to_string();

    let deploy_id = Uuid::new_v4();

    // Create provisioning channel (reuse the same provision_logs map from AppState)
    let (tx, _) = broadcast::channel::<ProvisionStep>(32);
    {
        let mut logs = state.provision_logs.lock().unwrap_or_else(|e| e.into_inner());
        logs.insert(deploy_id, (Vec::new(), tx, Instant::now()));
    }
    // Track deploy ownership for SSE log access control
    {
        let mut owners = state.deploy_owners.lock().unwrap_or_else(|e| e.into_inner());
        owners.insert(deploy_id, claims.sub);
    }

    let logs = state.provision_logs.clone();
    let agent = agent.clone();
    let db = state.db.clone();
    let user_id = claims.sub;
    let email = claims.email.clone();
    let app_name = body.name.clone();
    let template = body.template_id.clone();

    let deploy_domain = body.domain.clone().filter(|d| !d.is_empty());
    let deploy_ssl_email = body.ssl_email.clone().or_else(|| Some(claims.email.clone()));
    let deploy_memory = body.memory_mb;
    let deploy_cpu = body.cpu_percent;

    // Read reverse proxy preference (nginx or traefik)
    let reverse_proxy: Option<(String,)> = sqlx::query_as(
        "SELECT value FROM settings WHERE key = 'reverse_proxy'"
    ).fetch_optional(&state.db).await
        .map_err(|e| internal_error("docker app reverse proxy setting", e))?;
    let use_traefik = reverse_proxy.map(|(v,)| v == "traefik").unwrap_or(false);

    let mut agent_body = serde_json::json!({
        "template_id": body.template_id,
        "name": body.name,
        "port": body.port,
        "env": body.env.unwrap_or_default(),
        "user_id": user_id_for_agent,
        "gpu_enabled": body.gpu_enabled,
    });
    if let Some(ref domain) = deploy_domain {
        agent_body["domain"] = serde_json::json!(domain);
    }
    if let Some(ref ssl_email) = deploy_ssl_email {
        if deploy_domain.is_some() {
            agent_body["ssl_email"] = serde_json::json!(ssl_email);
        }
    }
    if let Some(mem) = deploy_memory {
        agent_body["memory_mb"] = serde_json::json!(mem);
    }
    if let Some(cpu) = deploy_cpu {
        agent_body["cpu_percent"] = serde_json::json!(cpu);
    }
    if use_traefik {
        agent_body["use_traefik"] = serde_json::json!(true);
    }

    // Spawn background deploy task
    tokio::spawn(async move {
        let emit = |step: &str, label: &str, status: &str, msg: Option<String>| {
            let ev = ProvisionStep {
                step: step.into(),
                label: label.into(),
                status: status.into(),
                message: msg,
            };
            if let Ok(mut map) = logs.lock() {
                if let Some((history, tx, _)) = map.get_mut(&deploy_id) {
                    history.push(ev.clone());
                    let _ = tx.send(ev);
                }
            }
        };

        // Step 1: Auto-create DNS record if domain is provided
        if let Some(ref domain) = deploy_domain {
            emit("dns", "Creating DNS record", "in_progress", None);

            // Extract parent domain (e.g., "mail.dockpanel.dev" → "dockpanel.dev")
            let parts: Vec<&str> = domain.splitn(3, '.').collect();
            let parent_domain = if parts.len() >= 3 {
                format!("{}.{}", parts[parts.len() - 2], parts[parts.len() - 1])
            } else {
                domain.clone()
            };

            // Look up DNS zone for this domain
            let zone: Option<(Uuid, String, Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
                "SELECT id, provider, cf_zone_id, cf_api_token, cf_api_email FROM dns_zones WHERE domain = $1 AND user_id = $2"
            )
            .bind(&parent_domain)
            .bind(user_id)
            .fetch_optional(&db)
            .await
            .ok()
            .flatten();

            if let Some((_zone_id, provider, cf_zone_id, cf_api_token, cf_api_email)) = zone {
                let server_ip = crate::helpers::detect_public_ip().await;

                if provider == "cloudflare" {
                    if let (Some(zone_id), Some(token)) = (cf_zone_id, cf_api_token) {
                        let client = reqwest::Client::new();
                        let headers = crate::helpers::cf_headers(&token, cf_api_email.as_deref());

                        let result = client
                            .post(&format!("https://api.cloudflare.com/client/v4/zones/{zone_id}/dns_records"))
                            .headers(headers)
                            .json(&serde_json::json!({
                                "type": "A",
                                "name": domain,
                                "content": server_ip,
                                "proxied": true,
                                "ttl": 1,
                            }))
                            .send()
                            .await;

                        match result {
                            Ok(resp) => {
                                let body = resp.json::<serde_json::Value>().await.ok();
                                let success = body.as_ref().and_then(|b| b.get("success")).and_then(|v| v.as_bool()).unwrap_or(false);
                                if success {
                                    emit("dns", "Creating DNS record", "done", None);
                                    tracing::info!("Auto-DNS: created A record {domain} → {server_ip}");
                                } else {
                                    let err_msg = body.as_ref()
                                        .and_then(|b| b.get("errors"))
                                        .and_then(|e| e.as_array())
                                        .and_then(|a| a.first())
                                        .and_then(|e| e.get("message"))
                                        .and_then(|m| m.as_str())
                                        .unwrap_or("Unknown error");
                                    emit("dns", "Creating DNS record", "error",
                                        Some(format!("DNS failed: {err_msg} — create manually")));
                                    tracing::warn!("Auto-DNS failed for {domain}: {err_msg}");
                                }
                            }
                            Err(e) => {
                                emit("dns", "Creating DNS record", "error",
                                    Some(format!("DNS API error: {e} — create manually")));
                            }
                        }
                    }
                }
                else if provider == "powerdns" {
                    // Get PowerDNS settings
                    let pdns: Vec<(String, String)> = sqlx::query_as(
                        "SELECT key, value FROM settings WHERE key IN ('pdns_api_url', 'pdns_api_key')"
                    ).fetch_all(&db).await.unwrap_or_default();
                    let pdns_url = pdns.iter().find(|(k,_)| k == "pdns_api_url").map(|(_,v)| v.clone());
                    let pdns_key_enc = pdns.iter().find(|(k,_)| k == "pdns_api_key").map(|(_,v)| v.clone());

                    if let (Some(url), Some(key_enc)) = (pdns_url, pdns_key_enc) {
                        let key = crate::services::secrets_crypto::decrypt_credential_from_env(&key_enc);
                        let client = reqwest::Client::new();
                        let zone_fqdn = if parent_domain.ends_with('.') { parent_domain.clone() } else { format!("{parent_domain}.") };

                        let result = client
                            .patch(&format!("{url}/api/v1/servers/localhost/zones/{zone_fqdn}"))
                            .header("X-API-Key", &key)
                            .json(&serde_json::json!({
                                "rrsets": [{
                                    "name": format!("{domain}."),
                                    "type": "A",
                                    "ttl": 300,
                                    "changetype": "REPLACE",
                                    "records": [{ "content": server_ip, "disabled": false }]
                                }]
                            }))
                            .send()
                            .await;

                        match result {
                            Ok(resp) if resp.status().is_success() => {
                                emit("dns", "Creating DNS record", "done", None);
                                tracing::info!("Auto-DNS (PowerDNS): created A record {domain} → {server_ip}");
                            }
                            Ok(resp) => {
                                let text = resp.text().await.unwrap_or_default();
                                emit("dns", "Creating DNS record", "error",
                                    Some(format!("PowerDNS error: {text} — create manually")));
                            }
                            Err(e) => {
                                emit("dns", "Creating DNS record", "error",
                                    Some(format!("PowerDNS API error: {e} — create manually")));
                            }
                        }
                    } else {
                        emit("dns", "Creating DNS record", "error",
                            Some("PowerDNS not configured — create record manually".into()));
                    }
                }
            } else {
                emit("dns", "Creating DNS record", "error",
                    Some(format!("No DNS zone found for {parent_domain} — create record manually")));
            }
        }

        // Step 2: Pull image + deploy container (+ proxy + SSL handled by agent)
        emit("pull", "Pulling Docker image", "in_progress", None);

        match agent.post("/apps/deploy", Some(agent_body)).await {
            Ok(result) => {
                emit("pull", "Pulling Docker image", "done", None);
                emit("start", "Starting container", "done", None);

                // Check if proxy/SSL were set up
                if deploy_domain.is_some() {
                    let has_proxy = result.get("proxy").is_some();
                    let has_ssl = result.get("ssl").and_then(|v| v.as_bool()).unwrap_or(false);
                    if has_proxy {
                        emit("proxy", "Configuring reverse proxy", "done", None);
                    }
                    if has_ssl {
                        emit("ssl", "Provisioning SSL certificate", "done", None);
                    } else if has_proxy {
                        emit("ssl", "SSL certificate", "error",
                            Some("Skipped — can be provisioned later".into()));
                    }
                }

                emit("complete", "App deployed", "done", None);

                tracing::info!("App deployed: {} ({}){}", app_name, template,
                    deploy_domain.as_ref().map(|d| format!(" → {d}")).unwrap_or_default());
                activity::log_activity(
                    &db, user_id, &email, "app.deploy",
                    Some("app"), Some(&app_name), Some(&template), None,
                ).await;

                crate::services::extensions::fire_event(&db, "app.deployed", serde_json::json!({
                    "name": app_name, "domain": deploy_domain,
                }));

                // GAP 12: Auto-create monitor for Docker app with domain
                if let Some(ref domain) = deploy_domain {
                    let url = format!("https://{domain}");
                    let _ = sqlx::query(
                        "INSERT INTO monitors (user_id, url, name, check_interval, status, enabled, monitor_type) \
                         VALUES ($1, $2, $3, 60, 'pending', TRUE, 'http') ON CONFLICT DO NOTHING"
                    )
                    .bind(user_id).bind(&url).bind(&format!("{app_name} ({domain})"))
                    .execute(&db).await;

                    // Auto-create status page component
                    let _ = sqlx::query(
                        "INSERT INTO status_page_components (user_id, name, description, group_name) \
                         SELECT $1, $2, $3, 'Docker Apps' WHERE EXISTS (SELECT 1 FROM status_page_config WHERE user_id = $1 AND enabled = TRUE)"
                    )
                    .bind(user_id).bind(&app_name)
                    .bind(format!("Docker app: {app_name}"))
                    .execute(&db).await;
                }
            }
            Err(e) => {
                emit("pull", "Pulling Docker image", "error", Some(format!("Deploy failed: {e}")));
                emit("complete", "Deploy failed", "error", None);
                tracing::error!("App deploy failed: {} ({}): {e}", app_name, template);

                crate::services::system_log::log_event(
                    &db,
                    "error",
                    "api",
                    &format!("App deploy failed: {} ({})", app_name, template),
                    Some(&e.to_string()),
                ).await;
            }
        }

        tokio::time::sleep(Duration::from_secs(30)).await;
        logs.lock().unwrap_or_else(|e| e.into_inner()).remove(&deploy_id);
    });

    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({
        "deploy_id": deploy_id,
        "message": "Deployment started",
    }))))
}

/// GET /api/apps/deploy/{deploy_id}/log — SSE stream of deploy progress.
pub async fn deploy_log(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(deploy_id): Path<Uuid>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, axum::BoxError>>>, ApiError> {
    // Verify the requesting user owns this deploy (or is admin)
    if claims.role != "admin" {
        let owners = state.deploy_owners.lock().unwrap_or_else(|e| e.into_inner());
        if owners.get(&deploy_id) != Some(&claims.sub) {
            return Err(err(StatusCode::FORBIDDEN, "Not your deploy"));
        }
    }

    let (snapshot, rx) = {
        let logs = state.provision_logs.lock().unwrap_or_else(|e| e.into_inner());
        match logs.get(&deploy_id) {
            Some((history, tx, _)) => (history.clone(), Some(tx.subscribe())),
            None => (Vec::new(), None),
        }
    };

    let rx = rx.ok_or_else(|| err(StatusCode::NOT_FOUND, "No active deploy"))?;

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

/// GET /api/apps — List deployed Docker apps.
pub async fn list_apps(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let result = agent
        .get("/apps")
        .await
        .map_err(|e| agent_error("Docker apps", e))?;

    Ok(Json(result))
}

/// POST /api/apps/{container_id}/stop — Stop an app.
pub async fn stop_app(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }

    let agent_path = format!("/apps/{}/stop", container_id);
    agent
        .post(&agent_path, None)
        .await
        .map_err(|e| agent_error("Container stop", e))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/apps/{container_id}/start — Start an app.
pub async fn start_app(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }

    let agent_path = format!("/apps/{}/start", container_id);
    agent
        .post(&agent_path, None)
        .await
        .map_err(|e| agent_error("Container start", e))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/apps/{container_id}/restart — Restart an app.
pub async fn restart_app(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }

    let agent_path = format!("/apps/{}/restart", container_id);
    agent
        .post(&agent_path, None)
        .await
        .map_err(|e| agent_error("Container restart", e))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /api/apps/{container_id}/logs — Get app logs.
pub async fn app_logs(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }

    let agent_path = format!("/apps/{}/logs", container_id);
    let result = agent
        .get(&agent_path)
        .await
        .map_err(|e| agent_error("Container logs", e))?;

    Ok(Json(result))
}

/// POST /api/apps/{container_id}/update — Pull latest image and recreate container (async with SSE).
pub async fn update_app(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(container_id): Path<String>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    require_admin(&claims.role)?;
    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }

    let deploy_id = Uuid::new_v4();

    let (tx, _) = broadcast::channel::<ProvisionStep>(32);
    {
        let mut logs = state.provision_logs.lock().unwrap_or_else(|e| e.into_inner());
        logs.insert(deploy_id, (Vec::new(), tx, Instant::now()));
    }
    {
        let mut owners = state.deploy_owners.lock().unwrap_or_else(|e| e.into_inner());
        owners.insert(deploy_id, claims.sub);
    }

    let logs = state.provision_logs.clone();
    let agent = agent.clone();
    let db = state.db.clone();
    let user_id = claims.sub;
    let email = claims.email.clone();
    let cid = container_id.clone();

    tokio::spawn(async move {
        let emit = |step: &str, label: &str, status: &str, msg: Option<String>| {
            let ev = ProvisionStep {
                step: step.into(), label: label.into(), status: status.into(), message: msg,
            };
            if let Ok(mut map) = logs.lock() {
                if let Some((history, tx, _)) = map.get_mut(&deploy_id) {
                    history.push(ev.clone());
                    let _ = tx.send(ev);
                }
            }
        };

        emit("pull", "Pulling latest image", "in_progress", None);

        let agent_path = format!("/apps/{}/update", cid);
        match agent.post(&agent_path, None).await {
            Ok(result) => {
                let blue_green = result
                    .get("blue_green")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                emit("pull", "Pulling latest image", "done", None);
                if blue_green {
                    emit("health", "Health check passed", "done", None);
                    emit("swap", "Traffic swapped (zero-downtime)", "done", None);
                    emit("cleanup", "Old container removed", "done", None);
                    emit("complete", "App updated (zero-downtime)", "done", None);
                } else {
                    emit("recreate", "Recreating container", "done", None);
                    emit("complete", "App updated", "done", None);
                }
                activity::log_activity(
                    &db, user_id, &email, "app.update",
                    Some("app"), Some(&cid), None, None,
                ).await;
                tracing::info!(
                    "App updated{}: {cid}",
                    if blue_green { " (blue-green)" } else { "" }
                );
            }
            Err(e) => {
                emit("pull", "Pulling latest image", "error", Some(format!("{e}")));
                emit("complete", "Update failed", "error", None);
                tracing::error!("App update failed: {cid}: {e}");
            }
        }

        tokio::time::sleep(Duration::from_secs(60)).await;
        logs.lock().unwrap_or_else(|e| e.into_inner()).remove(&deploy_id);
    });

    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({
        "deploy_id": deploy_id,
        "message": "Update started",
    }))))
}

/// GET /api/apps/{container_id}/env — Get container environment variables.
pub async fn app_env(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }

    let agent_path = format!("/apps/{}/env", container_id);
    let result = agent
        .get(&agent_path)
        .await
        .map_err(|e| agent_error("Container env", e))?;

    Ok(Json(result))
}

/// PUT /api/apps/{container_id}/env — Update container environment variables.
pub async fn update_env(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(container_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }
    let result = agent
        .put(&format!("/apps/{container_id}/env"), body)
        .await
        .map_err(|e| agent_error("Update env", e))?;
    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "app.update_env",
        Some("app"),
        Some(&container_id),
        None,
        None,
    )
    .await;
    Ok(Json(result))
}

/// GET /api/apps/{container_id}/stats — Get container resource stats.
pub async fn container_stats(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }
    let result = agent
        .get(&format!("/apps/{container_id}/stats"))
        .await
        .map_err(|e| agent_error("Container stats", e))?;
    Ok(Json(result))
}

/// PUT /api/apps/{container_id}/image — Change Docker app image tag.
pub async fn update_image(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(container_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }

    let image = body.get("image")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();

    if image.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "image is required (e.g., postgres:17)"));
    }

    // Validate image format: allow alphanumeric, dots, dashes, slashes, colons, underscores
    if image.len() > 256 || image.contains(' ') || image.contains('\0') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid image format"));
    }

    let result = agent
        .post(
            &format!("/apps/{container_id}/change-image"),
            Some(serde_json::json!({ "image": image })),
        )
        .await
        .map_err(|e| agent_error("Change image", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "app.change_image",
        Some("app"), Some(&container_id), Some(image), None,
    ).await;

    tracing::info!("App image changed: {container_id} → {image}");
    Ok(Json(result))
}

/// PUT /api/apps/{container_id}/limits — Update CPU/memory limits on a running container.
pub async fn update_limits(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(container_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }

    let memory_mb = body.get("memory_mb").and_then(|v| v.as_u64());
    let cpu_percent = body.get("cpu_percent").and_then(|v| v.as_u64());

    if memory_mb.is_none() && cpu_percent.is_none() {
        return Err(err(StatusCode::BAD_REQUEST, "At least one of memory_mb or cpu_percent is required"));
    }

    if let Some(mem) = memory_mb {
        if mem < 4 || mem > 65536 {
            return Err(err(StatusCode::BAD_REQUEST, "memory_mb must be between 4 and 65536"));
        }
    }

    if let Some(cpu) = cpu_percent {
        if cpu == 0 || cpu > 10000 {
            return Err(err(StatusCode::BAD_REQUEST, "cpu_percent must be between 1 and 10000"));
        }
    }

    let result = agent
        .post(
            &format!("/apps/{container_id}/update-limits"),
            Some(serde_json::json!({
                "memory_mb": memory_mb,
                "cpu_percent": cpu_percent,
            })),
        )
        .await
        .map_err(|e| agent_error("Update limits", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "app.update_limits",
        Some("app"), Some(&container_id), None, None,
    ).await;

    tracing::info!("App limits updated: {container_id} (mem: {:?}MB, cpu: {:?}%)", memory_mb, cpu_percent);
    Ok(Json(result))
}

/// GET /api/apps/{container_id}/shell-info — Get shell availability.
pub async fn shell_info(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }
    let result = agent
        .get(&format!("/apps/{container_id}/shell-info"))
        .await
        .map_err(|e| agent_error("Shell info", e))?;
    Ok(Json(result))
}

/// POST /api/apps/{container_id}/exec — Execute a command inside a container.
pub async fn exec_command(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(container_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }
    let result = agent
        .post(&format!("/apps/{container_id}/exec"), Some(body))
        .await
        .map_err(|e| agent_error("Container exec", e))?;
    Ok(Json(result))
}

/// GET /api/apps/{container_id}/volumes — Get volume info and sizes.
pub async fn container_volumes(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }
    let result = agent
        .get(&format!("/apps/{container_id}/volumes"))
        .await
        .map_err(|e| agent_error("Container volumes", e))?;
    Ok(Json(result))
}

/// POST /api/apps/registry-login — Login to a private Docker registry.
pub async fn registry_login(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let result = agent
        .post("/apps/registry-login", Some(body))
        .await
        .map_err(|e| agent_error("Registry login", e))?;
    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "app.registry_login",
        Some("registry"),
        None,
        None,
        None,
    )
    .await;
    Ok(Json(result))
}

/// GET /api/apps/registries — List configured registries.
pub async fn list_registries(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let result = agent
        .get("/apps/registries")
        .await
        .map_err(|e| agent_error("List registries", e))?;
    Ok(Json(result))
}

/// POST /api/apps/registry-logout — Logout from a registry.
pub async fn registry_logout(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let result = agent
        .post("/apps/registry-logout", Some(body))
        .await
        .map_err(|e| agent_error("Registry logout", e))?;
    Ok(Json(result))
}

/// GET /api/apps/images — List Docker images.
pub async fn list_images(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let result = agent
        .get("/apps/images")
        .await
        .map_err(|e| agent_error("Docker images", e))?;
    Ok(Json(result))
}

/// POST /api/apps/images/prune — Remove unused Docker images.
pub async fn prune_images(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let result = agent
        .post("/apps/images/prune", None)
        .await
        .map_err(|e| agent_error("Prune images", e))?;
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "app.prune_images",
        Some("docker"), None, None, None,
    ).await;
    Ok(Json(result))
}

/// DELETE /api/apps/images/{id} — Remove a specific Docker image.
pub async fn remove_image(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let result = agent
        .delete(&format!("/apps/images/{id}"))
        .await
        .map_err(|e| agent_error("Remove image", e))?;
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "app.remove_image",
        Some("docker"), Some(&id), None, None,
    ).await;
    Ok(Json(result))
}

/// POST /api/apps/{container_id}/snapshot — Commit container to image.
pub async fn snapshot_container(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(container_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }
    let result = agent
        .post(&format!("/apps/{container_id}/snapshot"), Some(body))
        .await
        .map_err(|e| agent_error("Container snapshot", e))?;
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "app.snapshot",
        Some("app"), Some(&container_id), None, None,
    ).await;
    Ok(Json(result))
}

/// POST /api/apps/compose/validate — Validate compose YAML with detailed feedback.
pub async fn compose_validate(
    State(_state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let yaml = body["yaml"]
        .as_str()
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing 'yaml' field"))?;

    if yaml.len() > 65536 {
        return Err(err(StatusCode::BAD_REQUEST, "YAML too large (max 64KB)"));
    }

    let result = agent
        .post("/apps/compose/validate", Some(serde_json::json!({ "yaml": yaml })))
        .await
        .map_err(|e| agent_error("Compose validate", e))?;

    Ok(Json(result))
}

/// POST /api/apps/compose/parse — Parse docker-compose.yml and preview services.
pub async fn compose_parse(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let yaml = body["yaml"]
        .as_str()
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing 'yaml' field"))?;

    if yaml.len() > 65536 {
        return Err(err(StatusCode::BAD_REQUEST, "YAML too large (max 64KB)"));
    }

    let result = agent
        .post("/apps/compose/parse", Some(serde_json::json!({ "yaml": yaml })))
        .await
        .map_err(|e| agent_error("Compose parse", e))?;

    Ok(Json(result))
}

/// POST /api/apps/compose/deploy — Deploy services from docker-compose.yml.
pub async fn compose_deploy(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    require_admin(&claims.role)?;

    let yaml = body["yaml"]
        .as_str()
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing 'yaml' field"))?;

    if yaml.len() > 65536 {
        return Err(err(StatusCode::BAD_REQUEST, "YAML too large (max 64KB)"));
    }

    // Validate Compose YAML for container escape vectors
    super::validate_compose_yaml(yaml)
        .map_err(|e| err(StatusCode::BAD_REQUEST, e))?;

    let result = agent
        .post("/apps/compose/deploy", Some(serde_json::json!({ "yaml": yaml })))
        .await
        .map_err(|e| agent_error("Docker deploy", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "app.compose_deploy",
        Some("app"), None, Some("compose"), None,
    ).await;

    Ok((StatusCode::CREATED, Json(result)))
}

/// DELETE /api/apps/{container_id} — Remove a deployed app.
pub async fn remove_app(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }

    let agent_path = format!("/apps/{}", container_id);
    let result = agent
        .delete(&agent_path)
        .await
        .map_err(|e| agent_error("Container removal", e))?;

    // Auto-cleanup DNS record if a domain was removed
    if let Some(domain_removed) = result.get("domain_removed").and_then(|v| v.as_str()) {
        let dns_domain = domain_removed.to_string();
        let dns_db = state.db.clone();
        let dns_user = claims.sub;
        tokio::spawn(async move {
            // Extract parent domain
            let parts: Vec<&str> = dns_domain.splitn(3, '.').collect();
            let parent = if parts.len() >= 3 {
                format!("{}.{}", parts[parts.len() - 2], parts[parts.len() - 1])
            } else {
                dns_domain.clone()
            };

            let zone: Option<(String, Option<String>, Option<String>, Option<String>)> = match sqlx::query_as(
                "SELECT provider, cf_zone_id, cf_api_token, cf_api_email FROM dns_zones WHERE domain = $1 AND user_id = $2"
            ).bind(&parent).bind(dns_user).fetch_optional(&dns_db).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("DB error fetching DNS zone for docker app cleanup: {e}");
                    None
                }
            };

            if let Some((provider, cf_zone_id, cf_api_token, cf_api_email)) = zone {
                let server_ip = crate::helpers::detect_public_ip().await;

                if provider == "cloudflare" {
                    if let (Some(zid), Some(tok)) = (cf_zone_id, cf_api_token) {
                        let client = reqwest::Client::new();
                        let headers = crate::helpers::cf_headers(&tok, cf_api_email.as_deref());
                        // Find the A record for this domain
                        if let Ok(resp) = client.get(&format!("https://api.cloudflare.com/client/v4/zones/{zid}/dns_records?type=A&name={dns_domain}"))
                            .headers(headers.clone()).send().await {
                            if let Ok(data) = resp.json::<serde_json::Value>().await {
                                if let Some(records) = data.get("result").and_then(|r| r.as_array()) {
                                    for record in records {
                                        if let (Some(rid), Some(content)) = (record.get("id").and_then(|v| v.as_str()), record.get("content").and_then(|v| v.as_str())) {
                                            if content == server_ip {
                                                let _ = client.delete(&format!("https://api.cloudflare.com/client/v4/zones/{zid}/dns_records/{rid}"))
                                                    .headers(headers.clone()).send().await;
                                                tracing::info!("Auto-DNS cleanup: deleted A record for app domain {dns_domain}");
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else if provider == "powerdns" {
                    let pdns: Vec<(String, String)> = sqlx::query_as(
                        "SELECT key, value FROM settings WHERE key IN ('pdns_api_url', 'pdns_api_key')"
                    ).fetch_all(&dns_db).await.unwrap_or_default();
                    let purl = pdns.iter().find(|(k,_)| k == "pdns_api_url").map(|(_,v)| v.clone());
                    let pkey_enc = pdns.iter().find(|(k,_)| k == "pdns_api_key").map(|(_,v)| v.clone());
                    if let (Some(url), Some(key_enc)) = (purl, pkey_enc) {
                        let key = crate::services::secrets_crypto::decrypt_credential_from_env(&key_enc);
                        let zfqdn = if parent.ends_with('.') { parent } else { format!("{parent}.") };
                        let _ = reqwest::Client::new()
                            .patch(&format!("{url}/api/v1/servers/localhost/zones/{zfqdn}"))
                            .header("X-API-Key", &key)
                            .json(&serde_json::json!({"rrsets":[{"name":format!("{dns_domain}."),"type":"A","ttl":300,"changetype":"DELETE","records":[]}]}))
                            .send().await;
                        tracing::info!("Auto-DNS cleanup (PowerDNS): deleted A record for app domain {dns_domain}");
                    }
                }
            }
        });
    }

    tracing::info!("App removed: {}", container_id);
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "app.remove",
        Some("app"), Some(&container_id), None, None,
    ).await;

    fire_event(&state.db, "app.removed", serde_json::json!({
        "container_id": container_id,
    }));

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /api/apps/updates — Check all containers for available image updates.
pub async fn check_updates(
    State(_state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let result = agent
        .get("/apps/update-check")
        .await
        .map_err(|e| agent_error("Update check", e))?;
    Ok(Json(result))
}

/// GET /api/apps/gpu-info — Get GPU availability information from the server.
pub async fn gpu_info(
    State(_state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let result = agent.get("/apps/gpu-info").await
        .map_err(|e| agent_error("GPU info", e))?;
    Ok(Json(result))
}

// ─── Container Isolation Policies (Admin) ──────────────────────

#[derive(serde::Deserialize)]
pub struct PolicyRequest {
    pub user_id: Option<Uuid>,
    pub max_containers: Option<i32>,
    pub max_memory_mb: Option<i64>,
    pub max_cpu_percent: Option<i32>,
    pub network_isolation: Option<bool>,
    pub allowed_images: Option<String>,
}

/// GET /api/container-policies — List all container policies.
pub async fn list_policies(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let policies: Vec<(Uuid, Uuid, i32, i64, i32, bool, Option<String>, chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as(
            "SELECT cp.id, cp.user_id, cp.max_containers, cp.max_memory_mb, cp.max_cpu_percent, \
             cp.network_isolation, cp.allowed_images, cp.created_at, cp.updated_at \
             FROM container_policies cp ORDER BY cp.created_at DESC"
        )
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("list container policies", e))?;

    // Also fetch user emails for display
    let items: Vec<serde_json::Value> = {
        let mut result = Vec::with_capacity(policies.len());
        for (id, uid, max_c, max_m, max_cpu, net_iso, allowed, created, updated) in &policies {
            let email: Option<(String,)> = sqlx::query_as("SELECT email FROM users WHERE id = $1")
                .bind(uid)
                .fetch_optional(&state.db)
                .await
                .ok()
                .flatten();
            result.push(serde_json::json!({
                "id": id,
                "user_id": uid,
                "user_email": email.map(|e| e.0),
                "max_containers": max_c,
                "max_memory_mb": max_m,
                "max_cpu_percent": max_cpu,
                "network_isolation": net_iso,
                "allowed_images": allowed,
                "created_at": created,
                "updated_at": updated,
            }));
        }
        result
    };

    Ok(Json(serde_json::json!({ "policies": items })))
}

/// POST /api/container-policies — Create a container policy for a user.
pub async fn create_policy(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<PolicyRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    require_admin(&claims.role)?;

    let user_id = body.user_id.ok_or_else(|| err(StatusCode::BAD_REQUEST, "user_id is required"))?;
    let max_containers = body.max_containers.unwrap_or(10).max(1).min(1000);
    let max_memory = body.max_memory_mb.unwrap_or(4096).max(128).min(1_048_576);
    let max_cpu = body.max_cpu_percent.unwrap_or(400).max(10).min(10000);
    let net_iso = body.network_isolation.unwrap_or(false);

    // Validate allowed_images (comma-separated, max 4KB)
    if let Some(ref imgs) = body.allowed_images {
        if imgs.len() > 4096 {
            return Err(err(StatusCode::BAD_REQUEST, "allowed_images too long"));
        }
    }

    // Verify user exists
    let user_exists: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("verify user", e))?;
    if user_exists.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "User not found"));
    }

    let id: (Uuid,) = sqlx::query_as(
        "INSERT INTO container_policies (user_id, max_containers, max_memory_mb, max_cpu_percent, network_isolation, allowed_images) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (user_id) DO UPDATE SET \
         max_containers = EXCLUDED.max_containers, max_memory_mb = EXCLUDED.max_memory_mb, \
         max_cpu_percent = EXCLUDED.max_cpu_percent, network_isolation = EXCLUDED.network_isolation, \
         allowed_images = EXCLUDED.allowed_images, updated_at = NOW() \
         RETURNING id"
    )
    .bind(user_id)
    .bind(max_containers)
    .bind(max_memory)
    .bind(max_cpu)
    .bind(net_iso)
    .bind(&body.allowed_images)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("create container policy", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "container_policy.created",
        Some("container_policy"), Some(&user_id.to_string()), None, None,
    ).await;

    Ok((StatusCode::CREATED, Json(serde_json::json!({ "ok": true, "id": id.0 }))))
}

/// GET /api/container-policies/{user_id} — Get policy for a specific user.
pub async fn get_policy(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(user_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Users can see their own policy; admins can see any
    if claims.sub != user_id {
        require_admin(&claims.role)?;
    }

    let policy: Option<(Uuid, i32, i64, i32, bool, Option<String>, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as(
            "SELECT id, max_containers, max_memory_mb, max_cpu_percent, network_isolation, allowed_images, updated_at \
             FROM container_policies WHERE user_id = $1"
        )
        .bind(user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("get container policy", e))?;

    match policy {
        Some((id, max_c, max_m, max_cpu, net_iso, allowed, updated)) => {
            Ok(Json(serde_json::json!({
                "id": id,
                "user_id": user_id,
                "max_containers": max_c,
                "max_memory_mb": max_m,
                "max_cpu_percent": max_cpu,
                "network_isolation": net_iso,
                "allowed_images": allowed,
                "updated_at": updated,
            })))
        }
        None => Ok(Json(serde_json::json!({ "policy": null }))),
    }
}

/// PUT /api/container-policies/{user_id} — Update a user's container policy.
pub async fn update_policy(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(user_id): Path<Uuid>,
    Json(body): Json<PolicyRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let result = sqlx::query(
        "UPDATE container_policies SET \
         max_containers = COALESCE($1, max_containers), \
         max_memory_mb = COALESCE($2, max_memory_mb), \
         max_cpu_percent = COALESCE($3, max_cpu_percent), \
         network_isolation = COALESCE($4, network_isolation), \
         allowed_images = COALESCE($5, allowed_images), \
         updated_at = NOW() \
         WHERE user_id = $6"
    )
    .bind(body.max_containers.map(|v| v.max(1).min(1000)))
    .bind(body.max_memory_mb.map(|v| v.max(128).min(1_048_576)))
    .bind(body.max_cpu_percent.map(|v| v.max(10).min(10000)))
    .bind(body.network_isolation)
    .bind(&body.allowed_images)
    .bind(user_id)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("update container policy", e))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Policy not found for this user"));
    }

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "container_policy.updated",
        Some("container_policy"), Some(&user_id.to_string()), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// DELETE /api/container-policies/{user_id} — Remove a user's container policy (reverts to no limits).
pub async fn delete_policy(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(user_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let result = sqlx::query("DELETE FROM container_policies WHERE user_id = $1")
        .bind(user_id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("delete container policy", e))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Policy not found"));
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /api/container-policies/{user_id}/usage — Get current resource usage vs policy.
pub async fn policy_usage(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(user_id): Path<Uuid>,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    if claims.sub != user_id {
        require_admin(&claims.role)?;
    }

    let policy: Option<(i32, i64, i32, bool)> = sqlx::query_as(
        "SELECT max_containers, max_memory_mb, max_cpu_percent, network_isolation FROM container_policies WHERE user_id = $1"
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("policy usage", e))?;

    // Count containers via agent
    let container_count = match agent.get("/apps").await {
        Ok(apps_json) => apps_json.as_array().map(|a| a.len()).unwrap_or(0),
        Err(_) => 0,
    };

    match policy {
        Some((max_c, max_m, max_cpu, net_iso)) => {
            Ok(Json(serde_json::json!({
                "containers": { "used": container_count, "max": max_c },
                "memory_mb": { "max": max_m },
                "cpu_percent": { "max": max_cpu },
                "network_isolation": net_iso,
                "has_policy": true,
            })))
        }
        None => {
            Ok(Json(serde_json::json!({
                "containers": { "used": container_count, "max": null },
                "has_policy": false,
            })))
        }
    }
}

// ─── Container Auto-Sleep / Scale to Zero ──────────────────────

#[derive(serde::Deserialize)]
pub struct SleepConfigRequest {
    pub auto_sleep_enabled: Option<bool>,
    pub sleep_after_minutes: Option<i32>,
}

/// GET /api/apps/{container_id}/sleep-config — Get sleep configuration for a container.
pub async fn get_sleep_config(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }

    let config: Option<(bool, i32, bool, Option<chrono::DateTime<chrono::Utc>>, Option<chrono::DateTime<chrono::Utc>>, i32)> =
        sqlx::query_as(
            "SELECT auto_sleep_enabled, sleep_after_minutes, is_sleeping, last_slept_at, last_woken_at, total_sleeps \
             FROM container_sleep_config WHERE container_id = $1"
        )
        .bind(&container_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("get sleep config", e))?;

    match config {
        Some((enabled, minutes, sleeping, slept_at, woken_at, total)) => {
            Ok(Json(serde_json::json!({
                "auto_sleep_enabled": enabled,
                "sleep_after_minutes": minutes,
                "is_sleeping": sleeping,
                "last_slept_at": slept_at,
                "last_woken_at": woken_at,
                "total_sleeps": total,
            })))
        }
        None => Ok(Json(serde_json::json!({
            "auto_sleep_enabled": false,
            "sleep_after_minutes": 30,
            "is_sleeping": false,
        }))),
    }
}

/// PUT /api/apps/{container_id}/sleep-config — Update sleep configuration.
pub async fn update_sleep_config(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(container_id): Path<String>,
    Json(body): Json<SleepConfigRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }

    let minutes = body.sleep_after_minutes.unwrap_or(30).max(5).min(1440);
    let enabled = body.auto_sleep_enabled.unwrap_or(false);

    // Resolve container name and domain from agent
    let mut container_name = container_id.clone();
    let mut domain: Option<String> = None;
    if let Ok(apps) = agent.get("/apps").await {
        if let Some(apps_arr) = apps.as_array() {
            for app in apps_arr {
                if app.get("container_id").and_then(|v| v.as_str()) == Some(&container_id) {
                    container_name = app.get("name").and_then(|v| v.as_str()).unwrap_or(&container_id).to_string();
                    domain = app.get("domain").and_then(|v| v.as_str()).map(String::from);
                    break;
                }
            }
        }
    }

    sqlx::query(
        "INSERT INTO container_sleep_config (container_id, container_name, domain, auto_sleep_enabled, sleep_after_minutes, last_activity_at) \
         VALUES ($1, $2, $3, $4, $5, NOW()) \
         ON CONFLICT (container_id) DO UPDATE SET \
         auto_sleep_enabled = $4, sleep_after_minutes = $5, container_name = $2, domain = $3, updated_at = NOW()"
    )
    .bind(&container_id)
    .bind(&container_name)
    .bind(&domain)
    .bind(enabled)
    .bind(minutes)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("update sleep config", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email,
        if enabled { "container.auto_sleep_enabled" } else { "container.auto_sleep_disabled" },
        Some("container"), Some(&container_name), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/apps/{container_id}/wake — Wake a sleeping container.
pub async fn wake_container(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }

    // Start the container
    agent.post(&format!("/apps/{container_id}/start"), None::<serde_json::Value>)
        .await
        .map_err(|e| agent_error("Wake container", e))?;

    // Update sleep state
    sqlx::query(
        "UPDATE container_sleep_config SET is_sleeping = false, last_woken_at = NOW(), \
         last_activity_at = NOW(), updated_at = NOW() WHERE container_id = $1"
    )
    .bind(&container_id)
    .execute(&state.db)
    .await
    .ok();

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "container.wake",
        Some("container"), Some(&container_id), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/apps/{container_id}/sleep — Manually sleep a container.
pub async fn sleep_container(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }

    agent.post(&format!("/apps/{container_id}/stop"), None::<serde_json::Value>)
        .await
        .map_err(|e| agent_error("Sleep container", e))?;

    sqlx::query(
        "UPDATE container_sleep_config SET is_sleeping = true, last_slept_at = NOW(), \
         total_sleeps = total_sleeps + 1, updated_at = NOW() WHERE container_id = $1"
    )
    .bind(&container_id)
    .execute(&state.db)
    .await
    .ok();

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "container.manual_sleep",
        Some("container"), Some(&container_id), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/apps/{container_id}/activity-ping — Record container activity (called by nginx or frontend).
pub async fn activity_ping(
    State(state): State<AppState>,
    AuthUser(_claims): AuthUser,
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if container_id.is_empty() || container_id.len() > 64
        || !container_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }

    sqlx::query(
        "UPDATE container_sleep_config SET last_activity_at = NOW() WHERE container_id = $1"
    )
    .bind(&container_id)
    .execute(&state.db)
    .await
    .ok();

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /api/apps/sleep-status — List all containers with sleep config (admin overview).
pub async fn sleep_status_list(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let configs: Vec<(String, String, Option<String>, bool, i32, bool, Option<chrono::DateTime<chrono::Utc>>, Option<chrono::DateTime<chrono::Utc>>, i32)> =
        sqlx::query_as(
            "SELECT container_id, container_name, domain, auto_sleep_enabled, sleep_after_minutes, \
             is_sleeping, last_slept_at, last_woken_at, total_sleeps \
             FROM container_sleep_config ORDER BY container_name"
        )
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("sleep status list", e))?;

    let items: Vec<serde_json::Value> = configs.iter().map(|(cid, name, domain, enabled, mins, sleeping, slept, woken, total)| {
        serde_json::json!({
            "container_id": cid,
            "container_name": name,
            "domain": domain,
            "auto_sleep_enabled": enabled,
            "sleep_after_minutes": mins,
            "is_sleeping": sleeping,
            "last_slept_at": slept,
            "last_woken_at": woken,
            "total_sleeps": total,
        })
    }).collect();

    Ok(Json(serde_json::json!({ "configs": items })))
}
