use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post, put},
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
            php_preset: None,
            app_command: None,
        };

        match nginx::render_site_config(&state.templates, domain, &site_config) {
            Ok(rendered) => {
                let config_path = format!("/etc/nginx/sites-enabled/{domain}.conf");
                let tmp_path = format!("{config_path}.tmp");
                let write_result = std::fs::write(&tmp_path, &rendered)
                    .and_then(|_| std::fs::rename(&tmp_path, &config_path));
                if let Err(e) = write_result {
                    // Clean up tmp file on failure
                    std::fs::remove_file(&tmp_path).ok();
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
                // Wait for DNS propagation before attempting SSL (up to 30 seconds)
                for i in 0..6u32 {
                    if i > 0 {
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                    match tokio::net::lookup_host(format!("{}:80", domain)).await {
                        Ok(_) => {
                            tracing::info!("DNS resolved for {domain} (attempt {}/6)", i + 1);
                            break;
                        }
                        Err(_) if i < 5 => {
                            tracing::info!("Waiting for DNS propagation for {}... ({}/6)", domain, i + 1);
                            continue;
                        }
                        Err(e) => {
                            tracing::warn!("DNS not propagated for {}: {} — trying SSL anyway", domain, e);
                            break;
                        }
                    }
                }

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
                                    php_preset: None,
                                    app_command: None,
                                };
                                match ssl::enable_ssl_for_site(&state.templates, domain, &ssl_site_config).await {
                                    Ok(()) => {
                                        response["ssl"] = serde_json::json!(true);
                                        tracing::info!("Auto-SSL: certificate provisioned for {domain}");
                                    }
                                    Err(e) => {
                                        tracing::warn!("Auto-SSL: enable_ssl_for_site failed for {domain}: {e}");
                                        response["ssl_warning"] = serde_json::json!(format!("SSL enable failed: {e} — retry from panel"));
                                        response["ssl_pending"] = serde_json::json!(true);
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!("Auto-SSL: cert provisioning failed for {domain}: {e}");
                                response["ssl_warning"] = serde_json::json!(format!("SSL provisioning failed: {e} — retry from panel"));
                                response["ssl_pending"] = serde_json::json!(true);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Auto-SSL: ACME account failed: {e}");
                        response["ssl_warning"] = serde_json::json!(format!("ACME account failed: {e} — retry from panel"));
                        response["ssl_pending"] = serde_json::json!(true);
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
/// Uses blue-green deployment (zero-downtime) when the app has a domain with nginx reverse proxy.
async fn update(
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container ID" })),
        ));
    }

    let result = docker_apps::update_app(&container_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
        })?;

    Ok(Json(serde_json::json!({
        "success": true,
        "container_id": result.container_id,
        "blue_green": result.blue_green,
    })))
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

#[derive(Deserialize)]
struct UpdateEnvRequest {
    env: HashMap<String, String>,
}

/// PUT /apps/{container_id}/env — Update environment variables and recreate container.
async fn update_env(
    Path(container_id): Path<String>,
    Json(body): Json<UpdateEnvRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container ID" })),
        ));
    }

    let new_id = docker_apps::update_env(&container_id, body.env)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
        })?;

    Ok(Json(serde_json::json!({ "success": true, "container_id": new_id })))
}

/// GET /apps/{container_id}/stats — Get live resource usage for a container.
async fn container_stats(
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container ID" })),
        ));
    }

    // Use docker stats --no-stream for a single snapshot
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::process::Command::new("docker")
            .args(["stats", "--no-stream", "--format", "{{.CPUPerc}}|{{.MemUsage}}|{{.MemPerc}}|{{.NetIO}}|{{.BlockIO}}|{{.PIDs}}", &container_id])
            .output(),
    )
    .await
    .map_err(|_| (StatusCode::GATEWAY_TIMEOUT, Json(serde_json::json!({"error": "Timeout"}))))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parts: Vec<&str> = stdout.trim().split('|').collect();

    if parts.len() >= 6 {
        Ok(Json(serde_json::json!({
            "cpu_percent": parts[0].trim_end_matches('%').trim(),
            "memory_usage": parts[1].trim(),
            "memory_percent": parts[2].trim_end_matches('%').trim(),
            "network_io": parts[3].trim(),
            "block_io": parts[4].trim(),
            "pids": parts[5].trim(),
        })))
    } else {
        Ok(Json(serde_json::json!({ "error": "Container not running or stats unavailable" })))
    }
}

/// GET /apps/{container_id}/shell-info — Get shell availability for a container.
async fn shell_info(
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container ID" })),
        ));
    }

    let name_output = tokio::process::Command::new("docker")
        .args(["inspect", "--format", "{{.Name}}", &container_id])
        .output()
        .await;
    let name = name_output
        .ok()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .trim_start_matches('/')
                .to_string()
        })
        .unwrap_or_default();

    let bash = tokio::process::Command::new("docker")
        .args(["exec", &container_id, "which", "bash"])
        .output()
        .await;
    let has_bash = bash.map(|o| o.status.success()).unwrap_or(false);

    let sh = tokio::process::Command::new("docker")
        .args(["exec", &container_id, "which", "sh"])
        .output()
        .await;
    let has_sh = sh.map(|o| o.status.success()).unwrap_or(false);

    Ok(Json(serde_json::json!({
        "name": name,
        "has_bash": has_bash,
        "has_sh": has_sh,
        "shell": if has_bash { "/bin/bash" } else if has_sh { "/bin/sh" } else { "" },
    })))
}

