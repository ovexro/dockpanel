# DockPanel

**Your server. Your rules. Your panel.**

A free, self-hosted, Docker-native server management panel built in Rust. No subscriptions, no vendor lock-in, no artificial limits.

~10MB binary | 12MB RAM | Installs in <60 seconds | x86_64 + ARM64

[Live Demo](https://demo.dockpanel.dev) | [Website](https://dockpanel.dev)

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

- **Site Management** — Static, PHP (multiple versions), Node.js, and reverse proxy sites with automatic Nginx configuration
- **Free SSL** — Automatic Let's Encrypt provisioning and renewal
- **Database Management** — MySQL and PostgreSQL in Docker containers with credential management
- **Docker Apps** — 34 one-click templates across 10 categories (databases, CMS, monitoring, analytics, tools, dev, storage, media, networking, security) + Docker Compose import
- **Web Terminal** — Full SSH terminal in your browser via WebSocket
- **File Manager** — Browse, edit, upload, and download files from the browser
- **Backups** — Scheduled backups with S3-compatible remote destinations and one-click restore
- **Monitoring** — Real-time CPU, RAM, disk, and network dashboards with uptime monitoring
- **Alerts** — Configurable alert rules with email, Slack, and Discord notifications
- **Security** — Firewall management, Fail2Ban, SSH hardening, security scanning with scoring
- **DNS Management** — Built-in DNS zone management with full record CRUD
- **Git Deploy** — Connect Git repos for automated deployments via webhooks
- **Staging** — Create staging environments, sync from production, push to live
- **Cron Jobs** — Create, edit, and manage cron jobs with execution tracking
- **Teams** — Multi-user access with admin/user roles and team-based permissions
- **Activity Log** — Full audit trail of all panel actions
- **CLI** — Full command-line interface (`dockpanel status`, `dockpanel sites`, `dockpanel diagnose`, etc.)
- **Infrastructure as Code** — Export/import server config as YAML (`dockpanel export`, `dockpanel apply`)
- **Smart Diagnostics** — Pattern-based issue detection with one-click fixes
- **Auto-Healing** — Automatic restart of crashed services, log cleanup, SSL renewal
- **2FA** — TOTP two-factor authentication with recovery codes
- **Multi-Server** — Manage unlimited servers from a single dashboard
- **ARM64** — Runs on Raspberry Pi, Oracle Cloud free-tier ARM, and any ARM64 server

## Architecture

```
Browser
  │
  ├── React SPA (Vite + Tailwind)
  │
  └── Nginx (reverse proxy + static files)
        │
        ├── /api/* ──→ API (Rust/Axum, port 3080)
        │                 │
        │                 ├── PostgreSQL 16 (Docker, port 5450)
        │                 │
        │                 └── Agent (Unix socket)
        │                       │
        │                       └── Docker, Nginx, SSL, Files, Backups, Terminal
        │
        └── /* ────→ Frontend dist/ (static files)
```

- **Agent** — Rust binary, runs as root via systemd. Manages host resources (Docker, Nginx configs, SSL certs, file system, backups, terminal). Listens on Unix socket.
- **API** — Rust binary, runs via systemd. Handles auth, database operations, alert engine, and proxies commands to the agent.
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
├── panel/
│   ├── agent/          # Rust agent (host-level operations)
│   ├── backend/        # Rust API server
│   ├── cli/            # Rust CLI binary
│   └── frontend/       # React frontend
├── scripts/
│   ├── install.sh      # Quick installer (curl | bash)
│   ├── setup.sh        # Full setup script
│   ├── update.sh       # Update to latest version
│   └── uninstall.sh    # Complete removal
├── website/            # Marketing site (dockpanel.dev)
└── docs/               # Documentation
```

## Development

### Prerequisites

- Rust 1.75+ (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`)
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

## License

MIT. See [LICENSE](LICENSE).
