use crate::safe_cmd::safe_command;
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
use crate::services::{nginx, ssl, traefik};

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
    /// When true, use Traefik file-based routing instead of nginx
    #[serde(default)]
    use_traefik: bool,
    /// User ID for container labeling and isolation
    user_id: Option<String>,
    /// Enable GPU passthrough (requires NVIDIA Container Toolkit)
    #[serde(default)]
    gpu_enabled: bool,
    /// Specific GPU device indices (e.g., [0, 2]) to assign to this container.
    /// When None or empty with gpu_enabled=true, all GPUs are assigned (legacy
    /// behavior). When Some(non-empty), only the listed indices are passed
    /// through via Docker's device_ids field. Ignored when gpu_enabled=false.
    gpu_indices: Option<Vec<u32>>,
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
        docker_apps::deploy_app(&body.template_id, &body.name, body.port, body.env, body.domain.as_deref(), body.memory_mb, body.cpu_percent, body.user_id.as_deref(), body.gpu_enabled, body.gpu_indices.clone())
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

    // Auto reverse proxy: Traefik (file-based dynamic config) or nginx
    if let Some(ref domain) = body.domain {
        if body.use_traefik {
            // --- Traefik mode: write a dynamic route config file ---
            let ssl = body.ssl_email.is_some();
            match traefik::write_route_config(domain, body.port, ssl) {
                Ok(()) => {
                    response["domain"] = serde_json::json!(domain);
                    response["proxy"] = serde_json::json!("traefik");
                    if ssl {
                        response["ssl"] = serde_json::json!(true);
                    }
                    tracing::info!("Auto-proxy (Traefik): {domain} → 127.0.0.1:{} (ssl={ssl})", body.port);
                }
                Err(e) => {
                    tracing::warn!("Auto-proxy (Traefik): failed to write route config for {domain}: {e}");
                    response["proxy_warning"] = serde_json::json!(format!("Traefik config failed: {e}"));
                }
            }
        } else {
            // --- nginx mode: create nginx config pointing to the app's port ---
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
                fastcgi_cache: None,
                redis_cache: None,
                redis_db: None,
                waf_enabled: None,
                waf_mode: None,
        csp_policy: None,
        permissions_policy: None,
        bot_protection: None,
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
                                if let Err(e) = nginx::reload().await {
                                    tracing::warn!("Auto-proxy: nginx reload failed after deploy for {domain}: {e}");
                                }
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

            // SSL provisioning (only if proxy was set up successfully, nginx mode only)
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
                                        fastcgi_cache: None,
                redis_cache: None,
                redis_db: None,
                waf_enabled: None,
                waf_mode: None,
        csp_policy: None,
        permissions_policy: None,
        bot_protection: None,
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
        safe_command("docker")
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

    let name_output = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        safe_command("docker")
            .args(["inspect", "--format", "{{.Name}}", &container_id])
            .output(),
    ).await;
    let name = name_output
        .ok()
        .and_then(|r| r.ok())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .trim_start_matches('/')
                .to_string()
        })
        .unwrap_or_default();

    let bash = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        safe_command("docker")
            .args(["exec", &container_id, "which", "bash"])
            .output(),
    ).await;
    let has_bash = bash.ok().and_then(|r| r.ok()).map(|o| o.status.success()).unwrap_or(false);

    let sh = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        safe_command("docker")
            .args(["exec", &container_id, "which", "sh"])
            .output(),
    ).await;
    let has_sh = sh.ok().and_then(|r| r.ok()).map(|o| o.status.success()).unwrap_or(false);

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

    // Block dangerous commands that could escape the container
    const CONTAINER_BLOCKED: &[&str] = &[
        "mount", "nsenter", "chroot", "/proc/1/", "/proc/sysrq", "docker", "kubectl",
        "unshare", "pivot_root", "setns", "capsh", "mknod", "debugfs", "kexec",
    ];
    let cmd_lower = command.to_lowercase();
    for pattern in CONTAINER_BLOCKED {
        if cmd_lower.contains(pattern) {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("Blocked command: contains '{pattern}'") })),
            ));
        }
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        safe_command("docker")
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

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        safe_command("docker")
            .args([
                "inspect",
                "--format",
                "{{range .Mounts}}{{.Source}}|{{.Destination}}|{{.Type}}\n{{end}}",
                &container_id,
            ])
            .output(),
    )
    .await
    .map_err(|_| (StatusCode::GATEWAY_TIMEOUT, Json(serde_json::json!({"error": "Timeout"}))))?
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

            let du = tokio::time::timeout(
                std::time::Duration::from_secs(15),
                safe_command("du")
                    .args(["-sb", source])
                    .output(),
            ).await;
            let size: u64 = du
                .ok()
                .and_then(|r| r.ok())
                .map(|o| {
                    String::from_utf8_lossy(&o.stdout)
                        .split_whitespace()
                        .next()
                        .unwrap_or("0")
                        .parse()
                        .unwrap_or(0)
                })
                .unwrap_or(0);

            let ls = tokio::time::timeout(
                std::time::Duration::from_secs(15),
                safe_command("ls")
                    .args(["-la", source])
                    .output(),
            ).await;
            let listing = ls
                .ok()
                .and_then(|r| r.ok())
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

    // Pass password via stdin to avoid leaking it in process args
    use tokio::io::AsyncWriteExt;
    let mut child = safe_command("docker")
        .args(["login", &body.server, "-u", &body.username, "--password-stdin"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(body.password.as_bytes()).await;
        drop(stdin);
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        child.wait_with_output(),
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

    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        safe_command("docker")
            .args(["logout", server])
            .output(),
    ).await;
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

    // Clean up proxy config (nginx + Traefik) + SSL certs if domain was set
    if let Some(ref domain) = domain {
        response["domain_removed"] = serde_json::json!(domain);

        // Remove Traefik dynamic route config (if it exists)
        traefik::remove_route_config(domain);

        // Remove nginx config
        let config_path = format!("/etc/nginx/sites-enabled/{domain}.conf");
        if std::path::Path::new(&config_path).exists() {
            std::fs::remove_file(&config_path).ok();
            if let Err(e) = nginx::reload().await {
                tracing::warn!("Auto-proxy cleanup: nginx reload failed after removing config for {domain}: {e}");
            }
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

/// POST /apps/compose/validate — Validate compose YAML with detailed feedback.
async fn compose_validate(
    Json(body): Json<ComposeParseRequest>,
) -> Json<serde_json::Value> {
    let mut errors: Vec<serde_json::Value> = Vec::new();
    let mut warnings: Vec<serde_json::Value> = Vec::new();
    let mut info: Vec<serde_json::Value> = Vec::new();

    // Try to parse
    match compose::parse_compose(&body.yaml) {
        Ok(services) => {
            info.push(serde_json::json!({
                "message": format!("{} service(s) found", services.len()),
            }));

            for svc in &services {
                // Check for latest tag
                if svc.image.ends_with(":latest") || !svc.image.contains(':') {
                    warnings.push(serde_json::json!({
                        "service": svc.name,
                        "message": "Using ':latest' tag — pin to a specific version for reproducible deploys",
                    }));
                }

                // Check for exposed privileged ports
                for port in &svc.ports {
                    if port.host < 1024 && port.host != 80 && port.host != 443 {
                        warnings.push(serde_json::json!({
                            "service": svc.name,
                            "message": format!("Privileged port {} — consider using a higher port", port.host),
                        }));
                    }
                }

                // Check for missing volumes on databases
                let db_images = ["postgres", "mysql", "mariadb", "mongo", "redis"];
                if db_images.iter().any(|db| svc.image.contains(db)) && svc.volumes.is_empty() {
                    warnings.push(serde_json::json!({
                        "service": svc.name,
                        "message": "Database service without volumes — data will be lost on container restart",
                    }));
                }

                // Check for missing restart policy
                if svc.restart.is_empty() || svc.restart == "no" {
                    info.push(serde_json::json!({
                        "service": svc.name,
                        "message": "No restart policy — container won't auto-restart. Consider 'unless-stopped'",
                    }));
                }

                // Check for missing health check env vars
                if svc.environment.is_empty() && db_images.iter().any(|db| svc.image.contains(db)) {
                    warnings.push(serde_json::json!({
                        "service": svc.name,
                        "message": "Database without environment variables — password/root may use defaults",
                    }));
                }
            }
        }
        Err(e) => {
            errors.push(serde_json::json!({
                "message": e,
            }));
        }
    }

    // YAML syntax check
    if body.yaml.contains('\t') {
        warnings.push(serde_json::json!({
            "message": "YAML contains tabs — use spaces for indentation to avoid parse errors",
        }));
    }

    Json(serde_json::json!({
        "valid": errors.is_empty(),
        "errors": errors,
        "warnings": warnings,
        "info": info,
    }))
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

/// GET /apps/images — List Docker images.
async fn list_images() -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        safe_command("docker")
            .args(["images", "--format", "{{.Repository}}|{{.Tag}}|{{.ID}}|{{.Size}}|{{.CreatedSince}}", "--no-trunc"])
            .output(),
    ).await
        .map_err(|_| (StatusCode::GATEWAY_TIMEOUT, Json(serde_json::json!({"error": "Timeout listing images"}))))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let images: Vec<serde_json::Value> = stdout.lines().filter(|l| !l.is_empty()).map(|l| {
        let parts: Vec<&str> = l.split('|').collect();
        serde_json::json!({
            "repository": parts.first().unwrap_or(&""),
            "tag": parts.get(1).unwrap_or(&""),
            "id": parts.get(2).unwrap_or(&""),
            "size": parts.get(3).unwrap_or(&""),
            "created": parts.get(4).unwrap_or(&""),
        })
    }).collect();

    Ok(Json(serde_json::json!({ "images": images })))
}

/// POST /apps/images/prune — Remove unused Docker images.
async fn prune_images_all() -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        safe_command("docker")
            .args(["image", "prune", "-af"])
            .output(),
    ).await
        .map_err(|_| (StatusCode::GATEWAY_TIMEOUT, Json(serde_json::json!({"error": "Image prune timed out (120s)"}))))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(Json(serde_json::json!({ "success": true, "output": stdout.trim() })))
}