/// POST /apps/{container_id}/exec — Execute a command inside a container.
async fn exec_command(
    Path(container_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container ID" })),
        ));
    }
    let command = body
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("ls");
    if command.is_empty() || command.len() > 1000 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid command" })),
        ));
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        tokio::process::Command::new("docker")
            .args(["exec", &container_id, "sh", "-c", command])
            .output(),
    )
    .await
    .map_err(|_| {
        (
            StatusCode::GATEWAY_TIMEOUT,
            Json(serde_json::json!({"error": "Command timed out (30s)"})),
        )
    })?
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    Ok(Json(serde_json::json!({
        "success": output.status.success(),
        "stdout": stdout.chars().take(50000).collect::<String>(),
        "stderr": stderr.chars().take(10000).collect::<String>(),
        "exit_code": output.status.code(),
    })))
}

/// GET /apps/{container_id}/volumes — Get volume info and sizes.
async fn container_volumes(
    Path(container_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid container ID" })),
        ));
    }

    let output = tokio::process::Command::new("docker")
        .args([
            "inspect",
            "--format",
            "{{range .Mounts}}{{.Source}}|{{.Destination}}|{{.Type}}\n{{end}}",
            &container_id,
        ])
        .output()
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut volumes = Vec::new();

    for line in stdout.lines() {
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() >= 3 {
            let source = parts[0];
            let dest = parts[1];
            let mount_type = parts[2];

            let du = tokio::process::Command::new("du")
                .args(["-sb", source])
                .output()
                .await;
            let size: u64 = du
                .ok()
                .map(|o| {
                    String::from_utf8_lossy(&o.stdout)
                        .split_whitespace()
                        .next()
                        .unwrap_or("0")
                        .parse()
                        .unwrap_or(0)
                })
                .unwrap_or(0);

            let ls = tokio::process::Command::new("ls")
                .args(["-la", source])
                .output()
                .await;
            let listing = ls
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                .unwrap_or_default();

            volumes.push(serde_json::json!({
                "source": source,
                "destination": dest,
                "type": mount_type,
                "size_bytes": size,
                "size_mb": (size as f64 / 1024.0 / 1024.0 * 10.0).round() / 10.0,
                "listing": listing.lines().take(20).collect::<Vec<_>>().join("\n"),
            }));
        }
    }

    Ok(Json(serde_json::json!({ "volumes": volumes })))
}

#[derive(Deserialize)]
struct RegistryLoginRequest {
    server: String,
    username: String,
    password: String,
}

/// POST /apps/registry-login — Login to a private Docker registry.
async fn registry_login(
    Json(body): Json<RegistryLoginRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if body.server.is_empty() || body.username.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Server and username required" })),
        ));
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        tokio::process::Command::new("docker")
            .args(["login", &body.server, "-u", &body.username, "-p", &body.password])
            .output(),
    )
    .await
    .map_err(|_| {
        (
            StatusCode::GATEWAY_TIMEOUT,
            Json(serde_json::json!({"error": "Login timed out"})),
        )
    })?
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;

    if output.status.success() {
        tracing::info!("Docker registry login: {} @ {}", body.username, body.server);
        Ok(Json(serde_json::json!({ "success": true, "server": body.server })))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": format!("Login failed: {}", stderr.chars().take(200).collect::<String>()) })),
        ))
    }
}

/// GET /apps/registries — List configured registries.
async fn list_registries() -> Json<serde_json::Value> {
    let config_path = "/root/.docker/config.json";
    let content = std::fs::read_to_string(config_path).unwrap_or_default();
    let config: serde_json::Value =
        serde_json::from_str(&content).unwrap_or(serde_json::json!({}));

    let auths = config.get("auths").and_then(|a| a.as_object());
    let servers: Vec<String> = auths
        .map(|a| a.keys().cloned().collect())
        .unwrap_or_default();

    Json(serde_json::json!({ "registries": servers }))
}

