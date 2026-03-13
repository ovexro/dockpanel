use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
use serde::Deserialize;
use std::collections::HashMap;

use super::{is_valid_container_id, is_valid_domain, is_valid_name, AppState};
use crate::routes::nginx::SiteConfig;
use crate::services::compose;
use crate::services::docker_apps;
use crate::services::{nginx, ssl};

#[derive(Deserialize)]
struct DeployRequest {
    template_id: String,
    name: String,
    port: u16,
    #[serde(default)]
    env: HashMap<String, String>,
    /// Optional domain for auto reverse proxy
    domain: Option<String>,
    /// Email for Let's Encrypt SSL (requires domain)
    ssl_email: Option<String>,
    /// Memory limit in MB (e.g., 512)
    memory_mb: Option<u64>,
    /// CPU limit as percentage (e.g., 50 = 50% of one core)
    cpu_percent: Option<u64>,
}

/// GET /apps/templates — List all available app templates.
async fn templates() -> Json<Vec<docker_apps::AppTemplate>> {
    Json(docker_apps::list_templates())
}

/// POST /apps/deploy — Deploy an app from a template, optionally with reverse proxy + SSL.
async fn deploy(
    State(state): State<AppState>,
    Json(body): Json<DeployRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_name(&body.name) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid app name" })),
        ));
    }

    if let Some(ref domain) = body.domain {
        if !is_valid_domain(domain) {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "Invalid domain format" })),
            ));
        }
    }

    let result =
        docker_apps::deploy_app(&body.template_id, &body.name, body.port, body.env, body.domain.as_deref(), body.memory_mb, body.cpu_percent)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e })),
                )
            })?;

    let mut response = serde_json::json!({
        "success": true,
        "container_id": result.container_id,
        "name": result.name,
        "port": result.port,
    });

    // Auto reverse proxy: create nginx config pointing to the app's port
    if let Some(ref domain) = body.domain {
        let site_config = SiteConfig {
            runtime: "proxy".to_string(),
            root: None,
            proxy_port: Some(body.port),
            php_socket: None,
            ssl: None,
            ssl_cert: None,
            ssl_key: None,
            rate_limit: None,
            max_upload_mb: None,
            php_memory_mb: None,
            php_max_workers: None,
            custom_nginx: None,
        };

        match nginx::render_site_config(&state.templates, domain, &site_config) {
            Ok(rendered) => {
                let config_path = format!("/etc/nginx/sites-enabled/{domain}.conf");
                if let Err(e) = std::fs::write(&config_path, &rendered) {
                    tracing::warn!("Auto-proxy: failed to write nginx config for {domain}: {e}");
                    response["proxy_warning"] = serde_json::json!(format!("Failed to write nginx config: {e}"));
                } else {
                    match nginx::test_config().await {
                        Ok(output) if output.success => {
                            nginx::reload().await.ok();
                            response["domain"] = serde_json::json!(domain);
                            response["proxy"] = serde_json::json!(true);
                            tracing::info!("Auto-proxy: {domain} → 127.0.0.1:{}", body.port);
                        }
                        Ok(output) => {
                            std::fs::remove_file(&config_path).ok();
                            tracing::warn!("Auto-proxy: nginx config test failed for {domain}: {}", output.stderr);
                            response["proxy_warning"] = serde_json::json!(format!("Nginx config test failed: {}", output.stderr));
                        }
                        Err(e) => {
                            std::fs::remove_file(&config_path).ok();
                            tracing::warn!("Auto-proxy: nginx test error for {domain}: {e}");
                            response["proxy_warning"] = serde_json::json!(format!("Nginx test error: {e}"));
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Auto-proxy: failed to render config for {domain}: {e}");
                response["proxy_warning"] = serde_json::json!(format!("Failed to render nginx config: {e}"));
            }
        }

        // SSL provisioning (only if proxy was set up successfully)
        if response.get("proxy").is_some() {
            if let Some(ref email) = body.ssl_email {
                match ssl::load_or_create_account(email).await {
                    Ok(account) => {
                        match ssl::provision_cert(&account, domain).await {
                            Ok(_cert_info) => {
                                let ssl_site_config = SiteConfig {
                                    runtime: "proxy".to_string(),
                                    root: None,
                                    proxy_port: Some(body.port),
                                    php_socket: None,
                                    ssl: None,
                                    ssl_cert: None,
                                    ssl_key: None,
                                    rate_limit: None,
                                    max_upload_mb: None,
                                    php_memory_mb: None,
                                    php_max_workers: None,
                                    custom_nginx: None,
                                };
                                match ssl::enable_ssl_for_site(&state.templates, domain, &ssl_site_config).await {
                                    Ok(()) => {
                                        response["ssl"] = serde_json::json!(true);
                                        tracing::info!("Auto-SSL: certificate provisioned for {domain}");
                                    }
                                    Err(e) => {
                                        tracing::warn!("Auto-SSL: enable_ssl_for_site failed for {domain}: {e}");
                                        response["ssl_warning"] = serde_json::json!(format!("SSL enable failed: {e}"));
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!("Auto-SSL: cert provisioning failed for {domain}: {e}");
                                response["ssl_warning"] = serde_json::json!(format!("SSL provisioning failed: {e}"));
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Auto-SSL: ACME account failed: {e}");
                        response["ssl_warning"] = serde_json::json!(format!("ACME account failed: {e}"));
                    }
                }
            }
        }
    }

    Ok(Json(response))
}

/// GET /apps — List all deployed apps.
async fn list() -> Result<Json<Vec<docker_apps::DeployedApp>>, (StatusCode, Json<serde_json::Value>)>
{
    let apps = docker_apps::list_deployed_apps().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
    })?;

    Ok(Json(apps))
}

/// POST /apps/{container_id}/stop — Stop a running app.
async fn stop(
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container ID" })),
        ));
    }

    docker_apps::stop_app(&container_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
        })?;

    Ok(Json(serde_json::json!({ "success": true })))
}

