# Changelog

All notable changes to DockPanel will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/).

## [2.6.5] - 2026-03-25

### Security
- **Research-driven security audit**: Studied CVEs from CyberPanel, HestiaCP, CloudPanel, VestaCP, Webmin, cPanel — then audited DockPanel against those attack patterns. 55 findings (12 HIGH, 28 MEDIUM, 15 LOW).
- **Command execution safety**: Added `safe_command()` module — `env_clear()` on all 341 `Command::new()` calls across 44 files. Prevents LD_PRELOAD/PATH hijacking.
- **Credential encryption at rest**: All stored credentials (DB passwords, SMTP, S3/SFTP, OAuth, TOTP, DKIM) encrypted with AES-256-GCM using dedicated key derivation.
- **Shell injection fix**: Rewrote database_backup.rs — piped `docker exec` + `gzip` instead of `bash -c` with interpolated strings.
- **Tar symlink attacks**: `--no-dereference` on backup creation, `--no-same-owner` on restore.
- **Session revocation**: `revoke_all_sessions` now actually works — auth middleware checks cached timestamp.
- **Deploy log IDOR**: Ownership verification on both git_deploys and docker_apps SSE streams.
- **Content Security Policy**: Added CSP header to frontend nginx config.
- **Docker exec denylist**: Added 7 escape-relevant commands (unshare, pivot_root, setns, capsh, mknod, debugfs, kexec).
- **Compose volume symlinks**: `canonicalize()` resolves symlinks before path validation.
- **nginx header inheritance**: Security headers re-declared in static asset location blocks.
- **WebSocket security**: Conditional upgrade (prevents h2c smuggling), `access_log off` on token-bearing WS locations.
- **S3 temp files**: RAII TempFileGuard with random names + 0600 permissions.
- **2FA validation**: Explicit HS256 + leeway=0 (was Validation::default()).
- **Account enumeration**: Registration returns generic response.
- **Git history scrubbed**: Removed all passwords, IPs, hostnames, sensitive screenshots from history via git-filter-repo.

## [2.6.1] - 2026-03-22

### Added (LOW Priority Gap Fixes)
- **Domain rename** — New `PUT /api/sites/{id}/domain` endpoint to rename a site's domain. Agent handler renames nginx config, site directory, SSL certs, log files, PHP-FPM pools, Fail2Ban jails, redirects, and htpasswd configs. Backend updates monitors, status page components, and logs activity
- **Auto-firewall for proxy ports** — Sites created with proxy/node/python runtime automatically get a UFW deny rule blocking external access to the allocated proxy port (traffic only allowed through nginx). Rule is auto-removed on site deletion
- **Laravel auto-migrations** — Site deploys for Laravel sites (`php_preset = "laravel"`) now auto-run `php artisan migrate --force` after successful deploy
- **One-time scheduled deploy** — New `POST /api/git-deploys/{id}/schedule` endpoint to schedule a deploy at a specific time. New `scheduled_deploy_at` column on `git_deploys`. Deploy scheduler checks for due one-time schedules every 60s and auto-clears after triggering. Cancel with `DELETE /api/git-deploys/{id}/schedule`
- **Change Docker app image** — New `PUT /api/apps/{container_id}/image` endpoint to change a running container's image tag. Pulls new image, stops old container, creates new one preserving volumes, rolls back on failure
- **Update Docker app resource limits** — New `PUT /api/apps/{container_id}/limits` endpoint to update CPU/memory limits on running containers via `docker update`. Accepts `memory_mb` and `cpu_percent`

## [2.6.0] - 2026-03-22

