<p align="center">
  <img src=".github/screenshots/dp-dashboard.png" alt="DockPanel Dashboard" width="800">
</p>

<h1 align="center">DockPanel</h1>

<p align="center">
  <strong>Your server. Your rules. Your panel.</strong><br>
  A free, self-hosted, Docker-native server management panel built in Rust.
</p>

<p align="center">
  <a href="https://github.com/ovexro/dockpanel/releases"><img src="https://img.shields.io/github/v/release/ovexro/dockpanel" alt="Release"></a>
  <a href="https://github.com/ovexro/dockpanel/actions"><img src="https://img.shields.io/github/actions/workflow/status/ovexro/dockpanel/ci.yml?label=CI" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-green.svg" alt="License: MIT"></a>
  <a href="https://demo.dockpanel.dev"><img src="https://img.shields.io/badge/demo-live-brightgreen" alt="Live Demo"></a>
</p>

<p align="center">
  <a href="https://demo.dockpanel.dev">Live Demo</a> &bull;
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

| | DockPanel | HestiaCP | CloudPanel | RunCloud |
|---|---|---|---|---|
| **Price** | Free | Free | Free | $8/mo+ |
| **Stack** | Rust + React | PHP | PHP | PHP (SaaS) |
| **Docker native** | Yes | No | No | No |
| **Multi-server** | Yes | No | No | Yes |
| **Git deploy** | Yes (blue-green) | No | No | Yes |
| **CLI + IaC** | Yes | Limited | No | No |
| **RAM usage** | ~60MB | ~200MB+ | ~150MB+ | SaaS |
| **ARM64** | Yes | Partial | No | No |
| **Self-hosted** | Yes | Yes | Yes | No |

## Screenshots

<details>
<summary><strong>Docker Apps</strong> — 54 one-click templates</summary>

![Docker Apps](.github/screenshots/dp-apps.png)
</details>

<details>
<summary><strong>Sites</strong> — PHP, Node.js, Python, static, reverse proxy</summary>

![Sites](.github/screenshots/dp-sites.png)
</details>

<details>
<summary><strong>Security</strong> — Firewall, Fail2Ban, SSH hardening, scanning</summary>

![Security](.github/screenshots/dp-security.png)
</details>

<details>
<summary><strong>Terminal</strong> — Full SSH in the browser</summary>

![Terminal](.github/screenshots/dp-terminal.png)
</details>

## Features

### Hosting
- **Sites** — Static, PHP (8.1-8.4), Node.js, Python, reverse proxy. Automatic Nginx config, SSL, PHP-FPM pools.
- **Databases** — MySQL/PostgreSQL in Docker. Built-in SQL browser. Auto-cleanup on site delete.
- **Docker Apps** — 54 templates (WordPress, Redis, PostgreSQL, Grafana, n8n, Gitea...). Compose stacks. Resource limits.
- **Git Deploy** — Push-to-deploy. Blue-green zero-downtime updates. Nixpacks (30+ languages). Preview environments.
- **WordPress Toolkit** — Multi-site dashboard, vulnerability scanning, security hardening, bulk updates.
- **CMS Install** — WordPress, Laravel, Drupal, Joomla, Symfony, CodeIgniter — one click.
- **Backups** — Scheduled, S3/SFTP remote destinations, one-click restore.

### Operations
- **Multi-Server** — Manage remote servers from one panel. Agent auto-registers.
- **DNS** — Cloudflare + PowerDNS. Zone templates, propagation checker, DNSSEC.
- **Mail** — Postfix + Dovecot + OpenDKIM. Webmail (Roundcube), spam filter (Rspamd), SMTP relay.
- **Monitoring** — HTTP/TCP/ping uptime checks, SLA tracking, public status pages, PagerDuty.
- **Terminal** — Full SSH with tabs, themes, sharing, session recording.

### Security
- **2FA/TOTP** — Two-factor authentication with recovery codes.
- **Firewall** — UFW management with smart port opener.
- **Fail2Ban** — View/ban/unban IPs, panel-specific jail.
- **SSH Hardening** — Disable password/root login, change port — one click.
- **Vulnerability Scanning** — Container scanning, file integrity, security headers.
- **Auto-Healing** — Restart crashed services, clean disk, renew expiring SSL.

### Developer Experience
- **CLI** — `dockpanel status`, `sites`, `apps`, `diagnose`, `export`, `apply`
- **Infrastructure as Code** — Export/import server config as YAML.
- **Smart Diagnostics** — 6 check categories with one-click fixes.
- **File Manager** — Browse, edit, upload files from the browser.
- **Command Palette** — Ctrl+K to navigate anywhere.

### Business
- **Reseller Accounts** — Admin → Reseller → User hierarchy with quotas.
- **White-Label** — Custom logo, colors, panel name per reseller.
- **OAuth/SSO** — Google, GitHub, GitLab login.
- **Extension API** — Webhook events with HMAC signing and scoped API keys.
- **Migration Wizard** — Import from cPanel, Plesk, HestiaCP.
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

**3 Rust binaries**: Agent (~20MB), API (~14MB), CLI (~1.7MB). Total RAM: ~60MB.

| Component | Tech | Role |
|-----------|------|------|
| Agent | Rust/Axum | Root-level host operations (Docker, Nginx, SSL, files) |
| API | Rust/Axum + SQLx | Auth, business logic, multi-server dispatch, background tasks |
| CLI | Rust/Clap | Command-line interface for automation |
| Frontend | React 19 + Vite + Tailwind 4 | Browser UI with 3 layout themes |

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
- [FEATURES.md](FEATURES.md) — Complete feature manifest (41 features, ~166 capabilities)
- [CHANGELOG.md](CHANGELOG.md) — Version history
- [SECURITY.md](SECURITY.md) — Security model and vulnerability reporting
- [CONTRIBUTING.md](CONTRIBUTING.md) — Development setup and PR process

## License

MIT. See [LICENSE](LICENSE).