/// DELETE /apps/images/{id} — Remove a specific Docker image.
async fn remove_image(Path(id): Path<String>) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // Validate image ID: alphanumeric + : / . - _ only
    let is_valid = !id.is_empty()
        && id.len() <= 256
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == ':' || c == '/' || c == '.' || c == '-' || c == '_');
    if !is_valid {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Invalid image ID"}))));
    }

    // Strip sha256: prefix if present (docker images --no-trunc includes it)
    let image_ref = if id.starts_with("sha256:") { &id } else { &id };
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        safe_command("docker")
            .args(["rmi", image_ref])
            .output(),
    ).await
        .map_err(|_| (StatusCode::GATEWAY_TIMEOUT, Json(serde_json::json!({"error": "Image removal timed out (60s)"}))))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err((StatusCode::CONFLICT, Json(serde_json::json!({"error": format!("Cannot remove: {}", stderr.chars().take(200).collect::<String>())}))));
    }
    Ok(Json(serde_json::json!({ "success": true })))
}

#[derive(Deserialize)]
struct SnapshotRequest {
    tag: Option<String>,
}

/// POST /apps/{container_id}/snapshot — Commit container to image.
async fn snapshot_container(
    Path(container_id): Path<String>,
    Json(body): Json<SnapshotRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "Invalid container ID" }))));
    }

    let tag = {
        let raw = body.tag.unwrap_or_else(|| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            now.to_string()
        });
        // Force-namespace with dockpanel-snapshot: prefix to prevent overwriting system images
        let suffix = raw.strip_prefix("dockpanel-snapshot:").unwrap_or(&raw);
        // Sanitise the suffix: only allow alphanumeric, -, _, .
        let safe_suffix: String = suffix.chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
            .take(128)
            .collect();
        if safe_suffix.is_empty() {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            format!("dockpanel-snapshot:{}", now)
        } else {
            format!("dockpanel-snapshot:{}", safe_suffix)
        }
    };

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        safe_command("docker")
            .args(["commit", &container_id, &tag])
            .output()
    ).await
        .map_err(|_| (StatusCode::GATEWAY_TIMEOUT, Json(serde_json::json!({"error": "Snapshot timed out"}))))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": format!("Snapshot failed: {stderr}")}))));
    }

    let image_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    tracing::info!("Container snapshot: {container_id} → {tag} ({image_id})");

    Ok(Json(serde_json::json!({ "success": true, "tag": tag, "image_id": image_id })))
}

