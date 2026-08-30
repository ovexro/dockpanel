use bollard::container::{
    Config, CreateContainerOptions, ListContainersOptions, RemoveContainerOptions,
    RenameContainerOptions, StartContainerOptions, StopContainerOptions,
};
use bollard::Docker;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::OnceLock;
use tera::Tera;
use crate::routes::docker_apps::TlsIntent;
use crate::safe_cmd::safe_command;
use crate::services::ownership;

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
    /// Whether THIS call touched TLS at all. `None` for a blue-green update,
    /// which swaps only the backend port on the existing vhost file and never
    /// re-renders it (see `blue_green_update`), so whatever certificate story
    /// the domain already had is untouched and there is nothing new to
    /// report. `Some` for a fresh deploy or a stop/recreate that rendered a
    /// vhost — mirrors `docker_apps::expose_domain`'s response shape so the
    /// panel's existing `provided_tls_refusal`-style check works unchanged.
    pub ssl: Option<bool>,
    pub tls_mode: Option<&'static str>,
    pub tls_certificate: Option<String>,
    pub tls_warning: Option<String>,
}

/// What securing a domain actually achieved, from [`apply_tls`].
struct TlsOutcome {
    ssl: bool,
    tls_mode: &'static str,
    tls_certificate: Option<String>,
    warning: Option<String>,
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
        // 900s, not 600s: this build now also carries whatever pre-build
        // install step the operator configured (folded into a RUN line
        // instead of a separate host-exec step — see auto_generate_dockerfile),
        // absorbing that step's old, separate 300s budget.
        std::time::Duration::from_secs(900),
        safe_command("docker")
            .args(&cmd_args)
            .env("DOCKER_BUILDKIT", "1")
            .current_dir(&deploy_dir)
            // safe_command sets no kill_on_drop, so a timed-out (dropped)
            // future would otherwise leave `docker build` running in the
            // background — see database.rs:317, migration.rs:91,
            // database_backup.rs:424, security_scanner.rs:633 for the same fix.
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| "docker build timed out (900s)".to_string())?
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

