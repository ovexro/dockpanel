use bollard::container::{Config, CreateContainerOptions, StartContainerOptions};
use bollard::image::CreateImageOptions;
use bollard::Docker;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio_stream::StreamExt;

/// Parsed docker-compose service definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeService {
    pub name: String,
    pub image: String,
    pub ports: Vec<PortMapping>,
    pub environment: HashMap<String, String>,
    pub volumes: Vec<String>,
    pub restart: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortMapping {
    pub host: u16,
    pub container: u16,
    pub protocol: String,
}

#[derive(Debug, Serialize)]
pub struct ComposeDeployResult {
    pub services: Vec<ServiceDeployResult>,
}

#[derive(Debug, Serialize)]
pub struct ServiceDeployResult {
    pub name: String,
    pub container_id: String,
    pub status: String,
    pub error: Option<String>,
}

/// Raw docker-compose YAML structure (partial — covers common fields).
#[derive(Deserialize)]
struct ComposeFile {
    services: Option<HashMap<String, ServiceDef>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ServiceDef {
    image: Option<String>,
    ports: Option<Vec<serde_yaml_ng::Value>>,
    environment: Option<EnvironmentDef>,
    volumes: Option<Vec<String>>,
    restart: Option<String>,
    container_name: Option<String>,
    // Dangerous fields — parsed so we can explicitly reject them
    privileged: Option<bool>,
    network_mode: Option<String>,
    pid: Option<String>,
    ipc: Option<String>,
    cap_add: Option<Vec<String>>,
    devices: Option<Vec<String>>,
    security_opt: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum EnvironmentDef {
    Map(HashMap<String, serde_yaml_ng::Value>),
    List(Vec<String>),
}

impl Default for EnvironmentDef {
    fn default() -> Self {
        EnvironmentDef::Map(HashMap::new())
    }
}

/// Parse a docker-compose.yml string into a list of services.
pub fn parse_compose(yaml: &str) -> Result<Vec<ComposeService>, String> {
    let compose: ComposeFile =
        serde_yaml_ng::from_str(yaml).map_err(|e| format!("Invalid YAML: {e}"))?;

    let services = compose
        .services
        .ok_or("No 'services' key found in compose file")?;

    let mut result = Vec::new();

    for (name, def) in &services {
        // Reject dangerous Docker options that could escape container isolation
        if def.privileged == Some(true) {
            return Err(format!("Service '{name}': privileged mode is not allowed"));
        }
        if let Some(ref mode) = def.network_mode {
            if mode == "host" || mode.starts_with("container:") {
                return Err(format!("Service '{name}': network_mode '{mode}' is not allowed"));
            }
        }
        if let Some(ref pid) = def.pid {
            if pid == "host" {
                return Err(format!("Service '{name}': pid mode 'host' is not allowed"));
            }
        }
        if let Some(ref ipc) = def.ipc {
            if ipc == "host" {
                return Err(format!("Service '{name}': ipc mode 'host' is not allowed"));
            }
        }
        if let Some(ref caps) = def.cap_add {
            let dangerous = ["SYS_ADMIN", "SYS_PTRACE", "SYS_RAWIO", "NET_ADMIN", "ALL"];
            for cap in caps {
                if dangerous.iter().any(|d| cap.to_uppercase() == *d) {
                    return Err(format!("Service '{name}': capability '{cap}' is not allowed"));
                }
            }
        }
        if def.devices.is_some() {
            return Err(format!("Service '{name}': device mounts are not allowed"));
        }

        let image = match &def.image {
            Some(img) => img.clone(),
            None => {
                // Skip services with build: context (we don't support building images)
                continue;
            }
        };

        // Parse ports
        let ports = parse_ports(&def.ports);

        // Parse environment
        let environment = match &def.environment {
            Some(EnvironmentDef::Map(map)) => map
                .iter()
                .map(|(k, v)| {
                    let val = match v {
                        serde_yaml_ng::Value::String(s) => s.clone(),
                        serde_yaml_ng::Value::Number(n) => n.to_string(),
                        serde_yaml_ng::Value::Bool(b) => b.to_string(),
                        _ => format!("{v:?}"),
                    };
                    (k.clone(), val)
                })
                .collect(),
            Some(EnvironmentDef::List(list)) => list
                .iter()
                .filter_map(|entry| {
                    let (k, v) = entry.split_once('=')?;
                    Some((k.to_string(), v.to_string()))
                })
                .collect(),
            None => HashMap::new(),
        };

        let volumes = def.volumes.clone().unwrap_or_default();
        let restart = def.restart.clone().unwrap_or_else(|| "no".into());

        result.push(ComposeService {
            name: def
                .container_name
                .clone()
                .unwrap_or_else(|| format!("dockpanel-compose-{name}")),
            image,
            ports,
            environment,
            volumes,
            restart,
        });
    }

    if result.is_empty() {
        return Err("No deployable services found (all services require build context?)".into());
    }

    Ok(result)
}

fn parse_ports(ports_val: &Option<Vec<serde_yaml_ng::Value>>) -> Vec<PortMapping> {
    let ports = match ports_val {
        Some(p) => p,
        None => return Vec::new(),
    };

    let mut result = Vec::new();

    for port in ports {
        let port_str = match port {
            serde_yaml_ng::Value::String(s) => s.clone(),
            serde_yaml_ng::Value::Number(n) => n.to_string(),
            _ => continue,
        };

        // Formats: "8080:80", "8080:80/tcp", "80" (same on host+container)
        let (mapping, protocol) = if port_str.contains('/') {
            let parts: Vec<&str> = port_str.splitn(2, '/').collect();
            (parts[0], parts[1])
        } else {
            (port_str.as_str(), "tcp")
        };

        if let Some((host, container)) = mapping.split_once(':') {
            // Handle IP:host:container format (e.g., "127.0.0.1:8080:80")
            let (host_port, container_port) = if host.contains('.') || host.contains(':') {
                // Has IP prefix, container is the third part
                match container.split_once(':') {
                    Some((hp, cp)) => (hp, cp),
                    None => (container, container),
                }
            } else {
                (host, container)
            };

            if let (Ok(h), Ok(c)) = (host_port.parse::<u16>(), container_port.parse::<u16>()) {
                result.push(PortMapping {
                    host: h,
                    container: c,
                    protocol: protocol.to_string(),
                });
            }
        } else if let Ok(p) = mapping.parse::<u16>() {
            result.push(PortMapping {
                host: p,
                container: p,
                protocol: protocol.to_string(),
            });
        }
    }

    result
}

/// Deploy all services from a parsed compose file.
/// If `stack_id` is provided, all containers get a `dockpanel.stack_id` label.
pub async fn deploy_compose(
    services: &[ComposeService],
    stack_id: Option<&str>,
) -> ComposeDeployResult {
    let docker = match Docker::connect_with_local_defaults() {
        Ok(d) => d,
        Err(e) => {
            return ComposeDeployResult {
                services: services
                    .iter()
                    .map(|s| ServiceDeployResult {
                        name: s.name.clone(),
                        container_id: String::new(),
                        status: "failed".into(),
                        error: Some(format!("Docker connect failed: {e}")),
                    })
                    .collect(),
            };
        }
    };

    let mut results = Vec::new();

    for svc in services {
        match deploy_service(&docker, svc, stack_id).await {
            Ok(container_id) => {
                results.push(ServiceDeployResult {
                    name: svc.name.clone(),
                    container_id,
                    status: "running".into(),
                    error: None,
                });
            }
            Err(e) => {
                results.push(ServiceDeployResult {
                    name: svc.name.clone(),
                    container_id: String::new(),
                    status: "failed".into(),
                    error: Some(e),
                });
            }
        }
    }

    ComposeDeployResult { services: results }
}

async fn deploy_service(
    docker: &Docker,
    svc: &ComposeService,
    stack_id: Option<&str>,
) -> Result<String, String> {
    // Pull image (with timeout to prevent hanging on Docker daemon issues)
    let pull_result = tokio::time::timeout(std::time::Duration::from_secs(300), async {
        let mut pull = docker.create_image(
            Some(CreateImageOptions {
                from_image: svc.image.as_str(),
                ..Default::default()
            }),
            None,
            None,
        );
        while let Some(result) = pull.next().await {
            if let Err(e) = result {
                tracing::warn!("Image pull warning for {}: {e}", svc.image);
            }
        }
    }).await;
    if pull_result.is_err() {
        return Err(format!("Image pull timed out for {}", svc.image));
    }

    // Build env vars
    let env_list: Vec<String> = svc
        .environment
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();

    // Port bindings
    let mut port_bindings = HashMap::new();
    let mut exposed_ports = HashMap::new();
    for pm in &svc.ports {
        let container_port = format!("{}/{}", pm.container, pm.protocol);
        port_bindings.insert(
            container_port.clone(),
            Some(vec![bollard::service::PortBinding {
                host_ip: Some("127.0.0.1".to_string()),
                host_port: Some(pm.host.to_string()),
            }]),
        );
        exposed_ports.insert(container_port, HashMap::new());
    }

    // Volume binds — validate that host paths are under the allowed prefix
    // and block mounting the Docker socket.
    let mut binds: Vec<String> = Vec::new();
    const ALLOWED_BIND_PREFIX: &str = "/var/lib/dockpanel/compose/";
    const BLOCKED_PATHS: &[&str] = &["/var/run/docker.sock", "/run/docker.sock"];

    for vol in &svc.volumes {
        if !vol.contains(':') {
            continue;
        }
        // Extract host path (everything before the first ':')
        let host_path = vol.split(':').next().unwrap_or("");

        // Skip named volumes (no leading /) — they are safe
        if !host_path.starts_with('/') && !host_path.starts_with('.') {
            binds.push(vol.clone());
            continue;
        }

        // Resolve symlinks and canonicalize the path to prevent symlink-based escapes
        let resolved = std::fs::canonicalize(host_path)
            .map_err(|_| format!("Volume mount path does not exist or is inaccessible: {}", host_path))?;
        let resolved_str = resolved.to_string_lossy();

        // Block docker socket
        for blocked in BLOCKED_PATHS {
            if resolved_str == *blocked {
                return Err(format!(
                    "Blocked volume mount: {} is not allowed",
                    host_path
                ));
            }
        }

        // Only allow paths under the sanctioned prefix
        if !resolved_str.starts_with(ALLOWED_BIND_PREFIX) {
            return Err(format!(
                "Blocked volume mount: host path '{}' must be under {}",
                host_path, ALLOWED_BIND_PREFIX
            ));
        }

        // Reject path traversal attempts
        if host_path.contains("..") {
            return Err(format!(
                "Blocked volume mount: path traversal not allowed in '{}'",
                host_path
            ));
        }

        // TOCTOU fix: use the resolved (canonicalized) path in the actual mount,
        // not the original user-supplied path which could be a symlink.
        let container_path = vol.split(':').nth(1).unwrap_or("");
        let options = vol.split(':').nth(2);
        let mut bind = format!("{}:{}", resolved_str, container_path);
        if let Some(opts) = options {
            bind = format!("{}:{}", bind, opts);
        }
        binds.push(bind);
    }

    // Restart policy
    let restart_policy = match svc.restart.as_str() {
        "always" => bollard::service::RestartPolicyNameEnum::ALWAYS,
        "unless-stopped" => bollard::service::RestartPolicyNameEnum::UNLESS_STOPPED,
        "on-failure" => bollard::service::RestartPolicyNameEnum::ON_FAILURE,
        _ => bollard::service::RestartPolicyNameEnum::NO,
    };

    let host_config = bollard::service::HostConfig {
        port_bindings: if port_bindings.is_empty() {
            None
        } else {
            Some(port_bindings)
        },
        binds: if binds.is_empty() { None } else { Some(binds) },
        restart_policy: Some(bollard::service::RestartPolicy {
            name: Some(restart_policy),
            ..Default::default()
        }),
        security_opt: Some(vec!["no-new-privileges:true".to_string()]),
        cap_drop: Some(vec!["ALL".to_string()]),
        cap_add: Some(vec![
            "CHOWN".to_string(),
            "SETUID".to_string(),
            "SETGID".to_string(),
            "NET_BIND_SERVICE".to_string(),
            "DAC_OVERRIDE".to_string(),
            "FOWNER".to_string(),
            "SETFCAP".to_string(),
        ]),
        ..Default::default()
    };

    let config = Config {
        image: Some(svc.image.clone()),
        env: if env_list.is_empty() {
            None
        } else {
            Some(env_list)
        },
        exposed_ports: if exposed_ports.is_empty() {
            None
        } else {
            Some(exposed_ports)
        },
        host_config: Some(host_config),
        labels: Some({
            let mut labels = HashMap::from([
                ("dockpanel.managed".to_string(), "true".to_string()),
                ("dockpanel.app.template".to_string(), "compose".to_string()),
                ("dockpanel.app.name".to_string(), svc.name.clone()),
            ]);
            if let Some(sid) = stack_id {
                labels.insert("dockpanel.stack_id".to_string(), sid.to_string());
            }
            labels
        }),
        ..Default::default()
    };

    let container = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        docker.create_container(
            Some(CreateContainerOptions {
                name: svc.name.as_str(),
                platform: None,
            }),
            config,
        ),
    )
    .await
    .map_err(|_| format!("Create container timed out for {}", svc.name))?
    .map_err(|e| format!("Failed to create container {}: {e}", svc.name))?;

    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        docker.start_container(&container.id, None::<StartContainerOptions<String>>),
    )
    .await
    .map_err(|_| format!("Start container timed out for {}", svc.name))?
    .map_err(|e| format!("Failed to start container {}: {e}", svc.name))?;

    tracing::info!("Compose service deployed: {} (image={})", svc.name, svc.image);

    Ok(container.id)
}
