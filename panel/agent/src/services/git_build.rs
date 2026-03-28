use bollard::container::{
    Config, CreateContainerOptions, ListContainersOptions, RemoveContainerOptions,
    RenameContainerOptions, StartContainerOptions, StopContainerOptions,
};
use bollard::Docker;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::OnceLock;
use tera::Tera;
use crate::safe_cmd::safe_command;

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
        let mut cmd = safe_command("git");
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
        let reset = safe_command("git")
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

        let mut cmd = safe_command("git");
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
    let hash_output = safe_command("git")
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
    let msg_output = safe_command("git")
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
/// Uses BuildKit for layer caching, supports build args and custom build context.
pub async fn build_image(
    name: &str,
    dockerfile_path: &str,
    commit_hash: &str,
    build_args: &HashMap<String, String>,
    build_context: &str,
) -> Result<BuildResult, String> {
    let deploy_dir = format!("{GIT_BASE_DIR}/{name}");
    let image_name = format!("dockpanel-git-{name}");
    let image_tag = format!("{image_name}:{commit_hash}");
    let latest_tag = format!("{image_name}:latest");

    // Validate build context (no path traversal)
    if build_context.contains("..") {
        return Err("Build context must not contain '..'".into());
    }
    let context_dir = if build_context == "." {
        deploy_dir.clone()
    } else {
        format!("{deploy_dir}/{build_context}")
    };
    if !std::path::Path::new(&context_dir).exists() {
        return Err(format!("Build context directory not found: {build_context}"));
    }

    tracing::info!("Building image {image_tag} from {deploy_dir} (context: {build_context})");

    let mut cmd_args: Vec<String> = vec![
        "build".into(),
        "--cache-from".into(), latest_tag.clone(),
    ];
    for (k, v) in build_args {
        cmd_args.push("--build-arg".into());
        cmd_args.push(format!("{k}={v}"));
    }
    // Dockerfile path: when build_context is a subdirectory, prefix it
    let full_dockerfile = if build_context == "." {
        dockerfile_path.to_string()
    } else {
        format!("{build_context}/{dockerfile_path}")
    };
    cmd_args.extend([
        "-t".into(), image_tag.clone(),
        "-t".into(), latest_tag.clone(),
        "-f".into(), full_dockerfile,
        context_dir.clone(),
    ]);

    let build = tokio::time::timeout(
        std::time::Duration::from_secs(600),
        safe_command("docker")
            .args(&cmd_args)
            .env("DOCKER_BUILDKIT", "1")
            .current_dir(&deploy_dir)
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
    ssl_email: Option<&str>,
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

                // After successful nginx setup for initial deploy with domain
                if let Some(email) = ssl_email {
                    // DNS propagation wait
                    for i in 0..6u32 {
                        if i > 0 { tokio::time::sleep(std::time::Duration::from_secs(5)).await; }
                        match tokio::net::TcpStream::connect(format!("{}:80", d)).await {
                            Ok(_) => break,
                            Err(_) if i < 5 => continue,
                            Err(_) => break,
                        }
                    }
                    match crate::services::ssl::load_or_create_account(email).await {
                        Ok(account) => {
                            match crate::services::ssl::provision_cert(&account, d).await {
                                Ok(_) => {
                                    let ssl_config = crate::routes::nginx::SiteConfig {
                                        runtime: "proxy".to_string(), root: None,
                                        proxy_port: Some(host_port), php_socket: None,
                                        ssl: None, ssl_cert: None, ssl_key: None,
                                        rate_limit: None, max_upload_mb: None,
                                        php_memory_mb: None, php_max_workers: None,
                                        custom_nginx: None, php_preset: None, app_command: None,
                                        fastcgi_cache: None,
                                    };
                                    if let Ok(()) = crate::services::ssl::enable_ssl_for_site(templates, d, &ssl_config).await {
                                        tracing::info!("Auto-SSL: certificate provisioned for {d}");
                                    }
                                }
                                Err(e) => tracing::warn!("Auto-SSL: cert provisioning failed for {d}: {e}"),
                            }
                        }
                        Err(e) => tracing::warn!("Auto-SSL: ACME account failed for {d}: {e}"),
                    }
                }
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
    let output = safe_command("docker")
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
            let rm = safe_command("docker")
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

/// Auto-detect language and generate a Dockerfile if none exists.
/// Returns the dockerfile path to use (either the existing one or "Dockerfile" for the generated one).
pub fn auto_generate_dockerfile(name: &str, dockerfile_path: &str, build_context: &str) -> Result<String, String> {
    let deploy_dir = format!("{GIT_BASE_DIR}/{name}");
    let context_dir = if build_context == "." { deploy_dir.clone() } else { format!("{deploy_dir}/{build_context}") };
    let df_path = std::path::Path::new(&context_dir).join(dockerfile_path);

    // If Dockerfile exists, use it as-is
    if df_path.exists() {
        return Ok(dockerfile_path.to_string());
    }

    tracing::info!("No Dockerfile found at {dockerfile_path} in {context_dir}, auto-detecting...");

    let generated = if std::path::Path::new(&context_dir).join("package.json").exists() {
        // Node.js
        let pkg = std::fs::read_to_string(std::path::Path::new(&context_dir).join("package.json")).unwrap_or_default();
        let has_build = pkg.contains("\"build\"");
        let has_next = pkg.contains("\"next\"");
        let has_nuxt = pkg.contains("\"nuxt\"");

        if has_next {
            // Next.js
            "FROM node:20-alpine AS builder\nWORKDIR /app\nCOPY package*.json ./\nRUN npm install\nCOPY . .\nRUN npm run build\n\nFROM node:20-alpine\nWORKDIR /app\nCOPY --from=builder /app/.next ./.next\nCOPY --from=builder /app/node_modules ./node_modules\nCOPY --from=builder /app/package.json ./\nCOPY --from=builder /app/public ./public\nEXPOSE 3000\nCMD [\"npm\", \"start\"]\n".to_string()
        } else if has_nuxt {
            // Nuxt
            "FROM node:20-alpine AS builder\nWORKDIR /app\nCOPY package*.json ./\nRUN npm install\nCOPY . .\nRUN npm run build\n\nFROM node:20-alpine\nWORKDIR /app\nCOPY --from=builder /app/.output ./.output\nEXPOSE 3000\nCMD [\"node\", \".output/server/index.mjs\"]\n".to_string()
        } else if has_build {
            // Generic Node.js with build step (SPA/React/Vue)
            "FROM node:20-alpine AS builder\nWORKDIR /app\nCOPY package*.json ./\nRUN npm install\nCOPY . .\nRUN npm run build\n\nFROM nginx:alpine\nCOPY --from=builder /app/dist /usr/share/nginx/html\nEXPOSE 80\n".to_string()
        } else {
            // Plain Node.js server
            "FROM node:20-alpine\nWORKDIR /app\nCOPY package*.json ./\nRUN npm install --omit=dev\nCOPY . .\nEXPOSE 3000\nCMD [\"node\", \"index.js\"]\n".to_string()
        }
    } else if std::path::Path::new(&context_dir).join("requirements.txt").exists() {
        // Python
        let reqs = std::fs::read_to_string(std::path::Path::new(&context_dir).join("requirements.txt"))
            .unwrap_or_default().to_lowercase();
        let has_django = reqs.contains("django");
        let has_flask = reqs.contains("flask");

        if has_django {
            "FROM python:3.12-slim\nWORKDIR /app\nCOPY requirements.txt .\nRUN pip install --no-cache-dir -r requirements.txt\nCOPY . .\nRUN python manage.py collectstatic --noinput 2>/dev/null || true\nEXPOSE 8000\nCMD [\"gunicorn\", \"--bind\", \"0.0.0.0:8000\", \"--workers\", \"2\", \"config.wsgi:application\"]\n".to_string()
        } else if has_flask {
            "FROM python:3.12-slim\nWORKDIR /app\nCOPY requirements.txt .\nRUN pip install --no-cache-dir -r requirements.txt\nCOPY . .\nEXPOSE 5000\nCMD [\"gunicorn\", \"--bind\", \"0.0.0.0:5000\", \"--workers\", \"2\", \"app:app\"]\n".to_string()
        } else {
            "FROM python:3.12-slim\nWORKDIR /app\nCOPY requirements.txt .\nRUN pip install --no-cache-dir -r requirements.txt\nCOPY . .\nEXPOSE 8000\nCMD [\"python\", \"app.py\"]\n".to_string()
        }
    } else if std::path::Path::new(&context_dir).join("go.mod").exists() {
        // Go
        "FROM golang:1.24-alpine AS builder\nWORKDIR /app\nCOPY go.mod go.sum ./\nRUN go mod download\nCOPY . .\nRUN CGO_ENABLED=0 go build -o server .\n\nFROM alpine:3.20\nWORKDIR /app\nCOPY --from=builder /app/server .\nEXPOSE 8080\nCMD [\"./server\"]\n".to_string()
    } else if std::path::Path::new(&context_dir).join("Cargo.toml").exists() {
        // Rust
        "FROM rust:1.94-slim AS builder\nWORKDIR /app\nCOPY . .\nRUN cargo build --release\n\nFROM debian:bookworm-slim\nCOPY --from=builder /app/target/release/* /usr/local/bin/\nEXPOSE 8080\nCMD [\"app\"]\n".to_string()
    } else if std::path::Path::new(&context_dir).join("composer.json").exists() {
        // PHP/Laravel
        "FROM php:8.3-fpm-alpine\nRUN apk add --no-cache nginx\nWORKDIR /app\nCOPY . .\nRUN curl -sS https://getcomposer.org/installer | php && php composer.phar install --no-dev --optimize-autoloader\nEXPOSE 80\nCMD [\"php\", \"-S\", \"0.0.0.0:80\", \"-t\", \"public\"]\n".to_string()
    } else if std::path::Path::new(&context_dir).join("Gemfile").exists() {
        // Ruby
        "FROM ruby:3.3-slim\nWORKDIR /app\nCOPY Gemfile Gemfile.lock ./\nRUN bundle install --without development test\nCOPY . .\nEXPOSE 3000\nCMD [\"bundle\", \"exec\", \"rails\", \"server\", \"-b\", \"0.0.0.0\"]\n".to_string()
    } else if std::path::Path::new(&context_dir).join("index.html").exists() {
        // Static site
        "FROM nginx:alpine\nCOPY . /usr/share/nginx/html\nEXPOSE 80\n".to_string()
    } else {
        return Err("No Dockerfile found and could not auto-detect project type. Supported: Node.js (package.json), Python (requirements.txt), Go (go.mod), Rust (Cargo.toml), PHP (composer.json), Ruby (Gemfile), Static (index.html)".into());
    };

    // Write generated Dockerfile
    let generated_path = std::path::Path::new(&context_dir).join("Dockerfile");
    std::fs::write(&generated_path, &generated)
        .map_err(|e| format!("Failed to write generated Dockerfile: {e}"))?;

    tracing::info!("Auto-generated Dockerfile for {name} in {context_dir}");
    Ok("Dockerfile".to_string())
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
        fastcgi_cache: None,
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
    base_host_config: &bollard::service::HostConfig,
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
        memory: base_host_config.memory,
        memory_swap: base_host_config.memory_swap,
        cpu_period: base_host_config.cpu_period,
        cpu_quota: base_host_config.cpu_quota,
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

// ---------------------------------------------------------------------------
// Nixpacks support
// ---------------------------------------------------------------------------

static NIXPACKS_PATH: OnceLock<Option<String>> = OnceLock::new();

/// Ensure nixpacks binary is available. Downloads on first use if not found.
pub async fn ensure_nixpacks() -> Option<String> {
    // Check cache
    if let Some(cached) = NIXPACKS_PATH.get() {
        return cached.clone();
    }

    // Check if already installed
    let check = safe_command("which")
        .arg("nixpacks")
        .output()
        .await;
    if let Ok(out) = check {
        if out.status.success() {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let _ = NIXPACKS_PATH.set(Some(path.clone()));
            return Some(path);
        }
    }

    // Try to download nixpacks
    tracing::info!("Nixpacks not found, downloading...");
    let arch = if cfg!(target_arch = "aarch64") { "aarch64" } else { "x86_64" };

    // Get latest release tag from GitHub API (no hardcoded version)
    let tag_cmd = safe_command("sh")
        .arg("-c")
        .arg("curl -sI https://github.com/railwayapp/nixpacks/releases/latest | grep -i '^location:' | sed 's|.*/tag/||' | tr -d '\\r\\n'")
        .output()
        .await;

    let version = if let Ok(ref out) = tag_cmd {
        let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
        // Validate version format strictly
        if v.starts_with('v') && v[1..].chars().all(|c| c.is_ascii_digit() || c == '.') && v.contains('.') {
            v
        } else {
            "v1.30.0".to_string()
        }
    } else {
        "v1.30.0".to_string()
    };

    let url = format!(
        "https://github.com/railwayapp/nixpacks/releases/download/{version}/nixpacks-{version}-{arch}-unknown-linux-musl.tar.gz"
    );
    tracing::info!("Downloading nixpacks {version} from {url}");

    let download = safe_command("sh")
        .arg("-c")
        .arg(format!(
            "curl -fsSL '{url}' | tar xz -C /tmp && mv /tmp/nixpacks /usr/local/bin/nixpacks && chmod +x /usr/local/bin/nixpacks"
        ))
        .output()
        .await;

    match download {
        Ok(out) if out.status.success() => {
            tracing::info!("Nixpacks installed to /usr/local/bin/nixpacks");
            let _ = NIXPACKS_PATH.set(Some("/usr/local/bin/nixpacks".into()));
            Some("/usr/local/bin/nixpacks".into())
        }
        Ok(out) => {
            tracing::warn!("Failed to download nixpacks: {}", String::from_utf8_lossy(&out.stderr));
            let _ = NIXPACKS_PATH.set(None);
            None
        }
        Err(e) => {
            tracing::warn!("Failed to download nixpacks: {e}");
            let _ = NIXPACKS_PATH.set(None);
            None
        }
    }
}

/// Build a Docker image using nixpacks (auto-detects language, no Dockerfile needed).
/// Returns (image_tag, build_output) on success.
pub async fn nixpacks_build(
    name: &str,
    commit_hash: &str,
    build_context: &str,
    env_vars: &std::collections::HashMap<String, String>,
) -> Result<(String, String), String> {
    let nixpacks_bin = ensure_nixpacks().await
        .ok_or_else(|| "Nixpacks not available".to_string())?;

    if build_context.contains("..") {
        return Err("Build context must not contain path traversal (..)".into());
    }

    let image_tag = format!("dockpanel-git-{name}:{commit_hash}");
    let context_dir = format!("/var/lib/dockpanel/git/{name}/{build_context}");

    // Set up persistent cache directory for faster rebuilds
    let cache_dir = format!("/var/cache/dockpanel/nixpacks/{name}");
    std::fs::create_dir_all(&cache_dir).ok();

    // Build nixpacks command
    let mut cmd = safe_command(&nixpacks_bin);
    cmd.arg("build")
        .arg(&context_dir)
        .arg("--name")
        .arg(&image_tag)
        .arg("--cache-key")
        .arg(name);

    // Set cache directory via environment variable
    cmd.env("NIXPACKS_CACHE_DIR", &cache_dir);

    // Pass environment variables
    for (key, value) in env_vars {
        cmd.arg("--env").arg(format!("{key}={value}"));
    }

    tracing::info!("Nixpacks build: {image_tag} from {context_dir} (cache: {cache_dir})");

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(600),
        cmd.output(),
    )
    .await
    .map_err(|_| "Nixpacks build timed out (600s)".to_string())?
    .map_err(|e| format!("Nixpacks build failed to start: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let full_output = format!("{stdout}\n{stderr}");

    if !output.status.success() {
        return Err(format!("Nixpacks build failed:\n{full_output}"));
    }

    // Also tag as :latest
    let _ = safe_command("docker")
        .args(["tag", &image_tag, &format!("dockpanel-git-{name}:latest")])
        .output()
        .await;

    tracing::info!("Nixpacks build succeeded: {image_tag}");
    Ok((image_tag, full_output))
}