/// Validate that an image reference contains only safe characters.
fn is_valid_image_ref(image: &str) -> bool {
    !image.is_empty()
        && image.len() <= 256
        && image.chars().all(|c| c.is_ascii_alphanumeric() || c == '/' || c == ':' || c == '.' || c == '-' || c == '_' || c == '@')
        && !image.starts_with('-')
}

/// POST /apps/{container_id}/change-image — Change a container's image tag.
/// Pulls the new image, stops the old container, starts a new one preserving volumes/env/ports/name.
async fn change_image(
    Path(container_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Invalid container ID"}))));
    }

    let image = body.get("image").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if image.is_empty() {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "image is required"}))));
    }

    if !is_valid_image_ref(&image) {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Invalid image reference: must be <= 256 chars, alphanumeric with / : . - _ @ only, and not start with -"}))));
    }

    // 1. Pull new image
    let pull_output = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        safe_command("docker")
            .args(["pull", &image])
            .output(),
    ).await
        .map_err(|_| (StatusCode::GATEWAY_TIMEOUT, Json(serde_json::json!({"error": "Image pull timed out"}))))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": format!("Pull failed: {e}")}))))?;

    if !pull_output.status.success() {
        let stderr = String::from_utf8_lossy(&pull_output.stderr);
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": format!("Failed to pull image: {stderr}")}))));
    }
    tracing::info!("Pulled image: {image}");

    // 2. Get current container info (name, volumes, env, ports)
    let inspect_output = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        safe_command("docker")
            .args(["inspect", "--format", "{{.Name}}", &container_id])
            .output(),
    ).await
        .map_err(|_| (StatusCode::GATEWAY_TIMEOUT, Json(serde_json::json!({"error": "Inspect timed out"}))))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    let container_name = String::from_utf8_lossy(&inspect_output.stdout)
        .trim()
        .trim_start_matches('/')
        .to_string();

    if container_name.is_empty() {
        return Err((StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "Container not found"}))));
    }

    // 3. Stop old container, rename it, create new one with same name
    let backup_name = format!("{container_name}-old-{}", &uuid::Uuid::new_v4().to_string()[..8]);

    // Stop
    if let Err(e) = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        safe_command("docker")
            .args(["stop", &container_id])
            .output(),
    ).await
        .map_err(|e| e.to_string())
        .and_then(|r| r.map_err(|e| e.to_string()))
        .map(|_| ())
    {
        tracing::warn!("change_image: failed to stop container {container_id}: {e}");
    }

    // Rename old container
    safe_command("docker")
        .args(["rename", &container_id, &backup_name])
        .output().await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": format!("Rename failed: {e}")}))))?;

    // 4. Create new container using `docker run` with --volumes-from to preserve data
    let run_output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        safe_command("docker")
            .args([
                "run", "-d",
                "--name", &container_name,
                "--volumes-from", &backup_name,
                "--network", "bridge",
                &image,
            ])
            .output(),
    ).await;

    match run_output {
        Ok(Ok(output)) if output.status.success() => {
            let new_id = String::from_utf8_lossy(&output.stdout).trim().to_string();

            // Remove old container
            if let Err(e) = tokio::time::timeout(
                std::time::Duration::from_secs(30),
                safe_command("docker")
                    .args(["rm", "-f", &backup_name])
                    .output(),
            ).await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string()))
                .map(|_| ())
            {
                tracing::warn!("change_image: failed to remove old container {backup_name}: {e}");
            }

            tracing::info!("Image changed for {container_name}: → {image} (new: {new_id})");
            Ok(Json(serde_json::json!({
                "success": true,
                "container_id": new_id,
                "image": image,
            })))
        }
        _ => {
            // Rollback: rename old container back and start it
            if let Err(e) = safe_command("docker")
                .args(["rename", &backup_name, &container_name])
                .output().await
            {
                tracing::warn!("change_image rollback: failed to rename {backup_name} back to {container_name}: {e}");
            }
            if let Err(e) = tokio::time::timeout(
                std::time::Duration::from_secs(30),
                safe_command("docker")
                    .args(["start", &container_name])
                    .output(),
            ).await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string()))
                .map(|_| ())
            {
                tracing::warn!("change_image rollback: failed to start {container_name}: {e}");
            }

            Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "Failed to create new container, rolled back to previous image"}))))
        }
    }
}

