<!-- Generated from panel/agent/src/services/docker_apps.rs. Do not edit by hand: -->
<!-- UPDATE_APPS_DOC=1 cargo test --manifest-path panel/agent/Cargo.toml docs_apps_page -->

# Apps

DockPanel ships 147 one-click Docker app templates in 14 categories.

Deploy one from the **Docker Apps** page in the panel. `dockpanel apps templates` prints the same catalog on your server.

A template starts one app container. Some apps also need a value from you, or a service you run yourself such as a database: check the app's own documentation. **Known issue** marks a template that currently does not work with a one-click deploy. **GPU** marks one that benefits from GPU passthrough.

## AI (10)

| App | ID | Description | Image |
|---|---|---|---|
| Dify | `dify` | Open-source LLM app development platform with RAG, agents, and workflows | `langgenius/dify-api:latest` |
| Flowise | `flowise` | Drag-and-drop LLM flow builder for LangChain applications · **Known issue** | `flowiseai/flowise:latest` |
| Langflow | `langflow` | Visual framework for building multi-agent and RAG applications | `langflowai/langflow:1.12.0` |
| LiteLLM Proxy | `litellm` | Unified proxy for 100+ LLM APIs (OpenAI, Anthropic, Cohere, etc.) | `ghcr.io/berriai/litellm:main-latest` |
| LocalAI | `localai` | Self-hosted OpenAI-compatible API for running LLMs, image and audio generation · **GPU** | `localai/localai:latest-gpu-nvidia-cuda-12` |
| Ollama | `ollama` | Run large language models locally (Llama, Mistral, Gemma, Phi) · **GPU** | `ollama/ollama:0.33.3` |
| Open WebUI | `open-webui` | ChatGPT-style web interface for Ollama and OpenAI-compatible APIs | `ghcr.io/open-webui/open-webui:main` |
| Text Generation WebUI | `text-generation-webui` | Gradio web UI for running large language models (oobabooga) · **GPU** | `atinoda/text-generation-webui:default` |
| vLLM | `vllm` | High-throughput, memory-efficient LLM inference server with OpenAI-compatible API · **GPU** | `vllm/vllm-openai:v0.28.0` |
| Whisper ASR | `whisper` | OpenAI Whisper speech-to-text service with REST API · **GPU** | `onerahmet/openai-whisper-asr-webservice:v1.10.0` |

## Analytics (8)

| App | ID | Description | Image |
|---|---|---|---|
| Apache Superset | `superset` | Modern data exploration and visualization platform with rich SQL editor | `apache/superset:latest` |
| Matomo | `matomo` | Google Analytics alternative that respects user privacy | `matomo:5` |
| Metabase | `metabase` | Business intelligence and analytics dashboard builder | `metabase/metabase:v0.58-lts` |
| Plausible Analytics | `plausible` | Privacy-friendly alternative to Google Analytics | `ghcr.io/plausible/community-edition:v3.2.1` |
| PostHog | `posthog` | Open-source product analytics with session recording, feature flags, and A/B testing | `posthog/posthog:latest` |
| Redash | `redash` | Connect to any data source, visualize, and share dashboards with your team | `redash/redash:8.0.0.b32245` |
| Shynet | `shynet` | Privacy-friendly web analytics without cookies or JavaScript | `milesmcc/shynet:v0.13.1` |
| Umami | `umami` | Simple, privacy-focused website analytics | `ghcr.io/umami-software/umami:postgresql-v2.15.1` |

## Automation (6)

| App | ID | Description | Image |
|---|---|---|---|
| Activepieces | `activepieces` | No-code workflow automation platform (Zapier alternative) | `activepieces/activepieces:latest` |
| Huginn | `huginn` | System for building agents that perform automated tasks online (IFTTT alternative) | `ghcr.io/huginn/huginn:latest` |
| n8n | `n8n` | Workflow automation tool with 200+ integrations | `n8nio/n8n:1` |
| Node-RED | `node-red` | Flow-based programming tool for wiring IoT, APIs, and online services | `nodered/node-red:5.0.6` |
| Temporal Server | `temporal` | Durable execution platform for reliable microservices and workflows | `temporalio/auto-setup:1.29.3` |
| Windmill | `windmill` | Developer platform for building internal tools, workflows, and scripts as code | `ghcr.io/windmill-labs/windmill:main` |

## CMS (4)

