use bollard::container::{
    Config, CreateContainerOptions, ListContainersOptions, RemoveContainerOptions,
    RenameContainerOptions, StartContainerOptions, StopContainerOptions,
};
use bollard::Docker;
use serde::Serialize;
use std::collections::HashMap;
use tera::Tera;
use tokio::process::Command;

const GIT_BASE_DIR: &str = "/var/lib/dockpanel/git";

#[derive(Debug, Serialize)]
pub struct CloneResult {
    pub commit_hash: String,
    pub commit_message: String,
}

#[derive(Debug, Serialize)]
pub struct BuildResult {
    pub image_tag: String,
    pub output: String,
}

#[derive(Debug, Serialize)]
pub struct GitDeployResult {
    pub container_id: String,
    pub blue_green: bool,
}

/// Clone or pull a git repository to `/var/lib/dockpanel/git/{name}/`.
/// Uses `--depth 50` for clone and `fetch + reset --hard` for pull.
pub async fn clone_or_pull(
    name: &str,
    repo_url: &str,
    branch: &str,
    key_path: Option<&str>,
) -> Result<CloneResult, String> {
    let repo_dir = format!("{GIT_BASE_DIR}/{name}");
    let git_dir = format!("{repo_dir}/.git");

    let env_ssh = match key_path {
        Some(k) => Some(crate::services::deploy::ssh_command(k)?),
        None => None,
    };

    if std::path::Path::new(&git_dir).exists() {
        // Fetch from remote
        let mut cmd = Command::new("git");
        cmd.args(["-C", &repo_dir, "fetch", "origin", branch])
            .env("GIT_TERMINAL_PROMPT", "0");
        if let Some(ref ssh) = env_ssh {
            cmd.env("GIT_SSH_COMMAND", ssh);
        }

        let fetch = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            cmd.output(),
        )
        .await
        .map_err(|_| "git fetch timed out (120s)".to_string())?
        .map_err(|e| format!("git fetch failed: {e}"))?;

        if !fetch.status.success() {
            let stderr = String::from_utf8_lossy(&fetch.stderr);
            return Err(format!("git fetch failed: {stderr}"));
        }

        // Reset to remote branch head
        let reset = Command::new("git")
            .args(["-C", &repo_dir, "reset", "--hard", &format!("origin/{branch}")])
            .output()
            .await
            .map_err(|e| format!("git reset failed: {e}"))?;

        if !reset.status.success() {
            let stderr = String::from_utf8_lossy(&reset.stderr);
            return Err(format!("git reset failed: {stderr}"));
        }

        tracing::info!("Git repo pulled: {name} (branch {branch})");
    } else {
        // Fresh clone
        std::fs::create_dir_all(&repo_dir)
            .map_err(|e| format!("Failed to create repo dir: {e}"))?;

        let mut cmd = Command::new("git");
        cmd.args([
            "clone", "--branch", branch, "--single-branch", "--depth", "50",
            repo_url, &repo_dir,
        ])
        .env("GIT_TERMINAL_PROMPT", "0");
        if let Some(ref ssh) = env_ssh {
            cmd.env("GIT_SSH_COMMAND", ssh);
        }

        let clone = tokio::time::timeout(
            std::time::Duration::from_secs(300),
            cmd.output(),
        )
        .await
        .map_err(|_| "git clone timed out (300s)".to_string())?
        .map_err(|e| format!("git clone failed: {e}"))?;

        if !clone.status.success() {
            let stderr = String::from_utf8_lossy(&clone.stderr);
            // Clean up partial clone
            std::fs::remove_dir_all(&repo_dir).ok();
            return Err(format!("git clone failed: {stderr}"));
        }

        tracing::info!("Git repo cloned: {name} (branch {branch})");
    }

    // Get current commit hash
    let hash_output = Command::new("git")
        .args(["-C", &repo_dir, "rev-parse", "--short", "HEAD"])
        .output()
        .await
        .map_err(|e| format!("Failed to get commit hash: {e}"))?;

    let commit_hash = if hash_output.status.success() {
        String::from_utf8_lossy(&hash_output.stdout).trim().to_string()
    } else {
        return Err("Failed to read commit hash".to_string());
    };

    // Get commit message
    let msg_output = Command::new("git")
        .args(["-C", &repo_dir, "log", "-1", "--format=%s"])
        .output()
        .await
        .map_err(|e| format!("Failed to get commit message: {e}"))?;

    let commit_message = if msg_output.status.success() {
        String::from_utf8_lossy(&msg_output.stdout).trim().to_string()
    } else {
        String::new()
    };

    Ok(CloneResult {
        commit_hash,
        commit_message,
    })
}

