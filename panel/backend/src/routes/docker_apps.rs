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
use crate::error::{err, agent_error, require_admin, ApiError};
use crate::routes::{is_valid_container_id, is_valid_name};
use crate::routes::sites::ProvisionStep;
use crate::services::activity;
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
}

/// GET /api/apps/templates — List available app templates.
pub async fn list_templates(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let result = state
        .agent
        .get("/apps/templates")
        .await
        .map_err(|e| agent_error("Docker apps", e))?;

    Ok(Json(result))
}

/// POST /api/apps/deploy — Deploy a Docker app from template (async with SSE progress).
pub async fn deploy(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
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

    let deploy_id = Uuid::new_v4();

    // Create provisioning channel (reuse the same provision_logs map from AppState)
    let (tx, _) = broadcast::channel::<ProvisionStep>(32);
    {
        let mut logs = state.provision_logs.lock().unwrap();
        logs.insert(deploy_id, (Vec::new(), tx, Instant::now()));
    }

    let logs = state.provision_logs.clone();
    let agent = state.agent.clone();
    let db = state.db.clone();
    let user_id = claims.sub;
    let email = claims.email.clone();
    let app_name = body.name.clone();
    let template = body.template_id.clone();

    let deploy_domain = body.domain.clone().filter(|d| !d.is_empty());
    let deploy_ssl_email = body.ssl_email.clone().or_else(|| Some(claims.email.clone()));
    let deploy_memory = body.memory_mb;
    let deploy_cpu = body.cpu_percent;

    let mut agent_body = serde_json::json!({
        "template_id": body.template_id,
        "name": body.name,
        "port": body.port,
        "env": body.env.unwrap_or_default(),
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
                // Detect server's public IP (try external service first, fallback to local detection)
                let server_ip = match reqwest::Client::new()
                    .get("https://api.ipify.org")
                    .timeout(std::time::Duration::from_secs(5))
                    .send().await
                {
                    Ok(resp) => {
                        let ip = resp.text().await.unwrap_or_default().trim().to_string();
                        if ip.is_empty() { String::new() } else { ip }
                    }
                    Err(_) => {
                        use std::net::UdpSocket;
                        UdpSocket::bind("0.0.0.0:0")
                            .and_then(|s| { s.connect("8.8.8.8:53")?; s.local_addr() })
                            .map(|a| a.ip().to_string())
                            .unwrap_or_default()
                    }
                };

                if provider == "cloudflare" {
                    if let (Some(zone_id), Some(token)) = (cf_zone_id, cf_api_token) {
                        let client = reqwest::Client::new();
                        let mut headers = reqwest::header::HeaderMap::new();
                        if let Some(em) = cf_api_email {
                            headers.insert("X-Auth-Email", em.parse().unwrap_or_else(|_| "".parse().unwrap()));
                            headers.insert("X-Auth-Key", token.parse().unwrap_or_else(|_| "".parse().unwrap()));
                        } else {
                            headers.insert("Authorization", format!("Bearer {token}").parse().unwrap_or_else(|_| "".parse().unwrap()));
                        }

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
                    let pdns_key = pdns.iter().find(|(k,_)| k == "pdns_api_key").map(|(_,v)| v.clone());

                    if let (Some(url), Some(key)) = (pdns_url, pdns_key) {
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
            }
            Err(e) => {
                emit("pull", "Pulling Docker image", "error", Some(format!("Deploy failed: {e}")));
                emit("complete", "Deploy failed", "error", None);
                tracing::error!("App deploy failed: {} ({}): {e}", app_name, template);
            }
        }

        tokio::time::sleep(Duration::from_secs(30)).await;
        logs.lock().unwrap().remove(&deploy_id);
    });

    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({
        "deploy_id": deploy_id,
        "message": "Deployment started",
    }))))
}