/// POST /apps/registry-logout — Logout from a registry.
async fn registry_logout(
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let server = body
        .get("server")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if server.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Server required"})),
        ));
    }

    let _ = tokio::process::Command::new("docker")
        .args(["logout", server])
        .output()
        .await;
    Ok(Json(serde_json::json!({ "success": true })))
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

    // Extract app metadata before removing the container
    let domain = docker_apps::get_app_domain(&container_id).await;
    let app_name = docker_apps::get_app_name(&container_id).await;

    docker_apps::remove_app(&container_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
        })?;

    let mut response = serde_json::json!({ "success": true });

    // Clean up nginx config + SSL certs if domain was set
    if let Some(ref domain) = domain {
        response["domain_removed"] = serde_json::json!(domain);

        // Remove nginx config
        let config_path = format!("/etc/nginx/sites-enabled/{domain}.conf");
        if std::path::Path::new(&config_path).exists() {
            std::fs::remove_file(&config_path).ok();
            nginx::reload().await.ok();
            tracing::info!("Auto-proxy cleanup: removed nginx config for {domain}");
        }

        // Remove SSL certificates (panel-provisioned)
        let ssl_dir = format!("/etc/dockpanel/ssl/{domain}");
        if std::path::Path::new(&ssl_dir).exists() {
            std::fs::remove_dir_all(&ssl_dir).ok();
            tracing::info!("SSL cleanup: removed certs for {domain}");
        }

        // Remove SSL certificates (certbot/Let's Encrypt)
        let le_live = format!("/etc/letsencrypt/live/{domain}");
        let le_archive = format!("/etc/letsencrypt/archive/{domain}");
        let le_renewal = format!("/etc/letsencrypt/renewal/{domain}.conf");
        if std::path::Path::new(&le_live).exists() {
            std::fs::remove_dir_all(&le_live).ok();
            std::fs::remove_dir_all(&le_archive).ok();
            std::fs::remove_file(&le_renewal).ok();
            tracing::info!("SSL cleanup: removed Let's Encrypt certs for {domain}");
        }

        // Remove nginx logs
        let access_log = format!("/var/log/nginx/{domain}.access.log");
        let error_log = format!("/var/log/nginx/{domain}.error.log");
        std::fs::remove_file(&access_log).ok();
        std::fs::remove_file(&error_log).ok();
    }

    // Clean up persistent volume data
    if let Some(ref name) = app_name {
        let volume_dir = format!("/var/lib/dockpanel/apps/{name}");
        if std::path::Path::new(&volume_dir).exists() {
            std::fs::remove_dir_all(&volume_dir).ok();
            tracing::info!("Volume cleanup: removed {volume_dir}");
        }
    }

    Ok(Json(response))
}

#[derive(Deserialize)]
struct ComposeParseRequest {
    yaml: String,
    stack_id: Option<String>,
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

    let result = compose::deploy_compose(&services, body.stack_id.as_deref()).await;
    Ok(Json(result))
}

#[derive(Deserialize)]
struct StackActionRequest {
    stack_id: String,
    action: String,
}

/// POST /apps/stack/action — Perform a lifecycle action on all containers in a stack.
async fn stack_action(
    Json(body): Json<StackActionRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !["start", "stop", "restart", "remove"].contains(&body.action.as_str()) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Action must be start, stop, restart, or remove" })),
        ));
    }

    // Find all containers with this stack_id
    let apps = docker_apps::list_deployed_apps().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
    })?;

    let stack_containers: Vec<&docker_apps::DeployedApp> = apps
        .iter()
        .filter(|a| a.stack_id.as_deref() == Some(&body.stack_id))
        .collect();

    if stack_containers.is_empty() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "No containers found for this stack" })),
        ));
    }

    let mut results = Vec::new();
    for app in &stack_containers {
        let cid = &app.container_id;
        let result = match body.action.as_str() {
            "start" => docker_apps::start_app(cid).await.map(|_| "started"),
            "stop" => docker_apps::stop_app(cid).await.map(|_| "stopped"),
            "restart" => docker_apps::restart_app(cid).await.map(|_| "restarted"),
            "remove" => docker_apps::remove_app(cid).await.map(|_| "removed"),
            _ => unreachable!(),
        };
        results.push(serde_json::json!({
            "container_id": cid,
            "name": app.name,
            "status": match &result {
                Ok(s) => *s,
                Err(_) => "failed",
            },
            "error": result.err(),
        }));
    }

    Ok(Json(serde_json::json!({
        "stack_id": body.stack_id,
        "action": body.action,
        "results": results,
    })))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/apps/templates", get(templates))
        .route("/apps/deploy", post(deploy))
        .route("/apps/compose/parse", post(compose_parse))
        .route("/apps/compose/deploy", post(compose_deploy))
        .route("/apps/stack/action", post(stack_action))
        .route("/apps/registries", get(list_registries))
        .route("/apps/registry-login", post(registry_login))
        .route("/apps/registry-logout", post(registry_logout))
        .route("/apps", get(list))
        .route("/apps/{container_id}", delete(remove))
        .route("/apps/{container_id}/stop", post(stop))
        .route("/apps/{container_id}/start", post(start))
        .route("/apps/{container_id}/restart", post(restart))
        .route("/apps/{container_id}/logs", get(logs))
        .route("/apps/{container_id}/env", get(get_env).put(update_env))
        .route("/apps/{container_id}/update", post(update))
        .route("/apps/{container_id}/stats", get(container_stats))
        .route("/apps/{container_id}/shell-info", get(shell_info))
        .route("/apps/{container_id}/exec", post(exec_command))
        .route("/apps/{container_id}/volumes", get(container_volumes))
}