/// POST /apps/{container_id}/start — Start a stopped app.
async fn start(
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container ID" })),
        ));
    }

    docker_apps::start_app(&container_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
        })?;

    Ok(Json(serde_json::json!({ "success": true })))
}

/// POST /apps/{container_id}/restart — Restart an app.
async fn restart(
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container ID" })),
        ));
    }

    docker_apps::restart_app(&container_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
        })?;

    Ok(Json(serde_json::json!({ "success": true })))
}

/// GET /apps/{container_id}/logs — Get app container logs.
async fn logs(
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container ID" })),
        ));
    }

    let output = docker_apps::get_app_logs(&container_id, 200)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
        })?;

    Ok(Json(serde_json::json!({ "logs": output })))
}

/// POST /apps/{container_id}/update — Pull latest image and recreate container.
async fn update(
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container ID" })),
        ));
    }

    let new_id = docker_apps::update_app(&container_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
        })?;

    Ok(Json(serde_json::json!({ "success": true, "container_id": new_id })))
}

/// GET /apps/{container_id}/env — Get container environment variables.
async fn get_env(
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container ID" })),
        ));
    }

    let env = docker_apps::get_app_env(&container_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
        })?;

    // Sensitive env var name patterns — mask values containing these substrings
    const SENSITIVE_PATTERNS: &[&str] = &[
        "PASSWORD", "SECRET", "KEY", "TOKEN", "CREDENTIAL", "AUTH",
    ];

    let env_map: Vec<serde_json::Value> = env
        .into_iter()
        .map(|(k, v)| {
            let upper = k.to_uppercase();
            let is_sensitive = SENSITIVE_PATTERNS
                .iter()
                .any(|pat| upper.contains(pat));
            let masked_value = if is_sensitive {
                "********".to_string()
            } else {
                v
            };
            serde_json::json!({ "key": k, "value": masked_value })
        })
        .collect();

    Ok(Json(serde_json::json!({ "env": env_map })))
}

/// DELETE /apps/{container_id} — Remove a deployed app and clean up its proxy.
async fn remove(
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container ID" })),
        ));
    }

    // Check for associated domain before removing the container
    let domain = docker_apps::get_app_domain(&container_id).await;

    docker_apps::remove_app(&container_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
        })?;

    // Clean up nginx config if domain was set
    let mut response = serde_json::json!({ "success": true });
    if let Some(ref domain) = domain {
        let config_path = format!("/etc/nginx/sites-enabled/{domain}.conf");
        if std::path::Path::new(&config_path).exists() {
            std::fs::remove_file(&config_path).ok();
            nginx::reload().await.ok();
            response["domain_removed"] = serde_json::json!(domain);
            tracing::info!("Auto-proxy cleanup: removed nginx config for {domain}");
        }
    }

    Ok(Json(response))
}

#[derive(Deserialize)]
struct ComposeParseRequest {
    yaml: String,
}

/// POST /apps/compose/parse — Parse docker-compose.yml and return services preview.
async fn compose_parse(
    Json(body): Json<ComposeParseRequest>,
) -> Result<Json<Vec<compose::ComposeService>>, (StatusCode, Json<serde_json::Value>)> {
    let services = compose::parse_compose(&body.yaml).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e })),
        )
    })?;

    Ok(Json(services))
}

/// POST /apps/compose/deploy — Deploy services from parsed compose file.
async fn compose_deploy(
    Json(body): Json<ComposeParseRequest>,
) -> Result<Json<compose::ComposeDeployResult>, (StatusCode, Json<serde_json::Value>)> {
    let services = compose::parse_compose(&body.yaml).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e })),
        )
    })?;

    let result = compose::deploy_compose(&services).await;
    Ok(Json(result))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/apps/templates", get(templates))
        .route("/apps/deploy", post(deploy))
        .route("/apps/compose/parse", post(compose_parse))
        .route("/apps/compose/deploy", post(compose_deploy))
        .route("/apps", get(list))
        .route("/apps/{container_id}", delete(remove))
        .route("/apps/{container_id}/stop", post(stop))
        .route("/apps/{container_id}/start", post(start))
        .route("/apps/{container_id}/restart", post(restart))
        .route("/apps/{container_id}/logs", get(logs))
        .route("/apps/{container_id}/env", get(get_env))
        .route("/apps/{container_id}/update", post(update))
}