/// POST /apps/{container_id}/update-limits — Update CPU/memory limits on a running container.
async fn update_container_limits(
    Path(container_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !is_valid_container_id(&container_id) {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Invalid container ID"}))));
    }

    let memory_mb = body.get("memory_mb").and_then(|v| v.as_u64());
    let cpu_percent = body.get("cpu_percent").and_then(|v| v.as_u64());

    let mut args = vec!["update".to_string()];

    if let Some(mem) = memory_mb {
        args.push(format!("--memory={}m", mem));
        args.push(format!("--memory-swap={}m", mem * 2)); // swap = 2x memory
    }

    if let Some(cpu) = cpu_percent {
        // cpu_percent maps to --cpus (100% = 1.0 CPU)
        let cpus = cpu as f64 / 100.0;
        args.push(format!("--cpus={:.2}", cpus));
    }

    args.push(container_id.clone());

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        safe_command("docker")
            .args(&args)
            .output(),
    ).await
        .map_err(|_| (StatusCode::GATEWAY_TIMEOUT, Json(serde_json::json!({"error": "Timeout"}))))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": format!("docker update failed: {stderr}")}))));
    }

    tracing::info!("Container limits updated: {container_id} (mem: {:?}MB, cpu: {:?}%)", memory_mb, cpu_percent);

    Ok(Json(serde_json::json!({
        "success": true,
        "memory_mb": memory_mb,
        "cpu_percent": cpu_percent,
    })))
}

