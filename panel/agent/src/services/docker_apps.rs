use bollard::container::{
    Config, CreateContainerOptions, ListContainersOptions, RemoveContainerOptions,
    RenameContainerOptions, StartContainerOptions, StopContainerOptions,
};
use bollard::image::CreateImageOptions;
use bollard::Docker;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio_stream::StreamExt;

// ── Container auto-update detection ────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ImageUpdateInfo {
    pub container_id: String,
    pub name: String,
    pub image: String,
    pub current_digest: Option<String>,
    pub remote_digest: Option<String>,
    pub update_available: bool,
    pub check_error: Option<String>,
}

struct ImageRef {
    registry: String,
    repository: String,
    tag: String,
}

/// Parse a Docker image reference into registry, repository, tag.
/// Examples:
///   "redis:7-alpine"           → registry-1.docker.io / library/redis / 7-alpine
///   "grafana/grafana:latest"   → registry-1.docker.io / grafana/grafana / latest
///   "ghcr.io/foo/bar:v1"      → ghcr.io / foo/bar / v1
fn parse_image_ref(image: &str) -> ImageRef {
    // Strip any @sha256:... digest suffix
    let image = image.split('@').next().unwrap_or(image);

    let has_registry = image.contains('/') && {
        let first = image.split('/').next().unwrap_or("");
        first.contains('.') || first.contains(':')
    };

    let (registry, rest) = if has_registry {
        let (reg, rest) = image.split_once('/').unwrap_or(("", image));
        (reg.to_string(), rest.to_string())
    } else if image.contains('/') {
        ("registry-1.docker.io".to_string(), image.to_string())
    } else {
        ("registry-1.docker.io".to_string(), format!("library/{image}"))
    };

    let (repository, tag) = if let Some((r, t)) = rest.rsplit_once(':') {
        (r.to_string(), t.to_string())
    } else {
        (rest, "latest".to_string())
    };

    ImageRef { registry, repository, tag }
}

/// Fetch the manifest digest from a Docker registry (Docker Hub, GHCR, or generic OCI).
async fn get_registry_digest(image_ref: &ImageRef) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("HTTP client: {e}"))?;

    let accept = "application/vnd.docker.distribution.manifest.list.v2+json, \
                  application/vnd.oci.image.index.v1+json, \
                  application/vnd.docker.distribution.manifest.v2+json, \
                  application/vnd.oci.image.manifest.v1+json";

    if image_ref.registry == "registry-1.docker.io" || image_ref.registry == "docker.io" {
        // Docker Hub auth
        let token_url = format!(
            "https://auth.docker.io/token?service=registry.docker.io&scope=repository:{}:pull",
            image_ref.repository
        );
        #[derive(Deserialize)]
        struct TokenResp { token: String }

        let token: TokenResp = client.get(&token_url)
            .send().await.map_err(|e| format!("Auth failed: {e}"))?
            .json().await.map_err(|e| format!("Auth parse: {e}"))?;

        let url = format!(
            "https://registry-1.docker.io/v2/{}/manifests/{}",
            image_ref.repository, image_ref.tag
        );
        let resp = client.head(&url)
            .header("Authorization", format!("Bearer {}", token.token))
            .header("Accept", accept)
            .send().await.map_err(|e| format!("Manifest fetch: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("Registry HTTP {}", resp.status()));
        }
        extract_digest(&resp)
    } else if image_ref.registry == "ghcr.io" {
        // GitHub Container Registry
        let token_url = format!(
            "https://ghcr.io/token?service=ghcr.io&scope=repository:{}:pull",
            image_ref.repository
        );
        #[derive(Deserialize)]
        struct TokenResp { token: String }

        let token: TokenResp = client.get(&token_url)
            .send().await.map_err(|e| format!("GHCR auth: {e}"))?
            .json().await.map_err(|e| format!("GHCR auth parse: {e}"))?;

        let url = format!(
            "https://ghcr.io/v2/{}/manifests/{}",
            image_ref.repository, image_ref.tag
        );
        let resp = client.head(&url)
            .header("Authorization", format!("Bearer {}", token.token))
            .header("Accept", accept)
            .send().await.map_err(|e| format!("GHCR manifest: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("GHCR HTTP {}", resp.status()));
        }
        extract_digest(&resp)
    } else {
        // Generic OCI registry — try anonymous access
        let url = format!(
            "https://{}/v2/{}/manifests/{}",
            image_ref.registry, image_ref.repository, image_ref.tag
        );
        let resp = client.head(&url)
            .header("Accept", accept)
            .send().await.map_err(|e| format!("Registry: {e}"))?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err("Auth required".to_string());
        }
        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }
        extract_digest(&resp)
    }
}

fn extract_digest(resp: &reqwest::Response) -> Result<String, String> {
    resp.headers()
        .get("docker-content-digest")
        .ok_or_else(|| "No digest header".to_string())?
        .to_str()
        .map(|s| s.to_string())
        .map_err(|e| format!("Invalid digest: {e}"))
}

/// Check all managed containers for available image updates by comparing
/// local RepoDigests against registry manifests.
pub async fn check_image_updates() -> Result<Vec<ImageUpdateInfo>, String> {
    let docker = Docker::connect_with_local_defaults()
        .map_err(|e| format!("Docker connect: {e}"))?;

    let apps = list_deployed_apps().await?;
    let mut results = Vec::with_capacity(apps.len());

    for app in &apps {
        let image = match &app.image {
            Some(img) => img.clone(),
            None => {
                results.push(ImageUpdateInfo {
                    container_id: app.container_id.clone(),
                    name: app.name.clone(),
                    image: "unknown".to_string(),
                    current_digest: None,
                    remote_digest: None,
                    update_available: false,
                    check_error: Some("No image info".to_string()),
                });
                continue;
            }
        };

        // Get local image digest from RepoDigests
        let local_digest = docker.inspect_image(&image).await.ok()
            .and_then(|info| info.repo_digests)
            .and_then(|d| d.into_iter().next())
            .and_then(|d| d.split('@').nth(1).map(|s| s.to_string()));

        let image_ref = parse_image_ref(&image);

        let (remote_digest, check_error, update_available) = match get_registry_digest(&image_ref).await {
            Ok(digest) => {
                let has_update = local_digest.as_ref().map_or(false, |local| local != &digest);
                (Some(digest), None, has_update)
            }
            Err(e) => (None, Some(e), false),
        };

        results.push(ImageUpdateInfo {
            container_id: app.container_id.clone(),
            name: app.name.clone(),
            image,
            current_digest: local_digest,
            remote_digest,
            update_available,
            check_error,
        });
    }

    Ok(results)
}

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
    /// True when the template materially benefits from GPU passthrough
    /// (LLM/diffusion/ASR inference). Frontend uses this to badge the
    /// template card and pre-tick the GPU toggle on the deploy form.
    pub gpu_recommended: bool,
}

/// Template IDs whose container materially benefits from GPU passthrough.
/// Source-of-truth list kept here instead of a per-template field to avoid
/// touching every static AppTemplateDef declaration when this set evolves.
static GPU_RECOMMENDED_TEMPLATES: &[&str] = &[
    "ollama",
    "localai",
    "vllm",
    "stable-diffusion-webui",
    "text-generation-webui",
    "whisper",
];

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
pub struct UpdateResult {
    pub container_id: String,
    pub blue_green: bool,
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
    pub image: Option<String>,
    pub volumes: Vec<String>,
    pub stack_id: Option<String>,
    pub user_id: Option<String>,
}

