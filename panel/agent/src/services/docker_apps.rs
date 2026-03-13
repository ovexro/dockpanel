use bollard::container::{
    Config, CreateContainerOptions, ListContainersOptions, RemoveContainerOptions,
    StartContainerOptions, StopContainerOptions,
};
use bollard::image::CreateImageOptions;
use bollard::Docker;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio_stream::StreamExt;

/// Template definition used in the static array (slice-based, no heap allocation).
struct AppTemplateDef {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    category: &'static str,
    image: &'static str,
    default_port: u16,
    container_port: &'static str,
    env_vars: &'static [EnvVarDef],
    volumes: &'static [&'static str],
}

struct EnvVarDef {
    name: &'static str,
    label: &'static str,
    default: &'static str,
    required: bool,
    secret: bool,
}

/// Serializable template returned to API consumers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppTemplate {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub image: String,
    pub default_port: u16,
    pub container_port: String,
    pub env_vars: Vec<EnvVar>,
    pub volumes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvVar {
    pub name: String,
    pub label: String,
    pub default: String,
    pub required: bool,
    pub secret: bool,
}

#[derive(Debug, Serialize)]
pub struct DeployResult {
    pub container_id: String,
    pub name: String,
    pub port: u16,
}

#[derive(Debug, Serialize)]
pub struct DeployedApp {
    pub container_id: String,
    pub name: String,
    pub template: String,
    pub status: String,
    pub port: Option<u16>,
    pub domain: Option<String>,
    pub health: Option<String>,
}