| App | ID | Description | Image |
|---|---|---|---|
| Directus | `directus` | Open data platform — instant REST & GraphQL API for any SQL database | `directus/directus:11` |
| Ghost | `ghost` | Professional publishing platform for blogs and newsletters | `ghost:5` |
| Listmonk | `listmonk` | High-performance self-hosted newsletter and mailing list manager | `listmonk/listmonk:v6.2.0` |
| WordPress (Docker) | `wordpress-docker` | World's most popular CMS as a Docker container with Apache and PHP | `wordpress:6-apache` |

## Communication (2)

| App | ID | Description | Image |
|---|---|---|---|
| Element Web | `element-web` | Feature-rich Matrix client for secure, decentralized communication | `vectorim/element-web:v1.12.27` |
| Matrix Synapse | `synapse` | Reference Matrix homeserver for decentralized, encrypted messaging | `matrixdotorg/synapse:latest` |

## Database (17)

| App | ID | Description | Image |
|---|---|---|---|
| Apache Cassandra | `cassandra` | Distributed wide-column NoSQL database for high availability at scale | `cassandra:5` |
| ArangoDB | `arangodb` | Multi-model database combining document, graph, and key-value in one engine | `arangodb:3.12` |
| ClickHouse | `clickhouse` | Column-oriented OLAP database for real-time analytics on large datasets | `clickhouse/clickhouse-server:26.8.2.7` |
| CouchDB | `couchdb` | Document-oriented NoSQL database with multi-master replication | `couchdb:3` |
| DragonflyDB | `dragonflydb` | Modern in-memory datastore compatible with Redis and Memcached APIs | `docker.dragonflydb.io/dragonflydb/dragonfly:latest` |
| InfluxDB | `influxdb` | Time series database for metrics, events, and real-time analytics | `influxdb:2` |
| KeyDB | `keydb` | Multi-threaded fork of Redis with active replication and flash storage support | `eqalpha/keydb:latest` |
| MariaDB | `mariadb` | Community-developed fork of MySQL with enhanced performance | `mariadb:11` |
| MongoDB | `mongo` | Document-oriented NoSQL database for modern apps | `mongo:7` |
| MySQL | `mysql` | The world's most popular open-source relational database | `mysql:8` |
| Neo4j | `neo4j` | Leading graph database for connected data with Cypher query language | `neo4j:5` |
| PostgreSQL | `postgres` | Advanced open-source relational database | `postgres:16-alpine` |
| QuestDB | `questdb` | High-performance time series database with SQL support and built-in web console | `questdb/questdb:10.0.1` |
| Redis | `redis` | In-memory data store for caching and message brokering | `redis:7-alpine` |
| SurrealDB | `surrealdb` | Multi-model database for web, mobile, serverless, and backend (SQL, graph, document) | `surrealdb/surrealdb:v3.2.4` |
| TimescaleDB | `timescaledb` | PostgreSQL extension for time-series data with automatic partitioning | `timescale/timescaledb:latest-pg16` |
| Valkey | `valkey` | High-performance key/value datastore (community fork of Redis) | `valkey/valkey:8-alpine` |

## Development (14)

| App | ID | Description | Image |
|---|---|---|---|
| Buildkite Agent | `buildkite-agent` | CI/CD agent that runs build jobs from the Buildkite platform | `buildkite/agent:3` |
| Docker Registry | `registry` | Private Docker image registry for storing and distributing container images | `registry:2` |
| Drone CI | `drone` | Container-native CI/CD platform with pipeline-as-code · **Known issue** | `drone/drone:2` |
| Forgejo | `forgejo` | Community-driven Git forge — lightweight Gitea fork with extra features | `codeberg.org/forgejo/forgejo:9` |
| Gitea | `gitea` | Lightweight self-hosted Git service with issues, pull requests, and CI/CD | `gitea/gitea:1` |
| Gitness | `gitness` | Open-source developer platform with Git hosting, pipelines, and code review (by Harness) | `harness/gitness:3.0.0-beta.11` |
| IT-Tools | `it-tools` | Collection of handy developer tools (JSON formatter, UUID generator, hash, base64, etc.) | `corentinth/it-tools:2024.10.22-7ca5933` |
| Jenkins | `jenkins` | Leading open-source CI/CD automation server | `jenkins/jenkins:lts` |
| Mailpit | `mailpit` | Email testing tool — catches outgoing emails for dev/testing | `axllent/mailpit:v1.30` |
| Nexus Repository | `nexus` | Universal artifact repository manager for Maven, npm, Docker, and more | `sonatype/nexus3:3.96.0` |
| SonarQube | `sonarqube` | Continuous code quality and security inspection platform | `sonarqube:10-community` |
| Verdaccio | `verdaccio` | Lightweight private npm proxy registry for hosting and caching packages | `verdaccio/verdaccio:6` |
| VS Code Server | `code-server` | Run VS Code in the browser — full IDE accessible anywhere | `codercom/code-server:4.131.0` |
| Woodpecker CI | `woodpecker` | Simple yet powerful CI/CD engine with great extensibility | `woodpeckerci/woodpecker-server:v3` |