/// Build a Docker image from the git repo directory.
/// Tags with both `dockpanel-git-{name}:{commit_hash}` and `dockpanel-git-{name}:latest`.
pub async fn build_image(
    name: &str,
    dockerfile_path: &str,
    commit_hash: &str,
) -> Result<BuildResult, String> {
    let repo_dir = format!("{GIT_BASE_DIR}/{name}");
    let image_name = format!("dockpanel-git-{name}");
    let image_tag = format!("{image_name}:{commit_hash}");
    let latest_tag = format!("{image_name}:latest");

    tracing::info!("Building image {image_tag} from {repo_dir}");

    let build = tokio::time::timeout(
        std::time::Duration::from_secs(600),
        Command::new("docker")
            .args([
                "build",
                "-t", &image_tag,
                "-t", &latest_tag,
                "-f", dockerfile_path,
                ".",
            ])
            .current_dir(&repo_dir)
            .output(),
    )
    .await
    .map_err(|_| "docker build timed out (600s)".to_string())?
    .map_err(|e| format!("docker build failed: {e}"))?;

    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr),
    );

    // Truncate to 100KB
    let output = if output.len() > 100_000 {
        format!("{}...\n[output truncated]", &output[..100_000])
    } else {
        output
    };

    if !build.status.success() {
        return Err(format!("docker build failed:\n{output}"));
    }

    tracing::info!("Image built successfully: {image_tag}");

    Ok(BuildResult { image_tag, output })
}

