use bollard::Docker;
use bollard::container::{Config, CreateContainerOptions, StartContainerOptions, NetworkingConfig};
use bollard::models::{HostConfig, PortBinding, EndpointSettings};
use bollard::network::CreateNetworkOptions;
use serde::Serialize;
use std::collections::HashMap;

const TRAEFIK_CONTAINER: &str = "dockpanel-traefik";
const TRAEFIK_IMAGE: &str = "traefik:v3.3";
const PROXY_NETWORK: &str = "dockpanel-proxy";
const TRAEFIK_CONFIG_DIR: &str = "/etc/dockpanel/traefik";

#[derive(Serialize)]
pub struct TraefikStatus {
    pub installed: bool,
    pub running: bool,
    pub version: String,
    pub dashboard_url: String,
}

/// Ensure the dockpanel-proxy Docker network exists.
pub async fn ensure_network(docker: &Docker) -> Result<(), String> {
    // Check if network already exists
    match docker.inspect_network::<String>(PROXY_NETWORK, None).await {
        Ok(_) => return Ok(()),
        Err(_) => {}
    }

    docker.create_network(CreateNetworkOptions {
        name: PROXY_NETWORK,
        driver: "bridge",
        ..Default::default()
    })
    .await
    .map_err(|e| format!("Failed to create network: {e}"))?;

    tracing::info!("Created Docker network: {PROXY_NETWORK}");
    Ok(())
}

/// Install and start Traefik as a Docker container.
pub async fn install(docker: &Docker, acme_email: &str) -> Result<TraefikStatus, String> {
    // Ensure network exists
    ensure_network(docker).await?;

    // Create config and dynamic config directories
    std::fs::create_dir_all(TRAEFIK_CONFIG_DIR).ok();
    std::fs::create_dir_all(format!("{TRAEFIK_CONFIG_DIR}/dynamic")).ok();

    // Set restrictive permissions for ACME storage directory
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(
            TRAEFIK_CONFIG_DIR,
            std::fs::Permissions::from_mode(0o700),
        );
    }

    // Check if already exists
    if let Ok(info) = docker.inspect_container(TRAEFIK_CONTAINER, None).await {
        if info.state.as_ref().and_then(|s| s.running).unwrap_or(false) {
            return Ok(TraefikStatus {
                installed: true,
                running: true,
                version: TRAEFIK_IMAGE.to_string(),
                dashboard_url: "http://127.0.0.1:8080".to_string(),
            });
        }
        // Exists but not running — start it
        docker.start_container(TRAEFIK_CONTAINER, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| format!("Failed to start Traefik: {e}"))?;
        return Ok(TraefikStatus {
            installed: true,
            running: true,
            version: TRAEFIK_IMAGE.to_string(),
            dashboard_url: "http://127.0.0.1:8080".to_string(),
        });
    }

    // Pull image
    use bollard::image::CreateImageOptions;
    use tokio_stream::StreamExt;
    let mut pull = docker.create_image(
        Some(CreateImageOptions { from_image: TRAEFIK_IMAGE, ..Default::default() }),
        None, None,
    );
    while let Some(result) = pull.next().await {
        if let Err(e) = result {
            return Err(format!("Failed to pull Traefik image: {e}"));
        }
    }

    // Build container config
    // NOTE: --api.insecure=true exposes the dashboard on port 8080 without auth.
    // This is safe because it's bound to 127.0.0.1 only (not externally accessible).
    let cmd: Vec<String> = vec![
        "--providers.docker=true".into(),
        "--providers.docker.exposedByDefault=false".into(),
        format!("--providers.docker.network={PROXY_NETWORK}"),
        // File provider for dynamic route configs (used when deploying apps with Traefik)
        "--providers.file.directory=/etc/traefik/dynamic".into(),
        "--providers.file.watch=true".into(),
        "--entrypoints.web.address=:80".into(),
        "--entrypoints.websecure.address=:443".into(),
        format!("--certificatesresolvers.letsencrypt.acme.email={acme_email}"),
        "--certificatesresolvers.letsencrypt.acme.storage=/etc/traefik/acme.json".into(),
        "--certificatesresolvers.letsencrypt.acme.httpchallenge.entrypoint=web".into(),
        "--api.dashboard=true".into(),
        "--api.insecure=true".into(),
        "--log.level=INFO".into(),
    ];

    let mut port_bindings = HashMap::new();
    // Traefik listens on internal ports 8880/8443 — nginx stays on 80/443 as front-door
    port_bindings.insert("80/tcp".to_string(), Some(vec![PortBinding {
        host_ip: Some("127.0.0.1".to_string()),
        host_port: Some("8880".to_string()),
    }]));
    port_bindings.insert("443/tcp".to_string(), Some(vec![PortBinding {
        host_ip: Some("127.0.0.1".to_string()),
        host_port: Some("8443".to_string()),
    }]));
    port_bindings.insert("8080/tcp".to_string(), Some(vec![PortBinding {
        host_ip: Some("127.0.0.1".to_string()),
        host_port: Some("8080".to_string()),
    }]));

    let mut labels = HashMap::new();
    labels.insert("dockpanel.managed".to_string(), "true".to_string());
    labels.insert("dockpanel.type".to_string(), "traefik".to_string());

    let mut endpoints = HashMap::new();
    endpoints.insert(PROXY_NETWORK.to_string(), EndpointSettings::default());

    let config = Config {
        image: Some(TRAEFIK_IMAGE.to_string()),
        cmd: Some(cmd),
        labels: Some(labels),
        host_config: Some(HostConfig {
            port_bindings: Some(port_bindings),
            binds: Some(vec![
                "/var/run/docker.sock:/var/run/docker.sock:ro".to_string(),
                format!("{TRAEFIK_CONFIG_DIR}:/etc/traefik"),
            ]),
            restart_policy: Some(bollard::models::RestartPolicy {
                name: Some(bollard::models::RestartPolicyNameEnum::UNLESS_STOPPED),
                ..Default::default()
            }),
            ..Default::default()
        }),
        networking_config: Some(NetworkingConfig {
            endpoints_config: endpoints,
        }),
        ..Default::default()
    };

    docker.create_container(
        Some(CreateContainerOptions { name: TRAEFIK_CONTAINER, ..Default::default() }),
        config,
    )
    .await
    .map_err(|e| format!("Failed to create Traefik container: {e}"))?;

    docker.start_container(TRAEFIK_CONTAINER, None::<StartContainerOptions<String>>)
        .await
        .map_err(|e| format!("Failed to start Traefik: {e}"))?;

    tracing::info!("Traefik installed and running on 127.0.0.1:8880 (HTTP), 127.0.0.1:8443 (HTTPS), 127.0.0.1:8080 (dashboard)");

    Ok(TraefikStatus {
        installed: true,
        running: true,
        version: TRAEFIK_IMAGE.to_string(),
        dashboard_url: "http://127.0.0.1:8080".to_string(),
    })
}

