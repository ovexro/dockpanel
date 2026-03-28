# DockPanel Feature Manifest

> **Last verified**: 2026-03-28 | **Version**: v2.6.7 | **Total**: 60+ major features, ~280 capabilities
>
> This file is the single source of truth for what DockPanel offers.
> Update it whenever features are added, changed, or removed.

## Hosting

| Feature | Description | Backend | Agent | Frontend | DB Tables |
|---------|-------------|---------|-------|----------|-----------|
| **Sites** | Static, PHP (8.1-8.4), Node.js, Python sites with nginx. Domain rename, auto-firewall for proxy ports, Laravel auto-migrations | `routes/sites.rs`, `ssl.rs`, `files.rs`, `deploy.rs` | `nginx.rs`, `php.rs`, `ssl.rs`, `files.rs`, `cms.rs` | `Sites.tsx`, `SiteDetail.tsx`, `Files.tsx` | `sites` |
| **Databases** | MySQL/PostgreSQL via Docker, SQL browser, schema viewer | `routes/databases.rs` | `database.rs` | `Databases.tsx` | `databases` |
| **Backups** | Scheduled backups, S3/SFTP/B2/GCS remote storage, one-click restore | `routes/backups.rs`, `backup_schedules.rs`, `backup_destinations.rs` | `backups.rs`, `remote_backup.rs` | `Backups.tsx` | `backups`, `backup_schedules`, `backup_destinations` |
| **Backup Orchestrator** | DB/volume/site backups, AES-256 encryption, restore verification, policies, health dashboard, auto-verifier | `routes/backup_orchestrator.rs` | `database_backup.rs`, `volume_backup.rs`, `encryption.rs`, `backup_verify.rs` | `BackupOrchestrator.tsx` | `backup_policies`, `database_backups`, `volume_backups`, `backup_verifications` |
| **Webhook Gateway** | Receive, inspect, route, replay webhooks. HMAC-SHA256/SHA1 verification, JSON path filtering, retry with backoff, delivery logging | `routes/webhook_gateway.rs` | — | `WebhookGateway.tsx` | `webhook_endpoints`, `webhook_deliveries`, `webhook_routes` |
| **Secrets Manager** | AES-256-GCM encrypted vaults, version history, auto-inject to .env, masked API, pull for CLI | `routes/secrets.rs`, `services/secrets_crypto.rs` | — | `SecretsManager.tsx` | `secret_vaults`, `secrets`, `secret_versions` |
| **Incident Management** | Incident lifecycle (investigating→resolved→postmortem), timeline updates, severity, affected components, postmortem | `routes/incidents.rs` | — | `IncidentManagement.tsx` | `managed_incidents`, `incident_updates`, `managed_incident_components` |
| **Public Status Page** | Customizable status page with component groups, incident history, subscriber notifications, overall status | `routes/incidents.rs` | — | `PublicStatusPage.tsx` | `status_page_config`, `status_page_components`, `status_page_subscribers` |
| **Cron Jobs** | Cron scheduling with manual execution and history | `routes/crons.rs` | `crons.rs` | `Crons.tsx` | (via agent crontab) |
| **Docker Apps** | 151 templates across 14 categories, Compose stacks, container lifecycle, registry, image tag change, live resource limits, GPU passthrough | `routes/docker_apps.rs`, `stacks.rs` | `docker_apps.rs` | `Apps.tsx` | `docker_stacks` |
| **Git Deploy** | Push-to-deploy, blue-green, Nixpacks (30+ langs), preview envs, one-time scheduled deploys | `routes/git_deploys.rs` | `git_build.rs` | `GitDeploys.tsx` | `git_deploys`, `git_deploy_history`, `git_previews` |
| **WordPress Toolkit** | Multi-site dashboard, vuln scanning (14 known), hardening (7 checks), bulk updates | `routes/wordpress.rs` | `wordpress.rs`, `wp_vulnerability.rs` | `WordPressToolkit.tsx`, `WordPress.tsx` | `wp_vuln_scans`, `wp_hardening` |
| **Migration Wizard** | Import from cPanel/HestiaCP — sites, databases, mail. Plesk (beta) | `routes/migration.rs` | `migration.rs` | `Migration.tsx` | `migrations` |
| **Staging** | Clone site to staging, sync to/from production | `routes/staging.rs` | `staging.rs` | (in SiteDetail) | `sites.parent_site_id` |

## Operations

