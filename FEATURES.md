# DockPanel Feature Manifest

> **Version**: v2.74.0 | **Total**: 60+ major features, ~285 capabilities
>
> This file is the single source of truth for what DockPanel offers.
> Update it whenever features are added, changed, or removed.
>
> The version stamp above and the app-template count below are checked against
> the source by `tests/docs-claims-pin-e2e.sh` on every commit. The feature rows
> themselves are maintained by hand, so treat them as a manifest of intent and
> [how DockPanel is tested](https://docs.dockpanel.dev/testing.html) as the
> record of what has actually been driven on a real server.

## Hosting

| Feature | Description | Backend | Agent | Frontend | DB Tables |
|---------|-------------|---------|-------|----------|-----------|
| **Sites** | Static, PHP (8.1–8.5), Node.js, Python sites with nginx. Domain rename, auto-firewall for proxy ports, Laravel auto-migrations | `routes/sites.rs`, `ssl.rs`, `files.rs`, `deploy.rs` | `nginx.rs`, `php.rs`, `ssl.rs`, `files.rs`, `cms.rs` | `Sites.tsx`, `SiteDetail.tsx`, `Files.tsx` | `sites` |
| **Databases** | MySQL/PostgreSQL via Docker, SQL browser, schema viewer, password reset | `routes/databases.rs` | `database.rs` | `Databases.tsx` | `databases` |
| **Backups** | Scheduled backups, S3/SFTP/B2/GCS remote storage, one-click restore | `routes/backups.rs`, `backup_schedules.rs`, `backup_destinations.rs` | `backups.rs`, `remote_backup.rs` | `Backups.tsx` | `backups`, `backup_schedules`, `backup_destinations` |
| **Backup Orchestrator** | DB/volume/site backups, AES-256 encryption, restore verification, policies, health dashboard, auto-verifier, **fleet-wide unified view across all servers with server+kind filters**, **end-to-end restore drills (site/db/volume), Restore Confidence SLA card, chain-of-trust JSON+PDF report across site/db/volume backups with per-arch sha256-pinned typst (v2.8.2)** | `routes/backup_orchestrator.rs` | `database_backup.rs`, `volume_backup.rs`, `encryption.rs`, `backup_verify.rs`, `backup_drills.rs`, `chain_report.rs` | `BackupOrchestrator.tsx` | `backup_policies`, `database_backups`, `volume_backups`, `backup_verifications`, `backup_drills` |
| **Webhook Gateway** | Receive, inspect, route, replay webhooks. HMAC-SHA256/SHA1 verification, JSON path filtering, retry with backoff, delivery logging | `routes/webhook_gateway.rs` | — | `WebhookGateway.tsx` | `webhook_endpoints`, `webhook_deliveries`, `webhook_routes` |
| **Secrets Manager** | AES-256-GCM encrypted vaults, version history, auto-inject to .env, masked API, pull for CLI, vault rename | `routes/secrets.rs`, `services/secrets_crypto.rs` | — | `SecretsManager.tsx` | `secret_vaults`, `secrets`, `secret_versions` |
| **Incident Management** | Incident lifecycle (investigating→resolved→postmortem), timeline updates, severity, affected components, postmortem | `routes/incidents.rs` | — | `IncidentManagement.tsx` | `managed_incidents`, `incident_updates`, `managed_incident_components` |
| **Public Status Page** | Customizable status page with component groups, incident history, subscriber notifications, overall status | `routes/incidents.rs` | — | `PublicStatusPage.tsx` | `status_page_config`, `status_page_components`, `status_page_subscribers` |
| **Cron Jobs** | Cron scheduling with manual execution and history | `routes/crons.rs` | `crons.rs` | `Crons.tsx` | (via agent crontab) |
| **Docker Apps** | 153 templates across 14 categories, Compose stacks, container lifecycle, registry, image tag change, live resource limits, GPU passthrough | `routes/docker_apps.rs`, `stacks.rs` | `docker_apps.rs` | `Apps.tsx` | `docker_stacks` |
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
| **Image Vulnerability Scanning** | Per-app CVE scanning with grype (self-contained install into `/var/lib/dockpanel/scanners/`), severity badge per app row, scheduled background rescans, soft deploy gate at critical/high/medium threshold. Defaults off. | `routes/image_scans.rs`, `services/image_scanner.rs` | `services/image_scanner.rs`, `routes/image_scan.rs` | `Apps.tsx` (badge + drawer), `Settings.tsx` (ImageScanSettings) |
| **Signed Releases + SBOM** | Every binary and SPDX 2.3 SBOM is signed in CI with cosign keyless via Sigstore (no long-lived key, recorded in Rekor transparency log). cargo-sbom emits per-crate SBOMs. Verification snippet in SECURITY.md. | — | — | — (release artifacts) |
| **Per-Image SBOM Generation** | On-demand SPDX 2.3 JSON SBOM for any deployed app's image (syft, self-contained install). One-click "Download SBOM" button in each app's scan drawer. Persisted in `image_sbom` (JSONB). Defaults off. | `routes/sboms.rs` | `services/sbom_scanner.rs`, `routes/sbom.rs` | `Apps.tsx` (drawer button), `Settings.tsx` (SbomSettings) |
| **Credential Encryption** | All stored credentials encrypted at rest with AES-256-GCM | `services/credential_crypto.rs` | — | — |
| **Content Security Policy** | CSP headers on frontend nginx config | — | — | `nginx.conf` |
| **Safe Command Execution** | `env_clear()` on all child processes to prevent environment hijacking | — | `safe_command.rs` | — |

## System

| Feature | Description | Backend | Agent | Frontend | Background Service |
|---------|-------------|---------|-------|----------|--------------------|
| **Dashboard** | Live CPU/RAM/disk/network, Docker summary, health score | `routes/dashboard.rs` | — | `Dashboard.tsx` | — |
| **Metrics** | Historical charts (24h), WebSocket live data | `routes/metrics.rs`, `ws_metrics.rs` | `system.rs` | (in Dashboard) | `metrics_collector.rs` |
| **Prometheus Endpoint** | `/api/metrics` in Prometheus exposition format (gauges for CPU/memory/disk/GPU per server, site counts, firing alerts by severity, build info). Disabled by default; scrape-token gated; constant-time SHA-256 comparison. Reads the latest row from the 30-second collector — zero extra load per scrape. | `routes/prometheus.rs`, `services/prometheus_exporter.rs` | — | `Settings.tsx` (PrometheusSettings) | — |
| **Alerts** | CPU/mem/disk thresholds, SSL expiry, service health, per-type muting for external channels | `routes/alerts.rs` | — | (in Settings) | `alert_engine.rs` |
| **Auto-Healing** | Restart crashed services, clean logs, renew SSL | — | — | (in Settings) | `auto_healer.rs` |
| **Diagnostics** | 6 check categories, one-click fixes | `routes/system.rs` | `diagnostics.rs` | (in Security) | — |
| **Traefik** | Alternative reverse proxy, auto-SSL, Docker discovery | `routes/system.rs` | `traefik.rs` | (in Settings) | — |
| **Service Installers** | PHP, Certbot, UFW, Fail2Ban, PowerDNS — one-click | `routes/system.rs` | `service_installer.rs` | (in Settings) | — |
| **Service Uninstall** | Uninstall routes for all 10 services (PHP, Certbot, UFW, Fail2Ban, PowerDNS, Redis, Node.js, Composer, mail server, PHP versions) | `routes/system.rs` | `service_installer.rs` | (in Settings) | — |
| **SSL Renew/Delete** | Force-renew and delete SSL certificates via instant_acme (RFC 9773 `replaces` hint on renew) | `routes/ssl.rs` | `ssl.rs` | `Certificates.tsx` | — |
| **ACME Profile Selection** | RFC 8555 profiles extension — default + per-site override for `classic` (90d), `tlsserver` (45d from 2026-05-13), `shortlived` (~6d) | `routes/ssl.rs` | `ssl.rs` | `Settings.tsx` | `settings` (`acme_default_profile`), `sites.ssl_profile` |
| **ARI-Driven Renewal** | ACME Renewal Information (RFC 9773) — auto-healer queries CA's suggested renewal window instead of static threshold; profile-aware fallback | `services/auto_healer.rs` | `services/ssl.rs` (`fetch_ari`) | — | `sites.ssl_renewal_at`, `sites.ssl_renewal_checked_at` |
| **User Suspend/Reset** | Suspend/unsuspend users with session invalidation, admin password reset | `routes/users.rs` | — | (in Settings) | `users` |
| **System Updates** | OS package updates, auto-updates toggle, reboot | `routes/system.rs` | `updates.rs` | (in Settings) | — |

## Admin

| Feature | Description | Backend | Frontend | DB Tables |
|---------|-------------|---------|----------|-----------|
| **Multi-Server** | Manage remote servers via HTTPS agents | `routes/servers.rs` | `Servers.tsx` | `servers` |
| **Reseller Accounts** | Admin→Reseller→User hierarchy, quotas, server allocation | `routes/resellers.rs`, `reseller_dashboard.rs` | `ResellerDashboard.tsx`, `ResellerUsers.tsx` | `reseller_profiles`, `reseller_servers` |
| **White-Label** | Per-reseller logo, panel name, accent color, hide branding | `routes/settings.rs` (branding endpoint) | (in CommandLayout, Login) | `reseller_profiles` |
| **Users** | CRUD, role assignment (admin/reseller/user) | `routes/users.rs` | (in Settings) | `users` |
| **API Keys** | Key CRUD and rotation. ⚠ **Not an authentication mechanism** — see §Withdrawn Claims | `routes/api_keys.rs` | (in Settings) | `api_keys` |
| **Extensions** | Webhook integrations, HMAC-signed events | `routes/extensions.rs` | `Extensions.tsx` | `extensions`, `extension_events` |
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

## Background Services (15 supervised)

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
| `image_scan_sweeper` | 30min | Rescan every running app's image past the configured interval (opt-in) |

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
| **GPU Passthrough** | NVIDIA Container Toolkit detection; per-container assignment (all GPUs or specific indices — pin app A to GPU 0, app B to GPU 1 on multi-GPU hosts) | `routes/docker_apps.rs` | `docker_apps.rs` | `Apps.tsx` |
| **GPU Monitoring** | Per-GPU utilization/VRAM/temperature/power/fan/driver/pstate from nvidia-smi, plus per-process VRAM table with PID→container resolution | `routes/docker_apps.rs` (`/apps/gpu-info`) | `docker_apps.rs` | `System.tsx` |
| **Horizontal Auto-Scaling** | Rule-based CPU thresholds, min/max replicas, cooldown | `routes/iac.rs` | — | (via Integrations) |

## Integrations (Advanced)

| Feature | Description | Backend | Frontend |
|---------|-------------|---------|----------|
| **Cloudflare Settings** | Zone security level, SSL mode, dev mode, cache purge | `routes/dns.rs` | `Dns.tsx` |
| **Cloudflare Tunnel** | Install cloudflared, token-based config, systemd service | `routes/system.rs` | `Settings.tsx` |
| **Wildcard SSL** | DNS-01 challenge via Cloudflare API, multi-part TLD support | `routes/sites.rs` | `SiteDetail.tsx` |
| **WHMCS Billing** | Webhook provisioning/suspension/termination, auto-create users | `routes/whmcs.rs` | `Integrations.tsx` |
| **Terraform/Pulumi** | IaC token management, resource listing API (sites, databases) | `routes/iac.rs` | `Integrations.tsx` |

## Database (Advanced)

| Feature | Description | Backend | Frontend |
|---------|-------------|---------|----------|
| **Visual Schema Browser** | Tables, columns, indexes, foreign key relationships in one view | `routes/databases.rs` | `Databases.tsx` |
| **Point-in-Time Recovery** | WAL archiving (PostgreSQL), binlog retention (MySQL), restore to timestamp | `routes/databases.rs` | `Databases.tsx` |

## Withdrawn Claims

Capabilities this project advertised and does not have. They are recorded rather
than quietly deleted, because a claim that merely disappears from one surface
tends to survive on the others — and because an evaluator who read the old text
deserves to find out what happened to it.

| Claim | Where it appeared | What is actually there | Withdrawn |
|-------|-------------------|------------------------|-----------|
| **Teams** — "multi-user access with role-based permissions", UI "(in Settings)" | `README.md`, `FEATURES.md`, `COMPARISON.md`, dockpanel.dev | `routes/teams.rs` is 477 lines of working, routed endpoints that grant **nothing**: `team_members` is read by that file and no other, so no authorization path anywhere consults team membership. There is no Teams UI — the word does not occur in the shipped frontend bundle — and the invite email links to `/teams/accept`, an SPA route that does not exist, so the recipient is redirected to the dashboard and the token is discarded. The schema attaches `team_id` to **`servers`**, not `sites`. | 2026-08-05 |
| **API Keys** — "programmatic access tokens", "scoped API keys" | `README.md`, `FEATURES.md`, `docs/api-reference.md` | Keys in all three families (`dp_`, `dpiac_`, `dpx_`) are generated, hashed, stored and handed to the operator with "won't be shown again" — and **no code path ever reads a stored hash back to authenticate a request**. The sole bearer-token extractor does exactly one thing with the value: JWT decode. `last_used_at` has no writer. The keys are real strings that open nothing. | 2026-08-05 |
| **App Migration** — "migrate containers between servers, progress tracking" | `README.md`, `FEATURES.md`, `Integrations.tsx` | The endpoint writes one row with `status='in_progress'` and there is **no `UPDATE app_migrations` anywhere in the repository** — no worker, no progress, no terminal state. The Integrations tab that reads the table is real, so an operator sees a migration that is permanently 0% and never completes. | 2026-08-05 |
| **Auto-sleep "scale to zero"** | `README.md` | Containers scale *to* zero and do not come back on their own: the Start control never clears `is_sleeping`, and the only endpoint that does has no caller in the frontend. Wording corrected to "stop idle containers"; the wake path is a tracked defect, not a claim. | 2026-08-05 |

## Verified Metrics

This table is the **measurement register**: every number DockPanel publishes about
itself — on this file, on the README, in the docs, and on dockpanel.dev — is quoted
from here, and `tests/docs-claims-pin-e2e.sh` fails the build when any surface
states a different figure for a metric named below.

`MB` means **MiB** (2²⁰ bytes) throughout — the unit `ls -lh`, `free -h` and
`docker stats` print. Mixing that with decimal MB is how the site ended up quoting
a binary size in one convention and a memory figure in the other.

The `Source` column is the derivation, and it decides which mechanism keeps the row
honest:

- **derived** — computed from source by `tests/docs-claims-pin-e2e.sh` on every
  commit. The command is in that suite, beside the assertion.
- **release** — read from the published release assets via the GitHub API by
  `tests/live-surfaces-check.sh`, which runs on a schedule rather than on a commit.
- **measured** — a reading taken on a real box, which no CI job can reproduce.
  These carry a date and expire: the scheduled check fails once one is older than
  its budget, because a measurement nobody has retaken is a claim, not a fact.

| Metric | Value | Source | Verified |
|--------|-------|--------|----------|
| API binary | 22 MB | release | v2.44.0 |
| Agent binary | 21 MB | release | v2.44.0 |
| CLI binary | 1.7 MB | release | v2.44.0 |
| Panel binaries, all three | 45 MB | release | v2.44.0 |
| API RAM (RSS) | ~14 MB | measured | 2026-07-27 |
| Agent RAM (RSS) | ~35 MB | measured | 2026-07-27 |
| Panel services RAM (agent + API) | ~49 MB | measured | 2026-07-27 |
| Full-stack RAM (with bundled PostgreSQL) | ~109 MB | measured | 2026-07-27 |
| App templates | 153 | derived | every commit |
| HTTP routes | 809 (527 backend + 282 agent) | derived | every commit |
| Regression-pin assertions | 1183 (34 suites) | derived | every commit |
| Frontend pages | 51 | derived | every commit |
| DB migrations | 104 | derived | every commit |
| Supervised background services | 15 | derived | every commit |

Five of these were wrong when the register was built (s272), some by a factor of
three, and one — the panel's own memory footprint, the headline claim of the
project — had been published at `~19 MB` since April against a real reading of
`~49 MB`. They were wrong on every surface at once, in both directions, which is
what a hand-maintained number does. Hence the register.

## Claims That Expire

Some published facts cannot rot on a commit and cannot be derived from anything
in this repository, because they are about the world rather than about DockPanel:
what a competitor charges, how much memory a competitor uses, whether a
screenshot still resembles the product. No CI job will ever catch these, because
CI only runs when something changes here, and nothing here changes when a
competitor raises its price.

So they expire instead. `tests/live-surfaces-check.sh` runs on a schedule rather
than on a commit, and fails once a claim is older than its budget. The failure
does not mean the claim is wrong — it means nobody has looked in long enough that
we no longer know.

| Claim | Where | Last verified | Budget (days) |
|-------|-------|---------------|---------------|
| Competitor RAM: cPanel 800 MB, Plesk 512 MB, HestiaCP 512 MB, CloudPanel 250 MB | COMPARISON.md, README.md, dockpanel.dev | 2026-03-21 | 180 |
| Competitor pricing and plan limits | COMPARISON.md, README.md | 2026-04-15 | 180 |
| Screenshots still resemble the shipped UI | README.md, dockpanel.dev | 2026-07-26 | 90 |
| The advertised install one-liner is the one we support | README.md, dockpanel.dev | 2026-07-27 | 180 |

The register's `measured` rows above expire too, on a 120-day budget declared in
that script.