/// Deploy or update a container from a locally-built git image.
///
/// - New container: create + start. If domain is provided, set up nginx reverse proxy.
/// - Existing container with domain + nginx config: blue-green zero-downtime update.
/// - Existing container without domain: stop old, remove, create new, start.
pub async fn deploy_or_update(
    name: &str,
    image_tag: &str,
    container_port: u16,
    host_port: u16,
    env_vars: HashMap<String, String>,
    domain: Option<&str>,
    templates: &Tera,
    memory_mb: Option<u64>,
    cpu_percent: Option<u64>,
) -> Result<GitDeployResult, String> {
    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;

    let container_name = format!("dockpanel-git-{name}");

    // Build environment list
    let env_list: Vec<String> = env_vars
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();

    // Build labels
    let mut labels = HashMap::from([
        ("dockpanel.managed".to_string(), "true".to_string()),
        ("dockpanel.type".to_string(), "git".to_string()),
        ("dockpanel.git.name".to_string(), name.to_string()),
    ]);
    if let Some(d) = domain {
        labels.insert("dockpanel.app.domain".to_string(), d.to_string());
    }

    // Port bindings: 127.0.0.1:{host_port} -> {container_port}/tcp
    let container_port_key = format!("{container_port}/tcp");
    let mut port_bindings = HashMap::new();
    port_bindings.insert(
        container_port_key.clone(),
        Some(vec![bollard::service::PortBinding {
            host_ip: Some("127.0.0.1".to_string()),
            host_port: Some(host_port.to_string()),
        }]),
    );

    let mut exposed_ports = HashMap::new();
    exposed_ports.insert(container_port_key, HashMap::new());

    let mut host_config = bollard::service::HostConfig {
        port_bindings: Some(port_bindings),
        restart_policy: Some(bollard::service::RestartPolicy {
            name: Some(bollard::service::RestartPolicyNameEnum::UNLESS_STOPPED),
            ..Default::default()
        }),
        ..Default::default()
    };

    if let Some(mem) = memory_mb {
        if mem > 0 {
            host_config.memory = Some((mem * 1024 * 1024) as i64);
            host_config.memory_swap = Some((mem * 2 * 1024 * 1024) as i64);
        }
    }
    if let Some(cpu) = cpu_percent {
        if cpu > 0 && cpu <= 100 {
            host_config.cpu_period = Some(100_000);
            host_config.cpu_quota = Some((cpu * 1000) as i64);
        }
    }

    // Check if container already exists
    let existing = find_container(&docker, &container_name).await;

    match existing {
        Some((container_id, existing_domain, existing_port)) => {
            // Container exists — check if blue-green is possible
            let has_nginx = existing_domain.is_some()
                && existing_port.is_some()
                && std::path::Path::new(&format!(
                    "/etc/nginx/sites-enabled/{}.conf",
                    existing_domain.as_deref().unwrap_or("")
                ))
                .exists();

            if has_nginx {
                let bg_domain = existing_domain.as_deref().unwrap();
                let old_port = existing_port.unwrap();

                tracing::info!(
                    "Blue-green update for git app {name}: domain={bg_domain}, old_port={old_port}"
                );

                return blue_green_update(
                    &docker,
                    &container_id,
                    &container_name,
                    image_tag,
                    &env_list,
                    &labels,
                    container_port,
                    old_port,
                    bg_domain,
                    &host_config,
                )
                .await;
            }

            // No domain/nginx — stop + remove + recreate
            tracing::info!("Replacing git container {container_name} (no domain, stop/start)");

            docker
                .stop_container(&container_id, Some(StopContainerOptions { t: 10 }))
                .await
                .ok();
            docker
                .remove_container(
                    &container_id,
                    Some(RemoveContainerOptions {
                        force: true,
                        v: false,
                        ..Default::default()
                    }),
                )
                .await
                .ok();

            let result = create_and_start(
                &docker,
                &container_name,
                image_tag,
                &env_list,
                &labels,
                &exposed_ports,
                host_config,
            )
            .await?;

            Ok(GitDeployResult {
                container_id: result,
                blue_green: false,
            })
        }
        None => {
            // Fresh deploy
            tracing::info!("Deploying new git container: {container_name}");

            let container_id = create_and_start(
                &docker,
                &container_name,
                image_tag,
                &env_list,
                &labels,
                &exposed_ports,
                host_config,
            )
            .await?;

            // Set up nginx reverse proxy if domain is provided
            if let Some(d) = domain {
                setup_nginx_proxy(templates, d, host_port).await?;
            }

            Ok(GitDeployResult {
                container_id,
                blue_green: false,
            })
        }
    }
}

/// Stop and remove a git-deployed container, plus its nginx config, SSL certs, and volume dir.
pub async fn cleanup_container(name: &str) -> Result<(), String> {
    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;

    let container_name = format!("dockpanel-git-{name}");

    // Inspect to find domain label before removing
    let domain = if let Ok(info) = docker.inspect_container(&container_name, None).await {
        info.config
            .and_then(|c| c.labels)
            .and_then(|l| l.get("dockpanel.app.domain").cloned())
    } else {
        None
    };

    // Stop container
    docker
        .stop_container(&container_name, Some(StopContainerOptions { t: 10 }))
        .await
        .ok();

    // Remove container
    docker
        .remove_container(
            &container_name,
            Some(RemoveContainerOptions {
                force: true,
                v: false,
                ..Default::default()
            }),
        )
        .await
        .ok();

    tracing::info!("Removed git container: {container_name}");

    // Remove nginx config
    if let Some(ref d) = domain {
        let config_path = format!("/etc/nginx/sites-enabled/{d}.conf");
        if std::path::Path::new(&config_path).exists() {
            std::fs::remove_file(&config_path).ok();
            tracing::info!("Removed nginx config: {config_path}");

            // Reload nginx after removing config
            match crate::services::nginx::test_config().await {
                Ok(output) if output.success => {
                    crate::services::nginx::reload().await.ok();
                }
                _ => {
                    tracing::warn!("Nginx test failed after removing config for {d}");
                }
            }
        }

        // Remove SSL certificates
        let ssl_dir = format!("/etc/dockpanel/ssl/{d}");
        if std::path::Path::new(&ssl_dir).exists() {
            std::fs::remove_dir_all(&ssl_dir).ok();
            tracing::info!("Removed SSL certs: {ssl_dir}");
        }
    }

    // Remove git repo / volume directory
    let repo_dir = format!("{GIT_BASE_DIR}/{name}");
    if std::path::Path::new(&repo_dir).exists() {
        std::fs::remove_dir_all(&repo_dir).ok();
        tracing::info!("Removed git repo dir: {repo_dir}");
    }

    Ok(())
}