### Fixed (Automation Gap Audit — Priority 1)
- **Auto-SSL DB update** — Background SSL provisioning now updates `ssl_enabled`, `ssl_cert_path`, `ssl_key_path`, `ssl_expiry` in the database and activates paused monitors (was silently succeeding without DB update)
- **Auto-SSL config preservation** — SSL provisioning now passes `php_preset` and `root_path` to the agent, preventing custom nginx config from being wiped
- **Pre-deploy backup** — All deploy paths (site deploy, git deploy manual, git deploy webhook/scheduled) now create a site backup before deploying
- **Pre-delete backup** — Site deletion creates a final backup before CASCADE-deleting the site record
- **Site deletion cleanup** — Now removes orphaned `status_page_components` matching the deleted domain
- **Database restore** — New `POST /db-backups/{db_name}/restore/{filename}` agent endpoint + `POST /api/backup-orchestrator/db-backups/{id}/restore` API endpoint. Supports MySQL/MariaDB, PostgreSQL, and MongoDB restore from backup files
- **Dashboard health score** — Now factors in backup freshness (-5 per stale site), security scan findings (-10 critical, -3 warning), and open incidents (-10 each)
- **Smart recommendations** — Dashboard intelligence endpoint returns actionable recommendations: stale backups, security findings, open incidents, expiring SSL, firing alerts, diagnostic issues. Rendered as a new Recommendations panel on the dashboard
- **Alert escalation** — Unacknowledged firing alerts re-notify with `[ESCALATED]` prefix after 15 minutes, then every 30 minutes. New `escalated_at` column + migration
- **Alert-to-incident correlation** — Before creating a new incident from an alert, checks for existing active incidents within 5 minutes. Appends as incident update instead of creating duplicates
- **Auto-healer restart limit** — Tracks restart count per service over 30-minute window. After 3 failed restarts, stops healing, creates critical incident, sends notification, and marks state as `exhausted`
- **Disk-full forecast alerting** — Computes disk fill rate from metrics history; alerts when disk projected full within 48h (critical if <12h)
- **Memory leak trend detection** — Compares recent vs older memory averages; warns when sustained >10% increase with usage above 60%
- **Docker container crash detection** — New `check_container_health` in alert engine detects exited, crash-looping, and unhealthy containers
- **Docker container auto-restart** — Auto-healer restarts exited/dead Docker containers with same 3-attempt limit as system services
- **Incidents pause deploys** — All 5 deploy paths (manual site, webhook site, manual git, webhook git, scheduled git) check for active critical/major incidents before proceeding
- **Security scanner auto-fix** — Auto-renews expiring SSL certificates detected by security scans (safe findings only, never auto-deletes)
- **Fail2Ban auto-configuration** — New sites auto-get a Fail2Ban jail monitoring their access log; removed on site deletion
- **Session management** — New `user_sessions` table, `GET /api/auth/sessions` (list with is_current flag), `DELETE /api/auth/sessions/{id}` (revoke), auto-cleanup of expired sessions
- **Notification center** — Bell icon with unread badge in all 4 layouts. New `panel_notifications` table, 4 API endpoints (list, unread-count, mark-read, mark-all-read), `/notifications` page with severity colors. Alerts auto-insert into notification center. 30-day retention cleanup. SSE real-time delivery. Wired into 18 event sources (deploys, incidents, backups, security, SSL, auto-healer, sites, auth)

### Fixed (Automation Gap Audit — MEDIUM Priority, 25 gaps)
- **Clone site auto-provisioning** — Clone now triggers auto-backup schedule, secrets vault, status page component, and site.created event
- **Composite site health** — New `GET /api/sites/{id}/health-summary` combining SSL, backup freshness, uptime, and composite score
- **"Backup Everything" preset** — New `POST /api/backup-orchestrator/policies/protect-all` one-click policy
- **Backup creation retry** — Policy executor retries failed backups once with 5s delay
- **Backup freshness alerting** — Proactive notification when sites have no backup in 48+ hours (throttled to once/hour)
- **Volume restore endpoint** — New `POST /api/backup-orchestrator/volume-backups/{id}/restore`
- **Deploy lock** — Concurrent deploys to same site blocked (checks for active building/deploying status)
- **Response time alerting** — Monitors warn when response time exceeds 5000ms threshold
- **Failed cron detection** — Manual cron execution fires alert on non-zero exit code
- **Postmortem auto-populate** — Transitioning to postmortem status auto-generates timeline template
- **/tmp cleanup + Docker prune** — Auto-healer now cleans /tmp (7d) and runs Docker system prune on disk pressure
- **Oversized log rotation** — Truncates individual log files larger than 500MB during cleanup
- **Welcome email** — New users receive welcome email with panel URL and credentials prompt
- **Audit log IPs** — Security-sensitive actions (site create/delete, user create/delete, security fix) now log client IP
- **Auto-rollback on deploy failure** — Failed site deploys auto-restore from pre-deploy backup
- **Generic webhook notifications** — New `notify_webhook_url` in alert rules for custom integrations (Telegram, Teams, etc.)
- **Weekly digest email** — Monday morning summary with 7-day alert/backup/incident/deploy counts to all admins
- **Post-deploy cache invalidation** — Nginx cache purge after successful deploy (fastcgi + proxy cache)
- **Reseller branding** — `GET /api/branding` now returns per-reseller logo/colors/name when applicable
- **Unified event timeline** — New `GET /api/dashboard/timeline` merging deploys, backups, incidents, alerts, scans

