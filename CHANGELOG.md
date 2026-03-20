# Changelog

All notable changes to DockPanel will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/).

## [2.0.3] - 2026-03-20

### Fixed — Comprehensive Audit (57 findings across 7 audit types)

#### Critical
- **Migration ordering**: `whitelabel_oauth` migration was running before `reseller_system` (ALTERing a table before it existed). Renumbered to `20260320050000`.
- **OAuth bypasses 2FA**: OAuth login issued full session without checking `totp_enabled`. Now redirects to 2FA challenge when enabled.
- **Setup script missing build tools**: Fresh VPS source builds failed — added `build-essential cmake pkg-config` installation.
- **No swap on x86_64 low-RAM VPS**: Swap creation only triggered on ARM. Now applies to all architectures when building from source.
- **install-agent.sh wrong env vars**: Remote agents never entered phone-home mode (`AGENT_TOKEN` vs `DOCKPANEL_SERVER_TOKEN`). Fixed to write both sets.
- **Systemd services never updated during upgrade**: `update.sh` now rewrites service files with current `ReadWritePaths` and hardening.
- **Required directories not created during upgrade**: `update.sh` now creates `/etc/postfix`, `/var/vmail`, and other directories needed by new features.

#### High
- **UFW blocks panel port 8443**: IP-based installs now open the configured panel port in UFW.
- **ExecStartPost hardcodes www-data**: Agent socket `chgrp` now auto-detects nginx group (`www-data` or `nginx`).
- **`read` prompt broken in curl-pipe-bash**: Domain prompt now reads from `/dev/tty` when stdin is piped.
- **Frontend path mismatch after upgrade**: `update.sh` now fixes nginx root path when switching between source and release modes.
- **config.rs default LISTEN_ADDR was 0.0.0.0:3000**: Changed to `127.0.0.1:3080` to match all scripts and nginx config.
- **uninstall.sh incomplete cleanup**: Now removes CLI binary, tmpfiles.d, crontab entries, `/var/www/acme`, `/var/lib/dockpanel`.
- **Stacks INSERT missing server_id**: Docker Compose stacks now include `server_id` in INSERT.
- **Staging site INSERT missing server_id**: Staging environments now inherit parent site's server_id.
- **No domain uniqueness across sites + git_deploys**: Cross-table domain conflict check prevents silent hijacking.
- **Blue-green deploy dropped resource limits**: New container now inherits `memory`/`cpu_period`/`cpu_quota` from config.
- **Git preview port has no unique constraint**: Added `UNIQUE INDEX` on `git_previews(host_port)`.
- **Site proxy_port has no unique constraint**: Added partial `UNIQUE INDEX` on `sites(proxy_port)`.
- **No terminal session limit**: Added `AtomicU32` counter with max 20 concurrent PTY sessions.

### Added
- **CONTRIBUTING.md**: Development setup, architecture overview, code style, PR process.
- **GitHub issue templates**: Bug report and feature request forms with structured fields.
- **GitHub PR template**: Checklist for builds, tests, and changelog.

### Changed
- **README.md**: Added badges (license, release, build), doc links, contributing section, phone-home disclosure.
- **.gitignore**: Added SSL material, database file patterns.

### Fixed — Adversarial Security Pentest
- **Rate limit bypass via X-Forwarded-For**: Login rate limiter now uses `X-Real-IP` (set by nginx, not forgeable) instead of `X-Forwarded-For`.
- **SSRF filter bypass in extensions**: Webhook URL validation replaced string-matching with DNS resolution + `is_loopback()`/`is_private()`/`is_link_local()` checks. Blocks hex IPs, decimal IPs, IPv6 loopback, DNS-to-localhost, cloud metadata.
- **Nginx version disclosure**: Added `server_tokens off` to nginx config.

### Fixed — Disaster Recovery
- **Agent fails after every reboot**: Removed `ReadWritePaths` and `PrivateTmp=yes` from agent systemd service (redundant with `ProtectSystem=no`, and caused NAMESPACE errors for missing dirs). Added `ExecStartPre` to create `/run/dockpanel`.
- **Health endpoint false "ok"**: `/api/health` now checks DB connectivity, returns `"degraded"` when database is unreachable.
- **StartLimitIntervalSec in wrong section**: Moved from `[Service]` to `[Unit]` in all 3 scripts.

