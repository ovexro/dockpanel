# Changelog

All notable changes to DockPanel will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/).

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
- **Docker Apps**: 34 one-click templates across 10 categories + Docker Compose import
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