## [2.5.2] - 2026-03-22

### Fixed (Theme & Layout Consistency Audit)
- **Clean-Dark rounding parity** — Added ~120 lines of structural overrides (cards, modals, tables, buttons, scrollbar, selection, focus rings, progress bars, code blocks) so Clean-Dark has round corners everywhere, matching Clean
- **Ember radius normalized** — `--radius-xl` and `--radius-2xl` were 2px smaller than all other themes; fixed to 16px/20px
- **Clean hardcoded border-radius → CSS variables** — All 11 instances of hardcoded `12px/8px/6px/4px` converted to `var(--radius-lg/md/sm/xs)` for theme consistency
- **Status dot glow per-theme** — Green glow was hardcoded for all themes; now uses theme-appropriate accent color (blue for Midnight/Clean-Dark, orange for Ember, teal for Arctic, blue for Clean)
- **Progress bar glow for Arctic & Clean** — Missing glow rules added for both light themes
- **Settings theme picker missing `data-color-scheme`** — Switching to light themes now correctly sets color scheme attribute
- **Default theme mismatch** — Settings.tsx fallback aligned to `midnight` (was `terminal`)
- **FOUC prevention** — Added inline script in index.html to apply theme before CSS loads
- **LayoutSwitcher light variant** — Replaced hardcoded `zinc/blue/white` colors with theme variables
- **2FA banner in all layouts** — Replaced `amber-*` (stock Tailwind) with `warn-*` (theme tokens)
- **NexusLayout logout hover** — `rose-400` replaced with `danger-400` theme token
- **PublicStatusPage full theme adoption** — 40+ hardcoded color references replaced with theme variables
- **Terminal.tsx** — `bg-gray-300` and `bg-red-500` replaced with theme tokens
- **Login.tsx** — Google OAuth button uses theme-mapped text/hover colors
- **Settings.tsx hardcoded colors** — 13 instances of `blue-500/red-500` replaced with `accent/danger` tokens
- **Dashboard stat grid square corners** — Added `rounded-lg overflow-hidden` to stat bar and system info grids; added explicit `rounded-lg` to metric cards, sparkline cards, onboarding section, and issues panels
- **Compact layout flat nav** — GlassLayout now respects `dp-flat-nav` setting (was only implemented in Sidebar layout)
- **Compact layout footer spacing** — Removed nested padding wrapper, aligned `px-3` to match Sidebar layout spacing
- **Layout switcher dropdown redesign** — Added `p-1` padding and `rounded-md` items to match panel dropdown style; compact mode hides label text to save space; removed bordered button style for cleaner ghost-button look

## [2.5.1] - 2026-03-22

### Fixed (Remaining 7 Gaps — Phase D)
- **GAP 7+21: Internal events bridge to webhook gateway** — `fire_event()` now also forwards events to webhook gateway routes with `filter_path=/event` and `filter_value={event_type}`. Users can subscribe gateway routes to any internal event.
- **GAP 12: Docker apps auto-get monitor + status component** — Docker apps deployed with a domain now auto-create an HTTP monitor and a status page component under "Docker Apps" group.
- **GAP 13: Git deploy auto-creates gateway endpoint** — New git deploys auto-create a webhook gateway endpoint for webhook inspection/replay capabilities.
- **GAP 16: Incident resolve cleans up alerts + components** — Resolving a managed incident auto-resolves linked alerts and clears status_override on affected status page components.
- **GAP 17: Vault export/import** — New `GET /api/secrets/vaults/{id}/export` and `POST /api/secrets/vaults/{id}/import` endpoints for encrypted vault backup and transfer between DockPanel instances.