/// Uninstall Traefik.
pub async fn uninstall(docker: &Docker) -> Result<(), String> {
    let _ = docker.stop_container(TRAEFIK_CONTAINER, None).await;
    let _ = docker.remove_container(TRAEFIK_CONTAINER, None).await;

    // Remove proxy network (ignore errors if containers are still connected)
    let _ = docker.remove_network(PROXY_NETWORK).await;

    // Clean up dynamic route configs
    let dynamic_dir = format!("{TRAEFIK_CONFIG_DIR}/dynamic");
    if std::path::Path::new(&dynamic_dir).exists() {
        std::fs::remove_dir_all(&dynamic_dir).ok();
    }

    tracing::info!("Traefik uninstalled");
    Ok(())
}

/// Get Traefik status.
pub async fn status(docker: &Docker) -> TraefikStatus {
    match docker.inspect_container(TRAEFIK_CONTAINER, None).await {
        Ok(info) => {
            let running = info.state.as_ref().and_then(|s| s.running).unwrap_or(false);
            TraefikStatus {
                installed: true,
                running,
                version: TRAEFIK_IMAGE.to_string(),
                dashboard_url: if running { "http://127.0.0.1:8080".to_string() } else { String::new() },
            }
        }
        Err(_) => TraefikStatus {
            installed: false,
            running: false,
            version: String::new(),
            dashboard_url: String::new(),
        },
    }
}

/// Write a Traefik dynamic route config file for an app.
/// Traefik auto-reloads file configs via the file provider (--providers.file.watch=true).
pub fn write_route_config(domain: &str, backend_port: u16, ssl: bool) -> Result<(), String> {
    let safe = route_key(domain);
    let dir = format!("{TRAEFIK_CONFIG_DIR}/dynamic");
    std::fs::create_dir_all(&dir).ok();

    let config = if ssl {
        format!(
            r#"http:
  routers:
    {safe}:
      rule: "Host(`{domain}`)"
      entryPoints:
        - web
      middlewares:
        - "{safe}-redirect"
      service: "{safe}"
    {safe}-secure:
      rule: "Host(`{domain}`)"
      entryPoints:
        - websecure
      service: "{safe}"
      tls:
        certResolver: "letsencrypt"
  middlewares:
    {safe}-redirect:
      redirectScheme:
        scheme: "https"
  services:
    {safe}:
      loadBalancer:
        servers:
          - url: "http://127.0.0.1:{backend_port}"
"#
        )
    } else {
        format!(
            r#"http:
  routers:
    {safe}:
      rule: "Host(`{domain}`)"
      entryPoints:
        - web
      service: "{safe}"
  services:
    {safe}:
      loadBalancer:
        servers:
          - url: "http://127.0.0.1:{backend_port}"
"#
        )
    };

    std::fs::write(format!("{dir}/{safe}.yml"), config)
        .map_err(|e| format!("Failed to write Traefik route config: {e}"))?;

    tracing::info!("Traefik route config written: {domain} → 127.0.0.1:{backend_port} (ssl={ssl})");
    Ok(())
}