## Media (17)

| App | ID | Description | Image |
|---|---|---|---|
| Audiobookshelf | `audiobookshelf` | Self-hosted audiobook and podcast server with mobile apps | `ghcr.io/advplyr/audiobookshelf:latest` |
| AzuraCast | `azuracast` | Self-hosted web radio suite — station management, streaming, playlists, and listener stats in one all-in-one container · **Known issue** | `ghcr.io/azuracast/azuracast:stable` |
| Bazarr | `bazarr` | Companion app for Sonarr and Radarr to manage and download subtitles | `lscr.io/linuxserver/bazarr:1.6.0` |
| Calibre-Web | `calibre-web` | Web-based e-book library manager with OPDS feed and reading interface | `lscr.io/linuxserver/calibre-web:0.6.27` |
| Immich | `immich` | High-performance self-hosted photo and video management | `ghcr.io/immich-app/immich-server:release` |
| Jellyfin | `jellyfin` | Free media server for movies, TV shows, music, and photos | `jellyfin/jellyfin:10` |
| Jellyseerr | `jellyseerr` | Media request management for Jellyfin, Emby, and Plex with auto-approval | `fallenbagel/jellyseerr:2.7.3` |
| Kavita | `kavita` | Self-hosted digital library for manga, comics, and books | `jvmilazz0/kavita:0.9.1` |
| Lidarr | `lidarr` | Music collection manager with automatic downloading and metadata management | `lscr.io/linuxserver/lidarr:3.1.0` |
| Navidrome | `navidrome` | Modern music server and streamer compatible with Subsonic/Airsonic clients | `deluan/navidrome:0.63.2` |
| Overseerr | `overseerr` | Media request management and discovery tool for Plex ecosystems | `lscr.io/linuxserver/overseerr:v1.35.0-ls158` |
| PhotoPrism | `photoprism` | AI-powered photo management app for browsing, organizing, and sharing | `photoprism/photoprism:260728` |
| Plex | `plex` | Media server for organizing and streaming movies, TV, music, and photos | `lscr.io/linuxserver/plex:1.43.3` |
| Prowlarr | `prowlarr` | Indexer manager and proxy for Sonarr, Radarr, Lidarr, and Readarr | `lscr.io/linuxserver/prowlarr:2.5.2` |
| Radarr | `radarr` | Movie collection manager with automatic downloading and organization | `lscr.io/linuxserver/radarr:6.3.0` |
| Sonarr | `sonarr` | TV series collection manager with automatic downloading and organization | `lscr.io/linuxserver/sonarr:4.0.19` |
| Tautulli | `tautulli` | Monitoring and tracking tool for Plex Media Server usage and statistics | `lscr.io/linuxserver/tautulli:2.18.1` |

## Monitoring (11)

| App | ID | Description | Image |
|---|---|---|---|
| Changedetection.io | `changedetection` | Website change detection and monitoring with notifications | `ghcr.io/dgtlmoon/changedetection.io:latest` |
| Checkmk | `checkmk` | Infrastructure and application monitoring with auto-discovery and alerting | `checkmk/check-mk-raw:2.3.0-latest` |
| Dozzle | `dozzle` | Real-time Docker container log viewer with a clean web interface | `amir20/dozzle:v10.9.2` |
| Glances | `glances` | Cross-platform system monitoring tool with web interface and API | `nicolargo/glances:latest-full` |
| Grafana | `grafana` | Open-source observability platform for metrics, logs, and traces | `grafana/grafana:13.1` |
| Grafana Loki | `loki` | Log aggregation system designed to work with Grafana | `grafana/loki:3` |
| Graylog | `graylog` | Centralized log management and analysis platform with alerting | `graylog/graylog:6.1` |
| Netdata | `netdata` | Real-time infrastructure monitoring with zero configuration | `netdata/netdata:stable` |
| Prometheus | `prometheus` | Monitoring system and time-series database for metrics | `prom/prometheus:v3` |
| Uptime Kuma | `uptime-kuma` | Self-hosted monitoring tool with notifications and status pages | `louislam/uptime-kuma:1` |
| Vector | `vector` | High-performance observability data pipeline for logs, metrics, and traces · **Known issue** | `timberio/vector:latest-alpine` |