/// Put a domain in front of a git deploy's local port, secured the way `tls`
/// asks — mirrors `docker_apps::expose_domain`'s `TlsIntent` handling so a git
/// deploy gets the same guarantee a Docker/Compose deploy already has for the
/// SAME reason: `Provided` renders straight to HTTPS through the registered
/// certificate and is the ONLY arm that reads it, `Acme` orders a Let's
/// Encrypt certificate exactly as this deploy path always has, and `None` is
/// the plain-HTTP proxy `setup_nginx_proxy` has always written.
///
/// Unlike `expose_domain`, a refused `Provided` certificate here falls back to
/// plain HTTP rather than leaving the vhost untouched. `expose_domain` protects
/// an EXISTING `:443` block from being silently downgraded (the HSTS-outage
/// trap its own doc comment describes at length) — but both call sites this
/// function has only reach it when there is nothing to protect: a brand new
/// container's first vhost, or the stop/recreate path's write of a domain
/// that just changed (a path that never held THIS domain before). Falling
/// back leaves the deploy reachable, with the refusal surfaced in
/// [`TlsOutcome::warning`] rather than swallowed — never a silent downgrade
/// of a certificate that was already live, because that case never reaches
/// this function (blue-green swaps only the backend port on the untouched
/// existing file — see `blue_green_update`).
async fn apply_tls(
    templates: &Tera,
    domain: &str,
    host_port: u16,
    tls: TlsIntent<'_>,
) -> Result<TlsOutcome, String> {
    match tls {
        TlsIntent::Provided { alias } => {
            let (cert_path, key_path) = crate::services::ssl::registry_paths(alias);
            let pem = match std::fs::read_to_string(&cert_path) {
                Ok(pem) => pem,
                Err(e) => {
                    tracing::warn!(
                        "Git deploy: {domain} names registered certificate {alias}, which is not \
                         on this server ({e}); falling back to plain HTTP"
                    );
                    setup_nginx_proxy(templates, domain, host_port).await?;
                    return Ok(TlsOutcome {
                        ssl: false,
                        tls_mode: "provided",
                        tls_certificate: Some(alias.to_string()),
                        warning: Some(format!(
                            "no certificate named {alias} is registered on this server ({e}); \
                             served over plain HTTP instead. Register the certificate and redeploy."
                        )),
                    });
                }
            };
            // Binding point 3 of #104, re-asked here: the panel already checked
            // at claim time, but the pairing on disk may have been replaced
            // since — the same defence in depth `expose_domain` applies.
            if let Err(reason) = crate::services::ssl::cert_covers_domain(&pem, domain) {
                tracing::warn!(
                    "Git deploy: registered certificate {alias} does not cover {domain}: {reason}"
                );
                setup_nginx_proxy(templates, domain, host_port).await?;
                return Ok(TlsOutcome {
                    ssl: false,
                    tls_mode: "provided",
                    tls_certificate: Some(alias.to_string()),
                    warning: Some(format!(
                        "the registered certificate {alias} cannot serve {domain}: {reason} \
                         Served over plain HTTP instead."
                    )),
                });
            }
            // Rendered through the ordinary renderer with the registry paths
            // named outright — NOT through `enable_ssl_for_site`, which
            // hardcodes the per-domain tree and would point the vhost at a
            // directory this certificate is deliberately not in.
            let site_config = crate::routes::nginx::SiteConfig {
                runtime: "proxy".to_string(),
                root: None,
                proxy_port: Some(host_port),
                php_socket: None,
                ssl: Some(true),
                ssl_cert: Some(cert_path),
                ssl_key: Some(key_path),
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
            let rendered =
                crate::services::nginx::render_site_config(templates, domain, &site_config)
                    .map_err(|e| format!("Failed to render nginx config: {e}"))?;
            let target = crate::services::nginx::vhost_target(domain);
            let config_path = target.path().to_string();
            let previous = std::fs::read_to_string(&config_path).ok();
            let tmp_path = format!("{config_path}.tmp");
            std::fs::write(&tmp_path, &rendered)
                .map_err(|e| format!("Failed to write nginx config: {e}"))?;
            std::fs::rename(&tmp_path, &config_path).map_err(|e| {
                std::fs::remove_file(&tmp_path).ok();
                format!("Failed to activate nginx config: {e}")
            })?;
            if !target.is_live() {
                tracing::info!(
                    "Git deploy: {domain} is disabled, saved the HTTPS route (certificate \
                     {alias}) to its parked configuration"
                );
                return Ok(TlsOutcome {
                    ssl: true,
                    tls_mode: "provided",
                    tls_certificate: Some(alias.to_string()),
                    warning: None,
                });
            }
            match crate::services::nginx::test_config().await {
                Ok(output) if output.success => {
                    crate::services::nginx::reload().await.ok();
                    tracing::info!(
                        "Git deploy: {domain} -> port {host_port} over registered certificate {alias}"
                    );
                    Ok(TlsOutcome {
                        ssl: true,
                        tls_mode: "provided",
                        tls_certificate: Some(alias.to_string()),
                        warning: None,
                    })
                }
                _ => {
                    let restored =
                        crate::services::nginx::restore_or_remove(&config_path, previous.as_deref());
                    Err(format!(
                        "Nginx config test failed for {domain}{}",
                        crate::services::nginx::restore_note(restored)
                    ))
                }
            }
        }
        TlsIntent::Acme { email } => {
            setup_nginx_proxy(templates, domain, host_port).await?;
            // DNS propagation wait
            for i in 0..6u32 {
                if i > 0 {
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
                match tokio::net::TcpStream::connect(format!("{domain}:80")).await {
                    Ok(_) => break,
                    Err(_) if i < 5 => continue,
                    Err(_) => break,
                }
            }
            match crate::services::ssl::load_or_create_account(email).await {
                Ok(account) => match crate::services::ssl::provision_cert(&account, domain, None).await
                {
                    Ok(_) => {
                        let ssl_config = crate::routes::nginx::SiteConfig {
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
                            redis_cache: None,
                            redis_db: None,
                            waf_enabled: None,
                            waf_mode: None,
                            csp_policy: None,
                            permissions_policy: None,
                            bot_protection: None,
                        };
                        if crate::services::ssl::enable_ssl_for_site(templates, domain, &ssl_config)
                            .await
                            .is_ok()
                        {
                            tracing::info!("Auto-SSL: certificate provisioned for {domain}");
                            return Ok(TlsOutcome {
                                ssl: true,
                                tls_mode: "acme",
                                tls_certificate: None,
                                warning: None,
                            });
                        }
                        Ok(TlsOutcome {
                            ssl: false,
                            tls_mode: "acme",
                            tls_certificate: None,
                            warning: Some(format!(
                                "certificate provisioned for {domain} but enabling it on the vhost failed"
                            )),
                        })
                    }
                    Err(e) => {
                        tracing::warn!("Auto-SSL: cert provisioning failed for {domain}: {e}");
                        Ok(TlsOutcome {
                            ssl: false,
                            tls_mode: "acme",
                            tls_certificate: None,
                            warning: Some(format!("certificate provisioning failed: {e}")),
                        })
                    }
                },
                Err(e) => {
                    tracing::warn!("Auto-SSL: ACME account failed for {domain}: {e}");
                    Ok(TlsOutcome {
                        ssl: false,
                        tls_mode: "acme",
                        tls_certificate: None,
                        warning: Some(format!("ACME account setup failed: {e}")),
                    })
                }
            }
        }
        TlsIntent::None => {
            setup_nginx_proxy(templates, domain, host_port).await?;
            Ok(TlsOutcome {
                ssl: false,
                tls_mode: "none",
                tls_certificate: None,
                warning: None,
            })
        }
    }
}

/// Deploy or update a container from a locally-built git image.
///
/// - New container: create + start. If domain is provided, set up nginx reverse proxy.
/// - Existing container with domain + nginx config: blue-green zero-downtime update.
/// - Existing container without domain: stop old, remove, create new, start.
///
/// `name` arrives already scoped by [`ownership::GitScope::scoped`]; `scope`
/// comes with it because the name alone cannot say whether the container found
/// under it is this deployment's.
pub async fn deploy_or_update(
    name: &str,
    scope: ownership::GitScope,
    image_tag: &str,
    container_port: u16,
    host_port: u16,
    env_vars: HashMap<String, String>,
    domain: Option<&str>,
    templates: &Tera,
    memory_mb: Option<u64>,
    cpu_percent: Option<u64>,
    tls: TlsIntent<'_>,
) -> Result<GitDeployResult, String> {
    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;

    let container_name = format!("dockpanel-git-{name}");

    // Build environment list
    let env_list: Vec<String> = env_vars
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();

    // Build labels. `dockpanel.git.kind` is what a later run reads back to tell
    // a deployment's container from a preview's — the two used to be
    // indistinguishable, and a container is the one thing here with no file to
    // open and ask.
    let mut labels = HashMap::from([
        ("dockpanel.managed".to_string(), "true".to_string()),
        ("dockpanel.type".to_string(), "git".to_string()),
        ("dockpanel.git.name".to_string(), name.to_string()),
        (
            ownership::GIT_KIND_LABEL.to_string(),
            scope.label().to_string(),
        ),
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

    // ⚠ NO BINDS, DELIBERATELY — and adding them here alone would be a data-loss
    // regression rather than the fix. Reported as #118: a Git Deploy container
    // gets no volume and no bind mount, every deploy replaces the container, and
    // anything the app wrote to its own filesystem is destroyed. The failure is
    // silent and delayed — everything works until the SECOND deploy — and it has
    // cost a reporter real customer uploads. Accepted as unbuilt, not declined;
    // `docs/guides/git-deploy.md` states the limitation and names what does
    // survive.
    //
    // SIX constraints, established while pricing it. Whoever builds this needs
    // all six.
    //
    // ⚠ This header said "five" and "needs all five" from the day it was
    // written until v2.157.0, above a list that has always run to SIX. A
    // contributor who obeyed it stopped exactly one item short — and item 6 is
    // the one that says shipping 1-5 alone DESTROYS the reporter's existing
    // data on the first deploy after the fix. Item 1 turns a patch into a
    // regression; item 6 turns a correct patch into data loss. It is last in
    // the list and first in consequence, so read to the end before writing any
    // of it. (`docs/guides/git-deploy.md` has said "six" correctly all along —
    // only this header lagged, and this header is what a contributor reads.)
    //   1. There are TWO `HostConfig` literals in this file. The blue-green path
    //      builds its own and copies only memory and CPU from the base, so binds
    //      added here alone would mount on the first deploy and UN-mount on the
    //      first blue-green update.
    //   2. Blue-green then needs the `shares_persistent_state` refusal Docker
    //      Apps already has, or the old and new containers hold the same host
    //      paths across a 30-second health check — which corrupts SQLite.
    //   3. Preview environments inherit this deploy path and must explicitly NOT
    //      inherit volumes, or a throwaway PR container writes into production
    //      data — and preview teardown by name would then delete it.
    //   4. Delete-time cleanup has to capture the data directory from the
    //      container's binds at the PRE-REMOVAL inspect; where the current
    //      cleanup runs, the container is already gone. Its "container already
    //      missing" arm also bypasses the ownership check before reaching
    //      `remove_dir_all`, which is fine for a re-clonable checkout and very
    //      much not once real data lives there.
    //   5. The field is container-path only, with the host side derived. This
    //      agent runs under `ProtectSystem=strict` with an explicit
    //      `ReadWritePaths`, so most host paths would fail at mkdir anyway, and
    //      a free-form bind of `/` or `/etc` is a container escape.
    //   6. The deploy that ADDS a volume must carry what is already in the
    //      container's writable layer onto the new mount. Constraints 1-5 all
    //      protect data written AFTER the volume exists; the reporter's data is
    //      in the writable layer right now, and every deploy path removes the
    //      container, which deletes it. Ship without this and the first deploy
    //      after the fix destroys the files the fix was built to save.
    //      `docker_apps::migrate_unmounted_volumes` already does exactly this
    //      for #110 and is the shape to follow, but it cannot be called as it
    //      stands: its discovery half returns empty for any container without a
    //      `dockpanel.app.template` label (a git container has none), and it
    //      takes `&[&'static str]` because template paths are compile-time
    //      constants while an operator's are runtime `String`s. Note also that
    //      Docker Apps refuses blue-green on TWO grounds — an unmigrated path as
    //      well as shared state — which is why constraint 2 alone is not enough.
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
    let existing = find_container(&docker, &container_name, scope).await;

    // A container holding this name is not necessarily this deployment's. If it
    // says it belongs to something else, stop here — every branch below either
    // force-removes it or repoints ITS domain at our build, and both are
    // irreversible. Refusing costs a failed deploy the operator can read.
    if let Some(ref found) = existing {
        if !found.owner.may_delete() {
            return Err(format!(
                "Refusing to deploy: the container {container_name} already exists and does not \
                 belong to this {} (it reports {:?}). Nothing was changed. Rename this deployment \
                 or remove the existing container first.",
                match scope {
                    ownership::GitScope::Preview | ownership::GitScope::PreviewLegacy => "preview",
                    ownership::GitScope::Deploy => "deployment",
                },
                found.owner
            ));
        }
    }

    match existing {
        Some(Found { id: container_id, domain: existing_domain, host_port: existing_port, .. }) => {
            // Does the request still name the domain the running container was
            // built for? If not, blue-green is the wrong tool: it swaps the
            // vhost of the domain read off the OLD container, and there is
            // exactly one call site for `setup_nginx_proxy` — the fresh-deploy
            // arm below — so the domain the operator just asked for would never
            // get a server block at all, on this deploy or any later one.
            let domain_unchanged = match (domain, existing_domain.as_deref()) {
                (Some(requested), Some(current)) => requested == current,
                (None, _) => true, // the request says nothing; keep what is there
                (Some(_), None) => false, // a domain is being added
            };

            // Container exists — check if blue-green is possible
            let has_nginx = domain_unchanged
                && existing_domain.is_some()
                && existing_port.is_some()
                && std::path::Path::new(&format!(
                    "{}/{}.conf",
                    super::nginx::sites_dir(),
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

            // No usable vhost to swap — stop + remove + recreate on the port the
            // panel allocated.
            tracing::info!(
                "Replacing git container {container_name} (stop/start; domain_unchanged={domain_unchanged})"
            );

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

            // The requested domain gets its server block here too, not only on a
            // first deploy. Adding or changing a git app's domain used to write
            // the new value into the container's label and stop — the panel
            // reported success and the hostname served nothing, permanently,
            // because every later deploy took this same branch.
            //
            // The PREVIOUS domain's vhost is not touched HERE, and that is
            // still right: this function cannot prove the old config is still
            // this deploy's, and tearing down one it does not own is the
            // mistake `services::ownership` exists to prevent.
            //
            // ⛔ But "it is stale, not dangerous" was WRONG, and this comment
            // said it for five months. The container keeps its host port across
            // a rename, so the old vhost went on proxying to a LIVE app with a
            // certificate that went on renewing — while the panel released the
            // claim on that name, leaving it claimable by another tenant who
            // would then be answered by the previous tenant's application.
            //
            // The teardown now exists as `release_domain_artifacts`, asks the
            // ownership question this function could not, and is driven from
            // the panel's update handler, which is the only caller that knows
            // both the old name and the new one.
            let mut tls_outcome = None;
            if let Some(d) = domain {
                let config_path = format!("{}/{d}.conf", super::nginx::sites_dir());
                if !domain_unchanged || !std::path::Path::new(&config_path).exists() {
                    // Neither trigger can name a path that was already
                    // serving THIS domain's HTTPS — a changed domain has
                    // never had a vhost at its new path, and a missing config
                    // means nothing is being served at all right now — so
                    // `apply_tls`'s plain-HTTP fallback on a `Provided`
                    // refusal has nothing live to downgrade here.
                    tls_outcome = Some(apply_tls(templates, d, host_port, tls).await?);
                }
            }

            Ok(GitDeployResult {
                container_id: result,
                blue_green: false,
                ssl: tls_outcome.as_ref().map(|o| o.ssl),
                tls_mode: tls_outcome.as_ref().map(|o| o.tls_mode),
                tls_certificate: tls_outcome.as_ref().and_then(|o| o.tls_certificate.clone()),
                tls_warning: tls_outcome.and_then(|o| o.warning),
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

            // Set up nginx reverse proxy — and secure it the way `tls` asks —
            // if a domain is provided. A brand new container's vhost has
            // nothing to downgrade, so `apply_tls`'s plain-HTTP fallback on a
            // `Provided` refusal simply leaves the deploy reachable.
            let tls_outcome = match domain {
                Some(d) => Some(apply_tls(templates, d, host_port, tls).await?),
                None => None,
            };

            Ok(GitDeployResult {
                container_id,
                blue_green: false,
                ssl: tls_outcome.as_ref().map(|o| o.ssl),
                tls_mode: tls_outcome.as_ref().map(|o| o.tls_mode),
                tls_certificate: tls_outcome.as_ref().and_then(|o| o.tls_certificate.clone()),
                tls_warning: tls_outcome.and_then(|o| o.warning),
            })
        }
    }
}

/// Take down the nginx vhost and certificates for `domain`, but ONLY while they
/// still belong to the container on `host_port`.
///
/// Extracted from [`cleanup_container`] so the DELETE path and the RENAME path
/// cannot drift: both must answer the same ownership question, and it is the
/// question `services::ownership` exists to ask. A vhost that no longer proxies
/// to this port has been re-claimed by something else since — a site, another
/// deploy — and removing it would be an outage for a third party.
///
/// ⚠ Deliberately infallible: every caller is finishing an operation the
/// operator already asked for, and failing their rename because a config file
/// was already gone would be worse than the leak. Each branch says what it did.
pub async fn release_domain_artifacts(domain: &str, host_port: Option<u16>) {
    let config_path = format!("{}/{domain}.conf", super::nginx::sites_dir());
    if std::path::Path::new(&config_path).exists() {
        if crate::services::ownership::app_vhost(&config_path, host_port).may_delete() {
            std::fs::remove_file(&config_path).ok();
            tracing::info!("Removed nginx config: {config_path}");

            match crate::services::nginx::test_config().await {
                Ok(output) if output.success => {
                    crate::services::nginx::reload().await.ok();
                }
                _ => {
                    tracing::warn!("Nginx test failed after removing config for {domain}");
                }
            }
        } else {
            tracing::warn!(
                "Leaving {config_path} in place: it does not proxy to this \
                 container's port, so {domain} is now served by something else. \
                 This runs unattended — taking that down would be an outage \
                 nobody was present for."
            );
        }
    }

    // SSL certificates — not if a wildcard is shared across the zone.
    let ssl_dir = format!("/etc/dockpanel/ssl/{domain}");
    if std::path::Path::new(&ssl_dir).exists() {
        if crate::services::ownership::cert_dir_in_use_elsewhere(domain) {
            tracing::warn!("Leaving {ssl_dir} in place: another vhost still points at it.");
        } else {
            std::fs::remove_dir_all(&ssl_dir).ok();
            tracing::info!("Removed SSL certs: {ssl_dir}");
        }
    }
}

/// Stop and remove a git-deployed container, plus its nginx config, SSL certs, and volume dir.
///
/// `scope` decides both which name space is addressed and what counts as proof
/// the container is the caller's. The path that matters is
/// [`ownership::GitScope::PreviewLegacy`]: it reaches into the OLD shared space
/// to clear a preview created before the two were separated, and the container
/// sitting there may be a production deployment that merely shares the name.
pub async fn cleanup_container(
    name: &str,
    scope: ownership::GitScope,
    known_domain: Option<&str>,
    known_port: Option<u16>,
) -> Result<(), String> {
    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;

    let container_name = format!("dockpanel-git-{name}");

    // Inspect to find the domain label AND the published port before removing.
    //
    // The port is what proves the vhost below is still this container's. The
    // label alone is not proof: a preview holding `feature.example.com` is
    // invisible to `domain_claim::find_occupant` (it queries sites, git_deploys
    // and the agent's /apps, and a preview container is in none of them), so a
    // real site can be created on that exact domain and pass every check — and
    // then this cleanup, running unattended on TTL expiry, deletes the site's
    // vhost and certificates five minutes later.
    let (domain, host_port) = match docker.inspect_container(&container_name, None).await {
        Ok(info) => {
            // Who does it say it is? A TTL sweep runs with nobody watching, and
            // the legacy preview space is shared with real deployments — so
            // there the caller's own recorded port has to agree as well.
            let live_port = info
                .host_config
                .as_ref()
                .and_then(crate::services::docker_apps::extract_host_port);
            let owner = ownership::git_container(
                info.config.as_ref().and_then(|c| c.labels.as_ref()),
                scope,
                known_port,
                live_port,
            );
            if !owner.may_delete() {
                tracing::warn!(
                    "Refusing to clean up {container_name}: it reports {owner:?} for this \
                     {scope:?} request. Leaving the container, its vhost, its certificates and \
                     its checkout in place — a stale preview is untidy, and removing a running \
                     deployment is an outage nobody was present for."
                );
                return Ok(());
            }
            (
                info.config
                    .as_ref()
                    .and_then(|c| c.labels.as_ref())
                    .and_then(|l| l.get("dockpanel.app.domain").cloned()),
                info.host_config
                    .as_ref()
                    .and_then(crate::services::docker_apps::extract_host_port),
            )
        }
        // The container is already gone. That used to end the vhost and
        // certificate cleanup here — `domain` was `None`, and everything below
        // hangs off `if let Some(d)` — so the crashed preview's server block and
        // its certificate outlived every record that named them. The caller
        // knows both from its own row; the port still has to match the vhost
        // before anything is removed, so this widens what can be tidied, not
        // what can be destroyed.
        Err(_) => (
            known_domain.map(str::to_string),
            known_port,
        ),
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

    // Remove nginx config + certs — only while they are still THIS container's.
    if let Some(ref d) = domain {
        release_domain_artifacts(d, host_port).await;
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

/// Whitelisted install command that applies to the detected Node.js project,
/// spliced in as the RUN line in place of the hardcoded default. npm/npm ci
/// run as-is; neither yarn nor pnpm is on `node:20-alpine`'s PATH by default,
/// but corepack is (bundled since Node 16.9+), so those two get a
/// `corepack enable &&` prefix. Returns (RUN line, whether it was applied).
fn node_install_run(pre_build_override: Option<&str>, default: &str) -> (String, bool) {
    if let Some(cmd) = pre_build_override {
        if cmd == "npm install" || cmd == "npm ci" {
            return (format!("RUN {cmd}"), true);
        }
        if cmd == "yarn install" || cmd == "pnpm install" {
            return (format!("RUN corepack enable && {cmd}"), true);
        }
    }
    (default.to_string(), false)
}

/// Whitelisted install command that applies to the detected Python project.
/// The default's `--no-cache-dir` flag is dropped when the operator's exact
/// whitelisted string is used, so what they typed is really what runs.
fn pip_install_run(pre_build_override: Option<&str>, default: &str) -> (String, bool) {
    if let Some(cmd) = pre_build_override {
        if cmd == "pip install -r requirements.txt" || cmd == "pip3 install -r requirements.txt" {
            return (format!("RUN {cmd}"), true);
        }
    }
    (default.to_string(), false)
}

/// Auto-detect language and generate a Dockerfile if none exists, optionally
/// splicing a whitelisted install command into it as a RUN line. This is the
/// only place `pre_build_override` (already validated against
/// `ALLOWED_PRE_BUILD` by the route) is used — it only ever becomes text
/// written into a Dockerfile that `docker build` later executes; it never
/// reaches a host shell.
///
/// Returns `(dockerfile_path, pre_build_applied, pre_build_note)`:
/// - `dockerfile_path` is the path to use (the existing one, or "Dockerfile"
///   for a generated one) — unchanged in meaning from before this signature
///   grew a 4th parameter.
/// - `pre_build_applied` is true only when `pre_build_override` was supplied
///   AND matched the detected language's install step.
/// - `pre_build_note` explains a non-applied override (repo already has a
///   Dockerfile; override doesn't match the detected language; detected
///   language has no install step at all) — `None` when there was nothing to
///   explain (no override supplied, or it applied cleanly).
pub fn auto_generate_dockerfile(
    name: &str,
    dockerfile_path: &str,
    build_context: &str,
    pre_build_override: Option<&str>,
) -> Result<(String, bool, Option<String>), String> {
    let deploy_dir = format!("{GIT_BASE_DIR}/{name}");
    let context_dir = if build_context == "." { deploy_dir.clone() } else { format!("{deploy_dir}/{build_context}") };
    let df_path = std::path::Path::new(&context_dir).join(dockerfile_path);

    // If Dockerfile exists, use it as-is. Never auto-splice a RUN line into a
    // Dockerfile this function doesn't own — it has no way to know where an
    // install step belongs relative to that file's own COPY instructions.
    if df_path.exists() {
        let note = pre_build_override
            .map(|_| "repo already has a Dockerfile — add the install step as a RUN line there".to_string());
        return Ok((dockerfile_path.to_string(), false, note));
    }

    tracing::info!("No Dockerfile found at {dockerfile_path} in {context_dir}, auto-detecting...");

    let mut applied = false;
    let mut note: Option<String> = None;

    let generated = if std::path::Path::new(&context_dir).join("package.json").exists() {
        // Node.js
        let pkg = std::fs::read_to_string(std::path::Path::new(&context_dir).join("package.json")).unwrap_or_default();
        let has_build = pkg.contains("\"build\"");
        let has_next = pkg.contains("\"next\"");
        let has_nuxt = pkg.contains("\"nuxt\"");

        if has_next {
            // Next.js
            let (install, ok) = node_install_run(pre_build_override, "RUN npm install");
            applied = ok;
            format!("FROM node:20-alpine AS builder\nWORKDIR /app\nCOPY package*.json ./\n{install}\nCOPY . .\nRUN npm run build\n\nFROM node:20-alpine\nWORKDIR /app\nCOPY --from=builder /app/.next ./.next\nCOPY --from=builder /app/node_modules ./node_modules\nCOPY --from=builder /app/package.json ./\nCOPY --from=builder /app/public ./public\nEXPOSE 3000\nCMD [\"npm\", \"start\"]\n")
        } else if has_nuxt {
            // Nuxt
            let (install, ok) = node_install_run(pre_build_override, "RUN npm install");
            applied = ok;
            format!("FROM node:20-alpine AS builder\nWORKDIR /app\nCOPY package*.json ./\n{install}\nCOPY . .\nRUN npm run build\n\nFROM node:20-alpine\nWORKDIR /app\nCOPY --from=builder /app/.output ./.output\nEXPOSE 3000\nCMD [\"node\", \".output/server/index.mjs\"]\n")
        } else if has_build {
            // Generic Node.js with build step (SPA/React/Vue)
            let (install, ok) = node_install_run(pre_build_override, "RUN npm install");
            applied = ok;
            format!("FROM node:20-alpine AS builder\nWORKDIR /app\nCOPY package*.json ./\n{install}\nCOPY . .\nRUN npm run build\n\nFROM nginx:alpine\nCOPY --from=builder /app/dist /usr/share/nginx/html\nEXPOSE 80\n")
        } else {
            // Plain Node.js server
            let (install, ok) = node_install_run(pre_build_override, "RUN npm install --omit=dev");
            applied = ok;
            format!("FROM node:20-alpine\nWORKDIR /app\nCOPY package*.json ./\n{install}\nCOPY . .\nEXPOSE 3000\nCMD [\"node\", \"index.js\"]\n")
        }
    } else if std::path::Path::new(&context_dir).join("requirements.txt").exists() {
        // Python
        let reqs = std::fs::read_to_string(std::path::Path::new(&context_dir).join("requirements.txt"))
            .unwrap_or_default().to_lowercase();
        let has_django = reqs.contains("django");
        let has_flask = reqs.contains("flask");
        let (install, ok) = pip_install_run(pre_build_override, "RUN pip install --no-cache-dir -r requirements.txt");
        applied = ok;

        if has_django {
            format!("FROM python:3.12-slim\nWORKDIR /app\nCOPY requirements.txt .\n{install}\nCOPY . .\nRUN python manage.py collectstatic --noinput 2>/dev/null || true\nEXPOSE 8000\nCMD [\"gunicorn\", \"--bind\", \"0.0.0.0:8000\", \"--workers\", \"2\", \"config.wsgi:application\"]\n")
        } else if has_flask {
            format!("FROM python:3.12-slim\nWORKDIR /app\nCOPY requirements.txt .\n{install}\nCOPY . .\nEXPOSE 5000\nCMD [\"gunicorn\", \"--bind\", \"0.0.0.0:5000\", \"--workers\", \"2\", \"app:app\"]\n")
        } else {
            format!("FROM python:3.12-slim\nWORKDIR /app\nCOPY requirements.txt .\n{install}\nCOPY . .\nEXPOSE 8000\nCMD [\"python\", \"app.py\"]\n")
        }
    } else if std::path::Path::new(&context_dir).join("go.mod").exists() {
        // Go — no ALLOWED_PRE_BUILD entry maps to this toolchain (module
        // fetch is `go mod download`, not a package-manager install command).
        if let Some(cmd) = pre_build_override {
            note = Some(format!("'{cmd}' has no matching install step for a Go project — ignored"));
        }
        "FROM golang:1.24-alpine AS builder\nWORKDIR /app\nCOPY go.mod go.sum ./\nRUN go mod download\nCOPY . .\nRUN CGO_ENABLED=0 go build -o server .\n\nFROM alpine:3.20\nWORKDIR /app\nCOPY --from=builder /app/server .\nEXPOSE 8080\nCMD [\"./server\"]\n".to_string()
    } else if std::path::Path::new(&context_dir).join("Cargo.toml").exists() {
        // Rust — the default already IS the whitelisted command.
        applied = pre_build_override == Some("cargo build --release");
        "FROM rust:1.94-slim AS builder\nWORKDIR /app\nCOPY . .\nRUN cargo build --release\n\nFROM debian:bookworm-slim\nCOPY --from=builder /app/target/release/* /usr/local/bin/\nEXPOSE 8080\nCMD [\"app\"]\n".to_string()
    } else if std::path::Path::new(&context_dir).join("composer.json").exists() {
        // PHP/Laravel. The installer bootstraps composer as a global
        // `/usr/local/bin/composer` binary (not a local composer.phar)
        // specifically so the whitelisted "composer install" string, when
        // supplied, is really what the install RUN line runs.
        let (install, ok) = if pre_build_override == Some("composer install") {
            ("RUN composer install".to_string(), true)
        } else {
            ("RUN composer install --no-dev --optimize-autoloader".to_string(), false)
        };
        applied = ok;
        format!("FROM php:8.3-fpm-alpine\nRUN apk add --no-cache nginx\nWORKDIR /app\nCOPY . .\nRUN curl -sS https://getcomposer.org/installer | php -- --install-dir=/usr/local/bin --filename=composer\n{install}\nEXPOSE 80\nCMD [\"php\", \"-S\", \"0.0.0.0:80\", \"-t\", \"public\"]\n")
    } else if std::path::Path::new(&context_dir).join("Gemfile").exists() {
        // Ruby
        let (install, ok) = if pre_build_override == Some("bundle install") {
            ("RUN bundle install".to_string(), true)
        } else {
            ("RUN bundle install --without development test".to_string(), false)
        };
        applied = ok;
        format!("FROM ruby:3.3-slim\nWORKDIR /app\nCOPY Gemfile Gemfile.lock ./\n{install}\nCOPY . .\nEXPOSE 3000\nCMD [\"bundle\", \"exec\", \"rails\", \"server\", \"-b\", \"0.0.0.0\"]\n")
    } else if std::path::Path::new(&context_dir).join("index.html").exists() {
        // Static site — nginx just serves files, no install step exists.
        if let Some(cmd) = pre_build_override {
            note = Some(format!("'{cmd}' has no matching install step for a static site — ignored"));
        }
        "FROM nginx:alpine\nCOPY . /usr/share/nginx/html\nEXPOSE 80\n".to_string()
    } else {
        return Err("No Dockerfile found and could not auto-detect project type. Supported: Node.js (package.json), Python (requirements.txt), Go (go.mod), Rust (Cargo.toml), PHP (composer.json), Ruby (Gemfile), Static (index.html)".into());
    };

    // A supplied override that didn't apply because it doesn't match the
    // detected language (e.g. "bundle install" against a Node repo) gets a
    // note too, unless a no-install-step branch above already set a more
    // specific one.
    if let Some(cmd) = pre_build_override {
        if !applied && note.is_none() {
            note = Some(format!("'{cmd}' doesn't apply to this project type — using the default install step"));
        }
    }

    // Write generated Dockerfile
    let generated_path = std::path::Path::new(&context_dir).join("Dockerfile");
    std::fs::write(&generated_path, &generated)
        .map_err(|e| format!("Failed to write generated Dockerfile: {e}"))?;

    tracing::info!("Auto-generated Dockerfile for {name} in {context_dir}");
    Ok(("Dockerfile".to_string(), applied, note))
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// What `find_container` found: its id, the domain it claims, its published
/// port, and — the field that was missing — who it says it belongs to.
struct Found {
    id: String,
    domain: Option<String>,
    host_port: Option<u16>,
    owner: crate::services::ownership::Owner,
}

/// Find an existing container by name.
///
/// A name is not a proof of ownership, which is why the `owner` field exists:
/// `list_containers` will happily hand back whatever holds the string, and until
/// v2.55.0 the callers acted on it. See the container section of
/// [`crate::services::ownership`] for what that cost.
async fn find_container(
    docker: &Docker,
    container_name: &str,
    scope: ownership::GitScope,
) -> Option<Found> {
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

    // A deploy is judged by its label alone: its name is unique per server and
    // nothing else writes into that space any more. The port arguments exist
    // for the legacy-preview space, which `deploy_or_update` never addresses —
    // nothing may CREATE there.
    let owner = ownership::git_container(container.labels.as_ref(), scope, None, host_port);

    Some(Found { id, domain, host_port, owner })
}

/// Clear the leftover of an interrupted swap, if what is standing there is ours
/// to clear. Returns `Err` when it is not, so the caller aborts rather than
/// building on top of a stranger's container.
///
/// "Does a container with this name exist" was never the right question:
/// `{name}-blue` was a name a real deployment could hold.
async fn clear_blue_leftover(docker: &Docker, blue_name: &str) -> Result<(), String> {
    let Ok(info) = docker.inspect_container(blue_name, None).await else {
        return Ok(()); // nothing there
    };
    let owner = ownership::blue_leftover(info.config.as_ref().and_then(|c| c.labels.as_ref()));
    if !owner.may_delete() {
        return Err(format!(
            "A container named {blue_name} exists and is not managed by DockPanel ({owner:?}); \
             refusing to force-remove it to make room for a blue-green swap"
        ));
    }
    docker
        .stop_container(blue_name, Some(StopContainerOptions { t: 5 }))
        .await
        .ok();
    docker
        .remove_container(
            blue_name,
            Some(RemoveContainerOptions {
                force: true,
                v: false,
                ..Default::default()
            }),
        )
        .await
        .ok();
    Ok(())
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
        // Clean up orphaned container on start failure. The explicit `false` here
        // dated from this feature's first commit and carried no reason; it is the
        // same start-failure shape as the app and database teardowns, where the
        // container never ran and its anonymous volumes are therefore empty.
        docker
            .remove_container(
                &container.id,
                Some(RemoveContainerOptions {
                    force: true,
                    v: true,
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
        redis_cache: None,
        redis_db: None,
        waf_enabled: None,
        waf_mode: None,
        csp_policy: None,
        permissions_policy: None,
        bot_protection: None,
    };

    let rendered = crate::services::nginx::render_site_config(templates, domain, &site_config)
        .map_err(|e| format!("Failed to render nginx config: {e}"))?;

    // A deploy to a site the operator took offline updates its parked body, so
    // the deploy is not lost and the site stays off the internet until somebody
    // enables it.
    let target = crate::services::nginx::vhost_target(domain);
    let config_path = target.path().to_string();
    // Snapshot first — a preview deploy synthesises `{branch}.{domain}` from a
    // pushed branch name, so this path can already belong to somebody else, and
    // `nginx -t` is a whole-server check an unrelated broken vhost can fail.
    let previous = std::fs::read_to_string(&config_path).ok();
    let tmp_path = format!("{config_path}.tmp");

    std::fs::write(&tmp_path, &rendered)
        .map_err(|e| format!("Failed to write nginx config: {e}"))?;

    std::fs::rename(&tmp_path, &config_path).map_err(|e| {
        std::fs::remove_file(&tmp_path).ok();
        format!("Failed to activate nginx config: {e}")
    })?;

    if !target.is_live() {
        tracing::info!(
            "Site {domain} is disabled: the deploy updated its parked configuration \
             (proxy -> port {host_port}) and nginx was not reloaded"
        );
        return Ok(());
    }

    match crate::services::nginx::test_config().await {
        Ok(output) if output.success => {
            crate::services::nginx::reload().await.ok();
            tracing::info!("Nginx proxy configured for {domain} -> port {host_port}");
        }
        _ => {
            let restored =
                crate::services::nginx::restore_or_remove(&config_path, previous.as_deref());
            return Err(format!(
                "Nginx config test failed for {domain}{}",
                crate::services::nginx::restore_note(restored)
            ));
        }
    }

    Ok(())
}

/// Blue-green zero-downtime update for a git container behind nginx.
///
/// 1. Find a free temp port
/// 2. Create the stand-in container (see `ownership::blue_name`) on a temp port
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

    let blue_name = ownership::blue_name(container_name);

    // Clean up the leftover of a failed previous attempt — if it is ours.
    clear_blue_leftover(docker, &blue_name).await?;

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

    // Traffic is now flowing to the new container. Promote it.
    //
    // ORDER MATTERS, and the old order was destroy-then-rename with both
    // results discarded by `.ok()`. When the removal failed the rename could not
    // possibly succeed — the name was still taken — and the function returned
    // `Ok` over a half-swapped host: the old container alive under the real
    // name, the new one alive as the stand-in, and nginx pointing at the stand-
    // in. The next deploy then resolved the OLD container, cleared the
    // "leftover" nginx was actually serving, and aborted on the port guard. A
    // reported-successful deploy that arms the next one to take the site down.
    //
    // So: free the name first, by a rename that destroys nothing and can be
    // undone. Only once the promotion is real does anything get removed.
    let retired_name = format!("{container_name}.retired");
    if let Err(e) = docker
        .rename_container(
            old_container_id,
            RenameContainerOptions { name: retired_name.clone() },
        )
        .await
    {
        // Nothing has been destroyed. Put traffic back on the old container and
        // fail honestly.
        crate::services::docker_apps::swap_nginx_proxy_port(domain, temp_port, old_port).ok();
        crate::services::nginx::reload().await.ok();
        cleanup_blue(docker, &new_container.id).await;
        return Err(format!(
            "Could not free the container name for promotion: {e}. Rolled back — the previous \
             container is still serving {domain}."
        ));
    }

    if let Err(e) = docker
        .rename_container(
            &new_container.id,
            RenameContainerOptions { name: container_name.to_string() },
        )
        .await
    {
        // The name is free and the old container is untouched and still running.
        docker
            .rename_container(
                old_container_id,
                RenameContainerOptions { name: container_name.to_string() },
            )
            .await
            .ok();
        crate::services::docker_apps::swap_nginx_proxy_port(domain, temp_port, old_port).ok();
        crate::services::nginx::reload().await.ok();
        cleanup_blue(docker, &new_container.id).await;
        return Err(format!(
            "Could not promote the new container: {e}. Rolled back — the previous container is \
             still serving {domain}."
        ));
    }

    // Promotion is committed. Removing the retired container is housekeeping: a
    // failure here leaks a stopped container under a name nothing can be created
    // as, which is why it is safe to only log it.
    docker
        .stop_container(&retired_name, Some(StopContainerOptions { t: 10 }))
        .await
        .ok();
    if let Err(e) = docker
        .remove_container(
            &retired_name,
            Some(RemoveContainerOptions {
                force: true,
                v: false,
                ..Default::default()
            }),
        )
        .await
    {
        tracing::warn!(
            "Blue-green for {container_name} promoted cleanly but {retired_name} could not be \
             removed: {e}. It is stopped and holds no port; remove it by hand."
        );
    }

    tracing::info!(
        "Git app updated (blue-green, zero-downtime): {container_name}, port {old_port} -> {temp_port}"
    );

    Ok(GitDeployResult {
        container_id: new_container.id,
        blue_green: true,
        // Only the backend port changed — the vhost's certificate story,
        // whatever it is, was never touched. Nothing new to report.
        ssl: None,
        tls_mode: None,
        tls_certificate: None,
        tls_warning: None,
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

/// Where the agent keeps its own nixpacks copy, and its build cache.
///
/// Both used to live outside the agent unit's `ReadWritePaths`
/// (`/usr/local/bin` under `ProtectSystem=strict`, `/var/cache/dockpanel` not
/// listed at all), so the download failed with "Read-only file system" and the
/// Dockerfile-less build path was unreachable on every hardened install (s261).
const NIXPACKS_DIR: &str = "/var/lib/dockpanel/bin";
const NIXPACKS_BIN: &str = "/var/lib/dockpanel/bin/nixpacks";
const NIXPACKS_CACHE_ROOT: &str = "/var/lib/dockpanel/nixpacks-cache";

/// Ensure nixpacks binary is available. Downloads on first use if not found.
pub async fn ensure_nixpacks() -> Option<String> {
    // Check cache
    if let Some(cached) = NIXPACKS_PATH.get() {
        return cached.clone();
    }

    // Our own copy first: NIXPACKS_BIN is outside the agent's SAFE_PATH, so
    // `which` cannot see it and every restart would re-download otherwise.
    if std::path::Path::new(NIXPACKS_BIN).is_file() {
        let _ = NIXPACKS_PATH.set(Some(NIXPACKS_BIN.into()));
        return Some(NIXPACKS_BIN.into());
    }

    // Check if already installed system-wide (operator-provided)
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
            "mkdir -p {NIXPACKS_DIR} && curl -fsSL '{url}' | tar xz -C {NIXPACKS_DIR} && chmod +x {NIXPACKS_BIN}"
        ))
        .output()
        .await;

    match download {
        Ok(out) if out.status.success() => {
            tracing::info!("Nixpacks installed to {NIXPACKS_BIN}");
            let _ = NIXPACKS_PATH.set(Some(NIXPACKS_BIN.into()));
            Some(NIXPACKS_BIN.into())
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
    let cache_dir = format!("{NIXPACKS_CACHE_ROOT}/{name}");
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
