<p align="center">
  <img src=".github/screenshots/dp-dashboard.png" alt="DockPanel Dashboard" width="800">
</p>

<h1 align="center">DockPanel</h1>

<p align="center">
  <strong>The most feature-packed free server panel ever built.</strong><br>
  Self-hosted. Docker-native. Written in Rust. Panel services run on <strong>~19MB of RAM</strong>. 733 API endpoints. 151 app templates. 425 E2E tests. ~41MB binaries. Zero subscriptions.
</p>

<p align="center">
  <a href="https://github.com/ovexro/dockpanel/releases"><img src="https://img.shields.io/github/v/release/ovexro/dockpanel" alt="Release"></a>
  <a href="https://github.com/ovexro/dockpanel/actions"><img src="https://img.shields.io/github/actions/workflow/status/ovexro/dockpanel/ci.yml?label=CI" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-BSL_1.1-blue.svg" alt="License: BSL 1.1"></a>
</p>

<p align="center">
  <a href="https://dockpanel.dev">Website</a> &bull;
  <a href="https://docs.dockpanel.dev">Docs</a> &bull;
  <a href="CHANGELOG.md">Changelog</a> &bull;
  <a href="https://github.com/ovexro/dockpanel/discussions">Discussions</a>
</p>

---

## Install

```bash
curl -sL https://dockpanel.dev/install.sh | sudo bash
```

Open `http://YOUR_SERVER_IP:8443`, create your admin account, done.

Supports Ubuntu 20+, Debian 11+, CentOS 9+, Rocky 9+, Fedora 39+, Amazon Linux 2023. x86_64 and ARM64.

## Why DockPanel?

No other free panel gives you Git push-to-deploy with blue-green zero-downtime updates, 151 one-click Docker app templates, a WAF, passkey login, GPU passthrough, multi-server management, reseller accounts, a developer CLI, and Infrastructure as Code — all while the panel services themselves use under 20MB of RAM. DockPanel does.

| | DockPanel | HestiaCP | CloudPanel | RunCloud |
|---|---|---|---|---|
| **Price** | **Free** | Free | Free | $8/mo+ |
| **Stack** | **Rust + React** | PHP | PHP | PHP (SaaS) |
| **Docker native** | **151 templates** | No | No | No |
| **Git deploy** | **Blue-green, zero-downtime** | No | No | Basic |
| **Multi-server** | **Unlimited** | No | No | Yes |
| **Reseller + white-label** | **Yes** | Reseller only | No | No |
| **CLI + IaC** | **Full CLI + YAML export** | Limited | No | No |
| **RAM usage (panel)** | **~19MB** | ~200MB+ | ~150MB+ | SaaS |
| **ARM64 / Homelab** | **Yes** | Partial | No | No |
| **Self-hosted** | **Yes** | Yes | Yes | No |

## Screenshots

<details>
<summary><strong>Dashboard</strong> — Live server metrics, 24h graphs, site overview, recent activity</summary>

![Dashboard](.github/screenshots/dp-dashboard.png)
</details>

<details>
<summary><strong>Sites</strong> — Static, PHP, Node.js, Python, reverse proxy with Nginx + SSL</summary>

![Sites](.github/screenshots/dp-sites.png)
</details>

<details>
<summary><strong>Site Detail</strong> — SSL, WAF, file manager, terminal, backups, resource limits, custom nginx</summary>

![Site Detail](.github/screenshots/dp-site-detail.png)
</details>

<details>
<summary><strong>Docker Apps</strong> — 151 one-click templates across 14 categories</summary>

![Docker Apps](.github/screenshots/dp-apps.png)
</details>

<details>
<summary><strong>Databases</strong> — MySQL/PostgreSQL, SQL browser, schema viewer, point-in-time recovery</summary>

![Databases](.github/screenshots/dp-databases.png)
</details>

<details>
<summary><strong>File Manager</strong> — Browse, edit, upload files from the browser</summary>

![File Manager](.github/screenshots/dp-file-manager.png)
</details>

<details>
<summary><strong>Terminal</strong> — Full SSH in the browser with tabs, themes, session recording</summary>

![Terminal](.github/screenshots/dp-terminal.png)
</details>

<details>
<summary><strong>Git Deploy</strong> — Push-to-deploy, atomic zero-downtime deploys, preview environments</summary>

![Git Deploy](.github/screenshots/dp-git-deploy.png)
</details>

<details>
<summary><strong>Monitoring</strong> — HTTP/TCP/ping uptime checks, SLA tracking, PagerDuty integration</summary>

![Monitoring](.github/screenshots/dp-monitoring.png)
</details>

<details>
<summary><strong>Security</strong> — Firewall, Fail2Ban, SSH hardening, vulnerability scanning, audit logs</summary>