## Networking (9)

| App | ID | Description | Image |
|---|---|---|---|
| AdGuard Home | `adguard-home` | Network-wide ad and tracker blocking DNS server with HTTPS filtering | `adguard/adguardhome:v0.107.79` |
| Caddy | `caddy` | Powerful web server with automatic HTTPS and easy configuration | `caddy:2-alpine` |
| Cloudflare Tunnel | `cloudflared` | Secure tunnel to expose local services to the internet via Cloudflare | `cloudflare/cloudflared:2026.8.3` |
| Nginx Proxy Manager | `nginx-proxy-manager` | Easy-to-use reverse proxy with free SSL and web-based management | `jc21/nginx-proxy-manager:2.15.1` |
| Pi-hole | `pihole` | Network-wide ad blocker and DNS sinkhole | `pihole/pihole:2024.07.0` |
| Tailscale | `tailscale` | Zero-config mesh VPN built on WireGuard for secure private networking | `tailscale/tailscale:v1.102.3` |
| Technitium DNS | `technitium-dns` | Open-source authoritative and recursive DNS server with web admin panel | `technitium/dns-server:15.4.0` |
| Traefik | `traefik` | Cloud-native reverse proxy and load balancer with automatic HTTPS | `traefik:v3` |
| WireGuard | `wireguard` | Modern, fast VPN tunnel with easy configuration | `lscr.io/linuxserver/wireguard:1.0.20260223` |

## Productivity (3)

| App | ID | Description | Image |
|---|---|---|---|
| Focalboard | `focalboard` | Open-source project management tool (Trello/Notion/Asana alternative by Mattermost) | `mattermost/focalboard:latest` |
| Joplin Server | `joplin-server` | Sync server for Joplin note-taking app with sharing and collaboration | `joplin/server:3.7.1` |
| Leantime | `leantime` | Open-source project management system for non-project managers (lean methodology) | `leantime/leantime:3.9.8` |

## Security (7)

| App | ID | Description | Image |
|---|---|---|---|
| Authelia | `authelia` | Single Sign-On and 2FA portal for securing web applications · **Known issue** | `authelia/authelia:4.39.22` |
| Authentik | `authentik` | Open-source identity provider with SSO, MFA, and user management | `ghcr.io/goauthentik/server:2024.12` |
| CrowdSec | `crowdsec` | Collaborative security engine analyzing logs and sharing threat intelligence | `crowdsecurity/crowdsec:v1.8.1` |
| Keycloak | `keycloak` | Open-source IAM with SSO, identity brokering, and social login. Requires 'start-dev' command override for dev mode. | `quay.io/keycloak/keycloak:26.7` |
| Trivy Server | `trivy` | Comprehensive vulnerability scanner for containers, filesystems, and IaC | `aquasec/trivy:0.74.0` |
| Vaultwarden | `vaultwarden` | Lightweight Bitwarden-compatible password manager server | `vaultwarden/server:1.37.1` |
| Wazuh Manager | `wazuh` | Open-source security platform for threat detection, compliance, and incident response · **Known issue** | `wazuh/wazuh-manager:4.9.2` |

## Storage (7)

| App | ID | Description | Image |
|---|---|---|---|
| File Browser | `filebrowser` | Web-based file manager with sharing, users, and customization | `filebrowser/filebrowser:v2.63.23` |
| Garage | `garage` | Lightweight S3-compatible distributed object storage for self-hosting · **Known issue** | `dxflrs/garage:v2.3.0` |
| MinIO | `minio` | High-performance S3-compatible object storage | `minio/minio:RELEASE.2025-09-07T16-13-09Z` |
| Nextcloud | `nextcloud` | Self-hosted productivity platform — files, calendar, contacts, and more | `nextcloud:30` |
| SeaweedFS | `seaweedfs` | Fast distributed storage system for billions of files with S3 API support | `chrislusf/seaweedfs:4.45` |
| Syncthing | `syncthing` | Continuous file synchronization between devices (Dropbox alternative) | `lscr.io/linuxserver/syncthing:2.1.3` |
| Zipline | `zipline` | ShareX/file upload server with URL shortening and image galleries | `ghcr.io/diced/zipline:latest` |

## Tools (32)