| Feature | Description | Backend | Agent | Frontend | DB Tables |
|---------|-------------|---------|-------|----------|-----------|
| **DNS** | Cloudflare + PowerDNS, zone templates, propagation, DNSSEC | `routes/dns.rs` | — | `Dns.tsx` | `dns_zones` |
| **Mail** | Postfix+Dovecot+OpenDKIM, Rspamd, Roundcube, SMTP relay, TLS | `routes/mail.rs` | `mail.rs`, `smtp.rs` | `Mail.tsx` | `mail_domains`, `mail_accounts`, `mail_aliases` |
| **Monitoring** | HTTP/TCP/ping uptime checks, SLA, public status page, PagerDuty | `routes/monitors.rs` | — | `Monitoring.tsx` | `monitors`, `monitor_checks`, `incidents` |
| **Logs** | Site/system/Docker/service logs, search, stream, stats, truncate | `routes/logs.rs`, `system_logs.rs` | `logs.rs` | `Logs.tsx` | `system_logs`, `activity_logs` |
| **Terminal** | Browser SSH via WebSocket, tabs, themes, sharing, recording | `routes/terminal.rs` | `terminal.rs` | `Terminal.tsx` | — |

## Security

| Feature | Description | Backend | Agent | Frontend |
|---------|-------------|---------|-------|----------|
| **Security Dashboard** | Overview, compliance report, login audit | `routes/security.rs` | `security.rs` | `Security.tsx` |
| **Firewall** | UFW rule management | `routes/security.rs` | `security.rs` | (in Security) |
| **Fail2Ban** | Jail management, ban/unban, panel jail | `routes/security.rs` | `security.rs` | (in Security) |
| **SSH Hardening** | Disable password/root, change port, key management | `routes/security.rs` | `security.rs` | (in Security) |
| **Security Scanning** | Automated audits with posture scoring | `routes/security_scans.rs` | — | (in Security) |
| **Credential Encryption** | All stored credentials encrypted at rest with AES-256-GCM | `services/credential_crypto.rs` | — | — |
| **Content Security Policy** | CSP headers on frontend nginx config | — | — | `nginx.conf` |
| **Safe Command Execution** | `env_clear()` on all child processes to prevent environment hijacking | — | `safe_command.rs` | — |

## System

| Feature | Description | Backend | Agent | Frontend | Background Service |
|---------|-------------|---------|-------|----------|--------------------|
| **Dashboard** | Live CPU/RAM/disk/network, Docker summary, health score | `routes/dashboard.rs` | — | `Dashboard.tsx` | — |
| **Metrics** | Historical charts (24h), WebSocket live data | `routes/metrics.rs`, `ws_metrics.rs` | `system.rs` | (in Dashboard) | `metrics_collector.rs` |
| **Alerts** | CPU/mem/disk thresholds, SSL expiry, service health | `routes/alerts.rs` | — | (in Monitoring) | `alert_engine.rs` |
| **Auto-Healing** | Restart crashed services, clean logs, renew SSL | — | — | (in Settings) | `auto_healer.rs` |
| **Diagnostics** | 6 check categories, one-click fixes | `routes/system.rs` | `diagnostics.rs` | (in Security) | — |
| **Traefik** | Alternative reverse proxy, auto-SSL, Docker discovery | `routes/system.rs` | `traefik.rs` | (in Settings) | — |
| **Service Installers** | PHP, Certbot, UFW, Fail2Ban, PowerDNS — one-click | `routes/system.rs` | `service_installer.rs` | (in Settings) | — |
| **Service Uninstall** | Uninstall routes for all 10 services (PHP, Certbot, UFW, Fail2Ban, PowerDNS, Redis, Node.js, Composer, mail server, PHP versions) | `routes/system.rs` | `service_installer.rs` | (in Settings) | — |
| **SSL Renew/Delete** | Force-renew and delete SSL certificates via certbot | `routes/ssl.rs` | `ssl.rs` | `Certificates.tsx` | — |
| **User Suspend/Reset** | Suspend/unsuspend users with session invalidation, admin password reset | `routes/users.rs` | — | (in Settings) | `users` |
| **System Updates** | OS package updates, auto-updates toggle, reboot | `routes/system.rs` | `updates.rs` | (in Settings) | — |

## Admin