### Automation Audit: Complete
All 21 identified gaps now addressed. Zero manual steps required for: backup scheduling, uptime monitoring, secret injection, incident creation, status page updates, or webhook delivery.

## [2.5.0] - 2026-03-22

### Fixed (21-Gap Automation Audit)
- **GAP 1: Backup policies now execute** — New `backup_policy_executor` background service runs every 60s, evaluates cron schedules, executes backup policies across sites, databases, and volumes. Policies are no longer dead config.
- **GAP 2: Verifier respects policy_id** — Backup verifier checks `verify_after_backup` flag. Policy executor triggers verification after successful backups.
- **GAP 3: Auto-incidents from monitoring** — When a monitor goes down, the system auto-creates a managed incident with timeline, links affected status page components, and auto-resolves when the monitor recovers.
- **GAP 4: Auto status page components** — New sites automatically get a status page component (if status page is enabled).
- **GAP 5: Auto-inject secrets on deploy** — After a successful deploy, the system checks for a linked vault with `auto_inject` secrets and injects them into the site's `.env` file automatically.
- **GAP 6: Auto-vault for new sites** — Every new site gets an auto-created secrets vault linked via `site_id`.
- **GAP 8: fire_event in all new features** — Backup orchestrator, incident management, and secrets manager now emit extension webhook events (`db_backup.created`, `incident.created`, `secrets.injected`, etc.).
- **GAP 9: Critical alerts create incidents** — Critical alerts and server offline/service down alerts auto-create managed incidents visible on the status page.
- **GAP 10: Backup failure creates incident** — When a backup policy has failures, a managed incident is auto-created.
- **GAP 14: Backup for ALL sites** — Removed the `site_count <= 1` gate. Every new site now gets a daily backup schedule automatically.
- **GAP 15: Auto-monitor with deferred activation** — New sites get a paused HTTP monitor that auto-activates after successful SSL provisioning (when DNS is confirmed working).
- **GAP 18: Webhook delivery cleanup** — Added 7-day retention cleanup for `webhook_deliveries` and 90-day for `backup_verifications` in the auto-healer retention cycle.
- **GAP 19: Subscribers notified of auto-downtime** — Status page subscribers now receive email notifications when monitors detect downtime, not just for manually-created incidents.
- **GAP 20: Policy encrypt flag works** — The backup policy executor passes the encrypt flag through to agent backup endpoints when `encrypt = TRUE`.

### Infrastructure
- New background service: `backup_policy_executor` (supervised, 60s interval) — 11th background service
- Modified: `uptime.rs` (auto-incidents + subscriber notifications), `alert_engine.rs` (critical→incident), `sites.rs` (auto-vault, auto-monitor, auto-component, backup for all), `ssl.rs` (activate monitors), `deploy.rs` (auto-inject secrets), `auto_healer.rs` (retention cleanup), `backup_orchestrator.rs` + `incidents.rs` + `secrets.rs` (fire_event calls)

## [2.4.0] - 2026-03-22

### Added
- **Webhook Gateway**: Receive, inspect, route, and replay incoming webhooks.
  - **Inbound endpoints**: Each gets a unique URL (`/api/webhooks/gateway/{token}`). Unlimited endpoints per user.
  - **Signature verification**: HMAC-SHA256 and HMAC-SHA1 modes for GitHub, Stripe, and other providers. Configurable header name and secret.
  - **Request inspector**: Full request logging — headers, body, source IP, signature validation status. Click any delivery to view complete details.
  - **Route builder**: Forward incoming webhooks to any destination URL. JSON path filtering (e.g., only forward `action=push`). Custom header injection. Configurable retry (0-10 attempts with exponential backoff).
  - **Replay**: Re-send any past delivery to all configured routes. Useful for debugging or recovery.
  - **Delivery tracking**: Per-route forwarding status, response body, duration. Endpoint-level counters.
  - **E2E test suite**: `tests/webhook-gateway-e2e.sh` — endpoint CRUD, webhook receive, delivery inspection, routes, replay, filtering.