| App | ID | Description | Image |
|---|---|---|---|
| Actual Budget | `actual-budget` | Privacy-focused local-first personal budgeting app | `actualbudget/actual-server:26.9.0` |
| Adminer | `adminer` | Lightweight database management UI supporting MySQL, PostgreSQL, SQLite | `adminer:4` |
| BookStack | `bookstack` | Simple, self-hosted platform for organizing and storing information | `lscr.io/linuxserver/bookstack:26.05.4` |
| Cal.com | `calcom` | Open-source scheduling infrastructure for appointments and meetings | `calcom/cal.com:v6.2.0` |
| Chatwoot | `chatwoot` | Open-source customer engagement platform with live chat and helpdesk | `chatwoot/chatwoot:latest-ce` |
| Dashy | `dashy` | Feature-rich self-hosted dashboard for your homelab and cloud stack | `lissy93/dashy:latest` |
| DocuSeal | `docuseal` | Open-source document signing platform (DocuSign alternative) | `docuseal/docuseal:3.2.3` |
| ERPNext | `erpnext` | Open-source ERP for manufacturing, distribution, retail, and services · **Known issue** | `frappe/erpnext:latest` |
| FreshRSS | `freshrss` | Self-hosted RSS feed aggregator with full-text search and mobile apps | `freshrss/freshrss:1.29.1` |
| Gotify | `gotify` | Simple server for sending and receiving push notifications via REST API | `gotify/server:3.1.0` |
| HedgeDoc | `hedgedoc` | Real-time collaborative Markdown editor for teams | `quay.io/hedgedoc/hedgedoc:1.12.0` |
| Homarr | `homarr` | Sleek server dashboard with drag-and-drop widgets and integrations | `ghcr.io/homarr-labs/homarr:latest` |
| Homepage | `homepage` | Highly customizable application dashboard with service integrations | `ghcr.io/gethomepage/homepage:latest` |
| Invoice Ninja | `invoice-ninja` | Full-featured invoicing, payments, and expense tracking for freelancers | `invoiceninja/invoiceninja:5` |
| Linkwarden | `linkwarden` | Collaborative bookmark manager with archiving, tagging, and collections | `ghcr.io/linkwarden/linkwarden:v2.16.2` |
| Mattermost | `mattermost` | Open-source Slack alternative for secure team collaboration | `mattermost/mattermost-team-edition:release-11` |
| Mealie | `mealie` | Self-hosted recipe manager with meal planning, shopping lists, and API | `ghcr.io/mealie-recipes/mealie:v3.25.1` |
| Meilisearch | `meilisearch` | Lightning-fast, typo-tolerant search engine | `getmeili/meilisearch:v1` |
| Memos | `memos` | Privacy-first lightweight note-taking service (Google Keep alternative) | `neosmemo/memos:stable` |
| NocoDB | `nocodb` | Open-source Airtable alternative — turn any database into a spreadsheet | `nocodb/nocodb:2026.08.2` |
| ntfy | `ntfy` | Simple push notification service using HTTP PUT/POST (UnifiedPush compatible) | `binwiederhier/ntfy:v2.28.0` |
| Outline | `outline` | Team knowledge base and wiki with a fast, beautiful editor | `outlinewiki/outline:1.9` |
| Paperless-ngx | `paperless-ngx` | Document management system that transforms physical documents into searchable archive | `ghcr.io/paperless-ngx/paperless-ngx:latest` |
| pgAdmin | `pgadmin` | Web-based PostgreSQL management and administration tool | `dpage/pgadmin4:8` |
| Rocket.Chat | `rocketchat` | Open-source team communication platform with channels, DMs, and video | `rocketchat/rocket.chat:8.7.0` |
| SearXNG | `searxng` | Privacy-respecting metasearch engine aggregating 70+ sources | `searxng/searxng:latest` |
| Stirling-PDF | `stirling-pdf` | Self-hosted PDF manipulation tool with merge, split, convert, and OCR | `frooodle/s-pdf:2.14.3` |
| Tandoor Recipes | `tandoor` | Recipe management and meal planning application | `vabene1111/recipes:2.6.13` |
| Trilium Notes | `trilium` | Hierarchical note-taking application with rich text, relations, and scripting | `triliumnext/notes:v0.95.0` |
| Typebot | `typebot` | Open-source chatbot builder with drag-and-drop visual editor | `baptistearno/typebot-builder:2` |
| Vikunja | `vikunja` | Open-source to-do and project management app (Todoist alternative) | `vikunja/vikunja:2.6.0` |
| Wiki.js | `wikijs` | Modern wiki engine with powerful features and beautiful interface | `ghcr.io/requarks/wiki:2` |