| Feature | Description | Backend | Frontend | DB Tables |
|---------|-------------|---------|----------|-----------|
| **Multi-Server** | Manage remote servers via HTTPS agents | `routes/servers.rs` | `Servers.tsx` | `servers` |
| **Reseller Accounts** | Admin→Reseller→User hierarchy, quotas, server allocation | `routes/resellers.rs`, `reseller_dashboard.rs` | `ResellerDashboard.tsx`, `ResellerUsers.tsx` | `reseller_profiles`, `reseller_servers` |
| **White-Label** | Per-reseller logo, panel name, accent color, hide branding | `routes/settings.rs` (branding endpoint) | (in CommandLayout, Login) | `reseller_profiles` |
| **Users** | CRUD, role assignment (admin/reseller/user) | `routes/users.rs` | (in Settings) | `users` |
| **Teams** | Create teams, invite members, assign roles | `routes/teams.rs` | (in Settings) | `teams`, `team_members`, `team_invites` |
| **API Keys** | Programmatic access tokens with rotation | `routes/api_keys.rs` | (in Settings) | `api_keys` |
| **Extensions** | Webhook integrations, HMAC-signed events, scoped API keys | `routes/extensions.rs` | `Extensions.tsx` | `extensions`, `extension_events` |
| **Activity Log** | Full audit trail of all mutations | `routes/activity.rs` | (in Logs) | `activity_logs` |
| **Settings** | SMTP, branding, retention, webhooks, export/import | `routes/settings.rs` | `Settings.tsx` | `settings` |

## Auth

| Feature | Description | Backend | Frontend |
|---------|-------------|---------|----------|
| **Login/Register** | Email+password auth, JWT sessions, email verification | `routes/auth.rs` | `Login.tsx`, `Register.tsx` |
| **2FA/TOTP** | QR setup, TOTP verify, 10 recovery codes, enforcement | `routes/auth.rs` | (in Login, Settings) |
| **Passkey/WebAuthn** | Passwordless login, biometric/security key auth, max 10 per user | `routes/passkeys.rs` | (in Login, Settings) |
| **OAuth/SSO** | Google, GitHub, GitLab OAuth 2.0 with auto-create | `routes/oauth.rs` | (in Login) |
| **Branding** | Public `/api/branding` with panel name, logo, colors, OAuth providers | `routes/settings.rs` | `BrandingContext.tsx` |

## Background Services (11 supervised)

| Service | Interval | Purpose |
|---------|----------|---------|
| `backup_scheduler` | per schedule | Execute scheduled backups |
| `server_monitor` | 60s | Check server health, update status |
| `uptime_monitor` | per monitor | HTTP/TCP uptime checks |
| `security_scanner` | daily | Automated security audits |
| `alert_engine` | 60s | Evaluate alert rules, fire notifications |
| `auto_healer` | 120s | Auto-fix crashed services, full disk, expiring SSL |
| `metrics_collector` | 60s | Store CPU/mem/disk history, 7-day retention |
| `deploy_scheduler` | 60s | Trigger cron-scheduled Git deploys |
| `preview_cleanup` | 300s | Remove expired preview environments |
| `backup_policy_executor` | per policy | Execute backup policies (retention, scheduling) |
| `backup_verifier` | per policy | Verify backup integrity after creation |

## CLI Commands

| Command | Description |
|---------|-------------|
| `dockpanel status` | Server status (CPU, memory, disk, uptime) |
| `dockpanel sites` | List all nginx sites |
| `dockpanel db` | List databases |
| `dockpanel apps` | List Docker apps |
| `dockpanel diagnose` | Run smart diagnostics |
| `dockpanel export -o config.yml` | Export server config as YAML |
| `dockpanel apply config.yml` | Apply IaC config (with --dry-run) |
| `dockpanel services` | Check service health |
| `dockpanel ssl status <domain>` | SSL certificate status |
| `dockpanel security` | Security overview |
| `dockpanel security scan` | Run security scan |
| `dockpanel logs -d <domain>` | View site logs |
| `dockpanel top` | Top processes by CPU |

## Performance & Caching