/// Prune old images for a git app, keeping the last `keep` images (by creation time).
/// The `:latest` tag is always excluded from pruning.
pub async fn prune_images(name: &str, keep: usize) -> Result<Vec<String>, String> {
    let image_prefix = format!("dockpanel-git-{name}");

    // List all images via CLI to get tags and creation times
    let output = Command::new("docker")
        .args([
            "images",
            "--format", "{{.Repository}}:{{.Tag}} {{.CreatedAt}}",
            &image_prefix,
        ])
        .output()
        .await
        .map_err(|e| format!("Failed to list images: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("docker images failed: {stderr}"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut images: Vec<(&str, &str)> = Vec::new();

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Format: "repo:tag YYYY-MM-DD HH:MM:SS ..."
        if let Some(space_idx) = line.find(' ') {
            let image_ref = &line[..space_idx];
            let created_at = &line[space_idx + 1..];

            // Skip :latest tag
            if image_ref.ends_with(":latest") {
                continue;
            }

            // Only include images matching our prefix
            if image_ref.starts_with(&image_prefix) {
                images.push((image_ref, created_at));
            }
        }
    }

    // Sort by creation time descending (newest first)
    images.sort_by(|a, b| b.1.cmp(a.1));

    // Skip the first `keep` images, remove the rest
    let mut removed = Vec::new();

    if images.len() > keep {
        for (image_ref, _) in &images[keep..] {
            let rm = Command::new("docker")
                .args(["rmi", image_ref])
                .output()
                .await;

            match rm {
                Ok(o) if o.status.success() => {
                    tracing::info!("Pruned image: {image_ref}");
                    removed.push(image_ref.to_string());
                }
                Ok(o) => {
                    let stderr = String::from_utf8_lossy(&o.stderr);
                    tracing::warn!("Failed to prune image {image_ref}: {stderr}");
                }
                Err(e) => {
                    tracing::warn!("Failed to prune image {image_ref}: {e}");
                }
            }
        }
    }

    Ok(removed)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Find an existing container by name. Returns (id, domain label, host port).
async fn find_container(
    docker: &Docker,
    container_name: &str,
) -> Option<(String, Option<String>, Option<u16>)> {
    let mut filters = HashMap::new();
    filters.insert("name", vec![container_name]);

    let containers = docker
        .list_containers(Some(ListContainersOptions {
            all: true,
            filters,
            ..Default::default()
        }))
        .await
        .ok()?;

    // Find exact match (list_containers does substring matching)
    let container = containers.iter().find(|c| {
        c.names
            .as_ref()
            .map(|names| names.iter().any(|n| n.trim_start_matches('/') == container_name))
            .unwrap_or(false)
    })?;

    let id = container.id.clone()?;

    let domain = container
        .labels
        .as_ref()
        .and_then(|l| l.get("dockpanel.app.domain").cloned());

    let host_port = container
        .ports
        .as_ref()
        .and_then(|ports| {
            ports.iter().find_map(|p| p.public_port)
        })
        .map(|p| p as u16);

    Some((id, domain, host_port))
}

/// Create and start a container. Returns the container ID.
async fn create_and_start(
    docker: &Docker,
    container_name: &str,
    image_tag: &str,
    env_list: &[String],
    labels: &HashMap<String, String>,
    exposed_ports: &HashMap<String, HashMap<(), ()>>,
    host_config: bollard::service::HostConfig,
) -> Result<String, String> {
    let config = Config {
        image: Some(image_tag.to_string()),
        env: if env_list.is_empty() {
            None
        } else {
            Some(env_list.to_vec())
        },
        exposed_ports: Some(exposed_ports.clone()),
        host_config: Some(host_config),
        labels: Some(labels.clone()),
        ..Default::default()
    };

    let container = docker
        .create_container(
            Some(CreateContainerOptions {
                name: container_name,
                platform: None,
            }),
            config,
        )
        .await
        .map_err(|e| format!("Failed to create container: {e}"))?;

    if let Err(e) = docker
        .start_container(&container.id, None::<StartContainerOptions<String>>)
        .await
    {
        // Clean up orphaned container on start failure
        docker
            .remove_container(
                &container.id,
                Some(RemoveContainerOptions {
                    force: true,
                    v: false,
                    ..Default::default()
                }),
            )
            .await
            .ok();
        return Err(format!("Failed to start container: {e}"));
    }

    tracing::info!("Container started: {container_name} ({})", &container.id[..12]);

    Ok(container.id)
}

/// Set up an nginx reverse proxy for a domain pointing to host_port.
async fn setup_nginx_proxy(
    templates: &Tera,
    domain: &str,
    host_port: u16,
) -> Result<(), String> {
    let site_config = crate::routes::nginx::SiteConfig {
        runtime: "proxy".to_string(),
        root: None,
        proxy_port: Some(host_port),
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

    let rendered = crate::services::nginx::render_site_config(templates, domain, &site_config)
        .map_err(|e| format!("Failed to render nginx config: {e}"))?;

    let config_path = format!("/etc/nginx/sites-enabled/{domain}.conf");
    let tmp_path = format!("{config_path}.tmp");

    std::fs::write(&tmp_path, &rendered)
        .map_err(|e| format!("Failed to write nginx config: {e}"))?;

    std::fs::rename(&tmp_path, &config_path).map_err(|e| {
        std::fs::remove_file(&tmp_path).ok();
        format!("Failed to activate nginx config: {e}")
    })?;

    match crate::services::nginx::test_config().await {
        Ok(output) if output.success => {
            crate::services::nginx::reload().await.ok();
            tracing::info!("Nginx proxy configured for {domain} -> port {host_port}");
        }
        _ => {
            std::fs::remove_file(&config_path).ok();
            return Err(format!(
                "Nginx config test failed for {domain}, config removed"
            ));
        }
    }

    Ok(())
}

/// Blue-green zero-downtime update for a git container behind nginx.
///
/// 1. Find a free temp port
/// 2. Create new container `{name}-blue` with temp port
/// 3. Health check the new container
/// 4. Swap nginx proxy_pass to temp port
/// 5. Test + reload nginx
/// 6. Stop + remove old container
/// 7. Rename new container to original name
/// 8. On any failure: rollback (remove new container, restore nginx)
async fn blue_green_update(
    docker: &Docker,
    old_container_id: &str,
    container_name: &str,
    image_tag: &str,
    env_list: &[String],
    labels: &HashMap<String, String>,
    container_port: u16,
    old_port: u16,
    domain: &str,
    _base_host_config: &bollard::service::HostConfig,
) -> Result<GitDeployResult, String> {
    let temp_port = crate::services::docker_apps::find_free_port()?;
    tracing::info!(
        "Blue-green update for {container_name}: old_port={old_port}, temp_port={temp_port}"
    );

    let blue_name = format!("{container_name}-blue");

    // Clean up stale blue container from a failed previous attempt
    if docker.inspect_container(&blue_name, None).await.is_ok() {
        docker
            .stop_container(&blue_name, Some(StopContainerOptions { t: 5 }))
            .await
            .ok();
        docker
            .remove_container(
                &blue_name,
                Some(RemoveContainerOptions {
                    force: true,
                    v: false,
                    ..Default::default()
                }),
            )
            .await
            .ok();
    }

    // Build port bindings for the temp port
    let container_port_key = format!("{container_port}/tcp");
    let mut port_bindings = HashMap::new();
    port_bindings.insert(
        container_port_key.clone(),
        Some(vec![bollard::service::PortBinding {
            host_ip: Some("127.0.0.1".to_string()),
            host_port: Some(temp_port.to_string()),
        }]),
    );

    let mut exposed_ports = HashMap::new();
    exposed_ports.insert(container_port_key, HashMap::new());

    let host_config = bollard::service::HostConfig {
        port_bindings: Some(port_bindings),
        restart_policy: Some(bollard::service::RestartPolicy {
            name: Some(bollard::service::RestartPolicyNameEnum::UNLESS_STOPPED),
            ..Default::default()
        }),
        ..Default::default()
    };

    let config = Config {
        image: Some(image_tag.to_string()),
        env: if env_list.is_empty() {
            None
        } else {
            Some(env_list.to_vec())
        },
        exposed_ports: Some(exposed_ports),
        host_config: Some(host_config),
        labels: Some(labels.clone()),
        ..Default::default()
    };

    // Create the blue container
    let new_container = docker
        .create_container(
            Some(CreateContainerOptions {
                name: blue_name.as_str(),
                platform: None,
            }),
            config,
        )
        .await
        .map_err(|e| format!("Failed to create blue container: {e}"))?;

    // Start the blue container
    if let Err(e) = docker
        .start_container(&new_container.id, None::<StartContainerOptions<String>>)
        .await
    {
        cleanup_blue(docker, &new_container.id).await;
        return Err(format!("Failed to start blue container: {e}"));
    }

    // Health check (30s timeout)
    if let Err(e) = crate::services::docker_apps::health_check_port(temp_port, 30).await {
        cleanup_blue(docker, &new_container.id).await;
        return Err(format!("Blue container health check failed: {e}"));
    }

    // Swap nginx proxy_pass to the new port
    if let Err(e) = crate::services::docker_apps::swap_nginx_proxy_port(domain, old_port, temp_port)
    {
        cleanup_blue(docker, &new_container.id).await;
        return Err(format!("Nginx port swap failed: {e}"));
    }

    // Test nginx config and reload
    match crate::services::nginx::test_config().await {
        Ok(output) if output.success => {
            if let Err(e) = crate::services::nginx::reload().await {
                // Rollback nginx + cleanup blue
                crate::services::docker_apps::swap_nginx_proxy_port(domain, temp_port, old_port)
                    .ok();
                cleanup_blue(docker, &new_container.id).await;
                return Err(format!("Nginx reload failed: {e}"));
            }
        }
        Ok(output) => {
            crate::services::docker_apps::swap_nginx_proxy_port(domain, temp_port, old_port).ok();
            cleanup_blue(docker, &new_container.id).await;
            return Err(format!("Nginx config test failed: {}", output.stderr));
        }
        Err(e) => {
            crate::services::docker_apps::swap_nginx_proxy_port(domain, temp_port, old_port).ok();
            cleanup_blue(docker, &new_container.id).await;
            return Err(format!("Nginx test error: {e}"));
        }
    }

    // Traffic is now flowing to the new container. Stop and remove old container.
    docker
        .stop_container(old_container_id, Some(StopContainerOptions { t: 10 }))
        .await
        .ok();
    docker
        .remove_container(
            old_container_id,
            Some(RemoveContainerOptions {
                force: true,
                v: false,
                ..Default::default()
            }),
        )
        .await
        .ok();

    // Rename blue container to the original name
    docker
        .rename_container(
            &new_container.id,
            RenameContainerOptions {
                name: container_name.to_string(),
            },
        )
        .await
        .ok();

    tracing::info!(
        "Git app updated (blue-green, zero-downtime): {container_name}, port {old_port} -> {temp_port}"
    );

    Ok(GitDeployResult {
        container_id: new_container.id,
        blue_green: true,
    })
}

/// Stop and force-remove a blue container during rollback.
async fn cleanup_blue(docker: &Docker, container_id: &str) {
    docker
        .stop_container(container_id, Some(StopContainerOptions { t: 5 }))
        .await
        .ok();
    docker
        .remove_container(
            container_id,
            Some(RemoveContainerOptions {
                force: true,
                v: false,
                ..Default::default()
            }),
        )
        .await
        .ok();
}