static TEMPLATES: &[AppTemplateDef] = &[
    AppTemplateDef {
        id: "wordpress",
        name: "WordPress",
        description: "The world's most popular CMS for blogs and websites",
        category: "CMS",
        image: "wordpress:latest",
        default_port: 8080,
        container_port: "80/tcp",
        env_vars: &[
            EnvVarDef {
                name: "WORDPRESS_DB_HOST",
                label: "Database Host",
                default: "",
                required: true,
                secret: false,
            },
            EnvVarDef {
                name: "WORDPRESS_DB_USER",
                label: "Database User",
                default: "",
                required: true,
                secret: false,
            },
            EnvVarDef {
                name: "WORDPRESS_DB_PASSWORD",
                label: "Database Password",
                default: "",
                required: true,
                secret: true,
            },
            EnvVarDef {
                name: "WORDPRESS_DB_NAME",
                label: "Database Name",
                default: "",
                required: true,
                secret: false,
            },
        ],
        volumes: &[],
    },
    AppTemplateDef {
        id: "ghost",
        name: "Ghost",
        description: "Professional publishing platform for blogs and newsletters",
        category: "CMS",
        image: "ghost:latest",
        default_port: 2368,
        container_port: "2368/tcp",
        env_vars: &[EnvVarDef {
            name: "url",
            label: "Site URL",
            default: "http://localhost:2368",
            required: false,
            secret: false,
        }],
        volumes: &[],
    },
    AppTemplateDef {
        id: "redis",
        name: "Redis",
        description: "In-memory data store for caching and message brokering",
        category: "Database",
        image: "redis:7-alpine",
        default_port: 6379,
        container_port: "6379/tcp",
        env_vars: &[],
        volumes: &[],
    },
    AppTemplateDef {
        id: "adminer",
        name: "Adminer",
        description: "Lightweight database management UI supporting MySQL, PostgreSQL, SQLite",
        category: "Tools",
        image: "adminer:latest",
        default_port: 8081,
        container_port: "8080/tcp",
        env_vars: &[],
        volumes: &[],
    },
    AppTemplateDef {
        id: "uptime-kuma",
        name: "Uptime Kuma",
        description: "Self-hosted monitoring tool with notifications and status pages",
        category: "Monitoring",
        image: "louislam/uptime-kuma:1",
        default_port: 3001,
        container_port: "3001/tcp",
        env_vars: &[],
        volumes: &["/app/data"],
    },
    AppTemplateDef {
        id: "portainer",
        name: "Portainer",
        description: "Docker management UI for containers, images, volumes, and networks",
        category: "Tools",
        image: "portainer/portainer-ce:latest",
        default_port: 9443,
        container_port: "9443/tcp",
        env_vars: &[],
        volumes: &["/data"],
    },
    AppTemplateDef {
        id: "n8n",
        name: "n8n",
        description: "Workflow automation tool with 200+ integrations",
        category: "Automation",
        image: "n8nio/n8n:latest",
        default_port: 5678,
        container_port: "5678/tcp",
        env_vars: &[
            EnvVarDef {
                name: "N8N_BASIC_AUTH_USER",
                label: "Admin Username",
                default: "admin",
                required: false,
                secret: false,
            },
            EnvVarDef {
                name: "N8N_BASIC_AUTH_PASSWORD",
                label: "Admin Password",
                default: "",
                required: false,
                secret: true,
            },
        ],
        volumes: &[],
    },
    AppTemplateDef {
        id: "gitea",
        name: "Gitea",
        description: "Lightweight self-hosted Git service with issues, pull requests, and CI/CD",
        category: "Development",
        image: "gitea/gitea:latest",
        default_port: 3000,
        container_port: "3000/tcp",
        env_vars: &[],
        volumes: &["/data", "/etc/timezone"],
    },
    // ─── Databases ──────────────────────────────────────────────────
    AppTemplateDef {
        id: "postgres",
        name: "PostgreSQL",
        description: "Advanced open-source relational database",
        category: "Database",
        image: "postgres:16-alpine",
        default_port: 5432,
        container_port: "5432/tcp",
        env_vars: &[
            EnvVarDef { name: "POSTGRES_USER", label: "Username", default: "postgres", required: true, secret: false },
            EnvVarDef { name: "POSTGRES_PASSWORD", label: "Password", default: "", required: true, secret: true },
            EnvVarDef { name: "POSTGRES_DB", label: "Database Name", default: "app", required: false, secret: false },
        ],
        volumes: &["/var/lib/postgresql/data"],
    },
    AppTemplateDef {
        id: "mysql",
        name: "MySQL",
        description: "The world's most popular open-source relational database",
        category: "Database",
        image: "mysql:8",
        default_port: 3306,
        container_port: "3306/tcp",
        env_vars: &[
            EnvVarDef { name: "MYSQL_ROOT_PASSWORD", label: "Root Password", default: "", required: true, secret: true },
            EnvVarDef { name: "MYSQL_DATABASE", label: "Database Name", default: "app", required: false, secret: false },
            EnvVarDef { name: "MYSQL_USER", label: "User", default: "", required: false, secret: false },
            EnvVarDef { name: "MYSQL_PASSWORD", label: "User Password", default: "", required: false, secret: true },
        ],
        volumes: &["/var/lib/mysql"],
    },
    AppTemplateDef {
        id: "mariadb",
        name: "MariaDB",
        description: "Community-developed fork of MySQL with enhanced performance",
        category: "Database",
        image: "mariadb:11",
        default_port: 3307,
        container_port: "3306/tcp",
        env_vars: &[
            EnvVarDef { name: "MARIADB_ROOT_PASSWORD", label: "Root Password", default: "", required: true, secret: true },
            EnvVarDef { name: "MARIADB_DATABASE", label: "Database Name", default: "app", required: false, secret: false },
        ],
        volumes: &["/var/lib/mysql"],
    },
    AppTemplateDef {
        id: "mongo",
        name: "MongoDB",
        description: "Document-oriented NoSQL database for modern apps",
        category: "Database",
        image: "mongo:7",
        default_port: 27017,
        container_port: "27017/tcp",
        env_vars: &[
            EnvVarDef { name: "MONGO_INITDB_ROOT_USERNAME", label: "Root Username", default: "admin", required: true, secret: false },
            EnvVarDef { name: "MONGO_INITDB_ROOT_PASSWORD", label: "Root Password", default: "", required: true, secret: true },
        ],
        volumes: &["/data/db"],
    },
    // ─── CMS & Content ──────────────────────────────────────────────
    AppTemplateDef {
        id: "strapi",
        name: "Strapi",
        description: "Open-source headless CMS with a customizable API",
        category: "CMS",
        image: "strapi/strapi:latest",
        default_port: 1337,
        container_port: "1337/tcp",
        env_vars: &[],
        volumes: &["/srv/app"],
    },
    AppTemplateDef {
        id: "directus",
        name: "Directus",
        description: "Open data platform — instant REST & GraphQL API for any SQL database",
        category: "CMS",
        image: "directus/directus:latest",
        default_port: 8055,
        container_port: "8055/tcp",
        env_vars: &[
            EnvVarDef { name: "ADMIN_EMAIL", label: "Admin Email", default: "admin@example.com", required: true, secret: false },
            EnvVarDef { name: "ADMIN_PASSWORD", label: "Admin Password", default: "", required: true, secret: true },
            EnvVarDef { name: "SECRET", label: "Secret Key", default: "", required: true, secret: true },
        ],
        volumes: &["/directus/uploads", "/directus/database"],
    },
    AppTemplateDef {
        id: "nextcloud",
        name: "Nextcloud",
        description: "Self-hosted productivity platform — files, calendar, contacts, and more",
        category: "Storage",
        image: "nextcloud:latest",
        default_port: 8082,
        container_port: "80/tcp",
        env_vars: &[
            EnvVarDef { name: "NEXTCLOUD_ADMIN_USER", label: "Admin Username", default: "admin", required: true, secret: false },
            EnvVarDef { name: "NEXTCLOUD_ADMIN_PASSWORD", label: "Admin Password", default: "", required: true, secret: true },
        ],
        volumes: &["/var/www/html"],
    },
    // ─── Monitoring & Analytics ─────────────────────────────────────
    AppTemplateDef {
        id: "grafana",
        name: "Grafana",
        description: "Open-source observability platform for metrics, logs, and traces",
        category: "Monitoring",
        image: "grafana/grafana:latest",
        default_port: 3002,
        container_port: "3000/tcp",
        env_vars: &[
            EnvVarDef { name: "GF_SECURITY_ADMIN_PASSWORD", label: "Admin Password", default: "admin", required: false, secret: true },
        ],
        volumes: &["/var/lib/grafana"],
    },
    AppTemplateDef {
        id: "prometheus",
        name: "Prometheus",
        description: "Monitoring system and time-series database for metrics",
        category: "Monitoring",
        image: "prom/prometheus:latest",
        default_port: 9090,
        container_port: "9090/tcp",
        env_vars: &[],
        volumes: &["/prometheus"],
    },
    AppTemplateDef {
        id: "plausible",
        name: "Plausible Analytics",
        description: "Privacy-friendly alternative to Google Analytics",
        category: "Analytics",
        image: "plausible/analytics:latest",
        default_port: 8000,
        container_port: "8000/tcp",
        env_vars: &[
            EnvVarDef { name: "SECRET_KEY_BASE", label: "Secret Key (64+ chars)", default: "", required: true, secret: true },
            EnvVarDef { name: "BASE_URL", label: "Site URL", default: "http://localhost:8000", required: true, secret: false },
        ],
        volumes: &[],
    },
    AppTemplateDef {
        id: "umami",
        name: "Umami",
        description: "Simple, privacy-focused website analytics",
        category: "Analytics",
        image: "ghcr.io/umami-software/umami:postgresql-latest",
        default_port: 3003,
        container_port: "3000/tcp",
        env_vars: &[
            EnvVarDef { name: "DATABASE_URL", label: "Postgres URL", default: "", required: true, secret: true },
        ],
        volumes: &[],
    },
    AppTemplateDef {
        id: "matomo",
        name: "Matomo",
        description: "Google Analytics alternative that respects user privacy",
        category: "Analytics",
        image: "matomo:latest",
        default_port: 8083,
        container_port: "80/tcp",
        env_vars: &[],
        volumes: &["/var/www/html"],
    },
    // ─── Tools & Utilities ──────────────────────────────────────────
    AppTemplateDef {
        id: "pgadmin",
        name: "pgAdmin",
        description: "Web-based PostgreSQL management and administration tool",
        category: "Tools",
        image: "dpage/pgadmin4:latest",
        default_port: 5050,
        container_port: "80/tcp",
        env_vars: &[
            EnvVarDef { name: "PGADMIN_DEFAULT_EMAIL", label: "Admin Email", default: "admin@example.com", required: true, secret: false },
            EnvVarDef { name: "PGADMIN_DEFAULT_PASSWORD", label: "Admin Password", default: "", required: true, secret: true },
        ],
        volumes: &["/var/lib/pgadmin"],
    },
    AppTemplateDef {
        id: "minio",
        name: "MinIO",
        description: "High-performance S3-compatible object storage",
        category: "Storage",
        image: "minio/minio:latest",
        default_port: 9000,
        container_port: "9000/tcp",
        env_vars: &[
            EnvVarDef { name: "MINIO_ROOT_USER", label: "Root User", default: "minioadmin", required: true, secret: false },
            EnvVarDef { name: "MINIO_ROOT_PASSWORD", label: "Root Password", default: "", required: true, secret: true },
        ],
        volumes: &["/data"],
    },
    AppTemplateDef {
        id: "vaultwarden",
        name: "Vaultwarden",
        description: "Lightweight Bitwarden-compatible password manager server",
        category: "Security",
        image: "vaultwarden/server:latest",
        default_port: 8084,
        container_port: "80/tcp",
        env_vars: &[
            EnvVarDef { name: "ADMIN_TOKEN", label: "Admin Token", default: "", required: false, secret: true },
        ],
        volumes: &["/data"],
    },
    AppTemplateDef {
        id: "meilisearch",
        name: "Meilisearch",
        description: "Lightning-fast, typo-tolerant search engine",
        category: "Tools",
        image: "getmeili/meilisearch:latest",
        default_port: 7700,
        container_port: "7700/tcp",
        env_vars: &[
            EnvVarDef { name: "MEILI_MASTER_KEY", label: "Master Key", default: "", required: true, secret: true },
        ],
        volumes: &["/meili_data"],
    },
    AppTemplateDef {
        id: "metabase",
        name: "Metabase",
        description: "Business intelligence and analytics dashboard builder",
        category: "Analytics",
        image: "metabase/metabase:latest",
        default_port: 3004,
        container_port: "3000/tcp",
        env_vars: &[],
        volumes: &["/metabase-data"],
    },
    // ─── Communication ──────────────────────────────────────────────
    AppTemplateDef {
        id: "nocodb",
        name: "NocoDB",
        description: "Open-source Airtable alternative — turn any database into a spreadsheet",
        category: "Tools",
        image: "nocodb/nocodb:latest",
        default_port: 8085,
        container_port: "8080/tcp",
        env_vars: &[],
        volumes: &["/usr/app/data"],
    },
    AppTemplateDef {
        id: "searxng",
        name: "SearXNG",
        description: "Privacy-respecting metasearch engine aggregating 70+ sources",
        category: "Tools",
        image: "searxng/searxng:latest",
        default_port: 8086,
        container_port: "8080/tcp",
        env_vars: &[],
        volumes: &[],
    },
    AppTemplateDef {
        id: "jellyfin",
        name: "Jellyfin",
        description: "Free media server for movies, TV shows, music, and photos",
        category: "Media",
        image: "jellyfin/jellyfin:latest",
        default_port: 8096,
        container_port: "8096/tcp",
        env_vars: &[],
        volumes: &["/config", "/cache"],
    },
    AppTemplateDef {
        id: "code-server",
        name: "VS Code Server",
        description: "Run VS Code in the browser — full IDE accessible anywhere",
        category: "Development",
        image: "codercom/code-server:latest",
        default_port: 8443,
        container_port: "8080/tcp",
        env_vars: &[
            EnvVarDef { name: "PASSWORD", label: "Access Password", default: "", required: true, secret: true },
        ],
        volumes: &["/home/coder"],
    },
    AppTemplateDef {
        id: "drone",
        name: "Drone CI",
        description: "Container-native CI/CD platform with pipeline-as-code",
        category: "Development",
        image: "drone/drone:latest",
        default_port: 8087,
        container_port: "80/tcp",
        env_vars: &[
            EnvVarDef { name: "DRONE_SERVER_HOST", label: "Server Host", default: "localhost", required: true, secret: false },
            EnvVarDef { name: "DRONE_SERVER_PROTO", label: "Protocol", default: "http", required: false, secret: false },
            EnvVarDef { name: "DRONE_RPC_SECRET", label: "RPC Secret", default: "", required: true, secret: true },
        ],
        volumes: &["/data"],
    },
    AppTemplateDef {
        id: "registry",
        name: "Docker Registry",
        description: "Private Docker image registry for storing and distributing container images",
        category: "Development",
        image: "registry:2",
        default_port: 5000,
        container_port: "5000/tcp",
        env_vars: &[],
        volumes: &["/var/lib/registry"],
    },
    AppTemplateDef {
        id: "mailhog",
        name: "MailHog",
        description: "Email testing tool — catches outgoing emails for dev/testing",
        category: "Development",
        image: "mailhog/mailhog:latest",
        default_port: 8025,
        container_port: "8025/tcp",
        env_vars: &[],
        volumes: &[],
    },
    AppTemplateDef {
        id: "pihole",
        name: "Pi-hole",
        description: "Network-wide ad blocker and DNS sinkhole",
        category: "Networking",
        image: "pihole/pihole:latest",
        default_port: 8088,
        container_port: "80/tcp",
        env_vars: &[
            EnvVarDef { name: "WEBPASSWORD", label: "Web Password", default: "", required: true, secret: true },
        ],
        volumes: &["/etc/pihole", "/etc/dnsmasq.d"],
    },
    AppTemplateDef {
        id: "loki",
        name: "Grafana Loki",
        description: "Log aggregation system designed to work with Grafana",
        category: "Monitoring",
        image: "grafana/loki:latest",
        default_port: 3100,
        container_port: "3100/tcp",
        env_vars: &[],
        volumes: &["/loki"],
    },
];