### Infrastructure
- New crate dependency: `sha1 0.10` for HMAC-SHA1 signature verification.
- New migration: `webhook_endpoints`, `webhook_deliveries`, `webhook_routes` tables.
- 8 new API endpoints (7 admin, 1 public inbound).
- Frontend: `WebhookGateway.tsx` with 3 tabs (Endpoints, Request Inspector, Routes).

## [2.3.0] - 2026-03-22

### Added
- **Secrets Manager**: AES-256-GCM encrypted secret storage with version history.
  - **Secret vaults**: Project-scoped vaults for organizing secrets (global or per-site).
  - **Encrypted storage**: All secret values encrypted with AES-256-GCM (random nonce per secret, key derived from JWT_SECRET via SHA-256).
  - **Secret types**: Environment variables, API keys, passwords, certificates, custom — with type-specific UI badges.
  - **Version history**: Every update creates a versioned snapshot. Full audit trail with who changed what and when.
  - **Auto-inject**: Mark secrets for automatic injection into site `.env` files on deploy. One-click inject from vault to site.
  - **Masked by default**: API returns masked values (`xxxx••••••••`) unless `?reveal=true` is explicitly requested.
  - **Pull endpoint**: `GET /api/secrets/vaults/{id}/pull` returns all secrets as decrypted key-value pairs (for CLI integration).
  - **Vault sidebar UI**: Split-pane layout with vault list on left, secrets table on right. Create/edit/delete with inline forms.
  - **E2E test suite**: `tests/secrets-manager-e2e.sh` — vault CRUD, secret CRUD, encryption roundtrip, version history, pull.

### Infrastructure
- New crate dependencies: `aes-gcm 0.10`, `base64 0.22` for AES-256-GCM encryption.
- New service: `secrets_crypto.rs` — encrypt/decrypt with nonce+ciphertext format, unit tests included.
- New migration: `secret_vaults`, `secrets`, `secret_versions` tables.
- 8 new API endpoints under `/api/secrets/`.
- Frontend: `SecretsManager.tsx` with vault browser, reveal toggle, version history panel.

## [2.2.0] - 2026-03-22

### Added
- **Incident Management**: Full incident lifecycle with real-time status updates.
  - **Managed incidents**: Create, track, and resolve incidents with status lifecycle (investigating → identified → monitoring → resolved → postmortem).
  - **Incident severity**: Minor, major, critical, and maintenance classifications.
  - **Incident timeline**: Post updates with status changes and messages. Full audit trail with author emails and timestamps.
  - **Postmortem support**: Attach post-incident analysis with publish control.
  - **Affected components**: Link incidents to status page components for targeted impact reporting.
- **Enhanced Status Page**: Production-grade public status page replacing the basic monitor list.
  - **Status page configuration**: Customizable title, description, logo URL, accent color, history display settings.
  - **Component groups**: Organize monitors into logical service components (e.g., "API Server", "Website") with grouping.
  - **Overall status indicator**: Automatically computed from component health (operational/degraded/major outage).
  - **Incident history**: Shows active incidents with full timeline, plus resolved incidents within configurable history window.
  - **Auto-detected downtime**: Legacy monitor-based incidents also displayed for complete visibility.
  - **Email subscribers**: Public subscribe/unsubscribe for incident notifications. Verified subscribers receive updates on status changes.
  - **Standalone public page**: Dark-themed, no-auth status page at `/status` with responsive layout.
- **Admin UI**: New "Incidents" page in Operations nav with 3 tabs (Incidents, Components, Settings).
- **11 new API endpoints**: Incidents CRUD + updates, status page config, components CRUD, subscribers, enhanced public endpoint.
- **E2E test suite**: `tests/incident-management-e2e.sh` covering full incident lifecycle, components, public page, subscribers.

### Infrastructure
- New migration: `status_page_config`, `status_page_components`, `status_page_component_monitors`, `managed_incidents`, `managed_incident_components`, `incident_updates`, `status_page_subscribers` tables.
- Frontend: `IncidentManagement.tsx` (admin), `PublicStatusPage.tsx` (public standalone).

## [2.1.0] - 2026-03-22