| Feature | Description | Backend | Agent | Frontend |
|---------|-------------|---------|-------|----------|
| **FastCGI Cache** | Per-site nginx FastCGI cache toggle + purge, smart bypass for logged-in users | `routes/sites.rs` | nginx templates | `SiteDetail.tsx` |
| **Redis Object Cache** | Per-site isolated Redis DB, WP auto-config via wp-cli | `routes/sites.rs` | `redis.rs` | `SiteDetail.tsx` |
| **Image Optimization** | Server-side WebP/AVIF conversion per site | `routes/sites.rs` | `image_optimization.rs` | `SiteDetail.tsx` |
| **CDN Integration** | BunnyCDN + Cloudflare CDN zones, cache purge, bandwidth stats | `routes/cdn.rs` | — | `Cdn.tsx` |
| **Auto-Optimization** | PHP-FPM worker analysis, nginx workers vs CPUs, memory/disk recommendations | proxied to agent | `recommendations.rs` | (via Settings) |

## Security (Advanced)

| Feature | Description | Backend | Agent | Frontend |
|---------|-------------|---------|-------|----------|
| **WAF** | ModSecurity3 + OWASP CRS v4, per-site detection/prevention mode, event viewer | `routes/sites.rs` | `waf.rs`, nginx integration | `SiteDetail.tsx` |
| **CSP Headers** | Per-site Content Security Policy editor with common presets | `routes/sites.rs` | nginx templates | `SiteDetail.tsx` |
| **Bot Protection** | Per-site bot rate limiting (off/basic/strict modes) | `routes/sites.rs` | nginx templates | `SiteDetail.tsx` |
| **Container Isolation** | Per-user container policies (max containers, memory, CPU, network isolation) | `routes/docker_apps.rs` | user labels | `ContainerPolicies.tsx` |

## Container Lifecycle

| Feature | Description | Backend | Agent | Frontend |
|---------|-------------|---------|-------|----------|
| **Auto-Sleep** | Stop idle containers after configurable inactivity, manual sleep/wake | `routes/docker_apps.rs`, `auto_healer.rs` | stop/start | `Apps.tsx` |
| **Auto-Update Detection** | Registry digest comparison, update badges, one-click update | `routes/docker_apps.rs` | `docker_apps.rs` | `Apps.tsx` |
| **GPU Passthrough** | NVIDIA Container Toolkit detection, --gpus flag on deploy | `routes/docker_apps.rs` | `docker_apps.rs` | `Apps.tsx` |
| **Horizontal Auto-Scaling** | Rule-based CPU thresholds, min/max replicas, cooldown | `routes/iac.rs` | — | (via Integrations) |

## Integrations (Advanced)

| Feature | Description | Backend | Frontend |
|---------|-------------|---------|----------|
| **Cloudflare Settings** | Zone security level, SSL mode, dev mode, cache purge | `routes/dns.rs` | `Dns.tsx` |
| **Cloudflare Tunnel** | Install cloudflared, token-based config, systemd service | `routes/system.rs` | `Settings.tsx` |
| **Wildcard SSL** | DNS-01 challenge via Cloudflare API, multi-part TLD support | `routes/sites.rs` | `SiteDetail.tsx` |
| **WHMCS Billing** | Webhook provisioning/suspension/termination, auto-create users | `routes/whmcs.rs` | `Integrations.tsx` |
| **Terraform/Pulumi** | IaC token management, resource listing API (sites, databases) | `routes/iac.rs` | `Integrations.tsx` |
| **App Migration** | Migrate containers between servers, progress tracking | `routes/whmcs.rs` | `Integrations.tsx` |

## Database (Advanced)

| Feature | Description | Backend | Frontend |
|---------|-------------|---------|----------|
| **Visual Schema Browser** | Tables, columns, indexes, foreign key relationships in one view | `routes/databases.rs` | `Databases.tsx` |
| **Point-in-Time Recovery** | WAL archiving (PostgreSQL), binlog retention (MySQL), restore to timestamp | `routes/databases.rs` | `Databases.tsx` |

## Verified Metrics

| Metric | Value | Verified |
|--------|-------|----------|
| Agent binary | 20 MB | 2026-03-23 |
| API binary | 19 MB | 2026-03-23 |
| CLI binary | 1.8 MB | 2026-03-23 |
| Agent RAM (RSS) | ~30 MB | 2026-03-19 |
| API RAM (RSS) | ~27 MB | 2026-03-19 |
| Total RAM | ~57 MB | 2026-03-19 |
| App templates | 151 (14 categories) | 2026-03-28 |
| API endpoints | 711 (456 backend + 255 agent) | 2026-03-28 |
| E2E tests | 89 | 2026-03-28 |
| Frontend pages | 50 | 2026-03-28 |
| DB migrations | 80 | 2026-03-28 |
| Background services | 11 | 2026-03-22 |