![Security](.github/screenshots/dp-security.png)
</details>

<details>
<summary><strong>Backups</strong> — Scheduled backups, S3/SFTP destinations, Restic incremental, one-click restore</summary>

![Backups](.github/screenshots/dp-backups.png)
</details>

<details>
<summary><strong>DNS</strong> — Cloudflare + PowerDNS, zone management, cache purge, security settings</summary>

![DNS](.github/screenshots/dp-dns.png)
</details>

<details>
<summary><strong>Mail</strong> — Postfix + Dovecot + DKIM, Roundcube webmail, Rspamd spam filter</summary>

![Mail](.github/screenshots/dp-email.png)
</details>

<details>
<summary><strong>Cron Jobs</strong> — Scheduled tasks with output logging</summary>

![Cron Jobs](.github/screenshots/dp-crons.png)
</details>

<details>
<summary><strong>System</strong> — Services, updates, diagnostics, auto-healing</summary>

![System](.github/screenshots/dp-system.png)
</details>

<details>
<summary><strong>Settings</strong> — Branding, notifications, alert channels, account security</summary>

![Settings](.github/screenshots/dp-settings.png)
</details>

<details>
<summary><strong>Login</strong> — Email/password + passkey (WebAuthn) support</summary>

![Login](.github/screenshots/dp-login.png)
</details>

## Features

### Hosting
- **Sites** — Static, PHP (8.1-8.4), Node.js, Python, reverse proxy. Automatic Nginx config, SSL, PHP-FPM pools.
- **Databases** — MySQL/PostgreSQL in Docker. Built-in SQL browser, visual schema browser, point-in-time recovery (WAL/binlog). Auto-cleanup on site delete.
- **Docker Apps** — 151 templates across 14 categories (AI, CMS, Database, Media, Monitoring, and more). Compose stacks. Resource limits. GPU passthrough.
- **Git Deploy** — Push-to-deploy. Atomic zero-downtime deploys (Capistrano-style). Nixpacks (30+ languages). Preview environments.
- **WordPress Toolkit** — Multi-site dashboard, vulnerability scanning, security hardening, bulk updates.
- **CMS Install** — WordPress, Laravel, Drupal, Joomla, Symfony, CodeIgniter — one click.
- **Backups** — Scheduled, S3/SFTP remote destinations, one-click restore. Restic incremental (encrypted, deduplicated).
- **Backup Orchestrator** — DB/volume backups, AES-256 encryption, restore verification, cross-resource policies, S3/SFTP/B2/GCS destinations, health dashboard.
- **CDN** — BunnyCDN and Cloudflare CDN management. Cache purge, bandwidth stats, pull zone discovery.
- **Image Optimization** — Server-side WebP/AVIF conversion per site.
- **Secrets Manager** — AES-256-GCM encrypted vaults, version history, auto-inject to .env, masked API, CLI pull endpoint.
- **Webhook Gateway** — Inbound endpoints with unique URLs, HMAC-SHA256/SHA1 verification, request inspector, route builder, retry/replay.

### Operations
- **Multi-Server** — Manage remote servers from one panel. Agent auto-registers.
- **DNS** — Cloudflare + PowerDNS. Zone templates, propagation checker, DNSSEC. Cloudflare cache purge, security settings, Cloudflare Tunnel.
- **Container Management** — Auto-sleep (scale to zero), auto-update detection, per-user isolation policies, app migration between servers.
- **Mail** — Postfix + Dovecot + OpenDKIM. Webmail (Roundcube), spam filter (Rspamd), SMTP relay.
- **Monitoring** — HTTP/TCP/ping uptime checks, SLA tracking, PagerDuty integration.
- **Incident Management** — Full lifecycle (investigating, identified, monitoring, resolved, postmortem), severity levels, timeline, affected components.
- **Public Status Page** — Standalone dark-themed page at `/status`, component groups, email subscribers, overall status auto-computed from checks.
- **Terminal** — Full SSH with tabs, themes, sharing, session recording.

### Security
- **Passkey/WebAuthn** — Passwordless login with biometrics or security keys. Plus 2FA/TOTP with recovery codes.
- **WAF** — ModSecurity3 + OWASP CRS v4 per site. Detection or prevention mode. Event viewer.
- **CSP & Bot Protection** — Per-site Content Security Policy headers and bot rate limiting.
- **Firewall** — UFW management with smart port opener.
- **Fail2Ban** — View/ban/unban IPs, panel-specific jail.
- **SSH Hardening** — Disable password/root login, change port — one click.
- **Vulnerability Scanning** — Container scanning, file integrity, security headers.
- **Auto-Healing** — Restart crashed services, clean disk, renew expiring SSL, auto-sleep idle containers.

