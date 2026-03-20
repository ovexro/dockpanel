# DockPanel

[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)
[![Release](https://img.shields.io/github/v/release/ovexro/dockpanel)](https://github.com/ovexro/dockpanel/releases)
[![Build](https://img.shields.io/github/actions/workflow/status/ovexro/dockpanel/release.yml)](https://github.com/ovexro/dockpanel/actions)

**Your server. Your rules. Your panel.**

A free, self-hosted, Docker-native server management panel built in Rust. No subscriptions, no vendor lock-in, no artificial limits.

v2.0.3 | ~35MB binaries | ~60MB RAM | Installs in <60 seconds | x86_64 + ARM64

[Live Demo](https://panel.example.com) | [Website](https://dockpanel.dev) | [Docs](https://docs.dockpanel.dev) | [Discussions](https://github.com/ovexro/dockpanel/discussions) | [Changelog](CHANGELOG.md)

## Quick Start

```bash
curl -sL https://dockpanel.dev/install.sh | sudo bash
```

Or clone and run manually:

```bash
git clone https://github.com/ovexro/dockpanel.git /opt/dockpanel
cd /opt/dockpanel
sudo bash scripts/setup.sh
```

After installation, open `http://YOUR_SERVER_IP:8443` and create your admin account.

## Features

### Hosting & Sites
- **Site Management** — Static, PHP (multiple versions), Node.js, Python, and reverse proxy sites with automatic Nginx configuration
- **Free SSL** — Automatic Let's Encrypt provisioning and renewal
- **Database Management** — MySQL and PostgreSQL in Docker containers with built-in SQL browser
- **Docker Apps** — 54 one-click templates across 10 categories + Docker Compose stack management
- **Git Deploy** — Push-to-deploy with blue-green zero-downtime updates, Nixpacks auto-detection (30+ languages), preview environments with TTL cleanup
- **WordPress Toolkit** — Multi-site dashboard, vulnerability scanning (14+ known exploits), security hardening (7 checks, 6 auto-fixable), bulk updates
- **One-Click CMS** — WordPress, Laravel, Drupal, Joomla, Symfony, CodeIgniter with database + SSL provisioning
- **Staging** — Create staging environments, sync from production, push to live

### Infrastructure
- **Multi-Server** — Manage unlimited remote servers from a single panel with per-server resource scoping
- **Traefik Option** — Choose nginx or Traefik as reverse proxy for Docker apps (auto-SSL, service discovery)
- **DNS Management** — Cloudflare + PowerDNS with zone templates, propagation checker, DNSSEC
- **Email Management** — Full mail server (Postfix + Dovecot + OpenDKIM), webmail, spam filtering, SMTP relay
- **Backups** — Scheduled backups with S3-compatible remote destinations and one-click restore

### Monitoring & Security
- **Real-Time Dashboards** — CPU, RAM, disk, network, Docker containers, historical sparkline charts
- **Uptime Monitoring** — HTTP/TCP/ping checks with SLA tracking, public status pages, PagerDuty
- **Alerts** — CPU/memory/disk thresholds, server offline, SSL expiry, service health — email, Slack, Discord
- **Security** — Firewall, Fail2Ban, SSH hardening, vulnerability scanning, compliance reports, auto-healing
- **2FA** — TOTP two-factor authentication with recovery codes

### Developer Experience
- **Web Terminal** — Full SSH terminal with tabs, themes, sharing, session recording
- **File Manager** — Browse, edit, upload, and download files from the browser
- **CLI** — Full command-line interface (`dockpanel status`, `dockpanel sites`, `dockpanel diagnose`)
- **Infrastructure as Code** — Export/import server config as YAML (`dockpanel export`, `dockpanel apply`)
- **Smart Diagnostics** — Pattern-based issue detection with one-click fixes

### Business Features
- **Reseller Accounts** — Admin → Reseller → User hierarchy with quotas, server allocation, and branding
- **White-Label** — Custom logo, panel name, accent color, and hide DockPanel branding per reseller
- **OAuth / SSO** — Login via Google, GitHub, or GitLab (OAuth 2.0)
- **Extension API** — Webhook-based integrations with HMAC-signed event delivery and scoped API keys
- **Migration Wizard** — Import sites, databases, and email from cPanel, Plesk, or HestiaCP
- **Teams** — Multi-user access with admin/reseller/user roles and team-based permissions
- **Activity Log** — Full audit trail of all panel actions
- **ARM64** — Runs on Raspberry Pi, Oracle Cloud free-tier ARM, and any ARM64 server

## Architecture

```
Browser
  |
  +-- React SPA (Vite + Tailwind)
  |
  +-- Nginx (reverse proxy + static files)
        |
        +-- /api/* --> API (Rust/Axum, port 3080)
        |                 |
        |                 +-- PostgreSQL 16 (Docker, port 5450)
        |                 |
        |                 +-- Agent (Unix socket or HTTPS for remote)
        |                       |
        |                       +-- Docker, Nginx, SSL, Files, Backups, Terminal
        |
        +-- /* -----> Frontend dist/ (static files)
```

- **Agent** — Rust binary, runs as root via systemd. Manages host resources (Docker, Nginx, SSL, file system, backups, terminal). Unix socket locally, HTTPS for remote servers. In multi-server mode (`AGENT_LISTEN_TCP` env var), the agent listens on a TCP port for remote panel connections. When configured with `DOCKPANEL_CENTRAL_URL`, it periodically reports metrics to the central panel — this is opt-in and only used for multi-server management.
- **API** — Rust binary, runs via systemd. Handles auth, database operations, alert engine, multi-server dispatch, and proxies commands to agents.
- **Frontend** — React 19 SPA with lazy-loaded pages. Served directly by Nginx from `dist/`.

## Requirements

- Ubuntu 20.04+, Debian 11+, CentOS 9+, Rocky Linux 9+, Fedora 39+, or Amazon Linux 2023
- x86_64 or ARM64 (aarch64)
- 512MB RAM minimum (1GB recommended)
- Docker (installed automatically)
- Nginx (installed automatically)

## Directory Structure

```
dockpanel/
+-- panel/
|   +-- agent/          # Rust agent (host-level operations)
|   +-- backend/        # Rust API server
|   +-- cli/            # Rust CLI binary
|   +-- frontend/       # React frontend
+-- scripts/
|   +-- install.sh      # Quick installer (curl | bash)
|   +-- install-agent.sh # Remote agent installer (multi-server)
|   +-- setup.sh        # Full setup script
|   +-- update.sh       # Update to latest version
|   +-- uninstall.sh    # Complete removal
+-- website/            # Marketing site (dockpanel.dev)
```

## Development

### Prerequisites

- Rust 1.94+ (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`)
- Node.js 20+ and npm

### Build

```bash
# Agent
cd panel/agent && cargo build --release

# API
cd panel/backend && cargo build --release

# CLI
cd panel/cli && cargo build --release

# Frontend
cd panel/frontend && npm install && npx vite build
```

### Running Locally

1. Start PostgreSQL (Docker): `docker run -d --name dockpanel-postgres -e POSTGRES_USER=dockpanel -e POSTGRES_PASSWORD=dockpanel -e POSTGRES_DB=dockpanel -p 5450:5432 postgres:16`
2. Create `/etc/dockpanel/api.env` with: `DATABASE_URL=postgres://dockpanel:dockpanel@localhost:5450/dockpanel` and `JWT_SECRET=<random-64-char-hex>`
3. Start the agent: `sudo ./panel/agent/target/release/dockpanel-agent`
4. Start the API: `./panel/backend/target/release/dockpanel-api`
5. Start the frontend dev server: `cd panel/frontend && npm run dev`

### CLI Usage

```bash
dockpanel status              # Server status
dockpanel sites               # List sites
dockpanel apps                # List Docker apps
dockpanel diagnose            # Run diagnostics
dockpanel export -o config.yml  # Export server config
dockpanel apply config.yml    # Apply config from YAML
```

## Update

```bash
sudo bash /opt/dockpanel/scripts/update.sh
```

## Uninstall

```bash
sudo bash /opt/dockpanel/scripts/uninstall.sh
```

## Documentation

- [FEATURES.md](FEATURES.md) — Complete feature manifest with implementation details
- [CHANGELOG.md](CHANGELOG.md) — Version history and release notes
- [SECURITY.md](SECURITY.md) — Security model and vulnerability reporting
- [docs/CONFIGURATION.md](docs/CONFIGURATION.md) — Environment variables and directory structure
- [CONTRIBUTING.md](CONTRIBUTING.md) — Development setup and contribution guidelines

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, code style, and PR process.

## License

MIT. See [LICENSE](LICENSE).