### Added
- **Backup Orchestrator**: New centralized backup management system for databases, Docker volumes, and sites.
  - **Database backups**: MySQL/MariaDB (`mysqldump`), PostgreSQL (`pg_dump`), and MongoDB (`mongodump`) dump + restore via Docker exec. Compressed with gzip.
  - **Docker volume backups**: Back up any Docker volume to `.tar.gz` using a temporary Alpine container. Restore volumes with one click.
  - **Encryption at rest**: Optional AES-256-CBC encryption (PBKDF2, 100k iterations) for all backup types via OpenSSL. Encrypted files get `.enc` suffix, originals are auto-deleted.
  - **Automatic restore verification**: Verify backups by spinning up temporary database containers and restoring dumps, or extracting archives to temp directories. Checks file integrity, table counts, and entry points.
  - **Backup policies**: Cross-resource policies with cron scheduling, destination selection, retention count, encryption toggle, and auto-verification.
  - **Backup health dashboard**: Global overview with total counts, storage usage, 24h success/failure rates, active policies, verification stats, and stale backup warnings.
  - **Background verifier**: Supervised service running every 6 hours that automatically verifies unverified backups and fires alerts on failures.
  - **B2 and GCS destinations**: Backblaze B2 and Google Cloud Storage now supported as backup destinations (S3-compatible API).
  - **CLI commands**: `dockpanel backup db-create`, `db-list`, `vol-create`, `vol-list`, `verify`, `health` — full backup management from the command line.
  - **E2E test suite**: Dedicated backup orchestrator test script (`tests/backup-orchestrator-e2e.sh`) covering health, policies CRUD, database backup lifecycle with verification.
- **Nav item**: "Backups" in Operations section links to the new Backup Orchestrator page.

### Infrastructure
- New migration: `backup_policies`, `database_backups`, `volume_backups`, `backup_verifications` tables.
- Extended `backup_destinations` with `encryption_enabled`, `encryption_key` columns, and B2/GCS dtype support.
- Agent: 4 new services (`database_backup`, `volume_backup`, `encryption`, `backup_verify`) + 3 new route modules.
- Backend: `backup_orchestrator` routes (11 endpoints), `backup_verifier` supervised background service.
- Frontend: `BackupOrchestrator.tsx` page with 5 tabs (Overview, Policies, DB Backups, Volume Backups, Verifications).

## [2.0.6] - 2026-03-21

### Fixed
- **Nexus themes decoupled from layout**: Nexus and Nexus Dark themes were previously locked to the Nexus layout only. They are now independent color themes that work with any layout (Terminal, Glass, Atlas, Nexus). Theme cycling (Ctrl+K) and Settings picker now include all 6 themes.

### Improved
- **Premium card depth**: Dark theme cards (Terminal, Midnight, Ember, Nexus Dark) now have subtle box shadows creating layered depth instead of flat rectangles.
- **Progress bar polish**: All progress bars now have rounded ends and a subtle accent-colored glow per theme (green/blue/orange).
- **Bolder status indicators**: Status dots (online/offline/warning) are larger (10px) with colored glow halos for better visibility on dense pages.
- **Theme picker expanded**: Settings appearance panel now shows all 6 themes (was 4) with accurate mini-previews including Nexus Dark and Nexus Light.
- **Layout switcher description**: Nexus layout description updated to "Modern SaaS, flat nav" (was "Light, clean SaaS" which was misleading since dark themes now work with it).

## [2.0.5] - 2026-03-21

### Added
- **Nexus Dark theme**: Premium dark mode for the Nexus layout with sun/moon toggle. GitHub Dark-inspired three-layer depth palette, Inter font, rounded corners, blue accent. Persists across sessions.
- **Sidebar group labels**: Navigation groups (Reseller, Operations, Admin) now display small uppercase labels in the Command layout sidebar.
- **Glass sidebar tooltips**: Native browser tooltips show nav item names when the Glass layout sidebar is collapsed.
- **Card elevation system**: Three elevation levels (`.elevation-1/2/3`), `.card-interactive` hover effects, `.hover-lift` card animations. Applied to dashboard cards, sites table, mail service cards, app templates, server/monitor items.
- **Page header system**: Sticky `page-header` bar with title, subtitle, and action buttons. Applied to 13 pages (Dashboard, Sites, Databases, Apps, Security, Settings, Servers, Mail, Monitoring, DNS, Users, Git Deploy, Alerts).
- **Login background gradient**: Subtle radial gradient that adapts per theme (green/blue/teal/orange).
- **Modal portal system**: `dp-modal` / `dp-modal-overlay` CSS classes for Nexus-compatible modal styling across 15 modals in 6 pages.