/// GET /apps/update-check — Check all managed containers for available image updates.
async fn update_check() -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    match docker_apps::check_image_updates().await {
        Ok(results) => {
            let count = results.iter().filter(|r| r.update_available).count();
            Ok(Json(serde_json::json!({
                "updates": results,
                "updates_available": count,
            })))
        }
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e})))),
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/apps/templates", get(templates))
        .route("/apps/update-check", get(update_check))
        .route("/apps/deploy", post(deploy))
        .route("/apps/compose/parse", post(compose_parse))
        .route("/apps/compose/validate", post(compose_validate))
        .route("/apps/compose/deploy", post(compose_deploy))
        .route("/apps/stack/action", post(stack_action))
        .route("/apps/registries", get(list_registries))
        .route("/apps/registry-login", post(registry_login))
        .route("/apps/registry-logout", post(registry_logout))
        .route("/apps/images", get(list_images))
        .route("/apps/images/prune", post(prune_images_all))
        .route("/apps/images/{id}", delete(remove_image))
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
        .route("/apps/{container_id}/snapshot", post(snapshot_container))
        .route("/apps/{container_id}/change-image", post(change_image))
        .route("/apps/{container_id}/update-limits", post(update_container_limits))
        .route("/apps/gpu-info", get(gpu_info))
}

