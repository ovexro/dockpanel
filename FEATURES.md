# DockPanel Feature Manifest

> **Last verified**: 2026-03-20 | **Version**: v2.0.2 | **Total**: 41 major features, ~166 capabilities
>
> This file is the single source of truth for what DockPanel offers.
> Update it whenever features are added, changed, or removed.

## Hosting

| Feature | Description | Backend | Agent | Frontend | DB Tables |
|---------|-------------|---------|-------|----------|-----------|
| **Sites** | Static, PHP (7.4-8.3), Node.js, Python sites with nginx | `routes/sites.rs`, `ssl.rs`, `files.rs` | `nginx.rs`, `php.rs`, `ssl.rs`, `files.rs`, `cms.rs` | `Sites.tsx`, `SiteDetail.tsx`, `Files.tsx` | `sites` |
| **Databases** | MySQL/PostgreSQL via Docker, SQL browser, schema viewer | `routes/databases.rs` | `database.rs` | `Databases.tsx` | `databases` |
| **Backups** | Scheduled backups, S3/SFTP remote storage, one-click restore | `routes/backups.rs`, `backup_schedules.rs`, `backup_destinations.rs` | `backups.rs`, `remote_backup.rs` | `Backups.tsx` | `backups`, `backup_schedules`, `backup_destinations` |
| **Cron Jobs** | Cron scheduling with manual execution and history | `routes/crons.rs` | `crons.rs` | `Crons.tsx` | (via agent crontab) |
| **Docker Apps** | 54 templates, Compose stacks, container lifecycle, registry | `routes/docker_apps.rs`, `stacks.rs` | `docker_apps.rs` | `Apps.tsx` | `docker_stacks` |
| **Git Deploy** | Push-to-deploy, blue-green, Nixpacks (30+ langs), preview envs | `routes/git_deploys.rs` | `git_build.rs` | `GitDeploys.tsx` | `git_deploys`, `git_deploy_history`, `git_previews` |
| **WordPress Toolkit** | Multi-site dashboard, vuln scanning (14 known), hardening (7 checks), bulk updates | `routes/wordpress.rs` | `wordpress.rs`, `wp_vulnerability.rs` | `WordPressToolkit.tsx`, `WordPress.tsx` | `wp_vuln_scans`, `wp_hardening` |
| **Migration Wizard** | Import from cPanel/Plesk/HestiaCP — sites, databases, mail | `routes/migration.rs` | `migration.rs` | `Migration.tsx` | `migrations` |
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
| **OAuth/SSO** | Google, GitHub, GitLab OAuth 2.0 with auto-create | `routes/oauth.rs` | (in Login) |
| **Branding** | Public `/api/branding` with panel name, logo, colors, OAuth providers | `routes/settings.rs` | `BrandingContext.tsx` |

## Background Services (9 supervised)

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

## Verified Metrics

| Metric | Value | Verified |
|--------|-------|----------|
| Agent binary | 20 MB | 2026-03-19 |
| API binary | 14 MB | 2026-03-19 |
| CLI binary | 1.7 MB | 2026-03-19 |
| Agent RAM (RSS) | ~30 MB | 2026-03-19 |
| API RAM (RSS) | ~27 MB | 2026-03-19 |
| Total RAM | ~57 MB | 2026-03-19 |
| App templates | 54 | 2026-03-19 |
| API endpoints | 50+ tested | 2026-03-19 |
| Frontend pages | 19 | 2026-03-19 |
| DB tables | 45+ | 2026-03-19 |
| Background services | 9 | 2026-03-19 |
