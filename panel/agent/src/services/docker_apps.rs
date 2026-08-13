use bollard::container::{
    Config, CreateContainerOptions, ListContainersOptions, RemoveContainerOptions,
    RenameContainerOptions, StartContainerOptions, StopContainerOptions,
};
use bollard::image::CreateImageOptions;
use bollard::Docker;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio_stream::StreamExt;

/// Where a template app's persistent volumes are bind-mounted from.
///
/// Named once because three places have to agree: the deploy that creates the
/// directory, the prefix check that keeps a volume from escaping it, and the
/// removal that decides whether this container is entitled to delete it.
pub const APP_DATA_DIR: &str = "/var/lib/dockpanel/apps";

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

/// Startup arguments for the templates whose image default `CMD` does not start a
/// server. Kept as a keyed list rather than a per-template field for the reason
/// given above `GPU_RECOMMENDED_TEMPLATES` — every static declaration in the
/// catalogue would otherwise have to be edited to add a field all but one leaves
/// empty.
///
/// Absence is the meaningful default: an image whose entrypoint already serves must
/// keep deciding its own startup, so upstream can change it without this catalogue
/// silently overriding them.
///
/// `minio` needs it because `minio/minio`'s default `CMD` is `["minio"]`, which
/// prints usage and **exits 0**. Nothing here crashes — the container simply has no
/// supervised process, so `unless-stopped` restarts it forever at `ExitCode: 0`
/// (issue #101). `/data` is the path the template already mounts a volume at, so the
/// served directory and the persisted directory are the same one; a `server` command
/// pointed anywhere else would come up healthy and lose every object on restart.
static TEMPLATE_CMD: &[(&str, &[&str])] = &[
    ("minio", &["server", "/data", "--console-address", ":9001"]),
    // Each entry below was established by RUNNING the image and observing it
    // both ways: with no command it exits, with this command it stays up and
    // answers on its port. `cloudflared`'s own default Cmd is ["version"], so
    // every deploy printed a version string and stopped; `tunnel run` reads
    // TUNNEL_TOKEN from the environment the template already collects, so the
    // token never has to reach the command line.
    ("cloudflared", &["tunnel", "run"]),
    ("ntfy", &["serve"]),
    ("trivy", &["server", "--listen", "0.0.0.0:8080"]),
    ("keycloak", &["start-dev"]),
];