### Developer Experience
- **CLI** — `dockpanel status`, `sites`, `apps`, `diagnose`, `export`, `apply`
- **Infrastructure as Code** — Export/import server config as YAML. Terraform/Pulumi provider API with scoped IaC tokens.
- **Smart Diagnostics** — 6 check categories with one-click fixes. Auto-optimization recommendations.
- **File Manager** — Browse, edit, upload files from the browser.
- **Command Palette** — Ctrl+K to navigate anywhere.
- **Nginx FastCGI Cache** — Per-site toggle with smart bypass for logged-in users.
- **Redis Object Cache** — Per-site isolated Redis DB with WP auto-config.

### Themes & Layouts
- **6 Themes** — Terminal (hacker green), Midnight (navy blue), Ember (warm amber), Arctic (light teal), Clean (light blue SaaS), Clean Dark (GitHub-dark).
- **3 Layouts** — Sidebar (full sidebar nav), Compact (collapsible icon rail), Topbar (horizontal navbar).

### Business
- **Reseller Accounts** — Admin → Reseller → User hierarchy with quotas.
- **White-Label** — Custom logo, colors, panel name per reseller.
- **OAuth/SSO** — Google, GitHub, GitLab login.
- **Extension API** — Webhook events with HMAC signing and scoped API keys.
- **WHMCS Integration** — Provisioning, suspension, termination hooks. Auto-create users from billing.
- **Horizontal Auto-Scaling** — Rule-based CPU thresholds with min/max replicas and cooldown.
- **Migration Wizard** — Import from cPanel, HestiaCP. Plesk (beta). App migration between servers.
- **Teams** — Multi-user access with role-based permissions.

## Architecture

```
Browser → React 19 SPA → Nginx
                           ├── /api/* → API (Rust/Axum)
                           │              ├── PostgreSQL 16
                           │              └── Agent (Unix socket / HTTPS)
                           │                     └── Docker, Nginx, SSL, files, terminal
                           └── /*     → Frontend (static files)
```

**3 Rust binaries**: Agent (~21MB), API (~20MB), CLI (~1MB). Runtime RAM: ~12MB agent + ~7MB API ≈ 19MB for the panel itself; ~85MB with the bundled PostgreSQL. 11 background services.

| Component | Tech | Role |
|-----------|------|------|
| Agent | Rust/Axum | Root-level host operations (Docker, Nginx, SSL, files) |
| API | Rust/Axum + SQLx | Auth, business logic, multi-server dispatch, background tasks |
| CLI | Rust/Clap | Command-line interface for automation |
| Frontend | React 19 + Vite + Tailwind 4 | Browser UI with 6 themes + 3 layouts |

## Security

DockPanel has undergone six rounds of security auditing (260+ vulnerabilities found and fixed). Credentials are encrypted at rest with AES-256-GCM. All child processes run with sanitized environments. See [SECURITY.md](SECURITY.md) for details.

## Development

```bash
git clone https://github.com/ovexro/dockpanel.git && cd dockpanel

# Start database
docker run -d --name dockpanel-postgres \
  -e POSTGRES_USER=dockpanel -e POSTGRES_PASSWORD=dockpanel -e POSTGRES_DB=dockpanel \
  -p 5450:5432 postgres:16

# Build
cargo build --release --manifest-path panel/agent/Cargo.toml
cargo build --release --manifest-path panel/backend/Cargo.toml
cargo build --release --manifest-path panel/cli/Cargo.toml
cd panel/frontend && npm install && npx vite build
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for full development setup.

## CLI

```bash
dockpanel status              # Server status (CPU, RAM, disk)
dockpanel sites               # List all sites
dockpanel apps                # List Docker apps
dockpanel diagnose            # Run smart diagnostics
dockpanel export -o config.yml  # Export server config as YAML
dockpanel apply config.yml    # Apply config (Infrastructure as Code)
```

## Update / Uninstall

```bash
sudo bash /opt/dockpanel/scripts/update.sh     # Update
sudo bash /opt/dockpanel/scripts/uninstall.sh   # Remove
```

## Documentation

- [Live Docs](https://docs.dockpanel.dev) — Getting started, guides, configuration
- [FEATURES.md](FEATURES.md) — Complete feature manifest (60+ features, ~280 capabilities)
- [CHANGELOG.md](CHANGELOG.md) — Version history
- [SECURITY.md](SECURITY.md) — Security model and vulnerability reporting
- [CONTRIBUTING.md](CONTRIBUTING.md) — Development setup and PR process

## License

Business Source License 1.1. Free to use on your own servers. See [LICENSE](LICENSE) for details.