### Improved
- **Button color hierarchy**: Only primary CTAs (Create Site, Run Scan, Add Record) stay green. All secondary/utility buttons (Customize, Restart Nginx, Export, Refresh, etc.) use neutral gray — breaks the green monotone across 6 pages, ~25 buttons.
- **Dynamic progress bar colors**: CPU/Memory/Disk bars change from green (<70%) → amber (70-90%) → red (>90%). Disk uses 80/90 thresholds. Rounded ends with smooth 500ms transitions.
- **Dashboard visual hierarchy**: Metric cards with elevation, 24h chart fade-in animation, staggered stat grid, collapsible onboarding wizard (auto-collapses after 3+ steps, persists to localStorage).
- **Sidebar footer redesign**: User avatar circle with initial, hover-reveal logout button, descriptive health status ("Connected"/"Disconnected" replaces "OK"/"!"). Applied to both Command and Glass layouts.
- **Typography for non-terminal themes**: Midnight and Ember now remove uppercase/tracking like Nexus. All 5 sans-serif themes get 15px body text for better Inter readability.
- **Security card grid**: Changed from 5-column with orphan card to balanced 3-column grid with equal `min-h-[140px]` heights.
- **Table hover states**: `table-row-hover` class added to Security, DNS, and Users table rows with theme-aware hover colors.
- **Onboarding wizard**: Completed steps show a solid green circle with white checkmark. Collapsible with compact "Setup: X/5 complete" view.
- **Ember theme contrast**: Lightened surfaces and brightened orange accent for better text readability.
- **Atlas layout nav**: Added `shrink-0` to nav items so they scroll horizontally instead of compressing.
- **Richer empty states**: Sites, Databases, Git Deploys, Monitors, and Crons pages show contextual feature descriptions instead of bare "No X yet" text.
- **Login page**: Removed bulky "Made with Rust" gear icon, replaced with minimal "Powered by Rust" text. Card shadows added.

### Fixed
- **Theme switching: Nexus→Terminal white screen**: Switching from Nexus layout to any other layout left `dp-theme=nexus` (white) active, rendering a white Terminal layout. Fixed with `dp-pre-nexus-theme` save/restore in LayoutSwitcher, NexusLayout, useLayoutState, and main.tsx IIFE.
- **Nexus modal clipping**: Modals in Nexus layout were clipped by `overflow-hidden` on the main wrapper, hiding the top fields. Fixed with `createPortal` to render at `document.body`.
- **Nexus modal contrast**: Modal cards in Nexus light had the same `#f9fafb` background as the page (invisible). Fixed with `dp-modal` class providing white background, strong shadow, and proper text colors.
- **Page header spacing**: Added `margin-bottom: 1.25rem` to `.page-header` for consistent spacing between header and content.
- **Nexus light theme: tinted selection buttons**: Migration source cards, Settings proxy selector, and all `bg-rust-500/10`-style toggle buttons were rendering as solid blue blobs. Fixed with properly unescaped selectors.
- **Nexus light theme: accent toggle visibility**: `bg-accent-500/15` toggles now render with readable blue tint and text.

## [2.0.4] - 2026-03-20