/// GET /api/apps/deploy/{deploy_id}/log — SSE stream of deploy progress.
pub async fn deploy_log(
    State(state): State<AppState>,
    AuthUser(_claims): AuthUser,
    Path(deploy_id): Path<Uuid>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, axum::BoxError>>>, ApiError> {
    let (snapshot, rx) = {
        let logs = state.provision_logs.lock().unwrap();
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
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let result = state
        .agent
        .get("/apps")
        .await
        .map_err(|e| agent_error("Docker apps", e))?;

    Ok(Json(result))
}

/// POST /api/apps/{container_id}/stop — Stop an app.
pub async fn stop_app(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }

    let agent_path = format!("/apps/{}/stop", container_id);
    state
        .agent
        .post(&agent_path, None)
        .await
        .map_err(|e| agent_error("Container stop", e))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/apps/{container_id}/start — Start an app.
pub async fn start_app(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }

    let agent_path = format!("/apps/{}/start", container_id);
    state
        .agent
        .post(&agent_path, None)
        .await
        .map_err(|e| agent_error("Container start", e))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/apps/{container_id}/restart — Restart an app.
pub async fn restart_app(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }

    let agent_path = format!("/apps/{}/restart", container_id);
    state
        .agent
        .post(&agent_path, None)
        .await
        .map_err(|e| agent_error("Container restart", e))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /api/apps/{container_id}/logs — Get app logs.
pub async fn app_logs(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }

    let agent_path = format!("/apps/{}/logs", container_id);
    let result = state
        .agent
        .get(&agent_path)
        .await
        .map_err(|e| agent_error("Container logs", e))?;

    Ok(Json(result))
}

/// POST /api/apps/{container_id}/update — Pull latest image and recreate container (async with SSE).
pub async fn update_app(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(container_id): Path<String>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    require_admin(&claims.role)?;
    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }

    let deploy_id = Uuid::new_v4();

    let (tx, _) = broadcast::channel::<ProvisionStep>(32);
    {
        let mut logs = state.provision_logs.lock().unwrap();
        logs.insert(deploy_id, (Vec::new(), tx, Instant::now()));
    }

    let logs = state.provision_logs.clone();
    let agent = state.agent.clone();
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
            Ok(_) => {
                emit("pull", "Pulling latest image", "done", None);
                emit("recreate", "Recreating container", "done", None);
                emit("complete", "App updated", "done", None);
                activity::log_activity(
                    &db, user_id, &email, "app.update",
                    Some("app"), Some(&cid), None, None,
                ).await;
                tracing::info!("App updated: {cid}");
            }
            Err(e) => {
                emit("pull", "Pulling latest image", "error", Some(format!("{e}")));
                emit("complete", "Update failed", "error", None);
                tracing::error!("App update failed: {cid}: {e}");
            }
        }

        tokio::time::sleep(Duration::from_secs(60)).await;
        logs.lock().unwrap().remove(&deploy_id);
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
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }

    let agent_path = format!("/apps/{}/env", container_id);
    let result = state
        .agent
        .get(&agent_path)
        .await
        .map_err(|e| agent_error("Container env", e))?;

    Ok(Json(result))
}

/// POST /api/apps/compose/parse — Parse docker-compose.yml and preview services.
pub async fn compose_parse(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let yaml = body["yaml"]
        .as_str()
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing 'yaml' field"))?;

    if yaml.len() > 65536 {
        return Err(err(StatusCode::BAD_REQUEST, "YAML too large (max 64KB)"));
    }

    let result = state
        .agent
        .post("/apps/compose/parse", Some(serde_json::json!({ "yaml": yaml })))
        .await
        .map_err(|e| agent_error("Compose parse", e))?;

    Ok(Json(result))
}

/// POST /api/apps/compose/deploy — Deploy services from docker-compose.yml.
pub async fn compose_deploy(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<serde_json::Value>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    require_admin(&claims.role)?;

    let yaml = body["yaml"]
        .as_str()
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing 'yaml' field"))?;

    if yaml.len() > 65536 {
        return Err(err(StatusCode::BAD_REQUEST, "YAML too large (max 64KB)"));
    }

    let result = state
        .agent
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
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    if !is_valid_container_id(&container_id) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container ID"));
    }

    let agent_path = format!("/apps/{}", container_id);
    let result = state
        .agent
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

            let zone: Option<(String, Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
                "SELECT provider, cf_zone_id, cf_api_token, cf_api_email FROM dns_zones WHERE domain = $1 AND user_id = $2"
            ).bind(&parent).bind(dns_user).fetch_optional(&dns_db).await.ok().flatten();

            if let Some((provider, cf_zone_id, cf_api_token, cf_api_email)) = zone {
                let server_ip = match reqwest::Client::new()
                    .get("https://api.ipify.org")
                    .timeout(std::time::Duration::from_secs(5))
                    .send().await
                {
                    Ok(resp) => resp.text().await.unwrap_or_default().trim().to_string(),
                    Err(_) => {
                        std::net::UdpSocket::bind("0.0.0.0:0")
                            .and_then(|s| { s.connect("8.8.8.8:53")?; s.local_addr() })
                            .map(|a| a.ip().to_string()).unwrap_or_default()
                    }
                };

                if provider == "cloudflare" {
                    if let (Some(zid), Some(tok)) = (cf_zone_id, cf_api_token) {
                        let client = reqwest::Client::new();
                        let mut headers = reqwest::header::HeaderMap::new();
                        if let Some(em) = cf_api_email {
                            headers.insert("X-Auth-Email", em.parse().unwrap_or_else(|_| "".parse().unwrap()));
                            headers.insert("X-Auth-Key", tok.parse().unwrap_or_else(|_| "".parse().unwrap()));
                        } else {
                            headers.insert("Authorization", format!("Bearer {tok}").parse().unwrap_or_else(|_| "".parse().unwrap()));
                        }
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
                    let pkey = pdns.iter().find(|(k,_)| k == "pdns_api_key").map(|(_,v)| v.clone());
                    if let (Some(url), Some(key)) = (purl, pkey) {
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

    Ok(Json(serde_json::json!({ "ok": true })))
}