### Fixed — UX Walkthrough (fresh VPS testing)
- **Secure cookie over HTTP**: Login cookie conditionally sets `Secure` flag based on `BASE_URL` scheme. `SameSite` changed from `Strict` to `Lax` (Strict blocked OAuth redirects).
- **Site document root not created**: Agent now creates `/var/www/{domain}/public/` with a default `index.html` during site provisioning.
- **PHP site without PHP check**: Agent validates PHP-FPM socket exists before writing PHP nginx config. Returns clear error with install instructions.

### Fixed — Supply Chain
- **`serde_yaml` archived**: Replaced with `serde_yml` in agent and CLI (serde_yaml maintainer archived the crate in 2024).
- **MailHog abandoned**: Replaced `mailhog/mailhog` template with `axllent/mailpit` (MailHog last updated 2020).
- **Stale build templates**: Updated `rust:1.82-slim` → `rust:1.94-slim`, `golang:1.23-alpine` → `golang:1.24-alpine`.

### Fixed — Code Quality
- **Cloudflare auth header deduplication**: 5 inline blocks → shared `helpers::cf_headers()`.
- **Server IP detection deduplication**: 6 inline blocks → shared `helpers::detect_public_ip()`.
- **Agent semaphore split**: Long-running ops (Docker builds) use separate 5-permit semaphore, quick requests keep 20.
- **Extension webhook rate limiting**: Max 20 concurrent deliveries with atomic counter.
- **DB pool acquire timeout**: 5-second timeout prevents indefinite blocking.
- **Uptime monitor N+1 query**: Maintenance window check batched into single query.

## [2.0.2] - 2026-03-20

### Changed
- **Version alignment**: All Cargo.toml and package.json versions bumped to 2.0.2 (were 0.1.0/1.0.0). API health endpoint and CLI --version now report correct version.
- **Binary size claims**: Marketing site, README, and FAQ updated from "~20MB" (agent-only) to "~35MB" (total of agent + API + CLI) for honest comparison.
- **Template count**: FAQ corrected from 53 to 54 app templates.
- **OS support**: Hero section now includes Rocky Linux 9+ alongside other supported distros.

### Fixed
- **install-agent.sh binary naming**: Was downloading `dockpanel-agent-x86_64` / `dockpanel-agent-aarch64` but GitHub Releases publishes `dockpanel-agent-linux-amd64` / `dockpanel-agent-linux-arm64`. Fixed to match release naming.
- **install-agent.sh apt-get hardcoding**: Now detects package manager (apt/dnf/yum) instead of hardcoding apt-get. CentOS, Rocky, Fedora, and Amazon Linux now supported for remote agent installs.
- **install-agent.sh server-id persistence**: `--server-id` was accepted but never written to config. Now persisted to `/etc/dockpanel/api.env` as `SERVER_ID`.
- **install-agent.sh tmpfiles.d**: Added `/run/dockpanel` tmpfiles.d entry so socket directory survives reboots.
- **install-agent.sh systemd hardening**: Remote agent service now matches local agent hardening (MemoryMax, LimitNOFILE, PrivateTmp, ProtectKernelLogs/Modules).
- **update.sh pre-built binary path**: Added `INSTALL_FROM_RELEASE=1` support so ARM users who installed via release binaries can update without Rust toolchain.
- **update.sh redundant health check**: Removed duplicate wait-for-health loop after rollback-capable check.

## [2.0.0] - 2026-03-19