static TEMPLATES: &[AppTemplateDef] = &[
    // WordPress, Drupal, Joomla, PrestaShop moved to Sites (native PHP install)
    AppTemplateDef {
        id: "ghost",
        name: "Ghost",
        description: "Professional publishing platform for blogs and newsletters",
        category: "CMS",
        image: "ghost:5",
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
        image: "adminer:4",
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
        image: "portainer/portainer-ce:2",
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
        image: "n8nio/n8n:1",
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
        image: "gitea/gitea:1",
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
        image: "strapi/strapi:4",
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
        image: "directus/directus:11",
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
        image: "nextcloud:30",
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
        image: "grafana/grafana:11",
        default_port: 3002,
        container_port: "3000/tcp",
        env_vars: &[
            EnvVarDef { name: "GF_SECURITY_ADMIN_PASSWORD", label: "Admin Password", default: "", required: true, secret: true },
        ],
        volumes: &["/var/lib/grafana"],
    },
    AppTemplateDef {
        id: "prometheus",
        name: "Prometheus",
        description: "Monitoring system and time-series database for metrics",
        category: "Monitoring",
        image: "prom/prometheus:v3",
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
        image: "plausible/analytics:v2",
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
        image: "ghcr.io/umami-software/umami:postgresql-v2.15.1",
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
        image: "matomo:5",
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
        image: "dpage/pgadmin4:8",
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
        image: "vaultwarden/server:1",
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
        image: "getmeili/meilisearch:v1",
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
        image: "metabase/metabase:v0.52",
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
        image: "jellyfin/jellyfin:10",
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
        image: "codercom/code-server:4",
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
        image: "drone/drone:2",
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
        id: "mailpit",
        name: "Mailpit",
        description: "Email testing tool — catches outgoing emails for dev/testing",
        category: "Development",
        image: "axllent/mailpit:v1",
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
        image: "pihole/pihole:2024.07.0",
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
        image: "grafana/loki:3",
        default_port: 3100,
        container_port: "3100/tcp",
        env_vars: &[],
        volumes: &["/loki"],
    },
    // ─── Wiki / Docs ─────────────────────────────────────────────
    AppTemplateDef {
        id: "wikijs",
        name: "Wiki.js",
        description: "Modern wiki engine with powerful features and beautiful interface",
        category: "Tools",
        image: "ghcr.io/requarks/wiki:2",
        default_port: 3005,
        container_port: "3000/tcp",
        env_vars: &[
            EnvVarDef { name: "DB_TYPE", label: "Database Type", default: "sqlite", required: false, secret: false },
        ],
        volumes: &["/wiki/data"],
    },
    AppTemplateDef {
        id: "bookstack",
        name: "BookStack",
        description: "Simple, self-hosted platform for organizing and storing information",
        category: "Tools",
        image: "lscr.io/linuxserver/bookstack:latest",
        default_port: 6875,
        container_port: "80/tcp",
        env_vars: &[
            EnvVarDef { name: "APP_URL", label: "Application URL", default: "", required: true, secret: false },
            EnvVarDef { name: "DB_HOST", label: "Database Host", default: "", required: true, secret: false },
            EnvVarDef { name: "DB_USER", label: "Database User", default: "bookstack", required: false, secret: false },
            EnvVarDef { name: "DB_PASS", label: "Database Password", default: "", required: true, secret: true },
            EnvVarDef { name: "DB_DATABASE", label: "Database Name", default: "bookstack", required: false, secret: false },
        ],
        volumes: &["/config"],
    },
    AppTemplateDef {
        id: "outline",
        name: "Outline",
        description: "Team knowledge base and wiki with a fast, beautiful editor",
        category: "Tools",
        image: "outlinewiki/outline:0.82",
        default_port: 3006,
        container_port: "3000/tcp",
        env_vars: &[
            EnvVarDef { name: "SECRET_KEY", label: "Secret Key", default: "", required: true, secret: true },
            EnvVarDef { name: "URL", label: "Application URL", default: "", required: true, secret: false },
            EnvVarDef { name: "DATABASE_URL", label: "Database URL", default: "", required: true, secret: false },
            EnvVarDef { name: "REDIS_URL", label: "Redis URL", default: "redis://localhost:6379", required: false, secret: false },
        ],
        volumes: &["/var/lib/outline/data"],
    },
    // ─── Communication ───────────────────────────────────────────
    AppTemplateDef {
        id: "mattermost",
        name: "Mattermost",
        description: "Open-source Slack alternative for secure team collaboration",
        category: "Tools",
        image: "mattermost/mattermost-team-edition:10",
        default_port: 8065,
        container_port: "8065/tcp",
        env_vars: &[
            EnvVarDef { name: "MM_SQLSETTINGS_DATASOURCE", label: "Database Connection String", default: "", required: true, secret: false },
        ],
        volumes: &["/mattermost/config", "/mattermost/data", "/mattermost/logs"],
    },
    AppTemplateDef {
        id: "rocketchat",
        name: "Rocket.Chat",
        description: "Open-source team communication platform with channels, DMs, and video",
        category: "Tools",
        image: "registry.rocket.chat/rocketchat/rocket.chat:7",
        default_port: 3007,
        container_port: "3000/tcp",
        env_vars: &[
            EnvVarDef { name: "MONGO_URL", label: "MongoDB URL", default: "", required: true, secret: false },
            EnvVarDef { name: "ROOT_URL", label: "Root URL", default: "", required: true, secret: false },
        ],
        volumes: &["/app/uploads"],
    },
    AppTemplateDef {
        id: "discourse",
        name: "Discourse",
        description: "Modern forum and community discussion platform",
        category: "Tools",
        image: "bitnami/discourse:3",
        default_port: 3008,
        container_port: "3000/tcp",
        env_vars: &[
            EnvVarDef { name: "DISCOURSE_HOST", label: "Discourse Hostname", default: "", required: true, secret: false },
            EnvVarDef { name: "DISCOURSE_DATABASE_HOST", label: "Database Host", default: "", required: true, secret: false },
            EnvVarDef { name: "DISCOURSE_DATABASE_PASSWORD", label: "Database Password", default: "", required: true, secret: true },
            EnvVarDef { name: "DISCOURSE_REDIS_HOST", label: "Redis Host", default: "redis", required: false, secret: false },
        ],
        volumes: &["/bitnami/discourse"],
    },
    // ─── Media ───────────────────────────────────────────────────
    AppTemplateDef {
        id: "immich",
        name: "Immich",
        description: "High-performance self-hosted photo and video management",
        category: "Media",
        image: "ghcr.io/immich-app/immich-server:release",
        default_port: 2283,
        container_port: "3001/tcp",
        env_vars: &[
            EnvVarDef { name: "DB_HOSTNAME", label: "Database Hostname", default: "", required: true, secret: false },
            EnvVarDef { name: "DB_USERNAME", label: "Database Username", default: "immich", required: false, secret: false },
            EnvVarDef { name: "DB_PASSWORD", label: "Database Password", default: "", required: true, secret: true },
            EnvVarDef { name: "DB_DATABASE_NAME", label: "Database Name", default: "immich", required: false, secret: false },
            EnvVarDef { name: "REDIS_HOSTNAME", label: "Redis Hostname", default: "redis", required: false, secret: false },
        ],
        volumes: &["/usr/src/app/upload"],
    },
    AppTemplateDef {
        id: "photoprism",
        name: "PhotoPrism",
        description: "AI-powered photo management app for browsing, organizing, and sharing",
        category: "Media",
        image: "photoprism/photoprism:latest",
        default_port: 2342,
        container_port: "2342/tcp",
        env_vars: &[
            EnvVarDef { name: "PHOTOPRISM_ADMIN_PASSWORD", label: "Admin Password", default: "", required: true, secret: true },
            EnvVarDef { name: "PHOTOPRISM_SITE_URL", label: "Site URL", default: "http://localhost:2342", required: false, secret: false },
        ],
        volumes: &["/photoprism/storage", "/photoprism/originals"],
    },
    // ─── Security ────────────────────────────────────────────────
    AppTemplateDef {
        id: "authentik",
        name: "Authentik",
        description: "Open-source identity provider with SSO, MFA, and user management",
        category: "Security",
        image: "ghcr.io/goauthentik/server:2024.12",
        default_port: 9001,
        container_port: "9000/tcp",
        env_vars: &[
            EnvVarDef { name: "AUTHENTIK_SECRET_KEY", label: "Secret Key", default: "", required: true, secret: true },
            EnvVarDef { name: "AUTHENTIK_REDIS__HOST", label: "Redis Host", default: "redis", required: false, secret: false },
            EnvVarDef { name: "AUTHENTIK_POSTGRESQL__HOST", label: "PostgreSQL Host", default: "", required: true, secret: false },
            EnvVarDef { name: "AUTHENTIK_POSTGRESQL__PASSWORD", label: "PostgreSQL Password", default: "", required: true, secret: true },
        ],
        volumes: &["/media", "/templates"],
    },
    AppTemplateDef {
        id: "keycloak",
        name: "Keycloak",
        description: "Open-source IAM with SSO, identity brokering, and social login. Requires 'start-dev' command override for dev mode.",
        category: "Security",
        image: "quay.io/keycloak/keycloak:26",
        default_port: 8180,
        container_port: "8080/tcp",
        env_vars: &[
            EnvVarDef { name: "KEYCLOAK_ADMIN", label: "Admin Username", default: "admin", required: false, secret: false },
            EnvVarDef { name: "KEYCLOAK_ADMIN_PASSWORD", label: "Admin Password", default: "", required: true, secret: true },
            EnvVarDef { name: "KC_DB", label: "Database Type", default: "postgres", required: false, secret: false },
            EnvVarDef { name: "KC_DB_URL", label: "Database JDBC URL", default: "", required: true, secret: false },
        ],
        volumes: &[],
    },
    // ─── Dev Tools ───────────────────────────────────────────────
    AppTemplateDef {
        id: "woodpecker",
        name: "Woodpecker CI",
        description: "Simple yet powerful CI/CD engine with great extensibility",
        category: "Development",
        image: "woodpeckerci/woodpecker-server:2",
        default_port: 8000,
        container_port: "8000/tcp",
        env_vars: &[
            EnvVarDef { name: "WOODPECKER_OPEN", label: "Open Registration", default: "true", required: false, secret: false },
            EnvVarDef { name: "WOODPECKER_ADMIN", label: "Admin User", default: "", required: true, secret: false },
            EnvVarDef { name: "WOODPECKER_HOST", label: "Server Host URL", default: "", required: true, secret: false },
            EnvVarDef { name: "WOODPECKER_AGENT_SECRET", label: "Agent Secret", default: "", required: true, secret: true },
        ],
        volumes: &["/var/lib/woodpecker"],
    },
    AppTemplateDef {
        id: "sonarqube",
        name: "SonarQube",
        description: "Continuous code quality and security inspection platform",
        category: "Development",
        image: "sonarqube:10-community",
        default_port: 9002,
        container_port: "9000/tcp",
        env_vars: &[
            EnvVarDef { name: "SONAR_JDBC_URL", label: "JDBC URL", default: "jdbc:h2:tcp://localhost/sonar", required: false, secret: false },
        ],
        volumes: &["/opt/sonarqube/data", "/opt/sonarqube/logs", "/opt/sonarqube/extensions"],
    },
    AppTemplateDef {
        id: "forgejo",
        name: "Forgejo",
        description: "Community-driven Git forge — lightweight Gitea fork with extra features",
        category: "Development",
        image: "codeberg.org/forgejo/forgejo:9",
        default_port: 3009,
        container_port: "3000/tcp",
        env_vars: &[],
        volumes: &["/data", "/etc/timezone"],
    },
    // ─── Business / Productivity ─────────────────────────────────
    AppTemplateDef {
        id: "invoice-ninja",
        name: "Invoice Ninja",
        description: "Full-featured invoicing, payments, and expense tracking for freelancers",
        category: "Tools",
        image: "invoiceninja/invoiceninja:5",
        default_port: 9003,
        container_port: "80/tcp",
        env_vars: &[
            EnvVarDef { name: "APP_URL", label: "Application URL", default: "", required: true, secret: false },
            EnvVarDef { name: "APP_KEY", label: "Application Key", default: "", required: true, secret: true },
            EnvVarDef { name: "DB_HOST", label: "Database Host", default: "", required: true, secret: false },
            EnvVarDef { name: "DB_DATABASE", label: "Database Name", default: "ninja", required: false, secret: false },
            EnvVarDef { name: "DB_USERNAME", label: "Database Username", default: "ninja", required: false, secret: false },
            EnvVarDef { name: "DB_PASSWORD", label: "Database Password", default: "", required: true, secret: true },
        ],
        volumes: &["/var/app/public", "/var/app/storage"],
    },
    AppTemplateDef {
        id: "erpnext",
        name: "ERPNext",
        description: "Open-source ERP for manufacturing, distribution, retail, and services",
        category: "Tools",
        image: "frappe/erpnext:latest",
        default_port: 8080,
        container_port: "8080/tcp",
        env_vars: &[],
        volumes: &["/home/frappe/frappe-bench/sites"],
    },
    AppTemplateDef {
        id: "calcom",
        name: "Cal.com",
        description: "Open-source scheduling infrastructure for appointments and meetings",
        category: "Tools",
        image: "calcom/cal.com:v4",
        default_port: 3010,
        container_port: "3000/tcp",
        env_vars: &[
            EnvVarDef { name: "DATABASE_URL", label: "Database URL", default: "", required: true, secret: false },
            EnvVarDef { name: "NEXTAUTH_SECRET", label: "NextAuth Secret", default: "", required: true, secret: true },
            EnvVarDef { name: "CALENDSO_ENCRYPTION_KEY", label: "Encryption Key", default: "", required: true, secret: true },
        ],
        volumes: &[],
    },
    // ─── Support ─────────────────────────────────────────────────
    AppTemplateDef {
        id: "chatwoot",
        name: "Chatwoot",
        description: "Open-source customer engagement platform with live chat and helpdesk",
        category: "Tools",
        image: "chatwoot/chatwoot:v3",
        default_port: 3011,
        container_port: "3000/tcp",
        env_vars: &[
            EnvVarDef { name: "SECRET_KEY_BASE", label: "Secret Key Base", default: "", required: true, secret: true },
            EnvVarDef { name: "FRONTEND_URL", label: "Frontend URL", default: "", required: true, secret: false },
            EnvVarDef { name: "POSTGRES_HOST", label: "PostgreSQL Host", default: "", required: true, secret: false },
            EnvVarDef { name: "POSTGRES_PASSWORD", label: "PostgreSQL Password", default: "", required: true, secret: true },
            EnvVarDef { name: "REDIS_URL", label: "Redis URL", default: "redis://redis:6379", required: false, secret: false },
        ],
        volumes: &["/app/storage"],
    },
    AppTemplateDef {
        id: "typebot",
        name: "Typebot",
        description: "Open-source chatbot builder with drag-and-drop visual editor",
        category: "Tools",
        image: "baptistearno/typebot-builder:2",
        default_port: 3012,
        container_port: "3000/tcp",
        env_vars: &[
            EnvVarDef { name: "DATABASE_URL", label: "Database URL", default: "", required: true, secret: false },
            EnvVarDef { name: "NEXTAUTH_URL", label: "NextAuth URL", default: "", required: true, secret: false },
            EnvVarDef { name: "ENCRYPTION_SECRET", label: "Encryption Secret", default: "", required: true, secret: true },
        ],
        volumes: &[],
    },
    // Roundcube and Rspamd removed — use Mail → Webmail and Mail → Spam Filter instead
    // ─── AI / Machine Learning ──────────────────────────────────
    AppTemplateDef {
        id: "ollama",
        name: "Ollama",
        description: "Run large language models locally (Llama, Mistral, Gemma, Phi)",
        category: "AI",
        image: "ollama/ollama:latest",
        default_port: 11434,
        container_port: "11434/tcp",
        env_vars: &[
            EnvVarDef { name: "OLLAMA_KEEP_ALIVE", label: "Model idle timeout", default: "5m", required: false, secret: false },
            EnvVarDef { name: "OLLAMA_NUM_PARALLEL", label: "Max parallel requests", default: "1", required: false, secret: false },
        ],
        volumes: &["/root/.ollama"],
    },
    AppTemplateDef {
        id: "open-webui",
        name: "Open WebUI",
        description: "ChatGPT-style web interface for Ollama and OpenAI-compatible APIs",
        category: "AI",
        image: "ghcr.io/open-webui/open-webui:main",
        default_port: 3101,
        container_port: "8080/tcp",
        env_vars: &[
            EnvVarDef { name: "OLLAMA_BASE_URL", label: "Ollama URL", default: "http://host.docker.internal:11434", required: false, secret: false },
        ],
        volumes: &["/app/backend/data"],
    },
    AppTemplateDef {
        id: "localai",
        name: "LocalAI",
        description: "Self-hosted OpenAI-compatible API for running LLMs, image and audio generation",
        category: "AI",
        // GPU-aware default. Operators on CPU-only hosts can switch the image
        // to localai/localai:latest-cpu via the Image field on the deploy form.
        image: "localai/localai:latest-gpu-nvidia-cuda-12",
        default_port: 8296,
        container_port: "8080/tcp",
        env_vars: &[],
        volumes: &["/build/models"],
    },
    // ─── Dashboards ─────────────────────────────────────────────
    AppTemplateDef {
        id: "homepage",
        name: "Homepage",
        description: "Highly customizable application dashboard with service integrations",
        category: "Tools",
        image: "ghcr.io/gethomepage/homepage:latest",
        default_port: 3013,
        container_port: "3000/tcp",
        env_vars: &[],
        volumes: &["/app/config"],
    },
    AppTemplateDef {
        id: "homarr",
        name: "Homarr",
        description: "Sleek server dashboard with drag-and-drop widgets and integrations",
        category: "Tools",
        image: "ghcr.io/homarr-labs/homarr:latest",
        default_port: 7575,
        container_port: "7575/tcp",
        env_vars: &[],
        volumes: &["/appdata"],
    },
    AppTemplateDef {
        id: "dashy",
        name: "Dashy",
        description: "Feature-rich self-hosted dashboard for your homelab and cloud stack",
        category: "Tools",
        image: "lissy93/dashy:latest",
        default_port: 4000,
        container_port: "8080/tcp",
        env_vars: &[],
        volumes: &["/app/user-data"],
    },
    // ─── Documents / Productivity ────────────────────────────────
    AppTemplateDef {
        id: "paperless-ngx",
        name: "Paperless-ngx",
        description: "Document management system that transforms physical documents into searchable archive",
        category: "Tools",
        image: "ghcr.io/paperless-ngx/paperless-ngx:latest",
        default_port: 8010,
        container_port: "8000/tcp",
        env_vars: &[
            EnvVarDef { name: "PAPERLESS_SECRET_KEY", label: "Secret Key", default: "", required: true, secret: true },
            EnvVarDef { name: "PAPERLESS_ADMIN_USER", label: "Admin Username", default: "admin", required: false, secret: false },
            EnvVarDef { name: "PAPERLESS_ADMIN_PASSWORD", label: "Admin Password", default: "", required: true, secret: true },
        ],
        volumes: &["/usr/src/paperless/data", "/usr/src/paperless/media"],
    },
    AppTemplateDef {
        id: "stirling-pdf",
        name: "Stirling-PDF",
        description: "Self-hosted PDF manipulation tool with merge, split, convert, and OCR",
        category: "Tools",
        image: "frooodle/s-pdf:latest",
        default_port: 8182,
        container_port: "8080/tcp",
        env_vars: &[],
        volumes: &["/usr/share/tessdata", "/configs"],
    },
    AppTemplateDef {
        id: "actual-budget",
        name: "Actual Budget",
        description: "Privacy-focused local-first personal budgeting app",
        category: "Tools",
        image: "actualbudget/actual-server:latest",
        default_port: 5006,
        container_port: "5006/tcp",
        env_vars: &[],
        volumes: &["/data"],
    },
    AppTemplateDef {
        id: "hedgedoc",
        name: "HedgeDoc",
        description: "Real-time collaborative Markdown editor for teams",
        category: "Tools",
        image: "quay.io/hedgedoc/hedgedoc:latest",
        default_port: 3014,
        container_port: "3000/tcp",
        env_vars: &[
            EnvVarDef { name: "CMD_DB_URL", label: "Database URL", default: "", required: true, secret: false },
            EnvVarDef { name: "CMD_SESSION_SECRET", label: "Session Secret", default: "", required: true, secret: true },
            EnvVarDef { name: "CMD_DOMAIN", label: "Domain", default: "", required: false, secret: false },
        ],
        volumes: &["/hedgedoc/public/uploads"],
    },
    AppTemplateDef {
        id: "vikunja",
        name: "Vikunja",
        description: "Open-source to-do and project management app (Todoist alternative)",
        category: "Tools",
        image: "vikunja/vikunja:latest",
        default_port: 3456,
        container_port: "3456/tcp",
        env_vars: &[
            EnvVarDef { name: "VIKUNJA_SERVICE_JWTSECRET", label: "JWT Secret", default: "", required: true, secret: true },
        ],
        volumes: &["/app/vikunja/files"],
    },
    AppTemplateDef {
        id: "memos",
        name: "Memos",
        description: "Privacy-first lightweight note-taking service (Google Keep alternative)",
        category: "Tools",
        image: "neosmemo/memos:stable",
        default_port: 5230,
        container_port: "5230/tcp",
        env_vars: &[],
        volumes: &["/var/opt/memos"],
    },
    AppTemplateDef {
        id: "trilium",
        name: "Trilium Notes",
        description: "Hierarchical note-taking application with rich text, relations, and scripting",
        category: "Tools",
        image: "triliumnext/notes:latest",
        default_port: 8383,
        container_port: "8080/tcp",
        env_vars: &[],
        volumes: &["/home/node/trilium-data"],
    },
    AppTemplateDef {
        id: "docuseal",
        name: "DocuSeal",
        description: "Open-source document signing platform (DocuSign alternative)",
        category: "Tools",
        image: "docuseal/docuseal:latest",
        default_port: 3015,
        container_port: "3000/tcp",
        env_vars: &[],
        volumes: &["/data"],
    },
    // ─── Media ──────────────────────────────────────────────────
    AppTemplateDef {
        id: "navidrome",
        name: "Navidrome",
        description: "Modern music server and streamer compatible with Subsonic/Airsonic clients",
        category: "Media",
        image: "deluan/navidrome:latest",
        default_port: 4533,
        container_port: "4533/tcp",
        env_vars: &[],
        volumes: &["/data", "/music"],
    },
    AppTemplateDef {
        id: "audiobookshelf",
        name: "Audiobookshelf",
        description: "Self-hosted audiobook and podcast server with mobile apps",
        category: "Media",
        image: "ghcr.io/advplyr/audiobookshelf:latest",
        default_port: 13378,
        container_port: "80/tcp",
        env_vars: &[],
        volumes: &["/config", "/metadata", "/audiobooks", "/podcasts"],
    },
    AppTemplateDef {
        id: "calibre-web",
        name: "Calibre-Web",
        description: "Web-based e-book library manager with OPDS feed and reading interface",
        category: "Media",
        image: "lscr.io/linuxserver/calibre-web:latest",
        default_port: 8283,
        container_port: "8083/tcp",
        env_vars: &[
            EnvVarDef { name: "PUID", label: "User ID", default: "1000", required: false, secret: false },
            EnvVarDef { name: "PGID", label: "Group ID", default: "1000", required: false, secret: false },
        ],
        volumes: &["/config", "/books"],
    },
    AppTemplateDef {
        id: "kavita",
        name: "Kavita",
        description: "Self-hosted digital library for manga, comics, and books",
        category: "Media",
        image: "jvmilazz0/kavita:latest",
        default_port: 5001,
        container_port: "5000/tcp",
        env_vars: &[],
        volumes: &["/kavita/config", "/manga"],
    },
    AppTemplateDef {
        id: "plex",
        name: "Plex",
        description: "Media server for organizing and streaming movies, TV, music, and photos",
        category: "Media",
        image: "lscr.io/linuxserver/plex:latest",
        default_port: 32400,
        container_port: "32400/tcp",
        env_vars: &[
            EnvVarDef { name: "PUID", label: "User ID", default: "1000", required: false, secret: false },
            EnvVarDef { name: "PGID", label: "Group ID", default: "1000", required: false, secret: false },
            EnvVarDef { name: "PLEX_CLAIM", label: "Plex Claim Token", default: "", required: false, secret: true },
        ],
        volumes: &["/config", "/tv", "/movies"],
    },
    // ─── RSS / Link Management ──────────────────────────────────
    AppTemplateDef {
        id: "freshrss",
        name: "FreshRSS",
        description: "Self-hosted RSS feed aggregator with full-text search and mobile apps",
        category: "Tools",
        image: "freshrss/freshrss:latest",
        default_port: 8284,
        container_port: "80/tcp",
        env_vars: &[],
        volumes: &["/var/www/FreshRSS/data", "/var/www/FreshRSS/extensions"],
    },
    AppTemplateDef {
        id: "linkwarden",
        name: "Linkwarden",
        description: "Collaborative bookmark manager with archiving, tagging, and collections",
        category: "Tools",
        image: "ghcr.io/linkwarden/linkwarden:latest",
        default_port: 3016,
        container_port: "3000/tcp",
        env_vars: &[
            EnvVarDef { name: "DATABASE_URL", label: "Database URL", default: "", required: true, secret: false },
            EnvVarDef { name: "NEXTAUTH_SECRET", label: "NextAuth Secret", default: "", required: true, secret: true },
            EnvVarDef { name: "NEXTAUTH_URL", label: "NextAuth URL", default: "", required: true, secret: false },
        ],
        volumes: &["/data/data"],
    },
    AppTemplateDef {
        id: "changedetection",
        name: "Changedetection.io",
        description: "Website change detection and monitoring with notifications",
        category: "Monitoring",
        image: "ghcr.io/dgtlmoon/changedetection.io:latest",
        default_port: 5555,
        container_port: "5000/tcp",
        env_vars: &[],
        volumes: &["/datastore"],
    },
    // ─── Recipes ────────────────────────────────────────────────
    AppTemplateDef {
        id: "tandoor",
        name: "Tandoor Recipes",
        description: "Recipe management and meal planning application",
        category: "Tools",
        image: "vabene1111/recipes:latest",
        default_port: 8285,
        container_port: "8080/tcp",
        env_vars: &[
            EnvVarDef { name: "SECRET_KEY", label: "Secret Key", default: "", required: true, secret: true },
            EnvVarDef { name: "DB_ENGINE", label: "DB Engine", default: "django.db.backends.sqlite3", required: false, secret: false },
        ],
        volumes: &["/opt/recipes/mediafiles", "/opt/recipes/staticfiles"],
    },
    AppTemplateDef {
        id: "mealie",
        name: "Mealie",
        description: "Self-hosted recipe manager with meal planning, shopping lists, and API",
        category: "Tools",
        image: "ghcr.io/mealie-recipes/mealie:latest",
        default_port: 9925,
        container_port: "9000/tcp",
        env_vars: &[
            EnvVarDef { name: "BASE_URL", label: "Base URL", default: "", required: false, secret: false },
        ],
        volumes: &["/app/data"],
    },
    // ─── Communication ──────────────────────────────────────────
    AppTemplateDef {
        id: "ntfy",
        name: "ntfy",
        description: "Simple push notification service using HTTP PUT/POST (UnifiedPush compatible)",
        category: "Tools",
        image: "binwiederhier/ntfy:latest",
        default_port: 8286,
        container_port: "80/tcp",
        env_vars: &[],
        volumes: &["/var/cache/ntfy", "/etc/ntfy"],
    },
    AppTemplateDef {
        id: "gotify",
        name: "Gotify",
        description: "Simple server for sending and receiving push notifications via REST API",
        category: "Tools",
        image: "gotify/server:latest",
        default_port: 8287,
        container_port: "80/tcp",
        env_vars: &[
            EnvVarDef { name: "GOTIFY_DEFAULTUSER_PASS", label: "Admin Password", default: "", required: true, secret: true },
        ],
        volumes: &["/app/data"],
    },
    // ─── DNS ────────────────────────────────────────────────────
    AppTemplateDef {
        id: "adguard-home",
        name: "AdGuard Home",
        description: "Network-wide ad and tracker blocking DNS server with HTTPS filtering",
        category: "Networking",
        image: "adguard/adguardhome:latest",
        default_port: 3017,
        container_port: "3000/tcp",
        env_vars: &[],
        volumes: &["/opt/adguardhome/work", "/opt/adguardhome/conf"],
    },
    AppTemplateDef {
        id: "technitium-dns",
        name: "Technitium DNS",
        description: "Open-source authoritative and recursive DNS server with web admin panel",
        category: "Networking",
        image: "technitium/dns-server:latest",
        default_port: 5380,
        container_port: "5380/tcp",
        env_vars: &[],
        volumes: &["/etc/dns"],
    },
    // ─── Networking / Proxy ─────────────────────────────────────
    AppTemplateDef {
        id: "nginx-proxy-manager",
        name: "Nginx Proxy Manager",
        description: "Easy-to-use reverse proxy with free SSL and web-based management",
        category: "Networking",
        image: "jc21/nginx-proxy-manager:latest",
        default_port: 8181,
        container_port: "81/tcp",
        env_vars: &[],
        volumes: &["/data", "/etc/letsencrypt"],
    },
    AppTemplateDef {
        id: "wireguard",
        name: "WireGuard",
        description: "Modern, fast VPN tunnel with easy configuration",
        category: "Networking",
        image: "lscr.io/linuxserver/wireguard:latest",
        default_port: 51820,
        container_port: "51820/udp",
        env_vars: &[
            EnvVarDef { name: "PUID", label: "User ID", default: "1000", required: false, secret: false },
            EnvVarDef { name: "PGID", label: "Group ID", default: "1000", required: false, secret: false },
            EnvVarDef { name: "PEERS", label: "Number of Peers", default: "3", required: false, secret: false },
            EnvVarDef { name: "SERVERURL", label: "Server URL/IP", default: "", required: true, secret: false },
        ],
        volumes: &["/config"],
    },
    // ─── Monitoring (additional) ────────────────────────────────
    AppTemplateDef {
        id: "dozzle",
        name: "Dozzle",
        description: "Real-time Docker container log viewer with a clean web interface",
        category: "Monitoring",
        image: "amir20/dozzle:latest",
        default_port: 9999,
        container_port: "8080/tcp",
        env_vars: &[],
        volumes: &[],
    },
    AppTemplateDef {
        id: "glances",
        name: "Glances",
        description: "Cross-platform system monitoring tool with web interface and API",
        category: "Monitoring",
        image: "nicolargo/glances:latest-full",
        default_port: 61208,
        container_port: "61208/tcp",
        env_vars: &[
            EnvVarDef { name: "GLANCES_OPT", label: "Glances Options", default: "-w", required: false, secret: false },
        ],
        volumes: &[],
    },
    AppTemplateDef {
        id: "netdata",
        name: "Netdata",
        description: "Real-time infrastructure monitoring with zero configuration",
        category: "Monitoring",
        image: "netdata/netdata:stable",
        default_port: 19999,
        container_port: "19999/tcp",
        env_vars: &[],
        volumes: &["/etc/netdata", "/var/lib/netdata", "/var/cache/netdata"],
    },
    // ─── Storage / Files ────────────────────────────────────────
    AppTemplateDef {
        id: "filebrowser",
        name: "File Browser",
        description: "Web-based file manager with sharing, users, and customization",
        category: "Storage",
        image: "filebrowser/filebrowser:latest",
        default_port: 8288,
        container_port: "80/tcp",
        env_vars: &[],
        volumes: &["/srv", "/database/filebrowser.db"],
    },
    AppTemplateDef {
        id: "syncthing",
        name: "Syncthing",
        description: "Continuous file synchronization between devices (Dropbox alternative)",
        category: "Storage",
        image: "lscr.io/linuxserver/syncthing:latest",
        default_port: 8384,
        container_port: "8384/tcp",
        env_vars: &[
            EnvVarDef { name: "PUID", label: "User ID", default: "1000", required: false, secret: false },
            EnvVarDef { name: "PGID", label: "Group ID", default: "1000", required: false, secret: false },
        ],
        volumes: &["/config", "/data"],
    },
    // ─── Development (additional) ───────────────────────────────
    AppTemplateDef {
        id: "it-tools",
        name: "IT-Tools",
        description: "Collection of handy developer tools (JSON formatter, UUID generator, hash, base64, etc.)",
        category: "Development",
        image: "corentinth/it-tools:latest",
        default_port: 8289,
        container_port: "80/tcp",
        env_vars: &[],
        volumes: &[],
    },
    AppTemplateDef {
        id: "jenkins",
        name: "Jenkins",
        description: "Leading open-source CI/CD automation server",
        category: "Development",
        image: "jenkins/jenkins:lts",
        default_port: 8290,
        container_port: "8080/tcp",
        env_vars: &[],
        volumes: &["/var/jenkins_home"],
    },
    // Woodpecker CI already exists above — skipped duplicate
    // ─── Databases (additional) ─────────────────────────────────
    AppTemplateDef {
        id: "clickhouse",
        name: "ClickHouse",
        description: "Column-oriented OLAP database for real-time analytics on large datasets",
        category: "Database",
        image: "clickhouse/clickhouse-server:latest",
        default_port: 8123,
        container_port: "8123/tcp",
        env_vars: &[],
        volumes: &["/var/lib/clickhouse", "/var/log/clickhouse-server"],
    },
    AppTemplateDef {
        id: "influxdb",
        name: "InfluxDB",
        description: "Time series database for metrics, events, and real-time analytics",
        category: "Database",
        image: "influxdb:2",
        default_port: 8292,
        container_port: "8086/tcp",
        env_vars: &[
            EnvVarDef { name: "DOCKER_INFLUXDB_INIT_MODE", label: "Init Mode", default: "setup", required: false, secret: false },
            EnvVarDef { name: "DOCKER_INFLUXDB_INIT_USERNAME", label: "Admin Username", default: "admin", required: true, secret: false },
            EnvVarDef { name: "DOCKER_INFLUXDB_INIT_PASSWORD", label: "Admin Password", default: "", required: true, secret: true },
            EnvVarDef { name: "DOCKER_INFLUXDB_INIT_ORG", label: "Organization", default: "myorg", required: true, secret: false },
            EnvVarDef { name: "DOCKER_INFLUXDB_INIT_BUCKET", label: "Bucket Name", default: "default", required: true, secret: false },
        ],
        volumes: &["/var/lib/influxdb2"],
    },
    AppTemplateDef {
        id: "valkey",
        name: "Valkey",
        description: "High-performance key/value datastore (community fork of Redis)",
        category: "Database",
        image: "valkey/valkey:8-alpine",
        default_port: 6380,
        container_port: "6379/tcp",
        env_vars: &[],
        volumes: &["/data"],
    },
    AppTemplateDef {
        id: "couchdb",
        name: "CouchDB",
        description: "Document-oriented NoSQL database with multi-master replication",
        category: "Database",
        image: "couchdb:3",
        default_port: 5984,
        container_port: "5984/tcp",
        env_vars: &[
            EnvVarDef { name: "COUCHDB_USER", label: "Admin Username", default: "admin", required: true, secret: false },
            EnvVarDef { name: "COUCHDB_PASSWORD", label: "Admin Password", default: "", required: true, secret: true },
        ],
        volumes: &["/opt/couchdb/data"],
    },
    // ─── Security ───────────────────────────────────────────────
    AppTemplateDef {
        id: "crowdsec",
        name: "CrowdSec",
        description: "Collaborative security engine analyzing logs and sharing threat intelligence",
        category: "Security",
        image: "crowdsecurity/crowdsec:latest",
        default_port: 6060,
        container_port: "6060/tcp",
        env_vars: &[],
        volumes: &["/etc/crowdsec", "/var/lib/crowdsec/data"],
    },
    // ─── Automation (additional) ────────────────────────────────
    AppTemplateDef {
        id: "node-red",
        name: "Node-RED",
        description: "Flow-based programming tool for wiring IoT, APIs, and online services",
        category: "Automation",
        image: "nodered/node-red:latest",
        default_port: 1880,
        container_port: "1880/tcp",
        env_vars: &[],
        volumes: &["/data"],
    },
    AppTemplateDef {
        id: "activepieces",
        name: "Activepieces",
        description: "No-code workflow automation platform (Zapier alternative)",
        category: "Automation",
        image: "activepieces/activepieces:latest",
        default_port: 8293,
        container_port: "80/tcp",
        env_vars: &[
            EnvVarDef { name: "AP_ENGINE_EXECUTABLE_PATH", label: "Engine Path", default: "dist/packages/engine/main.js", required: false, secret: false },
            EnvVarDef { name: "AP_ENCRYPTION_KEY", label: "Encryption Key", default: "", required: true, secret: true },
            EnvVarDef { name: "AP_JWT_SECRET", label: "JWT Secret", default: "", required: true, secret: true },
        ],
        volumes: &[],
    },
    // ─── CMS (additional) ───────────────────────────────────────
    AppTemplateDef {
        id: "wordpress-docker",
        name: "WordPress (Docker)",
        description: "World's most popular CMS as a Docker container with Apache and PHP",
        category: "CMS",
        image: "wordpress:6-apache",
        default_port: 8294,
        container_port: "80/tcp",
        env_vars: &[
            EnvVarDef { name: "WORDPRESS_DB_HOST", label: "Database Host", default: "", required: true, secret: false },
            EnvVarDef { name: "WORDPRESS_DB_USER", label: "Database User", default: "wordpress", required: true, secret: false },
            EnvVarDef { name: "WORDPRESS_DB_PASSWORD", label: "Database Password", default: "", required: true, secret: true },
            EnvVarDef { name: "WORDPRESS_DB_NAME", label: "Database Name", default: "wordpress", required: true, secret: false },
        ],
        volumes: &["/var/www/html"],
    },
    AppTemplateDef {
        id: "listmonk",
        name: "Listmonk",
        description: "High-performance self-hosted newsletter and mailing list manager",
        category: "CMS",
        image: "listmonk/listmonk:latest",
        default_port: 9100,
        container_port: "9000/tcp",
        env_vars: &[
            EnvVarDef { name: "LISTMONK_app__address", label: "Listen Address", default: "0.0.0.0:9000", required: false, secret: false },
            EnvVarDef { name: "LISTMONK_db__host", label: "DB Host", default: "", required: true, secret: false },
            EnvVarDef { name: "LISTMONK_db__port", label: "DB Port", default: "5432", required: false, secret: false },
            EnvVarDef { name: "LISTMONK_db__user", label: "DB User", default: "listmonk", required: false, secret: false },
            EnvVarDef { name: "LISTMONK_db__password", label: "DB Password", default: "", required: true, secret: true },
            EnvVarDef { name: "LISTMONK_db__database", label: "DB Name", default: "listmonk", required: false, secret: false },
        ],
        volumes: &["/listmonk/uploads"],
    },
    // ─── Analytics (additional) ─────────────────────────────────
    AppTemplateDef {
        id: "shynet",
        name: "Shynet",
        description: "Privacy-friendly web analytics without cookies or JavaScript",
        category: "Analytics",
        image: "milesmcc/shynet:latest",
        default_port: 8295,
        container_port: "8080/tcp",
        env_vars: &[
            EnvVarDef { name: "DJANGO_SECRET_KEY", label: "Secret Key", default: "", required: true, secret: true },
            EnvVarDef { name: "DB_NAME", label: "Database Name", default: "shynet", required: false, secret: false },
        ],
        volumes: &[],
    },
    // ─── Image / Upload ─────────────────────────────────────────
    AppTemplateDef {
        id: "zipline",
        name: "Zipline",
        description: "ShareX/file upload server with URL shortening and image galleries",
        category: "Storage",
        image: "ghcr.io/diced/zipline:latest",
        default_port: 3018,
        container_port: "3000/tcp",
        env_vars: &[
            EnvVarDef { name: "CORE_SECRET", label: "Core Secret", default: "", required: true, secret: true },
            EnvVarDef { name: "CORE_DATABASE_URL", label: "Database URL", default: "", required: true, secret: false },
        ],
        volumes: &["/zipline/uploads", "/zipline/public"],
    },
    // ─── Authentication ─────────────────────────────────────────
    AppTemplateDef {
        id: "authelia",
        name: "Authelia",
        description: "Single Sign-On and 2FA portal for securing web applications",
        category: "Security",
        image: "authelia/authelia:latest",
        default_port: 9091,
        container_port: "9091/tcp",
        env_vars: &[],
        volumes: &["/config"],
    },
    // ─── AI / Machine Learning (additional) ─────────────────────
    AppTemplateDef {
        id: "stable-diffusion-webui",
        name: "Stable Diffusion WebUI",
        description: "Browser-based interface for Stable Diffusion image generation",
        category: "AI",
        image: "ghcr.io/abetlen/stable-diffusion-webui:latest",
        default_port: 7860,
        container_port: "7860/tcp",
        env_vars: &[
            EnvVarDef { name: "CLI_ARGS", label: "CLI Arguments", default: "--listen", required: false, secret: false },
        ],
        volumes: &["/app/models", "/app/outputs"],
    },
    AppTemplateDef {
        id: "text-generation-webui",
        name: "Text Generation WebUI",
        description: "Gradio web UI for running large language models (oobabooga)",
        category: "AI",
        // Pinned to stable default tag instead of nightly so deploys don't drift
        image: "atinoda/text-generation-webui:default",
        default_port: 7861,
        container_port: "7860/tcp",
        env_vars: &[
            EnvVarDef { name: "EXTRA_LAUNCH_ARGS", label: "Extra Launch Args", default: "--listen", required: false, secret: false },
        ],
        volumes: &["/app/models", "/app/characters", "/app/loras"],
    },
    AppTemplateDef {
        id: "whisper",
        name: "Whisper ASR",
        description: "OpenAI Whisper speech-to-text service with REST API",
        category: "AI",
        image: "onerahmet/openai-whisper-asr-webservice:latest",
        default_port: 9300,
        container_port: "9000/tcp",
        env_vars: &[
            EnvVarDef { name: "ASR_MODEL", label: "Model Size", default: "base", required: false, secret: false },
        ],
        volumes: &[],
    },
    AppTemplateDef {
        id: "litellm",
        name: "LiteLLM Proxy",
        description: "Unified proxy for 100+ LLM APIs (OpenAI, Anthropic, Cohere, etc.)",
        category: "AI",
        image: "ghcr.io/berriai/litellm:main-latest",
        default_port: 4100,
        container_port: "4000/tcp",
        env_vars: &[
            EnvVarDef { name: "LITELLM_MASTER_KEY", label: "Master API Key", default: "", required: true, secret: true },
        ],
        volumes: &[],
    },
    AppTemplateDef {
        id: "flowise",
        name: "Flowise",
        description: "Drag-and-drop LLM flow builder for LangChain applications",
        category: "AI",
        image: "flowiseai/flowise:latest",
        default_port: 3020,
        container_port: "3000/tcp",
        env_vars: &[
            EnvVarDef { name: "FLOWISE_USERNAME", label: "Username", default: "admin", required: false, secret: false },
            EnvVarDef { name: "FLOWISE_PASSWORD", label: "Password", default: "", required: false, secret: true },
        ],
        volumes: &["/root/.flowise"],
    },
    AppTemplateDef {
        id: "langflow",
        name: "Langflow",
        description: "Visual framework for building multi-agent and RAG applications",
        category: "AI",
        image: "langflowai/langflow:latest",
        default_port: 7862,
        container_port: "7860/tcp",
        env_vars: &[
            EnvVarDef { name: "LANGFLOW_AUTO_LOGIN", label: "Auto Login", default: "false", required: false, secret: false },
        ],
        volumes: &["/app/langflow"],
    },
    AppTemplateDef {
        id: "dify",
        name: "Dify",
        description: "Open-source LLM app development platform with RAG, agents, and workflows",
        category: "AI",
        image: "langgenius/dify-api:latest",
        default_port: 5002,
        container_port: "5001/tcp",
        env_vars: &[
            EnvVarDef { name: "SECRET_KEY", label: "Secret Key", default: "", required: true, secret: true },
            EnvVarDef { name: "DB_USERNAME", label: "Database Username", default: "dify", required: true, secret: false },
            EnvVarDef { name: "DB_PASSWORD", label: "Database Password", default: "", required: true, secret: true },
            EnvVarDef { name: "DB_HOST", label: "Database Host", default: "localhost", required: true, secret: false },
            EnvVarDef { name: "REDIS_HOST", label: "Redis Host", default: "localhost", required: false, secret: false },
        ],
        volumes: &["/app/api/storage"],
    },
    AppTemplateDef {
        id: "vllm",
        name: "vLLM",
        description: "High-throughput, memory-efficient LLM inference server with OpenAI-compatible API",
        category: "AI",
        image: "vllm/vllm-openai:latest",
        default_port: 8000,
        container_port: "8000/tcp",
        env_vars: &[
            EnvVarDef { name: "MODEL", label: "Model (HuggingFace ID)", default: "meta-llama/Llama-3.2-1B-Instruct", required: true, secret: false },
            EnvVarDef { name: "HUGGING_FACE_HUB_TOKEN", label: "HuggingFace Token (gated models)", default: "", required: false, secret: true },
        ],
        volumes: &["/root/.cache/huggingface"],
    },
    // ─── Databases (new) ────────────────────────────────────────
    AppTemplateDef {
        id: "surrealdb",
        name: "SurrealDB",
        description: "Multi-model database for web, mobile, serverless, and backend (SQL, graph, document)",
        category: "Database",
        image: "surrealdb/surrealdb:latest",
        default_port: 8300,
        container_port: "8000/tcp",
        env_vars: &[
            EnvVarDef { name: "SURREAL_USER", label: "Root Username", default: "root", required: true, secret: false },
            EnvVarDef { name: "SURREAL_PASS", label: "Root Password", default: "", required: true, secret: true },
        ],
        volumes: &["/data"],
    },
    AppTemplateDef {
        id: "questdb",
        name: "QuestDB",
        description: "High-performance time series database with SQL support and built-in web console",
        category: "Database",
        image: "questdb/questdb:latest",
        default_port: 9009,
        container_port: "9000/tcp",
        env_vars: &[],
        volumes: &["/var/lib/questdb"],
    },
    AppTemplateDef {
        id: "timescaledb",
        name: "TimescaleDB",
        description: "PostgreSQL extension for time-series data with automatic partitioning",
        category: "Database",
        image: "timescale/timescaledb:latest-pg16",
        default_port: 5433,
        container_port: "5432/tcp",
        env_vars: &[
            EnvVarDef { name: "POSTGRES_USER", label: "Username", default: "postgres", required: true, secret: false },
            EnvVarDef { name: "POSTGRES_PASSWORD", label: "Password", default: "", required: true, secret: true },
        ],
        volumes: &["/var/lib/postgresql/data"],
    },
    AppTemplateDef {
        id: "keydb",
        name: "KeyDB",
        description: "Multi-threaded fork of Redis with active replication and flash storage support",
        category: "Database",
        image: "eqalpha/keydb:latest",
        default_port: 6381,
        container_port: "6379/tcp",
        env_vars: &[],
        volumes: &["/data"],
    },
    AppTemplateDef {
        id: "dragonflydb",
        name: "DragonflyDB",
        description: "Modern in-memory datastore compatible with Redis and Memcached APIs",
        category: "Database",
        image: "docker.dragonflydb.io/dragonflydb/dragonfly:latest",
        default_port: 6382,
        container_port: "6379/tcp",
        env_vars: &[],
        volumes: &["/data"],
    },
    AppTemplateDef {
        id: "cassandra",
        name: "Apache Cassandra",
        description: "Distributed wide-column NoSQL database for high availability at scale",
        category: "Database",
        image: "cassandra:5",
        default_port: 9042,
        container_port: "9042/tcp",
        env_vars: &[
            EnvVarDef { name: "CASSANDRA_CLUSTER_NAME", label: "Cluster Name", default: "DockPanelCluster", required: false, secret: false },
        ],
        volumes: &["/var/lib/cassandra"],
    },
    AppTemplateDef {
        id: "neo4j",
        name: "Neo4j",
        description: "Leading graph database for connected data with Cypher query language",
        category: "Database",
        image: "neo4j:5",
        default_port: 7474,
        container_port: "7474/tcp",
        env_vars: &[
            EnvVarDef { name: "NEO4J_AUTH", label: "Auth (user/pass)", default: "neo4j/changeme", required: true, secret: true },
        ],
        volumes: &["/data", "/logs"],
    },
    AppTemplateDef {
        id: "arangodb",
        name: "ArangoDB",
        description: "Multi-model database combining document, graph, and key-value in one engine",
        category: "Database",
        image: "arangodb:3.12",
        default_port: 8529,
        container_port: "8529/tcp",
        env_vars: &[
            EnvVarDef { name: "ARANGO_ROOT_PASSWORD", label: "Root Password", default: "", required: true, secret: true },
        ],
        volumes: &["/var/lib/arangodb3", "/var/lib/arangodb3-apps"],
    },
    // ─── Development (new) ──────────────────────────────────────
    AppTemplateDef {
        id: "gitness",
        name: "Gitness",
        description: "Open-source developer platform with Git hosting, pipelines, and code review (by Harness)",
        category: "Development",
        image: "harness/gitness:latest",
        default_port: 3030,
        container_port: "3000/tcp",
        env_vars: &[],
        volumes: &["/data"],
    },
    AppTemplateDef {
        id: "verdaccio",
        name: "Verdaccio",
        description: "Lightweight private npm proxy registry for hosting and caching packages",
        category: "Development",
        image: "verdaccio/verdaccio:6",
        default_port: 4873,
        container_port: "4873/tcp",
        env_vars: &[],
        volumes: &["/verdaccio/storage", "/verdaccio/conf"],
    },
    AppTemplateDef {
        id: "nexus",
        name: "Nexus Repository",
        description: "Universal artifact repository manager for Maven, npm, Docker, and more",
        category: "Development",
        image: "sonatype/nexus3:latest",
        default_port: 8320,
        container_port: "8081/tcp",
        env_vars: &[],
        volumes: &["/nexus-data"],
    },
    AppTemplateDef {
        id: "buildkite-agent",
        name: "Buildkite Agent",
        description: "CI/CD agent that runs build jobs from the Buildkite platform",
        category: "Development",
        image: "buildkite/agent:3",
        default_port: 8301,
        container_port: "8080/tcp",
        env_vars: &[
            EnvVarDef { name: "BUILDKITE_AGENT_TOKEN", label: "Agent Token", default: "", required: true, secret: true },
        ],
        volumes: &["/buildkite/builds"],
    },
    // ─── Media (new) ────────────────────────────────────────────
    AppTemplateDef {
        id: "jellyseerr",
        name: "Jellyseerr",
        description: "Media request management for Jellyfin, Emby, and Plex with auto-approval",
        category: "Media",
        image: "fallenbagel/jellyseerr:latest",
        default_port: 5055,
        container_port: "5055/tcp",
        env_vars: &[],
        volumes: &["/app/config"],
    },
    AppTemplateDef {
        id: "overseerr",
        name: "Overseerr",
        description: "Media request management and discovery tool for Plex ecosystems",
        category: "Media",
        image: "lscr.io/linuxserver/overseerr:latest",
        default_port: 5056,
        container_port: "5055/tcp",
        env_vars: &[
            EnvVarDef { name: "PUID", label: "User ID", default: "1000", required: false, secret: false },
            EnvVarDef { name: "PGID", label: "Group ID", default: "1000", required: false, secret: false },
        ],
        volumes: &["/config"],
    },
    AppTemplateDef {
        id: "bazarr",
        name: "Bazarr",
        description: "Companion app for Sonarr and Radarr to manage and download subtitles",
        category: "Media",
        image: "lscr.io/linuxserver/bazarr:latest",
        default_port: 6767,
        container_port: "6767/tcp",
        env_vars: &[
            EnvVarDef { name: "PUID", label: "User ID", default: "1000", required: false, secret: false },
            EnvVarDef { name: "PGID", label: "Group ID", default: "1000", required: false, secret: false },
        ],
        volumes: &["/config", "/movies", "/tv"],
    },
    AppTemplateDef {
        id: "prowlarr",
        name: "Prowlarr",
        description: "Indexer manager and proxy for Sonarr, Radarr, Lidarr, and Readarr",
        category: "Media",
        image: "lscr.io/linuxserver/prowlarr:latest",
        default_port: 9696,
        container_port: "9696/tcp",
        env_vars: &[
            EnvVarDef { name: "PUID", label: "User ID", default: "1000", required: false, secret: false },
            EnvVarDef { name: "PGID", label: "Group ID", default: "1000", required: false, secret: false },
        ],
        volumes: &["/config"],
    },
    AppTemplateDef {
        id: "radarr",
        name: "Radarr",
        description: "Movie collection manager with automatic downloading and organization",
        category: "Media",
        image: "lscr.io/linuxserver/radarr:latest",
        default_port: 7878,
        container_port: "7878/tcp",
        env_vars: &[
            EnvVarDef { name: "PUID", label: "User ID", default: "1000", required: false, secret: false },
            EnvVarDef { name: "PGID", label: "Group ID", default: "1000", required: false, secret: false },
        ],
        volumes: &["/config", "/movies", "/downloads"],
    },
    AppTemplateDef {
        id: "sonarr",
        name: "Sonarr",
        description: "TV series collection manager with automatic downloading and organization",
        category: "Media",
        image: "lscr.io/linuxserver/sonarr:latest",
        default_port: 8989,
        container_port: "8989/tcp",
        env_vars: &[
            EnvVarDef { name: "PUID", label: "User ID", default: "1000", required: false, secret: false },
            EnvVarDef { name: "PGID", label: "Group ID", default: "1000", required: false, secret: false },
        ],
        volumes: &["/config", "/tv", "/downloads"],
    },
    AppTemplateDef {
        id: "lidarr",
        name: "Lidarr",
        description: "Music collection manager with automatic downloading and metadata management",
        category: "Media",
        image: "lscr.io/linuxserver/lidarr:latest",
        default_port: 8686,
        container_port: "8686/tcp",
        env_vars: &[
            EnvVarDef { name: "PUID", label: "User ID", default: "1000", required: false, secret: false },
            EnvVarDef { name: "PGID", label: "Group ID", default: "1000", required: false, secret: false },
        ],
        volumes: &["/config", "/music", "/downloads"],
    },
    AppTemplateDef {
        id: "readarr",
        name: "Readarr",
        description: "Book and audiobook collection manager with automatic downloading",
        category: "Media",
        image: "lscr.io/linuxserver/readarr:develop",
        default_port: 8787,
        container_port: "8787/tcp",
        env_vars: &[
            EnvVarDef { name: "PUID", label: "User ID", default: "1000", required: false, secret: false },
            EnvVarDef { name: "PGID", label: "Group ID", default: "1000", required: false, secret: false },
        ],
        volumes: &["/config", "/books", "/downloads"],
    },
    AppTemplateDef {
        id: "tautulli",
        name: "Tautulli",
        description: "Monitoring and tracking tool for Plex Media Server usage and statistics",
        category: "Media",
        image: "lscr.io/linuxserver/tautulli:latest",
        default_port: 8183,
        container_port: "8181/tcp",
        env_vars: &[
            EnvVarDef { name: "PUID", label: "User ID", default: "1000", required: false, secret: false },
            EnvVarDef { name: "PGID", label: "Group ID", default: "1000", required: false, secret: false },
        ],
        volumes: &["/config"],
    },
    // ─── Monitoring (new) ───────────────────────────────────────
    AppTemplateDef {
        id: "zabbix",
        name: "Zabbix",
        description: "Enterprise-class open-source monitoring for networks, servers, and applications",
        category: "Monitoring",
        image: "zabbix/zabbix-appliance:7.0-latest",
        default_port: 8302,
        container_port: "80/tcp",
        env_vars: &[
            EnvVarDef { name: "ZBX_SERVER_HOST", label: "Server Host", default: "localhost", required: false, secret: false },
        ],
        volumes: &["/var/lib/zabbix"],
    },
    AppTemplateDef {
        id: "checkmk",
        name: "Checkmk",
        description: "Infrastructure and application monitoring with auto-discovery and alerting",
        category: "Monitoring",
        image: "checkmk/check-mk-raw:2.3.0-latest",
        default_port: 8303,
        container_port: "5000/tcp",
        env_vars: &[
            EnvVarDef { name: "CMK_PASSWORD", label: "Admin Password", default: "", required: true, secret: true },
            EnvVarDef { name: "CMK_SITE_ID", label: "Site ID", default: "cmk", required: false, secret: false },
        ],
        volumes: &["/omd/sites"],
    },
    AppTemplateDef {
        id: "graylog",
        name: "Graylog",
        description: "Centralized log management and analysis platform with alerting",
        category: "Monitoring",
        image: "graylog/graylog:6.1",
        default_port: 9200,
        container_port: "9000/tcp",
        env_vars: &[
            EnvVarDef { name: "GRAYLOG_PASSWORD_SECRET", label: "Password Secret (16+ chars)", default: "", required: true, secret: true },
            EnvVarDef { name: "GRAYLOG_ROOT_PASSWORD_SHA2", label: "Root Password SHA256 Hash", default: "", required: true, secret: true },
            EnvVarDef { name: "GRAYLOG_HTTP_EXTERNAL_URI", label: "External URI", default: "http://localhost:9000/", required: true, secret: false },
            EnvVarDef { name: "GRAYLOG_MONGODB_URI", label: "MongoDB URI", default: "mongodb://mongo:27017/graylog", required: true, secret: false },
            EnvVarDef { name: "GRAYLOG_ELASTICSEARCH_HOSTS", label: "Elasticsearch Hosts", default: "http://elasticsearch:9200", required: true, secret: false },
        ],
        volumes: &["/usr/share/graylog/data"],
    },
    AppTemplateDef {
        id: "vector",
        name: "Vector",
        description: "High-performance observability data pipeline for logs, metrics, and traces",
        category: "Monitoring",
        image: "timberio/vector:latest-alpine",
        default_port: 8304,
        container_port: "8686/tcp",
        env_vars: &[],
        volumes: &["/etc/vector", "/var/lib/vector"],
    },
    // ─── Productivity (new) ─────────────────────────────────────
    AppTemplateDef {
        id: "leantime",
        name: "Leantime",
        description: "Open-source project management system for non-project managers (lean methodology)",
        category: "Productivity",
        image: "leantime/leantime:latest",
        default_port: 8305,
        container_port: "80/tcp",
        env_vars: &[
            EnvVarDef { name: "LEAN_DB_HOST", label: "Database Host", default: "", required: true, secret: false },
            EnvVarDef { name: "LEAN_DB_USER", label: "Database User", default: "leantime", required: true, secret: false },
            EnvVarDef { name: "LEAN_DB_PASSWORD", label: "Database Password", default: "", required: true, secret: true },
            EnvVarDef { name: "LEAN_DB_DATABASE", label: "Database Name", default: "leantime", required: false, secret: false },
        ],
        volumes: &["/var/www/html/public/userfiles", "/var/www/html/userfiles"],
    },
    AppTemplateDef {
        id: "focalboard",
        name: "Focalboard",
        description: "Open-source project management tool (Trello/Notion/Asana alternative by Mattermost)",
        category: "Productivity",
        image: "mattermost/focalboard:latest",
        default_port: 8306,
        container_port: "8000/tcp",
        env_vars: &[],
        volumes: &["/opt/focalboard/data"],
    },
    AppTemplateDef {
        id: "joplin-server",
        name: "Joplin Server",
        description: "Sync server for Joplin note-taking app with sharing and collaboration",
        category: "Productivity",
        image: "joplin/server:latest",
        default_port: 22300,
        container_port: "22300/tcp",
        env_vars: &[
            EnvVarDef { name: "APP_BASE_URL", label: "Base URL", default: "", required: true, secret: false },
            EnvVarDef { name: "DB_CLIENT", label: "DB Client", default: "pg", required: false, secret: false },
            EnvVarDef { name: "POSTGRES_HOST", label: "PostgreSQL Host", default: "", required: true, secret: false },
            EnvVarDef { name: "POSTGRES_PORT", label: "PostgreSQL Port", default: "5432", required: false, secret: false },
            EnvVarDef { name: "POSTGRES_DATABASE", label: "Database Name", default: "joplin", required: false, secret: false },
            EnvVarDef { name: "POSTGRES_USER", label: "Database User", default: "joplin", required: true, secret: false },
            EnvVarDef { name: "POSTGRES_PASSWORD", label: "Database Password", default: "", required: true, secret: true },
        ],
        volumes: &[],
    },
    // ─── Communication (new) ────────────────────────────────────
    AppTemplateDef {
        id: "element-web",
        name: "Element Web",
        description: "Feature-rich Matrix client for secure, decentralized communication",
        category: "Communication",
        image: "vectorim/element-web:latest",
        default_port: 8307,
        container_port: "80/tcp",
        env_vars: &[],
        volumes: &["/app/config.json"],
    },
    AppTemplateDef {
        id: "synapse",
        name: "Matrix Synapse",
        description: "Reference Matrix homeserver for decentralized, encrypted messaging",
        category: "Communication",
        image: "matrixdotorg/synapse:latest",
        default_port: 8308,
        container_port: "8008/tcp",
        env_vars: &[
            EnvVarDef { name: "SYNAPSE_SERVER_NAME", label: "Server Name", default: "", required: true, secret: false },
            EnvVarDef { name: "SYNAPSE_REPORT_STATS", label: "Report Stats", default: "no", required: false, secret: false },
        ],
        volumes: &["/data"],
    },
    // ─── Networking (new) ───────────────────────────────────────
    AppTemplateDef {
        id: "traefik",
        name: "Traefik",
        description: "Cloud-native reverse proxy and load balancer with automatic HTTPS",
        category: "Networking",
        image: "traefik:v3",
        default_port: 8309,
        container_port: "8080/tcp",
        env_vars: &[],
        volumes: &["/etc/traefik", "/letsencrypt"],
    },
    AppTemplateDef {
        id: "caddy",
        name: "Caddy",
        description: "Powerful web server with automatic HTTPS and easy configuration",
        category: "Networking",
        image: "caddy:2-alpine",
        default_port: 8310,
        container_port: "80/tcp",
        env_vars: &[],
        volumes: &["/etc/caddy", "/data", "/config"],
    },
    AppTemplateDef {
        id: "cloudflared",
        name: "Cloudflare Tunnel",
        description: "Secure tunnel to expose local services to the internet via Cloudflare",
        category: "Networking",
        image: "cloudflare/cloudflared:latest",
        default_port: 8311,
        container_port: "8080/tcp",
        env_vars: &[
            EnvVarDef { name: "TUNNEL_TOKEN", label: "Tunnel Token", default: "", required: true, secret: true },
        ],
        volumes: &[],
    },
    AppTemplateDef {
        id: "tailscale",
        name: "Tailscale",
        description: "Zero-config mesh VPN built on WireGuard for secure private networking",
        category: "Networking",
        image: "tailscale/tailscale:latest",
        default_port: 8312,
        container_port: "41641/udp",
        env_vars: &[
            EnvVarDef { name: "TS_AUTHKEY", label: "Auth Key", default: "", required: true, secret: true },
            EnvVarDef { name: "TS_HOSTNAME", label: "Hostname", default: "dockpanel", required: false, secret: false },
        ],
        volumes: &["/var/lib/tailscale"],
    },
    // ─── Storage (new) ──────────────────────────────────────────
    AppTemplateDef {
        id: "garage",
        name: "Garage",
        description: "Lightweight S3-compatible distributed object storage for self-hosting",
        category: "Storage",
        image: "dxflrs/garage:v1.0",
        default_port: 3900,
        container_port: "3900/tcp",
        env_vars: &[],
        volumes: &["/var/lib/garage/data", "/var/lib/garage/meta", "/etc/garage"],
    },
    AppTemplateDef {
        id: "seaweedfs",
        name: "SeaweedFS",
        description: "Fast distributed storage system for billions of files with S3 API support",
        category: "Storage",
        image: "chrislusf/seaweedfs:latest",
        default_port: 9333,
        container_port: "9333/tcp",
        env_vars: &[],
        volumes: &["/data"],
    },
    // ─── Security (new) ─────────────────────────────────────────
    AppTemplateDef {
        id: "wazuh",
        name: "Wazuh Manager",
        description: "Open-source security platform for threat detection, compliance, and incident response",
        category: "Security",
        image: "wazuh/wazuh-manager:4.9.2",
        default_port: 1514,
        container_port: "1514/tcp",
        env_vars: &[],
        volumes: &["/var/ossec/api/configuration", "/var/ossec/etc", "/var/ossec/logs", "/var/ossec/queue"],
    },
    AppTemplateDef {
        id: "trivy",
        name: "Trivy Server",
        description: "Comprehensive vulnerability scanner for containers, filesystems, and IaC",
        category: "Security",
        image: "aquasec/trivy:latest",
        default_port: 8313,
        container_port: "8080/tcp",
        env_vars: &[],
        volumes: &["/root/.cache"],
    },
    // ─── Automation (new) ───────────────────────────────────────
    AppTemplateDef {
        id: "huginn",
        name: "Huginn",
        description: "System for building agents that perform automated tasks online (IFTTT alternative)",
        category: "Automation",
        image: "ghcr.io/huginn/huginn:latest",
        default_port: 3021,
        container_port: "3000/tcp",
        env_vars: &[
            EnvVarDef { name: "HUGINN_DATABASE_ADAPTER", label: "DB Adapter", default: "mysql2", required: false, secret: false },
            EnvVarDef { name: "HUGINN_DATABASE_HOST", label: "DB Host", default: "", required: true, secret: false },
            EnvVarDef { name: "HUGINN_DATABASE_NAME", label: "DB Name", default: "huginn", required: false, secret: false },
            EnvVarDef { name: "HUGINN_DATABASE_USERNAME", label: "DB Username", default: "huginn", required: true, secret: false },
            EnvVarDef { name: "HUGINN_DATABASE_PASSWORD", label: "DB Password", default: "", required: true, secret: true },
        ],
        volumes: &[],
    },
    AppTemplateDef {
        id: "windmill",
        name: "Windmill",
        description: "Developer platform for building internal tools, workflows, and scripts as code",
        category: "Automation",
        image: "ghcr.io/windmill-labs/windmill:main",
        default_port: 8314,
        container_port: "8000/tcp",
        env_vars: &[
            EnvVarDef { name: "DATABASE_URL", label: "PostgreSQL URL", default: "", required: true, secret: true },
        ],
        volumes: &[],
    },
    AppTemplateDef {
        id: "temporal",
        name: "Temporal Server",
        description: "Durable execution platform for reliable microservices and workflows",
        category: "Automation",
        image: "temporalio/auto-setup:latest",
        default_port: 8315,
        container_port: "8233/tcp",
        env_vars: &[
            EnvVarDef { name: "DB", label: "Database Type", default: "postgresql", required: false, secret: false },
            EnvVarDef { name: "POSTGRES_SEEDS", label: "PostgreSQL Host", default: "", required: true, secret: false },
            EnvVarDef { name: "POSTGRES_USER", label: "PostgreSQL User", default: "temporal", required: true, secret: false },
            EnvVarDef { name: "POSTGRES_PWD", label: "PostgreSQL Password", default: "", required: true, secret: true },
        ],
        volumes: &[],
    },
    // ─── Analytics (new) ────────────────────────────────────────
    AppTemplateDef {
        id: "superset",
        name: "Apache Superset",
        description: "Modern data exploration and visualization platform with rich SQL editor",
        category: "Analytics",
        image: "apache/superset:latest",
        default_port: 8321,
        container_port: "8088/tcp",
        env_vars: &[
            EnvVarDef { name: "SUPERSET_SECRET_KEY", label: "Secret Key", default: "", required: true, secret: true },
            EnvVarDef { name: "ADMIN_USERNAME", label: "Admin Username", default: "admin", required: false, secret: false },
            EnvVarDef { name: "ADMIN_PASSWORD", label: "Admin Password", default: "", required: true, secret: true },
        ],
        volumes: &["/app/superset_home"],
    },
    AppTemplateDef {
        id: "redash",
        name: "Redash",
        description: "Connect to any data source, visualize, and share dashboards with your team",
        category: "Analytics",
        image: "redash/redash:latest",
        default_port: 5100,
        container_port: "5000/tcp",
        env_vars: &[
            EnvVarDef { name: "REDASH_DATABASE_URL", label: "PostgreSQL URL", default: "", required: true, secret: true },
            EnvVarDef { name: "REDASH_REDIS_URL", label: "Redis URL", default: "redis://redis:6379/0", required: true, secret: false },
            EnvVarDef { name: "REDASH_SECRET_KEY", label: "Secret Key", default: "", required: true, secret: true },
        ],
        volumes: &[],
    },
    AppTemplateDef {
        id: "posthog",
        name: "PostHog",
        description: "Open-source product analytics with session recording, feature flags, and A/B testing",
        category: "Analytics",
        image: "posthog/posthog:latest",
        default_port: 8316,
        container_port: "8000/tcp",
        env_vars: &[
            EnvVarDef { name: "SECRET_KEY", label: "Secret Key", default: "", required: true, secret: true },
            EnvVarDef { name: "DATABASE_URL", label: "PostgreSQL URL", default: "", required: true, secret: true },
            EnvVarDef { name: "REDIS_URL", label: "Redis URL", default: "redis://redis:6379/", required: false, secret: false },
        ],
        volumes: &[],
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
        gpu_recommended: GPU_RECOMMENDED_TEMPLATES.contains(&def.id),
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
    user_id: Option<&str>,
    gpu_enabled: bool,
    gpu_indices: Option<Vec<u32>>,
) -> Result<DeployResult, String> {
    let template = TEMPLATES
        .iter()
        .find(|t| t.id == template_id)
        .ok_or_else(|| format!("Unknown template: {template_id}"))?;

    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;

    // Pull image (with timeout to prevent hanging on Docker daemon issues)
    let pull_result = tokio::time::timeout(std::time::Duration::from_secs(300), async {
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
    }).await;
    if pull_result.is_err() {
        return Err(format!("Image pull timed out for {}", template.image));
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

    // Volume binds — create dirs and canonicalize to prevent symlink TOCTOU
    let mut binds: Vec<String> = Vec::new();
    for vol in template.volumes {
        let host_dir = format!("/var/lib/dockpanel/apps/{name}{vol}");
        // Create directory before canonicalize (it must exist)
        std::fs::create_dir_all(&host_dir).ok();
        // Canonicalize to resolve any symlinks, then verify it's still under the allowed prefix
        let resolved = std::fs::canonicalize(&host_dir)
            .map_err(|e| format!("Volume path {host_dir} inaccessible: {e}"))?;
        let resolved_str = resolved.to_string_lossy();
        if !resolved_str.starts_with("/var/lib/dockpanel/apps/") {
            return Err(format!("Volume path {host_dir} escapes allowed prefix after canonicalization"));
        }
        binds.push(format!("{resolved_str}:{vol}"));
    }

    // NOTE: Portainer Docker socket auto-mount was removed for security.
    // Mounting the host Docker socket gives full host escape capabilities.
    // If Portainer needs Docker access, the admin should configure it separately
    // via docker-compose or manual volume mounts outside of DockPanel.

    let mut host_config = bollard::service::HostConfig {
        port_bindings: Some(port_bindings),
        restart_policy: Some(bollard::service::RestartPolicy {
            name: Some(bollard::service::RestartPolicyNameEnum::UNLESS_STOPPED),
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

    // GPU passthrough (requires NVIDIA Container Toolkit installed on host).
    // Two modes: all-GPUs (count: -1) vs specific indices (device_ids).
    // Setting both fields would be rejected by the daemon, so they're mutually
    // exclusive — device_ids wins when present, count is set otherwise.
    if gpu_enabled {
        let specific_ids: Option<Vec<String>> = gpu_indices
            .as_ref()
            .filter(|v| !v.is_empty())
            .map(|v| v.iter().map(|i| i.to_string()).collect());
        let (count_field, device_ids_field, log_target) = match &specific_ids {
            Some(ids) => (None, Some(ids.clone()), format!("specific GPU(s) [{}]", ids.join(","))),
            None => (Some(-1_i64), None, "all available GPUs".to_string()),
        };
        host_config.device_requests = Some(vec![
            bollard::service::DeviceRequest {
                driver: Some("nvidia".to_string()),
                count: count_field,
                device_ids: device_ids_field,
                capabilities: Some(vec![vec!["gpu".to_string(), "compute".to_string(), "utility".to_string()]]),
                ..Default::default()
            }
        ]);
        // GPU containers may need additional caps
        if let Some(ref mut caps) = host_config.cap_add {
            caps.push("SYS_ADMIN".to_string());
        }
        tracing::info!("GPU passthrough enabled for container {container_name}: {log_target}");
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
            if let Some(uid) = user_id {
                labels.insert("dockpanel.user.id".to_string(), uid.to_string());
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

    if let Err(e) = docker
        .start_container(&container.id, None::<StartContainerOptions<String>>)
        .await
    {
        // Clean up orphaned container on start failure
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
            let image = c.image.clone();

            // Extract volume mounts
            let volumes = c
                .mounts
                .as_ref()
                .map(|mounts| {
                    mounts
                        .iter()
                        .filter_map(|m| {
                            let src = m.source.as_deref().unwrap_or("?");
                            let dst = m.destination.as_deref().unwrap_or("?");
                            Some(format!("{src} → {dst}"))
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

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

            let stack_id = labels.get("dockpanel.stack_id").cloned();
            let user_id = labels.get("dockpanel.user.id").cloned();

            Some(DeployedApp {
                container_id: id.clone(),
                name,
                template: template.clone(),
                status,
                port,
                domain,
                health,
                image,
                volumes,
                stack_id,
                user_id,
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

/// Find a free port for the blue-green container by asking the OS.
pub(crate) fn find_free_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("Failed to find free port: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("Failed to get port: {e}"))?
        .port();
    drop(listener);
    Ok(port)
}

/// Health check: wait for a container to accept TCP connections on a port.
pub(crate) async fn health_check_port(port: u16, timeout_secs: u64) -> Result<(), String> {
    let deadline =
        tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    loop {
        match tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")).await {
            Ok(_) => return Ok(()),
            Err(_) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(format!(
                        "Container failed health check on port {port} after {timeout_secs}s"
                    ));
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }
    }
}

/// Swap the proxy_pass port in an existing nginx config file.
/// Returns Ok(()) on success, Err on failure.
pub(crate) fn swap_nginx_proxy_port(domain: &str, old_port: u16, new_port: u16) -> Result<(), String> {
    let config_path = format!("/etc/nginx/sites-enabled/{domain}.conf");
    let content = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read nginx config: {e}"))?;

    let old_pattern = format!("proxy_pass http://127.0.0.1:{old_port};");
    let new_pattern = format!("proxy_pass http://127.0.0.1:{new_port};");

    if !content.contains(&old_pattern) {
        return Err(format!(
            "Nginx config for {domain} does not contain proxy_pass on port {old_port}"
        ));
    }

    let new_content = content.replace(&old_pattern, &new_pattern);

    // Atomic write
    let tmp_path = format!("{config_path}.tmp");
    std::fs::write(&tmp_path, &new_content)
        .map_err(|e| format!("Failed to write nginx config: {e}"))?;
    std::fs::rename(&tmp_path, &config_path).map_err(|e| {
        std::fs::remove_file(&tmp_path).ok();
        format!("Failed to rename nginx config: {e}")
    })?;

    Ok(())
}

/// Extract the host port from a container's HostConfig port bindings.
pub(crate) fn extract_host_port(
    host_config: &bollard::service::HostConfig,
) -> Option<u16> {
    host_config
        .port_bindings
        .as_ref()?
        .values()
        .next()?
        .as_ref()?
        .first()?
        .host_port
        .as_ref()?
        .parse::<u16>()
        .ok()
}

/// Blue-green update: start new container → health check → swap nginx → remove old.
/// Returns UpdateResult on success. On ANY failure, rolls back and returns Err
/// so the caller can fall back to stop/start.
async fn blue_green_update(
    docker: &Docker,
    old_container_id: &str,
    name: &str,
    config: &bollard::models::ContainerConfig,
    host_config: &bollard::service::HostConfig,
    domain: &str,
    old_port: u16,
) -> Result<UpdateResult, String> {
    let temp_port = find_free_port()?;
    tracing::info!(
        "Blue-green update for {name}: old_port={old_port}, temp_port={temp_port}"
    );

    // Build new host_config with temp port
    let mut new_host_config = host_config.clone();
    if let Some(ref mut pb) = new_host_config.port_bindings {
        for bindings in pb.values_mut() {
            if let Some(bindings) = bindings {
                for binding in bindings.iter_mut() {
                    binding.host_port = Some(temp_port.to_string());
                }
            }
        }
    }

    // Create new container with a temp name
    let new_name = format!("{name}-blue");

    // Clean up stale blue container from a failed previous attempt
    if docker.inspect_container(&new_name, None).await.is_ok() {
        docker
            .stop_container(&new_name, Some(StopContainerOptions { t: 5 }))
            .await
            .ok();
        docker
            .remove_container(
                &new_name,
                Some(RemoveContainerOptions {
                    force: true,
                    v: false,
                    ..Default::default()
                }),
            )
            .await
            .ok();
    }

    let new_config = Config {
        image: config.image.clone(),
        env: config.env.clone(),
        exposed_ports: config.exposed_ports.clone(),
        labels: config.labels.clone(),
        host_config: Some(new_host_config),
        cmd: config.cmd.clone(),
        entrypoint: config.entrypoint.clone(),
        working_dir: if config.working_dir.as_deref() == Some("") {
            None
        } else {
            config.working_dir.clone()
        },
        ..Default::default()
    };

    let new_container = docker
        .create_container(
            Some(CreateContainerOptions {
                name: new_name.as_str(),
                platform: None,
            }),
            new_config,
        )
        .await
        .map_err(|e| format!("Failed to create blue container: {e}"))?;

    // Start new container
    if let Err(e) = docker
        .start_container(&new_container.id, None::<StartContainerOptions<String>>)
        .await
    {
        docker
            .remove_container(
                &new_container.id,
                Some(RemoveContainerOptions {
                    force: true,
                    v: false,
                    ..Default::default()
                }),
            )
            .await
            .ok();
        return Err(format!("Failed to start blue container: {e}"));
    }

    // Health check the new container (30s timeout)
    if let Err(e) = health_check_port(temp_port, 30).await {
        docker
            .stop_container(&new_container.id, Some(StopContainerOptions { t: 5 }))
            .await
            .ok();
        docker
            .remove_container(
                &new_container.id,
                Some(RemoveContainerOptions {
                    force: true,
                    v: false,
                    ..Default::default()
                }),
            )
            .await
            .ok();
        return Err(format!("Blue container health check failed: {e}"));
    }

    // Swap nginx to point to new container's port
    if let Err(e) = swap_nginx_proxy_port(domain, old_port, temp_port) {
        docker
            .stop_container(&new_container.id, Some(StopContainerOptions { t: 5 }))
            .await
            .ok();
        docker
            .remove_container(
                &new_container.id,
                Some(RemoveContainerOptions {
                    force: true,
                    v: false,
                    ..Default::default()
                }),
            )
            .await
            .ok();
        return Err(format!("Nginx port swap failed: {e}"));
    }

    // Test nginx config before reloading
    match crate::services::nginx::test_config().await {
        Ok(output) if output.success => {
            if let Err(e) = crate::services::nginx::reload().await {
                // Rollback nginx
                swap_nginx_proxy_port(domain, temp_port, old_port).ok();
                docker
                    .stop_container(&new_container.id, Some(StopContainerOptions { t: 5 }))
                    .await
                    .ok();
                docker
                    .remove_container(
                        &new_container.id,
                        Some(RemoveContainerOptions {
                            force: true,
                            v: false,
                            ..Default::default()
                        }),
                    )
                    .await
                    .ok();
                return Err(format!("Nginx reload failed: {e}"));
            }
        }
        Ok(output) => {
            // Rollback nginx + cleanup blue container
            swap_nginx_proxy_port(domain, temp_port, old_port).ok();
            docker
                .stop_container(&new_container.id, Some(StopContainerOptions { t: 5 }))
                .await
                .ok();
            docker
                .remove_container(
                    &new_container.id,
                    Some(RemoveContainerOptions {
                        force: true,
                        v: false,
                        ..Default::default()
                    }),
                )
                .await
                .ok();
            return Err(format!("Nginx config test failed: {}", output.stderr));
        }
        Err(e) => {
            swap_nginx_proxy_port(domain, temp_port, old_port).ok();
            docker
                .stop_container(&new_container.id, Some(StopContainerOptions { t: 5 }))
                .await
                .ok();
            docker
                .remove_container(
                    &new_container.id,
                    Some(RemoveContainerOptions {
                        force: true,
                        v: false,
                        ..Default::default()
                    }),
                )
                .await
                .ok();
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
                v: false,
                force: true,
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
                name: name.to_string(),
            },
        )
        .await
        .ok();

    tracing::info!(
        "App updated (blue-green, zero-downtime): {name}, port {old_port} → {temp_port}"
    );

    Ok(UpdateResult {
        container_id: new_container.id,
        blue_green: true,
    })
}

/// Update an app by pulling the latest image.
/// Uses blue-green deployment (zero-downtime) when the app has a domain with nginx reverse proxy.
/// Falls back to stop/start when no reverse proxy is configured.
pub async fn update_app(container_id: &str) -> Result<UpdateResult, String> {
    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;

    // Inspect the container to get its full config
    let info = docker
        .inspect_container(container_id, None)
        .await
        .map_err(|e| format!("Failed to inspect container: {e}"))?;

    let config = info.config.ok_or("No container config found")?;
    let host_config = info.host_config.ok_or("No host config found")?;
    let name = info
        .name
        .unwrap_or_default()
        .trim_start_matches('/')
        .to_string();
    let image = config.image.clone().ok_or("No image found")?;

    // Check if this app has a domain (nginx reverse proxy) for blue-green
    let domain = config
        .labels
        .as_ref()
        .and_then(|l| l.get("dockpanel.app.domain"))
        .cloned();
    let old_port = extract_host_port(&host_config);

    // Pull the latest image
    tracing::info!("Updating app {name}: pulling {image}");
    let mut pull = docker.create_image(
        Some(CreateImageOptions {
            from_image: image.as_str(),
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

    // Try blue-green if app has a domain and a known port
    if let (Some(domain), Some(old_port)) = (&domain, old_port) {
        // Check that nginx config exists for this domain
        let config_path = format!("/etc/nginx/sites-enabled/{domain}.conf");
        if std::path::Path::new(&config_path).exists() {
            match blue_green_update(
                &docker,
                container_id,
                &name,
                &config,
                &host_config,
                domain,
                old_port,
            )
            .await
            {
                Ok(result) => return Ok(result),
                Err(e) => {
                    tracing::warn!(
                        "Blue-green failed for {name}, falling back to stop/start: {e}"
                    );
                }
            }
        }
    }

    // Fallback: stop old → remove → create new → start (causes brief downtime)
    docker
        .stop_container(container_id, Some(StopContainerOptions { t: 10 }))
        .await
        .ok();
    docker
        .remove_container(
            container_id,
            Some(RemoveContainerOptions {
                v: false,
                force: true,
                ..Default::default()
            }),
        )
        .await
        .map_err(|e| format!("Failed to remove old container: {e}"))?;

    let new_config = Config {
        image: config.image,
        env: config.env,
        exposed_ports: config.exposed_ports,
        labels: config.labels,
        host_config: Some(host_config),
        cmd: config.cmd,
        entrypoint: config.entrypoint,
        working_dir: if config.working_dir.as_deref() == Some("") {
            None
        } else {
            config.working_dir
        },
        ..Default::default()
    };

    let container = docker
        .create_container(
            Some(CreateContainerOptions {
                name: name.as_str(),
                platform: None,
            }),
            new_config,
        )
        .await
        .map_err(|e| format!("Failed to create updated container: {e}"))?;

    docker
        .start_container(&container.id, None::<StartContainerOptions<String>>)
        .await
        .map_err(|e| format!("Failed to start updated container: {e}"))?;

    tracing::info!("App updated (stop/start): {name} ({image})");
    Ok(UpdateResult {
        container_id: container.id,
        blue_green: false,
    })
}

/// Get environment variables from a running container.
pub async fn get_app_env(container_id: &str) -> Result<Vec<(String, String)>, String> {
    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;

    let info = docker
        .inspect_container(container_id, None)
        .await
        .map_err(|e| format!("Failed to inspect container: {e}"))?;

    let env_list = info
        .config
        .and_then(|c| c.env)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| {
            let (k, v) = entry.split_once('=')?;
            Some((k.to_string(), v.to_string()))
        })
        .collect();

    Ok(env_list)
}

/// Get the domain label from a container, if set.
pub async fn get_app_domain(container_id: &str) -> Option<String> {
    let docker = Docker::connect_with_local_defaults().ok()?;
    let info = docker.inspect_container(container_id, None).await.ok()?;
    info.config?.labels?.get("dockpanel.app.domain").cloned()
}

/// Get the app name label from a container, if set.
pub async fn get_app_name(container_id: &str) -> Option<String> {
    let docker = Docker::connect_with_local_defaults().ok()?;
    let info = docker.inspect_container(container_id, None).await.ok()?;
    info.config?.labels?.get("dockpanel.app.name").cloned()
}

/// Update a container's environment variables by recreating it with the new env.
pub async fn update_env(
    container_id: &str,
    new_env: HashMap<String, String>,
) -> Result<String, String> {
    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;

    let info = docker
        .inspect_container(container_id, None)
        .await
        .map_err(|e| format!("Failed to inspect container: {e}"))?;

    let config = info.config.ok_or("No container config found")?;
    let host_config = info.host_config.ok_or("No host config found")?;
    let name = info
        .name
        .unwrap_or_default()
        .trim_start_matches('/')
        .to_string();

    // Build new env list
    let env_list: Vec<String> = new_env.iter().map(|(k, v)| format!("{k}={v}")).collect();

    // Stop and remove old container
    docker
        .stop_container(container_id, Some(StopContainerOptions { t: 10 }))
        .await
        .ok();
    docker
        .remove_container(
            container_id,
            Some(RemoveContainerOptions {
                v: false,
                force: true,
                ..Default::default()
            }),
        )
        .await
        .map_err(|e| format!("Failed to remove old container: {e}"))?;

    // Recreate with new env
    let new_config = Config {
        image: config.image,
        env: Some(env_list),
        exposed_ports: config.exposed_ports,
        labels: config.labels,
        host_config: Some(host_config),
        cmd: config.cmd,
        entrypoint: config.entrypoint,
        working_dir: if config.working_dir.as_deref() == Some("") {
            None
        } else {
            config.working_dir
        },
        ..Default::default()
    };

    let container = docker
        .create_container(
            Some(CreateContainerOptions {
                name: name.as_str(),
                platform: None,
            }),
            new_config,
        )
        .await
        .map_err(|e| format!("Failed to create container: {e}"))?;

    docker
        .start_container(&container.id, None::<StartContainerOptions<String>>)
        .await
        .map_err(|e| format!("Failed to start container: {e}"))?;

    tracing::info!(
        "Container env updated: {name} ({} vars)",
        new_env.len()
    );
    Ok(container.id)
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
