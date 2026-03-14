use bollard::container::{
    Config, CreateContainerOptions, ListContainersOptions, RemoveContainerOptions,
    StartContainerOptions, StopContainerOptions,
};
use bollard::Docker;
use std::collections::HashMap;

const DB_NETWORK: &str = "dockpanel-db";

#[derive(serde::Serialize)]
pub struct DbContainer {
    pub container_id: String,
    pub name: String,
    pub port: u16,
    pub engine: String,
    pub status: String,
}

/// Create a database container (MySQL or PostgreSQL).
pub async fn create_database(
    name: &str,
    engine: &str,
    password: &str,
    port: u16,
) -> Result<DbContainer, String> {
    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;

    // Ensure network exists
    ensure_network(&docker).await?;

    let (image, env, container_port) = match engine {
        "mysql" | "mariadb" => (
            "mariadb:11",
            vec![
                format!("MYSQL_DATABASE={name}"),
                format!("MYSQL_USER={name}"),
                format!("MYSQL_PASSWORD={password}"),
                "MYSQL_RANDOM_ROOT_PASSWORD=yes".to_string(),
            ],
            "3306/tcp",
        ),
        _ => (
            "postgres:16-alpine",
            vec![
                format!("POSTGRES_DB={name}"),
                format!("POSTGRES_USER={name}"),
                format!("POSTGRES_PASSWORD={password}"),
            ],
            "5432/tcp",
        ),
    };

    // Pull image if needed
    use bollard::image::CreateImageOptions;
    use tokio_stream::StreamExt;
    let mut pull = docker.create_image(
        Some(CreateImageOptions {
            from_image: image,
            ..Default::default()
        }),
        None,
        None,
    );
    while let Some(result) = pull.next().await {
        if let Err(e) = result {
            tracing::warn!("Image pull warning: {e}");
        }
    }

    let container_name = format!("dockpanel-db-{name}");

    let mut port_bindings = HashMap::new();
    port_bindings.insert(
        container_port.to_string(),
        Some(vec![bollard::service::PortBinding {
            host_ip: Some("127.0.0.1".to_string()),
            host_port: Some(port.to_string()),
        }]),
    );

    let host_config = bollard::service::HostConfig {
        port_bindings: Some(port_bindings),
        network_mode: Some(DB_NETWORK.to_string()),
        restart_policy: Some(bollard::service::RestartPolicy {
            name: Some(bollard::service::RestartPolicyNameEnum::UNLESS_STOPPED),
            ..Default::default()
        }),
        memory: Some(256 * 1024 * 1024), // 256MB
        ..Default::default()
    };

    let mut exposed_ports = HashMap::new();
    exposed_ports.insert(container_port.to_string(), HashMap::new());

    let config = Config {
        image: Some(image.to_string()),
        env: Some(env.clone()),
        exposed_ports: Some(exposed_ports),
        host_config: Some(host_config),
        labels: Some(HashMap::from([
            ("dockpanel.managed".to_string(), "true".to_string()),
            ("dockpanel.db.name".to_string(), name.to_string()),
            ("dockpanel.db.engine".to_string(), engine.to_string()),
        ])),
        ..Default::default()
    };

    let container = docker
        .create_container(
            Some(CreateContainerOptions {
                name: container_name.as_str(),
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
        let _ = docker
            .remove_container(
                &container.id,
                Some(bollard::container::RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await;
        return Err(format!("Failed to start container: {e}"));
    }

    tracing::info!("Database container created: {container_name} ({engine}, port {port})");

    Ok(DbContainer {
        container_id: container.id,
        name: container_name,
        port,
        engine: engine.to_string(),
        status: "running".to_string(),
    })
}

/// Remove a database container.
pub async fn remove_database(container_id: &str) -> Result<(), String> {
    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;

    // Stop first
    docker
        .stop_container(container_id, Some(StopContainerOptions { t: 10 }))
        .await
        .ok(); // Ignore if already stopped

    docker
        .remove_container(
            container_id,
            Some(RemoveContainerOptions {
                v: true, // remove volumes
                force: true,
                ..Default::default()
            }),
        )
        .await
        .map_err(|e| format!("Failed to remove container: {e}"))?;

    tracing::info!("Database container removed: {container_id}");
    Ok(())
}

/// List all DockPanel-managed database containers.
pub async fn list_databases() -> Result<Vec<DbContainer>, String> {
    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;

    let mut filters = HashMap::new();
    filters.insert("label", vec!["dockpanel.managed=true"]);

    let containers = docker
        .list_containers(Some(ListContainersOptions {
            all: true,
            filters,
            ..Default::default()
        }))
        .await
        .map_err(|e| format!("Failed to list containers: {e}"))?;

    let dbs = containers
        .into_iter()
        .filter_map(|c| {
            let labels = c.labels.as_ref()?;
            let _db_name = labels.get("dockpanel.db.name")?;
            let engine = labels.get("dockpanel.db.engine")?;
            let id = c.id.as_ref()?;

            let port = c
                .ports
                .as_ref()
                .and_then(|ports| ports.first())
                .and_then(|p| p.public_port)
                .unwrap_or(0) as u16;

            let status = c.state.unwrap_or_default();
            let name = c
                .names
                .as_ref()
                .and_then(|n| n.first())
                .map(|n| n.trim_start_matches('/').to_string())
                .unwrap_or_default();

            Some(DbContainer {
                container_id: id.clone(),
                name,
                port,
                engine: engine.clone(),
                status,
            })
        })
        .collect();

    Ok(dbs)
}

/// Ensure the dockpanel-db Docker network exists.
async fn ensure_network(docker: &Docker) -> Result<(), String> {
    use bollard::network::CreateNetworkOptions;

    match docker.inspect_network::<String>(DB_NETWORK, None).await {
        Ok(_) => Ok(()),
        Err(_) => {
            docker
                .create_network(CreateNetworkOptions {
                    name: DB_NETWORK,
                    driver: "bridge",
                    ..Default::default()
                })
                .await
                .map_err(|e| format!("Failed to create network: {e}"))?;
            tracing::info!("Created Docker network: {DB_NETWORK}");
            Ok(())
        }
    }
}