### Added — High-Impact Features
- **Multi-Server Management**: Manage unlimited remote servers from one panel. AgentRegistry dispatches to local (Unix socket) or remote (HTTPS) agents. Server selector in sidebar, test connection, install script for remote agents. ServerScope extractor with user ownership verification on every request.
- **Reseller / Multi-Tenant Accounts**: Admin → Reseller → User hierarchy. Reseller quotas (max users/sites/databases), server allocation, per-reseller branding (logo, colors, hide DockPanel name). Quota enforcement on site/database creation with counter sync.
- **Nixpacks Auto-Detection**: Build any app without a Dockerfile using Nixpacks (30+ languages). Dynamic version resolution from GitHub releases. Deploy pipeline: try Nixpacks → fall back to auto-detect (6 langs) → docker build. Build method tracked per deploy.
- **Preview Environments**: TTL-based auto-cleanup of preview deployments. Branch deletion webhook auto-removes previews. Configurable preview_ttl_hours per deploy. Background cleanup service (5-minute interval).
- **Migration Wizard**: Import sites, databases, and email from cPanel, Plesk, or HestiaCP. 4-step wizard: select source → analyze backup (auto-detect domains, DBs, mail) → select items → SSE-streamed import. cPanel full parser, Plesk/HestiaCP beta stubs.
- **WordPress Toolkit**: Multi-site WP dashboard with parallel detection. Vulnerability scanning against 14 known exploited plugins. Security hardening (7 checks, 6 auto-fixable via wp-cli). Bulk update plugins/themes/core across selected sites.
- **White-Label Branding**: Public `/api/branding` endpoint. Per-reseller logo_url, accent_color, panel_name, hide_branding. BrandingContext provider applies to sidebar + login page. Dynamic accent color via CSS variable.
- **OAuth / SSO Login**: Google, GitHub, GitLab via OAuth 2.0 authorization code flow. CSRF state tokens (10-minute expiry). GitHub private email fallback. Auto-create users on first OAuth login (configurable). Provider-colored login buttons.
- **Traefik Reverse Proxy**: Alternative to nginx for Docker app routing. Traefik v3.3 as Docker container with auto-SSL (Let's Encrypt ACME). File-based dynamic route configs with auto-watch. Install/uninstall/status management. Settings toggle in admin panel.
- **Plugin / Extension API**: Webhook-based integrations with HMAC-SHA256 signed event delivery. Extension CRUD with `dpx_` API keys and `whsec_` webhook secrets. Event types: site/backup/deploy/app/auth/ssl. Delivery log with status tracking. Secret rotation. SSRF protection on webhook URLs.

### Added — Feature Gap Analysis Enhancements
- **SQL Browser**: Built-in query editor for PostgreSQL and MariaDB with schema viewer
- **Node.js + Python Site Runtimes**: Managed systemd services with auto-port allocation
- **Docker Compose Stacks**: Full stack lifecycle (deploy, start, stop, restart, update, remove)
- **Blue-Green Zero-Downtime Deploy**: Docker app updates with traffic swap and rollback
- **Git Push-to-Deploy Pipeline**: Clone → build → deploy with webhook triggers and rollback
- **Container Health Checks**: Docker health status (healthy/unhealthy/starting) in Apps view
- **Container Logs Viewer**: Search, filter, auto-refresh, color-coded log levels
- **Command Palette (Ctrl+K)**: Global search across all panel pages
- **One-Click App Updates**: Pull latest image, preserve config, recreate container
- **34 App Templates**: Database, CMS, monitoring, analytics, tools, dev, storage, media, networking, security
- **Getting Started Wizard**: 5-step onboarding checklist

### Changed
- **Architecture**: Single-agent → multi-agent (AgentRegistry, AgentHandle enum, RemoteAgentClient)
- **Auth**: Added ResellerUser extractor, ServerScope with ownership verification
- **Database**: 8 new tables, server_id FK on all resource tables, reseller profiles, extensions, migrations
- **Frontend**: BrandingContext, ServerContext providers. 8 new pages (Servers, ResellerDashboard, ResellerUsers, Migration, WordPressToolkit, Extensions, plus per-site WP and Git Deploy enhancements)
- **Rust Edition**: 2024 (Rust 1.94)

### Security
- ServerScope verifies `server.user_id == claims.sub` on every request (prevents cross-user server access)
- OAuth: SameSite=Strict cookies, error callback handling, empty oauth_id validation, no auto-link to password accounts
- Extension API: SSRF protection (blocks private IPs, metadata endpoints), HMAC bypass fix, webhook secret rotation
- Migration wizard: command injection fix (direct docker args), path traversal validation, TAR --no-same-owner
- WordPress: domain path validation, targeted chown (not recursive), site path fallback
- Nixpacks: build_context path traversal validation, dynamic version resolution
- Traefik: ACME directory permissions (0700), network cleanup on uninstall
- Branding: logo_url validated (HTTP(S) only), accent_color validated (hex/rgb/hsl only)
- Reseller: quota enforcement wired up, server isolation for reseller users, counter sync on create/delete
- Preview: TTL reset on redeploy, MAKE_INTERVAL for PostgreSQL safety, cleanup error logging

### Fixed
- 100+ findings from 9 comprehensive audits across all features
- server_id filtering added to git_deploys, stacks, databases, dashboard, alerts list endpoints
- Compose deployments now correctly set build_method='compose'
- Preview cleanup query uses MAKE_INTERVAL instead of string concat
- fire_event() wired into site/backup/app handlers (was dead code)
- Traefik Docker app integration (was install-only with no functional routing)
- Frontend SecurityItem type mismatch in WordPress Toolkit fixed
- OAuth parameter mismatch (doc_root vs source_dir) in migration wizard fixed

## [1.1.0] - 2026-03-15

### Added
- **Email Management**: Full mail server with one-click install (Postfix + Dovecot + OpenDKIM). Domains, mailboxes, aliases, catch-all, quotas, autoresponders, DKIM signing, DNS helper (MX/SPF/DKIM/DMARC), mail queue viewer
- **PowerDNS**: Self-hosted DNS alongside Cloudflare. Provider selector, zone creation, record CRUD, setup guide
- **One-Click CMS Install**: WordPress, Drupal, Joomla — create site + database + install + SSL in one click from Sites page
- **Historical Charts**: SVG sparkline charts (CPU/Memory/Disk 24h) with background metrics collector (60s interval, 7-day retention)
- **Light Theme**: CSS variable overrides, sun/moon toggle in sidebar footer, localStorage persistence
- **One-Click Service Installers**: PHP-FPM, Certbot, UFW, Fail2Ban — install from Settings page
- **Smart Port Opener**: Port recognition (28+ ports), safety categories (safe/caution/blocked), quick presets (Web/Mail/Database)
- **SSH Key Management**: List/add/remove authorized keys with SHA256 fingerprints
- **Auto-Updates**: Toggle for unattended-upgrades security patches
- **Panel IP Whitelist**: Restrict panel access to specific IPs
- **Auto-SSL**: Automatic Let's Encrypt provisioning on site creation
- **Webhook Testing**: Test Slack/Discord webhooks from Settings
- **File Upload**: Base64 binary upload with path traversal protection
- **Webmail Template**: Roundcube one-click deploy from Docker Apps
- **Spam Filter Template**: Rspamd one-click deploy from Docker Apps
- **BUILD STABLE Badge**: Build status indicator in sidebar footer

### Changed
- **Harmonized Color Palette**: Green/amber/red at identical saturation/lightness (anchored at #22c55e). Custom `warn-*` and `danger-*` CSS scales. Zero stale emerald/amber/yellow references
- **Dashboard Redesign**: Bar metrics with centered text-5xl numbers (replaced ring gauges), neutral white numbers + gray progress bars (color only for warnings/critical), system info grid (replaced neofetch style)
- **Sidebar Overhaul**: Flat nav (no progressive disclosure), white active state with blinking _ cursor, 19px icons, spacing-only groups
- **Terminal Frame**: Unified bordered container (header + canvas in single frame)
- **Mobile Responsive**: Card layouts for Activity, Users, DNS records. Logs toolbar wrapping. Monitors polish
- **Contrast**: All text-dark-400 bumped to text-dark-300 globally (36 instances, 14 files) for WCAG compliance
- **Animations**: Page fade-up, stagger children, counting numbers, typewriter welcome, hover-lift. Respects prefers-reduced-motion
- **Login Page**: Logo updated to match sidebar brand
- **Apps/Sites Separation**: WordPress/Drupal/Joomla moved from Docker Apps to native PHP in Sites. 32 Docker templates remain for services and tools
- **502 Error UX**: "Agent offline" message with `systemctl restart` command instead of cryptic "Request failed (502)"
- **Security Score**: Prominence increase, singular/plural grammar fix
- **Apps Empty State**: Error message with icon when templates fail to load

### Fixed
- **Diagnostics**: Agent nginx -t check distinguishes [warn] from [emerg]/[error] — no false critical on cosmetic warnings
- **Document Root False Positives**: Changed ProtectHome=yes → read-only so agent can see /home/* directories
- **Agent Socket Persistence**: Added tmpfiles.d config + /run/nginx.pid to ReadWritePaths
- **Agent Permissions**: NoNewPrivileges=no, ReadWritePaths for mail/apt/etc paths — enables package installation
- **CUPS Disabled**: Removed unnecessary print service

### Security
- Setup script auto-installs UFW + Fail2Ban with default rules
- Smart firewall blocks dangerous ports (Telnet, NetBIOS, SMB, MSSQL)
- All cookie flags verified: HttpOnly, Secure, SameSite=Strict, Max-Age=7200

### Infrastructure
- Metrics collector background service (60s interval, 7-day retention)
- Mail config sync to Postfix/Dovecot via atomic file writes
- DKIM key generation via openssl RSA 2048-bit
- Setup script installs PHP, Certbot, UFW, Fail2Ban out of the box

## [1.0.0] - 2026-03-14

### Added
- **Core Panel**: Site management (static, PHP, proxy), database management (PostgreSQL, MariaDB), SSL (Let's Encrypt), file manager, web terminal, backups
- **Docker Apps**: 50+ one-click templates across 10 categories + Docker Compose import
- **CLI**: Full command-line interface — status, sites, db, apps, ssl, backup, logs, security, diagnose, export, apply
- **Infrastructure as Code**: YAML export/import of server configuration
- **Smart Diagnostics**: Pattern-based issue detection across 6 categories with one-click fixes
- **Auto-Healing**: Automatic restart of crashed services, log cleanup on full disk, SSL renewal
- **Alerting System**: 5 alert types (CPU/memory/disk thresholds, server offline, SSL expiry, service health, backup failure) with email, Slack, Discord notifications
- **2FA/TOTP**: Full two-factor authentication with QR setup and recovery codes
- **Dashboard Intelligence**: Health score (0-100), top active issues, SSL expiry countdowns
- **Docker Resource Limits**: Memory and CPU limits on container deploy
- **Container Management**: Health checks, logs viewer, environment viewer, one-click updates
- **Security**: Firewall management, Fail2Ban, SSH hardening, security scanning with scoring
- **DNS Management**: Cloudflare DNS zone management with full record CRUD
- **Git Deploy**: Webhook-triggered deployments from Git repos
- **Staging Environments**: Create staging copies, sync from production, push to live
- **Uptime Monitoring**: HTTP checks with configurable intervals and incident tracking
- **Teams**: Multi-user access with roles and team-based permissions
- **Activity Log**: Full audit trail of all admin actions
- **Multi-Server**: Manage unlimited servers from a single dashboard
- **ARM64 Support**: Pre-built binaries for Raspberry Pi and ARM64 servers
- **Auto Reverse Proxy**: Domain + SSL auto-configured when deploying Docker apps
- **Command Palette**: Ctrl+K global search across all panel pages
- **Notification Channels**: Email toggle, Slack/Discord webhook configuration
- **Custom Nginx Directives**: Per-site textarea for advanced nginx config
- **Onboarding Wizard**: 5-step getting started checklist for new users

### Security
- JWT auth with HttpOnly cookies + Bearer header support
- Token blacklist for logout with periodic cleanup
- Argon2 password hashing
- Rate limiting on login, 2FA, webhooks, and agent endpoints
- Systemd hardening (NoNewPrivileges, ProtectSystem, MemoryMax)
- Nginx rate limiting (30r/s on API)
- 12 CHECK constraints on database status/type fields
- Atomic nginx config writes (tmp+rename)

### Infrastructure
- Supervised background tasks with auto-restart on panic
- Statement timeout on all database pool connections (30s)
- Agent request timeout (60s)
- DB backup cron (daily, 7-day retention)
- Docker prune cron (weekly)