/// The `Cmd` to give a template's container, if the catalogue overrides it.
fn template_cmd(id: &str) -> Option<Vec<String>> {
    TEMPLATE_CMD
        .iter()
        .find(|(template_id, _)| *template_id == id)
        .map(|(_, args)| args.iter().map(|a| (*a).to_string()).collect())
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
pub struct UpdateResult {
    pub container_id: String,
    pub blue_green: bool,
    /// Container paths this update moved out of the writable layer and onto a bind
    /// mount. Non-empty only for an app deployed before its template declared the
    /// path — see `migrate_unmounted_volumes`. Reported so the operator learns their
    /// app just became durable rather than having it happen silently.
    pub migrated_volumes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct DeployedApp {
    pub container_id: String,
    pub name: String,
    pub template: String,
    pub status: String,
    pub port: Option<u16>,
    pub domain: Option<String>,
    /// Whether this box serves `domain` over HTTPS. Carried so the panel can
    /// link to the scheme that actually answers instead of assuming `https`
    /// (issue #96 — an operator terminating TLS upstream serves plain HTTP).
    /// Always false when there is no domain.
    pub ssl: bool,
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
        // Ghost keeps its SQLite database, themes and uploaded images here, and the
        // image declares it as a VOLUME. Undeclared, Docker satisfied that with an
        // ANONYMOUS volume — which `docker rm` leaves behind but the replacement
        // container never sees, so an update read as total data loss (#110).
        volumes: &["/var/lib/ghost/content"],
    },
    AppTemplateDef {
        id: "azuracast",
        name: "AzuraCast",
        description: "Self-hosted web radio suite — station management, streaming, playlists, and listener stats in one all-in-one container",
        category: "Media",
        image: "ghcr.io/azuracast/azuracast:stable",
        default_port: 8420,
        container_port: "80/tcp",
        env_vars: &[],
        volumes: &["/var/azuracast"],
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
        // The image declares /data as a VOLUME and Redis writes its RDB/AOF snapshots
        // there. This template is in the "Database" category and was the only one in it
        // with nothing declared, so every recreate started it from an empty dataset.
        volumes: &["/data"],
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
        image: "portainer/portainer-ce:lts",
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
        // `N8N_BASIC_AUTH_USER` / `N8N_BASIC_AUTH_PASSWORD` used to live here. n8n
        // REMOVED basic auth in 1.0 and this template pins `n8nio/n8n:1`, so both were
        // inert — the deploy form offered an "Admin Password" field that protected
        // nothing and told the operator their instance was credentialed when it was
        // not. Ownership is established by creating the owner account on first visit.
        env_vars: &[],
        // THE #110 PATH. n8n defaults to SQLite and keeps everything here —
        // database.sqlite (workflows, executions, the owner account), the credential
        // encryption key in .n8n/config, and any community nodes. The image declares NO
        // volume of its own, so with nothing declared here the whole lot lived in the
        // container's writable layer and `docker rm` deleted it: the reporter's n8n came
        // back at first-run state after pressing Update, and none of it was recoverable.
        volumes: &["/home/node/.n8n"],
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
        image: "grafana/grafana:13.1",
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
        image: "vaultwarden/server:1.37.1",
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
        image: "metabase/metabase:v0.58-lts",
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
        // SearXNG generates settings.yml — including its `secret_key` — into /etc/searxng
        // on first boot, and the image declares that path as a VOLUME. Only the config
        // half is declared here: /var/cache/searxng is a rebuildable cache and does not
        // need to survive anything.
        volumes: &["/etc/searxng"],
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
        image: "codercom/code-server:4.131.0",
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
        image: "axllent/mailpit:v1.30",
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
        image: "outlinewiki/outline:1.9",
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
        image: "mattermost/mattermost-team-edition:release-11",
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
        image: "rocketchat/rocket.chat:8.7.0",
        default_port: 3007,
        container_port: "3000/tcp",
        env_vars: &[
            EnvVarDef { name: "MONGO_URL", label: "MongoDB URL", default: "", required: true, secret: false },
            EnvVarDef { name: "ROOT_URL", label: "Root URL", default: "", required: true, secret: false },
        ],
        volumes: &["/app/uploads"],
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
        image: "quay.io/keycloak/keycloak:26.7",
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
        image: "woodpeckerci/woodpecker-server:v3",
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
        image: "calcom/cal.com:v6.2.0",
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
        image: "chatwoot/chatwoot:latest-ce",
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
            // Activepieces defaults to POSTGRES + a standalone Redis, neither of which
            // this single-container template deploys, so it could not start at all. The
            // embedded pair is what upstream documents for the one-container install —
            // and it is what makes the volume below meaningful, since PGLite is the
            // thing that writes into it. Both halves or neither.
            EnvVarDef { name: "AP_DB_TYPE", label: "Database Mode", default: "PGLITE", required: false, secret: false },
            EnvVarDef { name: "AP_REDIS_TYPE", label: "Queue Mode", default: "MEMORY", required: false, secret: false },
        ],
        // PGLite's embedded Postgres and the local settings live here (AP_CONFIG_PATH
        // defaults to ~/.activepieces, and the image runs as root, so HOME is /root).
        volumes: &["/root/.activepieces"],
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
    // The Readarr template was withdrawn s342. LinuxServer's image for it publishes
    // an OCI index with ZERO manifests on both of its tags — `docker pull` exits 1
    // with "no matching manifest for linux/amd64", while the Sonarr and Radarr
    // controls return four manifests each. The upstream project is archived, last
    // pushed 2025-06-27, so there is nothing to repoint to. Same call as the four
    // withdrawn in v2.96.0.
    //
    // ⚠ This shape is invisible to a classifier that keys on the manifest's HTTP
    // status: the request succeeds with 200 and a well-formed index that happens
    // to be empty. v2.96.0's sweep classified by error MESSAGE and could not see
    // it. Any future resolvability check has to assert the index is NON-EMPTY.
    //
    // The exact reference is deliberately not spelled here — this file is the
    // subject of pins that grep raw source, and a name written in a comment
    // silently satisfies the absence arm meant to keep it gone.
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
        image: "dxflrs/garage:v2.3.0",
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
/// What `GET /apps/{id}/env` substitutes for a value it will not show.
///
/// Exported because [`update_env`] has to recognise it coming back: the edit
/// dialog is seeded from the masked read, so the mask is what a caller sends
/// for every field they did not touch.
pub const ENV_MASK: &str = "********";

/// Env var names the shipped catalogue itself declares are NOT secret.
///
/// The mask is chosen by substring — `KEY`, `AUTH`, `TOKEN`, `SECRET`,
/// `PASSWORD` anywhere in the name — which is right for an unknown key and
/// wrong for several we ship ourselves: `KEYCLOAK_ADMIN` is a username,
/// `NEXTAUTH_URL` is a URL, `AUTHENTIK_POSTGRESQL__HOST` is a hostname. All
/// three contain a pattern; none is a secret; all three were shown as
/// `********`, which is unreadable and — before the sentinel in [`update_env`]
/// — unrecoverable once saved.
///
/// A name is allowlisted only if EVERY template that declares it says
/// `secret: false`, so a name reused as a secret elsewhere keeps its mask, and
/// a name the catalogue has never heard of falls through to the substring test.
/// The list can therefore only ever un-mask something we ourselves defined.
/// Merge the caller's environment over the container's own, treating
/// [`ENV_MASK`] as "leave this one alone".
///
/// `GET /apps/{id}/env` substitutes the mask for anything sensitive, the edit
/// dialog is seeded from exactly that response and sends every field back, and
/// **this container is the only place the real value is stored** — no table
/// holds a Docker app's environment. Without this, saving the dialog wrote the
/// literal `********` over every masked variable and then removed the container
/// that held the original, which for Vaultwarden's `ADMIN_TOKEN` and
/// code-server's `PASSWORD` is an auth downgrade to a published literal, and for
/// Meilisearch a container that refuses to start after the original is gone.
///
/// Returns the new `KEY=value` list and the names carried over, so the caller
/// can say what it did rather than doing it silently.
pub(crate) fn merge_env_preserving_masked(
    new_env: &HashMap<String, String>,
    container_env: &[String],
) -> (Vec<String>, Vec<String>) {
    let existing: HashMap<&str, &str> = container_env
        .iter()
        .filter_map(|kv| kv.split_once('='))
        .collect();

    let mut kept = Vec::new();
    let env_list = new_env
        .iter()
        .map(|(k, v)| {
            if v == ENV_MASK {
                if let Some(real) = existing.get(k.as_str()) {
                    kept.push(k.clone());
                    return format!("{k}={real}");
                }
            }
            format!("{k}={v}")
        })
        .collect();

    kept.sort();
    (env_list, kept)
}

pub fn catalogue_non_secret_env(name: &str) -> bool {
    let mut seen = false;
    for t in TEMPLATES {
        for v in t.env_vars {
            if v.name == name {
                if v.secret {
                    return false;
                }
                seen = true;
            }
        }
    }
    seen
}

pub fn list_templates() -> Vec<AppTemplate> {
    TEMPLATES.iter().map(to_app_template).collect()
}

// ── Volume ownership ───────────────────────────────────────────────

/// Who a template's volume directories have to belong to.
///
/// `deploy_app` creates each declared volume as a **root-owned** directory on the
/// host and bind-mounts it. That is right for most images, which start as root and
/// chown their own data directory in their entrypoint before dropping privileges —
/// `postgres`, `mysql` and `mongo` all do exactly that.
///
/// It is wrong for an image that declares a non-root `Config.User`, because that
/// process starts **already dropped** and can never chown its way out: it cannot
/// write the directory the template just mounted for it. Both ways that shows up
/// are bad, and the quiet one is worse — either the container exits with
/// `Permission denied`, or it comes up healthy and silently persists nothing.
///
/// Measured over the catalogue by reading every image's config: 42 of 148 images
/// run non-root, and 31 of those declare a volume.
#[derive(Debug, Clone, PartialEq, Eq)]
struct VolumeOwner {
    uid: u32,
    /// `None` when no group could be resolved. The directory then keeps the group
    /// it already has, which is enough — the owner can write either way.
    gid: Option<u32>,
    /// The literal `Config.User`, so the log names what it acted on.
    spec: String,
}

/// Split a Docker `User` spec into its user and group halves.
///
/// Docker accepts `user`, `uid`, `user:group`, `uid:gid` and the mixed forms.
fn split_user_spec(spec: &str) -> (&str, Option<&str>) {
    match spec.split_once(':') {
        Some((user, group)) => (user.trim(), Some(group.trim())),
        None => (spec.trim(), None),
    }
}

/// Does this `Config.User` mean root? An empty field does — it is simply unset,
/// which is how the overwhelming majority of images ship.
fn user_spec_is_root(spec: &str) -> bool {
    matches!(split_user_spec(spec).0, "" | "root" | "0")
}

/// Pull one regular file's bytes out of a `docker cp` tar stream.
///
/// A dependency-free reader for the only shape this ever asks for: one small
/// file. A ustar header is 512 bytes, the size sits at offset 124 as octal ASCII,
/// and the payload is padded to the next 512-byte boundary.
fn tar_extract_first_file(archive: &[u8]) -> Option<String> {
    let mut offset = 0usize;
    while offset + 512 <= archive.len() {
        let header = &archive[offset..offset + 512];
        // A zero block ends the archive.
        if header.iter().all(|byte| *byte == 0) {
            return None;
        }
        let size_field = std::str::from_utf8(&header[124..136]).ok()?;
        let size = usize::from_str_radix(size_field.trim_matches(|c| c == '\0' || c == ' '), 8).ok()?;
        let body = offset + 512;
        // '0' and NUL both mean "regular file"; skip anything else.
        if (header[156] == b'0' || header[156] == 0) && size > 0 {
            let end = body.checked_add(size)?;
            return String::from_utf8(archive.get(body..end)?.to_vec()).ok();
        }
        offset = body + size.div_ceil(512) * 512;
    }
    None
}

/// Read one file out of an already-pulled image **without running it**.
///
/// Creating a container is enough to give Docker's copy API a filesystem to read;
/// nothing is ever started. This is the only way to resolve a login name, because
/// a name like `nonroot` is defined by the image's own `/etc/passwd` and by
/// nothing on the host.
async fn read_file_from_image(docker: &Docker, image: &str, path: &str) -> Option<String> {
    let probe_name = format!("dockpanel-userprobe-{}", uuid::Uuid::new_v4());
    let created = docker
        .create_container(
            Some(CreateContainerOptions {
                name: probe_name.as_str(),
                platform: None,
            }),
            Config {
                image: Some(image.to_string()),
                // Never started, so this only has to satisfy `create` for an image
                // that declares no command of its own.
                cmd: Some(vec!["dockpanel-user-probe".to_string()]),
                ..Default::default()
            },
        )
        .await
        .ok()?;

    let mut stream = docker.download_from_container(
        &created.id,
        Some(bollard::container::DownloadFromContainerOptions { path }),
    );
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(bytes) => buf.extend_from_slice(&bytes),
            Err(_) => break,
        }
    }

    let _ = docker
        .remove_container(
            &created.id,
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await;

    tar_extract_first_file(&buf)
}

/// Resolve a login name to `(uid, gid)` from an image's own `/etc/passwd`.
fn passwd_lookup(passwd: &str, name: &str) -> Option<(u32, u32)> {
    passwd.lines().find_map(|line| {
        let mut fields = line.split(':');
        if fields.next()? != name {
            return None;
        }
        let _password = fields.next()?;
        Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
    })
}

/// Find the primary gid of a numeric uid in an image's own `/etc/passwd`.
fn passwd_gid_for_uid(passwd: &str, uid: u32) -> Option<u32> {
    passwd.lines().find_map(|line| {
        let mut fields = line.split(':');
        let _login = fields.next()?;
        let _password = fields.next()?;
        if fields.next()?.parse::<u32>().ok()? != uid {
            return None;
        }
        fields.next()?.parse().ok()
    })
}

/// Resolve a group name to a gid from an image's own `/etc/group`.
fn group_lookup(group: &str, name: &str) -> Option<u32> {
    group.lines().find_map(|line| {
        let mut fields = line.split(':');
        if fields.next()? != name {
            return None;
        }
        let _password = fields.next()?;
        fields.next()?.parse().ok()
    })
}

/// Work out who a template's volume directories must belong to, or `None` when
/// the image runs as root and today's behaviour is already correct.
///
/// ⚠ There is deliberately **no fallback**. Guessing a uid hands one tenant's data
/// directory to a different account, and `0777` would put a world-writable
/// directory on a host that runs other people's containers. When the spec cannot
/// be resolved this returns `None` and says so out loud, leaving the directory
/// root-owned — the same state as before, but no longer a silent one.
async fn resolve_volume_owner(docker: &Docker, image: &str) -> Option<VolumeOwner> {
    let spec = docker
        .inspect_image(image)
        .await
        .ok()?
        .config?
        .user
        .unwrap_or_default();
    let spec = spec.trim();
    if user_spec_is_root(spec) {
        return None;
    }
    let (user_part, group_part) = split_user_spec(spec);
    let numeric_uid = user_part.parse::<u32>().ok();

    // `/etc/passwd` is needed to translate a login name, and to find the primary
    // group of a bare numeric uid. A `uid:gid` pair needs nothing from the image.
    let passwd = if numeric_uid.is_none() || group_part.is_none() {
        read_file_from_image(docker, image, "/etc/passwd").await
    } else {
        None
    };

    let (uid, primary_gid) = match numeric_uid {
        Some(uid) => (
            uid,
            passwd.as_deref().and_then(|p| passwd_gid_for_uid(p, uid)),
        ),
        None => match passwd.as_deref().and_then(|p| passwd_lookup(p, user_part)) {
            Some((uid, gid)) => (uid, Some(gid)),
            None => {
                tracing::warn!(
                    "Image {image} runs as non-root user {spec:?} which could not be \
                     resolved from its own /etc/passwd; leaving volume directories \
                     root-owned. The app may be unable to write its data directory."
                );
                return None;
            }
        },
    };

    let gid = match group_part {
        Some(group) => match group.parse::<u32>() {
            Ok(gid) => Some(gid),
            Err(_) => read_file_from_image(docker, image, "/etc/group")
                .await
                .as_deref()
                .and_then(|g| group_lookup(g, group)),
        },
        None => primary_gid,
    };

    Some(VolumeOwner {
        uid,
        gid,
        spec: spec.to_string(),
    })
}

/// `chown` a path, leaving the group alone when none was resolved.
///
/// Deliberately **not** recursive. The owner of the directory is what lets the
/// image create its own files underneath; recursing would rewrite ownership of
/// data a previous deployment wrote, which is a bigger and riskier operation than
/// the defect calls for.
fn chown_to(path: &str, owner: &VolumeOwner) -> std::io::Result<()> {
    let c_path = std::ffi::CString::new(path)
        .map_err(|_| std::io::Error::other("volume path contains a NUL byte"))?;
    // `-1` means "leave this one as it is" — the same call shape `main.rs` uses to
    // set only the group on the agent socket.
    let gid = owner.gid.unwrap_or(u32::MAX);
    if unsafe { libc::chown(c_path.as_ptr(), owner.uid, gid) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
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

    // An image that runs as a non-root user starts already dropped and cannot chown
    // the directory we are about to create for it, so resolve who it needs to be
    // owned by before creating any. Root images resolve to `None` and are left
    // exactly as they were.
    let volume_owner = if template.volumes.is_empty() {
        None
    } else {
        resolve_volume_owner(&docker, template.image).await
    };

    // Volume binds — create dirs and canonicalize to prevent symlink TOCTOU
    let mut binds: Vec<String> = Vec::new();
    for vol in template.volumes {
        let host_dir = format!("{APP_DATA_DIR}/{name}{vol}");
        // Create directory before canonicalize (it must exist)
        std::fs::create_dir_all(&host_dir).ok();
        // Canonicalize to resolve any symlinks, then verify it's still under the allowed prefix
        let resolved = std::fs::canonicalize(&host_dir)
            .map_err(|e| format!("Volume path {host_dir} inaccessible: {e}"))?;
        let resolved_str = resolved.to_string_lossy();
        if !resolved_str.starts_with(&format!("{APP_DATA_DIR}/")) {
            return Err(format!("Volume path {host_dir} escapes allowed prefix after canonicalization"));
        }
        // Chown AFTER the prefix check, never before — the check is what proves the
        // path is one of ours, and a chown is exactly the operation a symlink escape
        // would want to borrow.
        if let Some(owner) = &volume_owner {
            match chown_to(&resolved_str, owner) {
                Ok(()) => tracing::info!(
                    "Volume {resolved_str} chowned to {}:{} for image user {:?}",
                    owner.uid,
                    owner.gid.map_or("unchanged".to_string(), |g| g.to_string()),
                    owner.spec
                ),
                Err(e) => tracing::warn!(
                    "Could not chown volume {resolved_str} to {}: {e}. The app runs as \
                     {:?} and may be unable to write its data directory.",
                    owner.uid,
                    owner.spec
                ),
            }
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
            // saturating_mul avoids a u64 overflow wrapping to a garbage/negative i64 limit
            // for an absurd memory_mb; the backend also clamps to 4..=65536 (s237 audit).
            let bytes = mem.saturating_mul(1024 * 1024).min(i64::MAX as u64) as i64;
            let swap = mem.saturating_mul(2 * 1024 * 1024).min(i64::MAX as u64) as i64;
            host_config.memory = Some(bytes);
            // Memory swap = 2x memory (allows some swap)
            host_config.memory_swap = Some(swap);
        }
    }
    if let Some(cpu) = cpu_percent {
        if cpu > 0 {
            // cpu_percent is % of ONE core (100 = 1.0 core); values > 100 request multiple
            // cores, so cpu_quota may exceed cpu_period. The previous `cpu <= 100` bound
            // silently dropped the limit for any multi-core request → the container ran with
            // UNLIMITED CPU (s237 audit — parity with update_limits which accepts 1..=10000).
            host_config.cpu_period = Some(100_000);
            host_config.cpu_quota = Some(cpu.saturating_mul(1000).min(i64::MAX as u64) as i64);
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
        // NOTE (s237 audit): GPU compute via the NVIDIA Container Toolkit does NOT need
        // CAP_SYS_ADMIN — the device_requests above grant GPU access. Granting SYS_ADMIN would
        // reverse the cap_drop ALL hardening (it enables mount(2)/cgroup container→host escape),
        // so it is deliberately NOT added; the container keeps the same minimal cap set as any
        // other template. A workload that genuinely needs it should be an explicit opt-in.
        tracing::info!("GPU passthrough enabled for container {container_name}: {log_target}");
    }

    if !binds.is_empty() {
        host_config.binds = Some(binds);
    }

    let mut exposed_ports = HashMap::new();
    exposed_ports.insert(template.container_port.to_string(), HashMap::new());

    let config = Config {
        image: Some(template.image.to_string()),
        // None for all but one template, which leaves the image's own CMD in charge.
        // See TEMPLATE_CMD: without this the container inherits a default that may
        // print usage and exit 0, which restarts forever instead of serving (#101).
        cmd: template_cmd(template.id),
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

            let ssl = domain
                .as_deref()
                .map(crate::services::nginx::domain_is_https)
                .unwrap_or(false);

            Some(DeployedApp {
                container_id: id.clone(),
                name,
                template: template.clone(),
                status,
                port,
                domain,
                ssl,
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

/// The mount destinations this container already has, read from BOTH spellings.
///
/// A container may express a mount as a `Binds` entry (`"src:dst[:opts]"`) or as a
/// `Mounts` entry carrying a `target`, and Docker accepts either. A census keyed on one
/// of them is blind to the other, so this reads both and returns the union.
fn mounted_destinations(host_config: &bollard::service::HostConfig) -> Vec<String> {
    let mut dests = Vec::new();
    if let Some(binds) = &host_config.binds {
        for bind in binds {
            // "<source>:<destination>[:<options>]" — the destination is the second
            // field, so split from the left and take index 1.
            if let Some(dest) = bind.split(':').nth(1) {
                if !dest.is_empty() {
                    dests.push(dest.to_string());
                }
            }
        }
    }
    if let Some(mounts) = &host_config.mounts {
        for m in mounts {
            if let Some(target) = m.target.as_ref().filter(|t| !t.is_empty()) {
                dests.push(target.clone());
            }
        }
    }
    dests
}

/// Paths this container's own template calls persistent that nothing actually mounts.
///
/// Returns empty for anything we are not entitled to touch: a container without
/// `dockpanel.managed=true`, one whose `dockpanel.app.template` names no template we
/// ship, or one whose declared paths are all mounted already.
fn unmounted_declared_volumes(
    config: &bollard::models::ContainerConfig,
    host_config: &bollard::service::HostConfig,
) -> Vec<&'static str> {
    let Some(labels) = config.labels.as_ref() else {
        return Vec::new();
    };
    if labels.get("dockpanel.managed").map(String::as_str) != Some("true") {
        return Vec::new();
    }
    let Some(template_id) = labels.get("dockpanel.app.template") else {
        return Vec::new();
    };
    let Some(template) = TEMPLATES.iter().find(|t| t.id == template_id) else {
        return Vec::new();
    };
    let mounted = mounted_destinations(host_config);
    template
        .volumes
        .iter()
        .copied()
        .filter(|vol| !mounted.iter().any(|d| d == vol))
        .collect()
}

/// Carry an app's data out of the container's writable layer and onto a bind mount,
/// on the way through a recreate.
///
/// Every recreate path stops the container, `docker rm`s it and rebuilds the replacement
/// from a `docker inspect` of the original — and `docker rm` deletes the writable layer.
/// For a container with no mount covering the directory its app writes to, that layer IS
/// the storage, so the recreate is unrecoverable deletion. GitHub #110 is exactly this: an
/// n8n deployed while the template declared `volumes: &[]` came back at first-run state
/// the first time its operator pressed Update, account and workflows gone.
///
/// Declaring the volume fixes the NEXT deploy and does nothing for the containers already
/// running. This closes that half — the destructive step becomes the repair. Limits, all
/// deliberate:
///
/// - Only paths the container's OWN template declares and the container does not mount.
///   An operator who arranged their own bind keeps it.
/// - Only into a host directory that is absent or empty. A non-empty one is somebody's
///   data and this declines rather than merging into it.
/// - **Any failure is returned, and every caller must abort the recreate before removing
///   the container.** A migration that half-worked followed by a `docker rm` would destroy
///   the very data it was called to save.
async fn migrate_unmounted_volumes(
    docker: &Docker,
    container_id: &str,
    name: &str,
    image: &str,
    missing: &[&'static str],
    host_config: &mut bollard::service::HostConfig,
) -> Result<Vec<String>, String> {
    if missing.is_empty() {
        return Ok(Vec::new());
    }
    let owner = resolve_volume_owner(docker, image).await;
    let mut binds = host_config.binds.clone().unwrap_or_default();
    let mut migrated = Vec::new();

    for vol in missing {
        let host_dir = format!("{APP_DATA_DIR}/{name}{vol}");
        if let Ok(mut entries) = std::fs::read_dir(&host_dir) {
            if entries.next().is_some() {
                return Err(format!(
                    "Refusing to migrate {vol} for {name}: {host_dir} already exists and is \
                     not empty. Move it aside and retry, or mount it yourself."
                ));
            }
        }
        std::fs::create_dir_all(&host_dir)
            .map_err(|e| format!("Failed to create {host_dir}: {e}"))?;
        // Canonicalize then re-check the prefix, exactly as `deploy_app` does — the
        // check is what proves the path is one of ours, and it has to happen after
        // symlinks are resolved.
        let resolved = std::fs::canonicalize(&host_dir)
            .map_err(|e| format!("Volume path {host_dir} inaccessible: {e}"))?;
        let resolved_str = resolved.to_string_lossy().to_string();
        if !resolved_str.starts_with(&format!("{APP_DATA_DIR}/")) {
            return Err(format!(
                "Volume path {host_dir} escapes allowed prefix after canonicalization"
            ));
        }

        // `<container>:<path>/.` copies the CONTENTS of the directory, so the data lands
        // directly in the host directory instead of one level down inside a copy of it.
        // `docker cp` preserves the numeric uid/gid the files carry in the container, so
        // a non-root app keeps ownership of its own data.
        let out = crate::safe_cmd::safe_command("docker")
            .args(["cp", &format!("{container_id}:{vol}/."), &resolved_str])
            .output()
            .await
            .map_err(|e| format!("Failed to run docker cp for {vol}: {e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            // A path the image never created carries nothing to lose. That is not a
            // failure — but the bind is still added, so the NEXT recreate has somewhere
            // durable to write.
            let absent = err.contains("No such container:path")
                || err.contains("no such file or directory")
                || err.contains("Could not find the file");
            if !absent {
                return Err(format!(
                    "Failed to copy {vol} out of {name}: {}",
                    err.trim()
                ));
            }
            tracing::info!(
                "Nothing to migrate at {vol} for {name} — the path does not exist in the \
                 container; mounting it empty so future updates preserve it"
            );
        }

        if let Some(owner) = &owner {
            if let Err(e) = chown_to(&resolved_str, owner) {
                tracing::warn!(
                    "Could not chown migrated volume {resolved_str} to {}: {e}. The app runs \
                     as {:?} and may be unable to write its data directory.",
                    owner.uid,
                    owner.spec
                );
            }
        }
        binds.push(format!("{resolved_str}:{vol}"));
        migrated.push((*vol).to_string());
        tracing::info!("Migrated {name}{vol} onto {resolved_str} — it now survives recreates");
    }

    host_config.binds = Some(binds);
    Ok(migrated)
}

/// Does this container hold state that a second copy of it would fight over?
///
/// Blue-green starts the replacement while the original is still serving, and only
/// stops the original after the health check and the nginx swap have both succeeded.
/// For that window — up to 30s of health checking plus an nginx test and reload — two
/// processes are running against the SAME host paths, because `blue_green_update`
/// clones the old `HostConfig` and rewrites only its port bindings.
///
/// That is harmless when the container's only state is its own writable layer: the
/// replacement gets a fresh one and nothing is shared. It is data corruption when the
/// state is a single-writer database sitting in a bind mount. Postgres, MySQL and Mongo
/// self-protect with a pidfile lock, so they merely fail to start and the caller falls
/// back to stop/start — noisy, but safe. **SQLite has no such lock**, so Ghost, Gitea,
/// Bookstack and Vaultwarden would interleave writes from two processes silently.
///
/// There is no way to ask an image whether it tolerates two writers, so the only sound
/// reading of "this container has a mount" is "assume it does not". Zero-downtime is
/// worth having; it is not worth a corrupted database, and the stop/start path this
/// falls back to is the same one every blue-green failure already takes.
fn shares_persistent_state(host_config: &bollard::service::HostConfig) -> bool {
    let has_binds = host_config.binds.as_ref().is_some_and(|b| !b.is_empty());
    let has_mounts = host_config.mounts.as_ref().is_some_and(|m| !m.is_empty());
    // `volumes_from` points at another container's mounts, which are shared by
    // definition. Anonymous volumes are deliberately NOT counted: Docker gives the
    // replacement its own, so there is nothing for the two to contend over.
    let has_volumes_from = host_config
        .volumes_from
        .as_ref()
        .is_some_and(|v| !v.is_empty());
    has_binds || has_mounts || has_volumes_from
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

    // Create new container with a temp name. The separator is `.` and not `-`
    // because `is_valid_name` rejects `.`: an app may legally be called
    // `api-blue`, and `dockpanel-app-api-blue` was therefore both its production
    // container AND the stand-in name for the app `api`.
    let new_name = crate::services::ownership::blue_name(name);

    // Clean up the leftover of a failed previous attempt — if it is ours.
    if let Ok(info) = docker.inspect_container(&new_name, None).await {
        let owner = crate::services::ownership::blue_leftover(
            info.config.as_ref().and_then(|c| c.labels.as_ref()),
        );
        if !owner.may_delete() {
            return Err(format!(
                "A container named {new_name} exists and is not managed by DockPanel \
                 ({owner:?}); refusing to force-remove it to make room for a blue-green swap"
            ));
        }
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
        // Blue-green is only reached when nothing needed migrating — see `update_app`.
        migrated_volumes: Vec::new(),
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
    let mut host_config = info.host_config.ok_or("No host config found")?;
    let name = info
        .name
        .unwrap_or_default()
        .trim_start_matches('/')
        .to_string();
    let image = config.image.clone().ok_or("No image found")?;

    // Anything this app's template calls persistent that the container does not mount is
    // living in the writable layer, which the recreate below deletes (#110). Decide that
    // BEFORE choosing a strategy: migrating needs the container stopped, which only the
    // stop/start path gives us.
    let unmounted = unmounted_declared_volumes(&config, &host_config);

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

    // Try blue-green if app has a domain and a known port — and only if running two
    // copies of it at once cannot corrupt anything. See `shares_persistent_state`.
    if let (Some(domain), Some(old_port)) = (&domain, old_port) {
        // Check that nginx config exists for this domain
        let config_path = format!("/etc/nginx/sites-enabled/{domain}.conf");
        if !unmounted.is_empty() {
            tracing::info!(
                "Not using blue-green for {name}: {} declared path(s) are unmounted and have \
                 to be migrated out of the writable layer first, which needs the container \
                 stopped. Using stop/start instead.",
                unmounted.len()
            );
        } else if shares_persistent_state(&host_config) {
            tracing::info!(
                "Not using blue-green for {name}: it has persistent mounts, and blue-green \
                 would run the old and new containers against the same host paths for the \
                 length of the health check. Using stop/start instead (brief downtime, no \
                 concurrent writers)."
            );
        } else if std::path::Path::new(&config_path).exists() {
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

    // Fallback: stop old → migrate → remove → create new → start (brief downtime)
    docker
        .stop_container(container_id, Some(StopContainerOptions { t: 10 }))
        .await
        .ok();

    // Stopped, so the app has flushed; not yet removed, so the data is still reachable.
    // This is the only window in which it can be rescued, and a failure here MUST abort
    // before the remove below — the whole point is to not destroy what we came to save.
    let migrated =
        migrate_unmounted_volumes(&docker, container_id, &name, &image, &unmounted, &mut host_config)
            .await
            .map_err(|e| {
                format!(
                    "Aborting update of {name} without removing it: {e}. The container is \
                     stopped and intact; start it again with `docker start {name}`."
                )
            })?;

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

    if migrated.is_empty() {
        tracing::info!("App updated (stop/start): {name} ({image})");
    } else {
        tracing::info!(
            "App updated (stop/start): {name} ({image}) — and {} path(s) migrated onto \
             persistent storage: {}",
            migrated.len(),
            migrated.join(", ")
        );
    }
    Ok(UpdateResult {
        container_id: container.id,
        blue_green: false,
        migrated_volumes: migrated,
    })
}

/// Change a managed container's image, preserving its full runtime configuration.
///
/// (s237 audit) The previous implementation recreated the container with a bare
/// `docker run -d --name X --volumes-from Y --network bridge <image>`, which dropped
/// EVERY hardening flag the original deploy applied — cap_drop ALL + the minimal cap_add
/// allowlist, security_opt no-new-privileges, the published `127.0.0.1:<port>` binding
/// (so the nginx reverse proxy 502'd), the restart policy, the memory/CPU limits, the env
/// vars, and the `dockpanel.managed` labels (so the container vanished from the panel).
/// This inspects the existing container and recreates it with the same `host_config`
/// (caps, security_opt, port bindings, restart, resource limits, GPU device requests, binds),
/// `labels`, `env`, and `exposed_ports`, only swapping the image. The new image's own
/// cmd/entrypoint apply (a tag bump may change them), matching the original run semantics.
pub async fn change_container_image(container_id: &str, new_image: &str) -> Result<String, String> {
    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;

    // Inspect the existing container to capture its full config before touching it.
    let info = docker
        .inspect_container(container_id, None)
        .await
        .map_err(|e| format!("Failed to inspect container: {e}"))?;

    let config = info.config.ok_or("No container config found")?;
    let mut host_config = info.host_config.ok_or("No host config found")?;
    let name = info
        .name
        .unwrap_or_default()
        .trim_start_matches('/')
        .to_string();
    if name.is_empty() {
        return Err("Container not found".to_string());
    }
    let old_image = config.image.clone().unwrap_or_default();
    let unmounted = unmounted_declared_volumes(&config, &host_config);

    // Pull the new image. create_image's stream may emit non-fatal warnings, so verify the
    // image exists locally afterwards rather than trusting the stream's per-chunk results.
    let mut pull = docker.create_image(
        Some(CreateImageOptions {
            from_image: new_image,
            ..Default::default()
        }),
        None,
        None,
    );
    let mut last_err: Option<String> = None;
    while let Some(result) = pull.next().await {
        if let Err(e) = result {
            last_err = Some(e.to_string());
        }
    }
    if docker.inspect_image(new_image).await.is_err() {
        return Err(format!(
            "Failed to pull image '{new_image}': {}",
            last_err.unwrap_or_else(|| "image not found after pull".to_string())
        ));
    }

    // Stop + remove the old container, KEEPING its volumes (v: false) so data persists.
    // ⚠ `v: false` preserves VOLUMES, which is not the same as preserving DATA: a
    // container with no mount keeps its state in the writable layer, and `docker rm`
    // deletes that whatever `v` says (#110). Migrate first, in the window where the
    // container is stopped but still exists, and abort without removing if that fails.
    docker
        .stop_container(container_id, Some(StopContainerOptions { t: 10 }))
        .await
        .ok();
    migrate_unmounted_volumes(
        &docker,
        container_id,
        &name,
        &old_image,
        &unmounted,
        &mut host_config,
    )
    .await
    .map_err(|e| {
        format!(
            "Aborting image change for {name} without removing it: {e}. The container is \
             stopped and intact; start it again with `docker start {name}`."
        )
    })?;
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

    // Recreate with the SAME hardening/networking/limits/labels/env, only the image changes.
    let new_config = Config {
        image: Some(new_image.to_string()),
        env: config.env,
        exposed_ports: config.exposed_ports,
        labels: config.labels,
        host_config: Some(host_config),
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
        .map_err(|e| format!("Failed to create container with new image: {e}"))?;

    docker
        .start_container(&container.id, None::<StartContainerOptions<String>>)
        .await
        .map_err(|e| format!("Failed to start container with new image: {e}"))?;

    tracing::info!("Image changed for {name}: → {new_image} (new: {})", container.id);
    Ok(container.id)
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

/// What a removal needs to know about a container *before* it is removed, so
/// the cleanup that follows can prove it owns each thing it deletes.
///
/// Read in one inspect because after `remove_app` there is nothing left to ask.
/// Every field is optional and a missing one means "cannot prove it" — the
/// callers treat that as a refusal, per [`crate::services::ownership`].
#[derive(Debug, Default, Clone)]
pub struct RemovalIdentity {
    /// `dockpanel.app.domain` — the vhost, cert dir and logs are named after it.
    pub domain: Option<String>,
    /// `dockpanel.app.name` — `/var/lib/dockpanel/apps/{name}` is named after it.
    pub name: Option<String>,
    /// The published host port. The app's vhost `proxy_pass`es to it, and it is
    /// the only thing in that file identifying which container it fronts.
    pub host_port: Option<u16>,
    /// Whether this container actually bind-mounts out of
    /// `/var/lib/dockpanel/apps/{name}`.
    ///
    /// A template deploy does; a compose service does not — its binds are
    /// confined to `/var/lib/dockpanel/compose/`, so an identically-named
    /// compose service was deleting a template app's data directory while
    /// owning nothing under it.
    pub owns_app_dir: bool,
}

/// Gather [`RemovalIdentity`] for `container_id`. Never fails: an unreachable
/// Docker yields all-none, which refuses every subsequent delete.
pub async fn removal_identity(container_id: &str) -> RemovalIdentity {
    let mut id = RemovalIdentity::default();
    let Ok(docker) = Docker::connect_with_local_defaults() else {
        return id;
    };
    let Ok(info) = docker.inspect_container(container_id, None).await else {
        return id;
    };

    if let Some(labels) = info.config.as_ref().and_then(|c| c.labels.as_ref()) {
        id.domain = labels.get("dockpanel.app.domain").cloned();
        id.name = labels.get("dockpanel.app.name").cloned();
    }

    if let Some(host_config) = info.host_config.as_ref() {
        id.host_port = extract_host_port(host_config);

        // Ownership of the data directory is decided by the container's own
        // mounts, not by its name label. `{prefix}` rather than `{prefix}/` so a
        // bind of the directory itself counts, and the trailing boundary is
        // checked explicitly so `/apps/data` cannot answer for `/apps/database`.
        if let (Some(name), Some(binds)) = (id.name.as_deref(), host_config.binds.as_ref()) {
            let prefix = format!("{APP_DATA_DIR}/{name}");
            id.owns_app_dir = binds.iter().any(|b| {
                let source = b.split(':').next().unwrap_or_default();
                source == prefix || source.starts_with(&format!("{prefix}/"))
            });
        }
    }

    id
}

/// True iff the container's labels mark it dockpanel-managed (`dockpanel.managed=true`).
/// This is the boundary that separates panel-managed app containers from the panel's OWN infra
/// (its postgres/api/agent containers carry no such label). Pure + unit-tested.
fn is_managed_labels(labels: Option<&HashMap<String, String>>) -> bool {
    labels
        .and_then(|l| l.get("dockpanel.managed"))
        .map(|v| v == "true")
        .unwrap_or(false)
}

/// Guard an action-handler target: inspect the container and reject unless it is dockpanel-managed.
/// Gives WRITE scope (stop/start/restart/logs/exec/remove/change-image/update/env/snapshot/limits)
/// the same `dockpanel.managed=true` boundary the `list` READ path already enforces, so an admin
/// cannot act on the panel's own infrastructure containers by id (confused-deputy footgun). For the
/// recreate paths (change_container_image/update_env) this runs on the ORIGINAL container before any
/// stop/remove, and the recreation preserves the label — so the new container stays managed.
pub async fn require_managed(container_id: &str) -> Result<(), String> {
    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;
    let info = docker
        .inspect_container(container_id, None)
        .await
        .map_err(|e| format!("Failed to inspect container: {e}"))?;
    if !is_managed_labels(info.config.as_ref().and_then(|c| c.labels.as_ref())) {
        return Err("not a dockpanel-managed container".to_string());
    }
    Ok(())
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
    let mut host_config = info.host_config.ok_or("No host config found")?;
    let name = info
        .name
        .unwrap_or_default()
        .trim_start_matches('/')
        .to_string();
    let image = config.image.clone().unwrap_or_default();
    let unmounted = unmounted_declared_volumes(&config, &host_config);

    // Build new env list, carrying over anything the caller sent back masked.
    let (env_list, kept) = merge_env_preserving_masked(&new_env, config.env.as_deref().unwrap_or(&[]));

    if !kept.is_empty() {
        tracing::info!(
            "update_env for {container_id}: {} masked value(s) carried over unchanged ({})",
            kept.len(),
            kept.join(", ")
        );
    }

    // Stop, migrate anything living in the writable layer, then remove.
    // Editing one environment variable recreates the container, so this door destroys an
    // unmounted app's data exactly as thoroughly as Update does (#110) — it just does not
    // sound like it. The migration has to happen before the remove, and a failure has to
    // abort before it.
    docker
        .stop_container(container_id, Some(StopContainerOptions { t: 10 }))
        .await
        .ok();
    migrate_unmounted_volumes(
        &docker,
        container_id,
        &name,
        &image,
        &unmounted,
        &mut host_config,
    )
    .await
    .map_err(|e| {
        format!(
            "Aborting environment update for {name} without removing it: {e}. The container \
             is stopped and intact; start it again with `docker start {name}`."
        )
    })?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `minio/minio`'s default `CMD` is `["minio"]`, which prints usage and exits
    /// 0 — so the container restarts forever at `ExitCode: 0` rather than serving
    /// (issue #101). Verified against the image s339: without these arguments the
    /// container reached `restarting` with 7 restarts and exit 0; with them it ran
    /// with 0 restarts and answered 200 on both the S3 port and the console.
    #[test]
    fn minio_overrides_a_cmd_that_would_exit_instead_of_serving() {
        let cmd = template_cmd("minio").expect("minio must carry startup arguments");
        assert_eq!(cmd, vec!["server", "/data", "--console-address", ":9001"]);

        // The served path must be the one the template persists, or the container
        // comes up healthy and silently loses every object on restart.
        let minio = TEMPLATES
            .iter()
            .find(|t| t.id == "minio")
            .expect("the minio template must exist");
        assert!(
            minio.volumes.contains(&"/data"),
            "minio serves /data but persists {:?}",
            minio.volumes
        );
    }

    /// Absence is the default for an image whose entrypoint already serves — but
    /// absence is NOT proof that a template is fine.
    ///
    /// ⚠ **MinIO is not the only template in this class.** A catalogue audit found
    /// at least seven more whose image cannot come up as shipped, verified from
    /// their image config: `cloudflared` (Cmd `["version"]` — prints a version and
    /// exits, every time, while the template collects a TUNNEL_TOKEN it has no way
    /// to use), `ntfy`, `trivy` and `surrealdb` (multi-command CLI entrypoints with
    /// a null Cmd, so a bare run prints help), plus `keycloak`, `authentik` and
    /// `vllm`. `keycloak`'s own description says it *"Requires 'start-dev' command
    /// override"* — written against a struct that then had no way to express one.
    /// Only MinIO was reported, reproduced and fixed; the rest are unfixed and each
    /// needs its own arguments established by running the image.
    #[test]
    fn every_cmd_override_is_live_and_well_formed() {
        assert!(template_cmd("minio").is_some());
        assert!(template_cmd("ghost").is_none());
        assert!(template_cmd("no-such-template").is_none());

        // Every override must name a real template, or it is dead configuration
        // that looks live.
        for (id, args) in TEMPLATE_CMD {
            assert!(
                TEMPLATES.iter().any(|t| t.id == *id),
                "TEMPLATE_CMD names '{id}', which is not in the catalogue"
            );
            assert!(!args.is_empty(), "'{id}' carries an empty command");
        }
    }

    #[test]
    fn managed_label_gate() {
        // Only dockpanel.managed=true passes; panel infra (no such label) and false are rejected.
        let managed = HashMap::from([("dockpanel.managed".to_string(), "true".to_string())]);
        assert!(is_managed_labels(Some(&managed)));

        let disabled = HashMap::from([("dockpanel.managed".to_string(), "false".to_string())]);
        assert!(!is_managed_labels(Some(&disabled)));

        // A panel-infra container (e.g. only compose labels) must be rejected.
        let infra = HashMap::from([("com.docker.compose.project".to_string(), "dockpanel".to_string())]);
        assert!(!is_managed_labels(Some(&infra)));

        assert!(!is_managed_labels(Some(&HashMap::new())));
        assert!(!is_managed_labels(None));
    }

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn a_masked_value_sent_back_leaves_the_real_one_alone() {
        // Exactly what the edit dialog posts after the user changes one field:
        // it was seeded from the MASKED read, so every untouched secret comes
        // back as the mask.
        let container = vec![
            "ADMIN_TOKEN=hunter2-the-real-one".to_string(),
            "PUID=1000".to_string(),
        ];
        let posted = env(&[("ADMIN_TOKEN", ENV_MASK), ("PUID", "1001")]);

        let (list, kept) = merge_env_preserving_masked(&posted, &container);

        assert!(
            list.contains(&"ADMIN_TOKEN=hunter2-the-real-one".to_string()),
            "the real value must survive; got {list:?}"
        );
        assert!(!list.iter().any(|kv| kv.contains(ENV_MASK)));
        assert!(list.contains(&"PUID=1001".to_string()), "edits still apply");
        assert_eq!(kept, vec!["ADMIN_TOKEN".to_string()]);
    }

    #[test]
    fn a_value_that_merely_looks_like_the_mask_is_not_resurrected() {
        // A NEW variable whose value the operator really did type as eight
        // asterisks has nothing to fall back to, and must be stored verbatim
        // rather than silently dropped.
        let (list, kept) = merge_env_preserving_masked(&env(&[("NEW", ENV_MASK)]), &[]);
        assert_eq!(list, vec![format!("NEW={ENV_MASK}")]);
        assert!(kept.is_empty());
    }

    #[test]
    fn a_container_env_entry_with_no_equals_does_not_derail_the_merge() {
        let container = vec!["MALFORMED".to_string(), "A=1".to_string()];
        let (list, _) = merge_env_preserving_masked(&env(&[("A", ENV_MASK)]), &container);
        assert_eq!(list, vec!["A=1".to_string()]);
    }

    #[test]
    fn the_catalogue_exempts_its_own_non_secrets_and_never_its_secrets() {
        // All four contain a SENSITIVE_PATTERN substring; none is a secret.
        for name in [
            "KEYCLOAK_ADMIN",
            "NEXTAUTH_URL",
            "AUTHENTIK_POSTGRESQL__HOST",
            "AUTHENTIK_REDIS__HOST",
        ] {
            assert!(
                catalogue_non_secret_env(name),
                "{name} is declared secret:false in the catalogue and must not be masked"
            );
        }
        // The real secrets sitting right beside them stay masked.
        for name in [
            "KEYCLOAK_ADMIN_PASSWORD",
            "NEXTAUTH_SECRET",
            "AUTHENTIK_SECRET_KEY",
            "AUTHENTIK_POSTGRESQL__PASSWORD",
        ] {
            assert!(!catalogue_non_secret_env(name), "{name} must stay masked");
        }
        // A name the catalogue has never heard of falls through to the
        // substring test rather than being exempted.
        assert!(!catalogue_non_secret_env("SOME_VENDOR_API_KEY"));
    }

    // ── Volume ownership ───────────────────────────────────────────

    /// An empty `User` is how almost every image ships and it means root, which is
    /// the case that must stay untouched: those images chown their own data
    /// directory in their entrypoint.
    #[test]
    fn root_user_specs_are_recognised_in_every_spelling() {
        for spec in ["", "root", "0", "0:0", "root:root", "  root  ", "0:1000"] {
            assert!(user_spec_is_root(spec), "{spec:?} names root");
        }
        for spec in ["nobody", "472", "nonroot", "65532:65532", "node-red", "1000"] {
            assert!(!user_spec_is_root(spec), "{spec:?} does not name root");
        }
    }

    #[test]
    fn user_specs_split_into_their_halves() {
        assert_eq!(split_user_spec("nobody"), ("nobody", None));
        assert_eq!(split_user_spec("65532:65532"), ("65532", Some("65532")));
        assert_eq!(split_user_spec("node:node"), ("node", Some("node")));
        assert_eq!(split_user_spec("1000:staff"), ("1000", Some("staff")));
    }

    /// The real `/etc/passwd` out of `prom/prometheus:v3`, whose `Config.User` is
    /// the NAME `nobody` — resolvable only from inside the image.
    #[test]
    fn a_login_name_resolves_from_the_images_own_passwd() {
        let passwd = "root:x:0:0:root:/root:/bin/ash\nnobody:x:65534:65534:nobody:/home:/bin/false\n";
        assert_eq!(passwd_lookup(passwd, "nobody"), Some((65534, 65534)));
        assert_eq!(passwd_lookup(passwd, "root"), Some((0, 0)));
        // A name the image does not define must fail rather than guess.
        assert_eq!(passwd_lookup(passwd, "grafana"), None);
        // A bare numeric uid still wants its primary group.
        assert_eq!(passwd_gid_for_uid(passwd, 65534), Some(65534));
        assert_eq!(passwd_gid_for_uid(passwd, 472), None);
    }

    #[test]
    fn a_group_name_resolves_from_the_images_own_group_file() {
        let group = "root:x:0:\nnogroup:x:65534:\nnode-red:x:1000:\n";
        assert_eq!(group_lookup(group, "nogroup"), Some(65534));
        assert_eq!(group_lookup(group, "node-red"), Some(1000));
        assert_eq!(group_lookup(group, "absent"), None);
    }

    /// The tar reader has to cope with what `docker cp` actually returns, and must
    /// stop rather than guess when it is handed something else.
    #[test]
    fn the_tar_reader_extracts_a_file_and_refuses_junk() {
        fn ustar(name: &str, body: &str) -> Vec<u8> {
            let mut block = vec![0u8; 512];
            block[..name.len()].copy_from_slice(name.as_bytes());
            let size = format!("{:011o}\0", body.len());
            block[124..124 + size.len()].copy_from_slice(size.as_bytes());
            block[156] = b'0';
            block.extend_from_slice(body.as_bytes());
            block.resize(512 + body.len().div_ceil(512) * 512, 0);
            block
        }

        let body = "nobody:x:65534:65534:nobody:/home:/bin/false\n";
        assert_eq!(tar_extract_first_file(&ustar("passwd", body)).as_deref(), Some(body));

        // An all-zero block is the end of an archive, not a file.
        assert_eq!(tar_extract_first_file(&vec![0u8; 1024]), None);
        // A truncated stream must not panic or return a partial read. The cut has
        // to land INSIDE the body (header 512 + body 44 = 556), or the archive is
        // merely missing its padding and still parses perfectly.
        let mut truncated = ustar("passwd", body);
        truncated.truncate(530);
        assert_eq!(tar_extract_first_file(&truncated), None);
        // Nothing at all.
        assert_eq!(tar_extract_first_file(&[]), None);
    }

    /// Every template whose image runs non-root and declares a volume needed the
    /// chown, so the catalogue must not quietly lose the volumes that make them
    /// victims. Measured s342 by reading all 148 images' config blobs from the
    /// registry: 42 run non-root, 31 of those declare a volume.
    ///
    /// ⚠ The cases are spelled `"id count"` rather than as `(id, count)` tuples on
    /// purpose. `app-template-images-pin-e2e.sh` §6 proves `TEMPLATE_CMD` carries
    /// no entry for the three templates a command cannot fix, by grepping this
    /// file's whitespace-stripped source for a parenthesised template id followed
    /// by a comma. A tuple here flattens to exactly that shape and turns another
    /// suite red against correct code. Test DATA is raw source like anything else,
    /// and so is this comment — that suite does not strip comments, so the shape
    /// must not be spelled out here either. Do not "tidy" this back into tuples.
    #[test]
    fn the_measured_victims_still_declare_the_volumes_they_could_not_write() {
        for case in [
            "prometheus 1",
            "grafana 1",
            "jenkins 1",
            "mattermost 3",
            "sonarqube 3",
            "node-red 1",
            "surrealdb 1",
        ] {
            let (id, count) = case.split_once(' ').expect("each case is `id count`");
            let count: usize = count.parse().expect("count parses");
            let template = TEMPLATES
                .iter()
                .find(|t| t.id == id)
                .unwrap_or_else(|| panic!("{id} must exist in the catalogue"));
            assert_eq!(
                template.volumes.len(),
                count,
                "{id} declares {:?}",
                template.volumes
            );
        }
    }

    /// Hits the local Docker daemon and the two images the fix was proven on, so
    /// it is not part of the ordinary suite. Run with:
    ///   cargo test -p dockpanel-agent volume_owner -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "requires a Docker daemon with the test images pulled"]
    async fn volume_owner_resolves_real_images_both_ways() {
        let docker = Docker::connect_with_unix_defaults().expect("docker");

        // A NAME, resolvable only from inside the image.
        let prometheus = resolve_volume_owner(&docker, "prom/prometheus:v3").await;
        assert_eq!(
            prometheus,
            Some(VolumeOwner {
                uid: 65534,
                gid: Some(65534),
                spec: "nobody".to_string()
            })
        );

        // A bare numeric uid, whose primary group still comes from the image.
        let grafana = resolve_volume_owner(&docker, "grafana/grafana:13.1").await;
        assert_eq!(grafana.as_ref().map(|o| o.uid), Some(472));

        // The control: a root image must resolve to None so nothing is chowned.
        assert_eq!(resolve_volume_owner(&docker, "nginx:alpine").await, None);
    }
}