/// Remove a Traefik dynamic route config file for an app.
pub fn remove_route_config(domain: &str) {
    let path = route_config_path(domain);
    if std::path::Path::new(&path).exists() {
        let _ = std::fs::remove_file(&path);
        tracing::info!("Traefik route config removed: {domain}");
    }
    // Boxes that ran an older agent have a file under the ambiguous name. Remove
    // it too, or a Traefik install that predates this fix keeps routing a domain
    // whose app is gone. Only safe because the legacy name is what THIS domain
    // would have been written as.
    let legacy = legacy_route_config_path(domain);
    if legacy != path && std::path::Path::new(&legacy).exists() {
        let _ = std::fs::remove_file(&legacy);
        tracing::info!("Traefik route config removed (legacy name): {domain}");
    }
}

/// Traefik router name / config filename for `domain`.
///
/// Mangling `.` to `-` alone is NOT injective over the domains `is_valid_domain`
/// accepts, because `-` is legal inside a label: `a.b.com` and `a-b.com` both
/// became `a-b-com`, so they shared one route file and one router name. The
/// second app deployed silently truncated the first one's route, removing either
/// deleted the other's, and `route_is_tls` answered for whichever won — feeding
/// `nginx::domain_is_https` and therefore the link the panel prints.
///
/// Escaping first makes it injective: a literal `-` doubles, so a single `-` can
/// only have come from a `.`.
fn route_key(domain: &str) -> String {
    let mut out = String::with_capacity(domain.len() + 4);
    for c in domain.chars() {
        match c {
            '-' => out.push_str("--"),
            '_' => out.push_str("__"),
            '.' => out.push('-'),
            ':' => out.push('_'),
            c => out.push(c),
        }
    }
    out
}

/// Path of the dynamic route config `write_route_config` writes for `domain`.
///
/// Factored out so the readers below and the writer above cannot disagree about
/// where the file lives — the same name-mangling has to be applied to find it.
fn route_config_path(domain: &str) -> String {
    format!("{TRAEFIK_CONFIG_DIR}/dynamic/{}.yml", route_key(domain))
}

/// The pre-fix, ambiguous path. Read only by `remove_route_config`, to clean up
/// files written by an agent older than this change.
fn legacy_route_config_path(domain: &str) -> String {
    let safe = domain.replace('.', "-").replace(':', "-");
    format!("{TRAEFIK_CONFIG_DIR}/dynamic/{safe}.yml")
}

#[cfg(test)]
mod route_key_tests {
    use super::route_key;

    #[test]
    fn distinct_domains_get_distinct_keys() {
        // The exact collision this replaced: both used to be "a-b-com".
        assert_ne!(route_key("a.b.com"), route_key("a-b.com"));
        assert_ne!(route_key("x-y.z.com"), route_key("x.y.z.com"));
        assert_ne!(route_key("a--b.com"), route_key("a-b-com"));
    }

    #[test]
    fn keys_contain_no_path_or_yaml_separators() {
        for d in ["a.b.com", "a-b.com", "hy-phen.example.co.uk"] {
            let k = route_key(d);
            assert!(!k.contains('.'), "{k} still has a dot");
            assert!(!k.contains('/'), "{k} has a path separator");
        }
    }

    #[test]
    fn the_key_is_stable_for_one_domain() {
        assert_eq!(route_key("app.example.com"), route_key("app.example.com"));
    }
}

/// Whether the Traefik route for `domain` terminates TLS.
///
/// `write_route_config` emits the `tls:` block only in its `ssl` arm, so the
/// file itself is the record of what this box actually serves. Returns `None`
/// when Traefik is not the proxy for this domain, so the caller can go on to
/// ask nginx rather than reading "no file" as "no TLS".
pub fn route_is_tls(domain: &str) -> Option<bool> {
    let config = std::fs::read_to_string(route_config_path(domain)).ok()?;
    Some(config.contains("tls:"))
}