/// Convert a static template definition to the owned serializable type.
fn to_app_template(def: &AppTemplateDef) -> AppTemplate {
    AppTemplate {
        id: def.id.to_string(),
        name: def.name.to_string(),
        description: def.description.to_string(),
        category: def.category.to_string(),
        image: def.image.to_string(),
        default_port: def.default_port,
        container_port: def.container_port.to_string(),
        env_vars: def
            .env_vars
            .iter()
            .map(|ev| EnvVar {
                name: ev.name.to_string(),
                label: ev.label.to_string(),
                default: ev.default.to_string(),
                required: ev.required,
                secret: ev.secret,
            })
            .collect(),
        volumes: def.volumes.iter().map(|v| v.to_string()).collect(),
    }
}

/// Return all available app templates.
pub fn list_templates() -> Vec<AppTemplate> {
    TEMPLATES.iter().map(to_app_template).collect()
}

/// Deploy an app from a template: pull image, create container, start it.
pub async fn deploy_app(
    template_id: &str,
    name: &str,
    port: u16,
    env: HashMap<String, String>,
    domain: Option<&str>,
    memory_mb: Option<u64>,
    cpu_percent: Option<u64>,
) -> Result<DeployResult, String> {
    let template = TEMPLATES
        .iter()
        .find(|t| t.id == template_id)
        .ok_or_else(|| format!("Unknown template: {template_id}"))?;

    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;

    // Pull image
    let mut pull = docker.create_image(
        Some(CreateImageOptions {
            from_image: template.image,
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

    let container_name = format!("dockpanel-app-{name}");

    // Build environment variables: merge template defaults with user-supplied values
    let mut env_list: Vec<String> = Vec::new();
    for ev in template.env_vars {
        let value = env
            .get(ev.name)
            .cloned()
            .unwrap_or_else(|| ev.default.to_string());
        if !value.is_empty() {
            env_list.push(format!("{}={}", ev.name, value));
        }
    }
    // Include any extra env vars the user passed that aren't in the template
    for (k, v) in &env {
        if !template.env_vars.iter().any(|ev| ev.name == k.as_str()) {
            env_list.push(format!("{k}={v}"));
        }
    }

    // Port bindings
    let mut port_bindings = HashMap::new();
    port_bindings.insert(
        template.container_port.to_string(),
        Some(vec![bollard::service::PortBinding {
            host_ip: Some("127.0.0.1".to_string()),
            host_port: Some(port.to_string()),
        }]),
    );

    // Volume binds
    let mut binds: Vec<String> = Vec::new();
    for vol in template.volumes {
        let host_dir = format!("/var/lib/dockpanel/apps/{name}{vol}");
        binds.push(format!("{host_dir}:{vol}"));
    }

    // Portainer needs the Docker socket mounted
    if template.id == "portainer" {
        binds.push("/var/run/docker.sock:/var/run/docker.sock".to_string());
    }

    let mut host_config = bollard::service::HostConfig {
        port_bindings: Some(port_bindings),
        restart_policy: Some(bollard::service::RestartPolicy {
            name: Some(bollard::service::RestartPolicyNameEnum::UNLESS_STOPPED),
            ..Default::default()
        }),
        ..Default::default()
    };

    // Docker resource limits
    if let Some(mem) = memory_mb {
        if mem > 0 {
            host_config.memory = Some((mem * 1024 * 1024) as i64);
            // Memory swap = 2x memory (allows some swap)
            host_config.memory_swap = Some((mem * 2 * 1024 * 1024) as i64);
        }
    }
    if let Some(cpu) = cpu_percent {
        if cpu > 0 && cpu <= 100 {
            // CPU quota: period * (percent/100)
            // Default period is 100000 (100ms)
            host_config.cpu_period = Some(100_000);
            host_config.cpu_quota = Some((cpu * 1000) as i64);
        }
    }

    if !binds.is_empty() {
        host_config.binds = Some(binds);
    }

    let mut exposed_ports = HashMap::new();
    exposed_ports.insert(template.container_port.to_string(), HashMap::new());

    let config = Config {
        image: Some(template.image.to_string()),
        env: if env_list.is_empty() {
            None
        } else {
            Some(env_list)
        },
        exposed_ports: Some(exposed_ports),
        host_config: Some(host_config),
        labels: Some({
            let mut labels = HashMap::from([
                ("dockpanel.managed".to_string(), "true".to_string()),
                (
                    "dockpanel.app.template".to_string(),
                    template.id.to_string(),
                ),
                ("dockpanel.app.name".to_string(), name.to_string()),
            ]);
            if let Some(domain) = domain {
                labels.insert("dockpanel.app.domain".to_string(), domain.to_string());
            }
            labels
        }),
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

    docker
        .start_container(&container.id, None::<StartContainerOptions<String>>)
        .await
        .map_err(|e| format!("Failed to start container: {e}"))?;

    tracing::info!(
        "App deployed: {container_name} (template={}, port={port})",
        template.id
    );

    Ok(DeployResult {
        container_id: container.id,
        name: container_name,
        port,
    })
}

/// List all deployed apps (containers with dockpanel.app.template label).
pub async fn list_deployed_apps() -> Result<Vec<DeployedApp>, String> {
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

    let apps = containers
        .into_iter()
        .filter_map(|c| {
            let labels = c.labels.as_ref()?;
            // Only include containers that have the app template label
            let template = labels.get("dockpanel.app.template")?;
            let id = c.id.as_ref()?;

            let port = c
                .ports
                .as_ref()
                .and_then(|ports| {
                    ports
                        .iter()
                        .find(|p| p.public_port.is_some())
                        .and_then(|p| p.public_port)
                })
                .map(|p| p as u16);

            let status = c.state.unwrap_or_default();
            let name = c
                .names
                .as_ref()
                .and_then(|n| n.first())
                .map(|n| n.trim_start_matches('/').to_string())
                .unwrap_or_default();

            let domain = labels.get("dockpanel.app.domain").cloned();

            // Extract health from human-readable status string (e.g., "Up 2 hours (healthy)")
            let health = c.status.as_deref().and_then(|s| {
                if s.contains("(healthy)") {
                    Some("healthy".to_string())
                } else if s.contains("(unhealthy)") {
                    Some("unhealthy".to_string())
                } else if s.contains("(health: starting)") {
                    Some("starting".to_string())
                } else {
                    None
                }
            });

            Some(DeployedApp {
                container_id: id.clone(),
                name,
                template: template.clone(),
                status,
                port,
                domain,
                health,
            })
        })
        .collect();

    Ok(apps)
}

/// Stop a running app container.
pub async fn stop_app(container_id: &str) -> Result<(), String> {
    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;

    docker
        .stop_container(container_id, Some(StopContainerOptions { t: 10 }))
        .await
        .map_err(|e| format!("Failed to stop container: {e}"))?;

    tracing::info!("App container stopped: {container_id}");
    Ok(())
}

/// Start a stopped app container.
pub async fn start_app(container_id: &str) -> Result<(), String> {
    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;

    docker
        .start_container(container_id, None::<StartContainerOptions<String>>)
        .await
        .map_err(|e| format!("Failed to start container: {e}"))?;

    tracing::info!("App container started: {container_id}");
    Ok(())
}

/// Restart an app container.
pub async fn restart_app(container_id: &str) -> Result<(), String> {
    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;

    docker
        .restart_container(container_id, Some(bollard::container::RestartContainerOptions { t: 10 }))
        .await
        .map_err(|e| format!("Failed to restart container: {e}"))?;

    tracing::info!("App container restarted: {container_id}");
    Ok(())
}

/// Get container logs (last N lines).
pub async fn get_app_logs(container_id: &str, tail: usize) -> Result<String, String> {
    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;

    use bollard::container::LogsOptions;

    let mut output = docker.logs(
        container_id,
        Some(LogsOptions::<String> {
            stdout: true,
            stderr: true,
            tail: tail.to_string(),
            ..Default::default()
        }),
    );

    let mut logs = String::new();
    while let Some(result) = output.next().await {
        match result {
            Ok(log) => logs.push_str(&log.to_string()),
            Err(e) => return Err(format!("Failed to read logs: {e}")),
        }
    }

    Ok(logs)
}

/// Get the domain label from a container, if set.
pub async fn get_app_domain(container_id: &str) -> Option<String> {
    let docker = Docker::connect_with_local_defaults().ok()?;
    let info = docker.inspect_container(container_id, None).await.ok()?;
    info.config?.labels?.get("dockpanel.app.domain").cloned()
}

/// Stop and remove an app container, optionally removing its volumes.
pub async fn remove_app(container_id: &str) -> Result<(), String> {
    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;

    // Stop first (ignore error if already stopped)
    docker
        .stop_container(container_id, Some(StopContainerOptions { t: 10 }))
        .await
        .ok();

    docker
        .remove_container(
            container_id,
            Some(RemoveContainerOptions {
                v: true,
                force: true,
                ..Default::default()
            }),
        )
        .await
        .map_err(|e| format!("Failed to remove container: {e}"))?;

    tracing::info!("App container removed: {container_id}");
    Ok(())
}
