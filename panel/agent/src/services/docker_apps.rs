use bollard::container::{
    Config, CreateContainerOptions, ListContainersOptions, RemoveContainerOptions,
    RenameContainerOptions, StartContainerOptions, StopContainerOptions,
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
            EnvVarDef { name: "GF_SECURITY_ADMIN_PASSWORD", label: "Admin Password", default: "admin", required: false, secret: true },
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