### Security
- **CORS lockdown**: Deny all cross-origin requests by default. Same-origin panel UI is unaffected. Previously defaulted to `AllowOrigin::any()` which allowed CSRF from any website.
- **Constant-time token comparison**: Agent auth middleware now uses `subtle::ConstantTimeEq` to prevent timing attacks on token validation.
- **Token hashing in database**: Agent tokens stored as SHA-256 hashes in `agent_token_hash` column. DB dump no longer exposes plaintext tokens for inbound auth.
- **Token rotation**: New `POST /auth/rotate-token` on agent + `POST /api/servers/{id}/rotate-token` on API. 60-second grace period for old token during rotation. Updates `api.env` on disk for persistence.
- **Secure cookie fix**: `BASE_URL` defaulted to `https://panel.example.com`, causing `Secure` flag on cookies over HTTP. Fixed — defaults to empty, setup script sets from domain.
- **jsonwebtoken upgraded 9 → 10.3.0**: Fixes type confusion vulnerability that could lead to authorization bypass.
- **serde_yml replaced with serde_yaml_ng**: `serde_yml` and `libyml` are unsound/unmaintained. Replaced with `serde_yaml_ng` v0.10.0.

### Fixed
- **Cascade cron cleanup**: Deleting a site now removes cron entries from the system crontab. Previously, DB records were cleaned via CASCADE but crontab entries were orphaned.
- **UFW port gap**: Setup script now adds panel ports (80, 443, 8443) to UFW even when the firewall is pre-existing. Previously skipped port rules if UFW was already installed.
- **Token rotation API→agent desync**: Rotating the agent token now updates the API's in-memory `AgentClient` token AND writes to `api.env` on disk. Previously left the API with the old token, breaking all agent communication.

### Added
- **CI pipeline** (`.github/workflows/ci.yml`): Rust clippy, frontend type check, build verification, unit tests, `cargo-audit` + `npm audit` security scanning. Runs on every push to main and PRs.
- **E2E test suite** (`tests/e2e.sh`): 62 tests across 27 categories — full CRUD lifecycle, security edge cases, zero-leftover cleanup. Run: `bash tests/e2e.sh <host> [port]`.
- **Deep E2E test suite** (`tests/deep-e2e.sh`): 51 tests for advanced features — WordPress install, backup restore, git deploy, reseller system, file operations, compose stacks, concurrent operations, extensions API.
- **29 unit tests**: Config parsing (BASE_URL defaults, Secure flag logic), token hashing, input validation (domains, names, container IDs, path traversal, pagination).
- **API reference** (`docs/api-reference.md`): 648 lines documenting all 371 endpoints with request bodies and examples.
- **Competitor comparison** (`COMPARISON.md`): Honest comparison vs HestiaCP, CloudPanel, RunCloud, CyberPanel, Ploi.
- **README overhaul**: Dashboard screenshot, comparison table, collapsible screenshot gallery, cleaner structure.
- **FUNDING.yml**: PayPal sponsor link (paypal.me/ovexro).

### Verified
- **Reboot recovery**: All services start automatically after server reboot. 62/62 E2E tests pass post-reboot.
- **Fresh install E2E**: Full install via `INSTALL_FROM_RELEASE=1` on clean Ubuntu 24.04 VPS — all features operational.

## [2.0.3] - 2026-03-20

### Added
- **Documentation site** at `docs.dockpanel.dev`: mdBook-generated, 8 pages (getting-started, troubleshooting, CLI reference, WordPress, Git deploy, email, multi-server, backups). 1855 lines.

### Changed
- **Docker app templates pinned**: 33 of 39 `:latest` tags replaced with specific major versions (e.g., `redis:7`, `ghost:5`, `grafana/grafana:11`). 6 kept at `:latest` due to non-standard versioning (minio, nocodb, etc.).
- **Auto-monitors removed**: Sites no longer auto-create uptime monitors on creation. Users create monitors manually when DNS is configured.

### Added — Documentation
- **8 documentation pages** at `docs/`: getting-started, troubleshooting, CLI reference, and 5 guides (WordPress, Git deploy, email, multi-server, backups). 1855 lines of practical, copy-paste-friendly docs.

### Fixed — Fresh Install E2E (real clean VPS test)
- **Local server not registered after setup**: API returned 503 on all requests after admin creation. Added `ensure_local_server()` call in the setup endpoint.
- **Site docroot missing /public/ subdirectory**: Agent created `/var/www/{domain}/` but nginx expected `/var/www/{domain}/public/`. Fixed to create the correct subdirectory.
- **Backup tar flag incompatibility**: Replaced `--no-dereference` with `-h` (POSIX-compatible).

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