/// GET /apps/gpu-info — Full GPU monitoring: utilization, VRAM, temperature, power, per-process usage.
async fn gpu_info() -> Json<serde_json::Value> {
    // Query comprehensive GPU metrics in one call
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        crate::safe_cmd::safe_command("nvidia-smi")
            .args([
                "--query-gpu=index,name,memory.total,memory.used,memory.free,utilization.gpu,utilization.memory,temperature.gpu,power.draw,power.limit,fan.speed,driver_version,pstate",
                "--format=csv,noheader,nounits"
            ])
            .output()
    ).await;

    let gpu_output = match output {
        Ok(Ok(out)) if out.status.success() => Some(out),
        _ => None,
    };

    let Some(gpu_out) = gpu_output else {
        return Json(serde_json::json!({
            "available": false,
            "gpus": [],
            "gpu_count": 0,
            "nvidia_toolkit_installed": false,
            "processes": [],
        }));
    };

    let stdout = String::from_utf8_lossy(&gpu_out.stdout);
    let gpus: Vec<serde_json::Value> = stdout.lines().filter(|l| !l.trim().is_empty()).map(|line| {
        // Split on ", " — GPU names can theoretically contain commas, so we parse
        // index (first field) and the 11 numeric/string fields from the right,
        // treating everything in between as the GPU name.
        let p: Vec<&str> = line.split(", ").collect();
        if p.len() >= 13 {
            // Normal case: exactly 13 fields (index + name + 11 metrics)
            let parse_u64 = |idx: usize| p.get(idx).and_then(|v| v.trim().parse::<u64>().ok());
            let _parse_f64 = |idx: usize| p.get(idx).and_then(|v| v.trim().parse::<f64>().ok());
            let _str_val = |idx: usize| p.get(idx).map(|v| v.trim()).unwrap_or("");
            // If there are extra commas (in GPU name), join the excess back into the name
            let name_end = p.len() - 11; // 11 fields after name
            let name = p[1..name_end].join(", ");
            serde_json::json!({
                "index": parse_u64(0).unwrap_or(0),
                "name": name.trim(),
                "memory_total_mb": p.get(name_end).and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(0),
                "memory_used_mb": p.get(name_end + 1).and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(0),
                "memory_free_mb": p.get(name_end + 2).and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(0),
                "utilization_gpu_pct": p.get(name_end + 3).and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(0),
                "utilization_memory_pct": p.get(name_end + 4).and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(0),
                "temperature_c": p.get(name_end + 5).and_then(|v| v.trim().parse::<u64>().ok()),
                "power_draw_w": p.get(name_end + 6).and_then(|v| v.trim().parse::<f64>().ok()),
                "power_limit_w": p.get(name_end + 7).and_then(|v| v.trim().parse::<f64>().ok()),
                "fan_speed_pct": p.get(name_end + 8).and_then(|v| v.trim().parse::<u64>().ok()),
                "driver_version": p.get(name_end + 9).map(|v| v.trim()).unwrap_or(""),
                "performance_state": p.get(name_end + 10).map(|v| v.trim()).unwrap_or(""),
            })
        } else {
            // Fallback: fewer fields than expected
            serde_json::json!({
                "index": p.first().and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(0),
                "name": p.get(1).unwrap_or(&"Unknown").trim(),
                "memory_total_mb": 0, "memory_used_mb": 0, "memory_free_mb": 0,
                "utilization_gpu_pct": 0, "utilization_memory_pct": 0,
                "temperature_c": null, "power_draw_w": null, "power_limit_w": null,
                "fan_speed_pct": null, "driver_version": "", "performance_state": "",
            })
        }
    }).collect();

    // Query per-process GPU usage (which PIDs are using which GPU and how much VRAM)
    let proc_output = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        crate::safe_cmd::safe_command("nvidia-smi")
            .args([
                "--query-compute-apps=pid,gpu_uuid,used_gpu_memory,name",
                "--format=csv,noheader,nounits"
            ])
            .output()
    ).await;

    let mut processes: Vec<serde_json::Value> = Vec::new();
    if let Ok(Ok(out)) = proc_output {
        if out.status.success() {
            let stdout = String::from_utf8_lossy(&out.stdout);
            for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
                let p: Vec<&str> = line.split(", ").collect();
                let pid = p.first().and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(0);

                // Try to resolve PID to a Docker container name
                let container_name = resolve_pid_to_container(pid).await;

                processes.push(serde_json::json!({
                    "pid": pid,
                    "gpu_uuid": p.get(1).map(|v| v.trim()).unwrap_or(""),
                    "vram_used_mb": p.get(2).and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(0),
                    "process_name": p.get(3).map(|v| v.trim()).unwrap_or(""),
                    "container_name": container_name,
                }));
            }
        }
    }

    // Check if NVIDIA Container Toolkit is installed
    let toolkit = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        crate::safe_cmd::safe_command("nvidia-container-cli")
            .arg("--version")
            .output()
    ).await
        .ok()
        .and_then(|r| r.ok())
        .map(|o| o.status.success())
        .unwrap_or(false);

    Json(serde_json::json!({
        "available": true,
        "gpus": gpus,
        "gpu_count": gpus.len(),
        "nvidia_toolkit_installed": toolkit,
        "processes": processes,
    }))
}

/// Resolve a host PID to a Docker container name (if it belongs to one).
async fn resolve_pid_to_container(pid: u64) -> Option<String> {
    // Read the cgroup of the process to find its container ID
    let cgroup = tokio::fs::read_to_string(format!("/proc/{pid}/cgroup")).await.ok()?;
    // Docker cgroup paths contain the container ID (64-char hex)
    let container_id = cgroup.lines()
        .filter_map(|line| {
            // Format: "0::/docker/<container_id>" or "0::/system.slice/docker-<id>.scope"
            let after_docker = line.split("/docker/").nth(1)
                .or_else(|| line.split("/docker-").nth(1));
            after_docker.map(|s| s.trim_end_matches(".scope").chars().take(12).collect::<String>())
        })
        .find(|id| id.len() >= 12)?;

    // Use docker inspect to get the container name
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        crate::safe_cmd::safe_command("docker")
            .args(["inspect", "--format", "{{.Name}}", &container_id])
            .output()
    ).await.ok()?.ok()?;

    if output.status.success() {
        let name = String::from_utf8_lossy(&output.stdout).trim().trim_start_matches('/').to_string();
        if !name.is_empty() { Some(name) } else { None }
    } else {
        None
    }
}
