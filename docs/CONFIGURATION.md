# Configuration Reference

## Environment Variables

### API Server (`dockpanel-api`)

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `DATABASE_URL` | Yes | — | PostgreSQL connection string (e.g., `postgresql://user:pass@host:5432/dbname`) |
| `JWT_SECRET` | Yes | — | Secret for signing JWT tokens. Must be at least 32 characters. Generate with: `openssl rand -hex 32` |
| `AGENT_TOKEN` | Yes | — | Shared secret for authenticating with the agent. Must match the agent's token file. |
| `AGENT_SOCKET` | No | `/var/run/dockpanel/agent.sock` | Path to the agent's Unix socket |
| `LISTEN_ADDR` | No | `0.0.0.0:3000` | Address and port the API listens on |
| `DB_MAX_CONNECTIONS` | No | `20` | Maximum PostgreSQL connection pool size |
| `BASE_URL` | No | `https://demo.dockpanel.dev` | Panel base URL (used for links in emails, webhooks) |
| `CORS_ORIGINS` | No | `https://demo.dockpanel.dev` | Comma-separated list of allowed CORS origins |
| `LOG_FORMAT` | No | `text` | Set to `json` for JSON structured logging |
| `STRIPE_SECRET_KEY` | No | — | Stripe secret key (only if billing is enabled) |
| `STRIPE_WEBHOOK_SECRET` | No | — | Stripe webhook signing secret (only if billing is enabled) |
| `RUST_LOG` | No | `info` | Log level (`error`, `warn`, `info`, `debug`, `trace`) |

### Docker Compose (`.env` file in `panel/`)

| Variable | Description |
|----------|-------------|
| `PANEL_DB_PASSWORD` | PostgreSQL password for the panel database |
| `PANEL_JWT_SECRET` | JWT signing secret (passed to API container) |
| `AGENT_TOKEN` | Agent authentication token |

### Agent (`dockpanel-agent`)

The agent reads its configuration from files, not environment variables:

| File | Description |
|------|-------------|
| `/etc/dockpanel/agent.token` | Authentication token (auto-generated on first run) |
| `/etc/dockpanel/ssl/` | SSL certificates and ACME account |

| Environment Variable | Default | Description |
|---------------------|---------|-------------|
| `RUST_LOG` | `info` | Log level |
| `LOG_FORMAT` | `text` | Set to `json` for JSON structured logging |

## Directory Structure

| Path | Purpose |
|------|---------|
| `/etc/dockpanel/` | Configuration directory |
| `/etc/dockpanel/agent.token` | Agent authentication token |
| `/etc/dockpanel/api.env` | API environment file (systemd deployments) |
| `/etc/dockpanel/ssl/` | SSL certificates per domain |
| `/etc/dockpanel/ssl/acme-account.json` | Let's Encrypt ACME account credentials |
| `/var/run/dockpanel/agent.sock` | Agent Unix socket |
| `/var/backups/dockpanel/` | Site backups (compressed tarballs) |
| `/var/www/acme/` | ACME HTTP-01 challenge webroot |
| `/var/www/{domain}/` | Site document roots |

## Ports

| Port | Service | Configurable |
|------|---------|-------------|
| 8443 | Panel Nginx (default) | `PANEL_PORT` env var in setup.sh |
| 3000 | API (inside Docker) | `LISTEN_ADDR` env var |
| 3062 | API (Docker host mapping) | `docker-compose.yml` |
| 3063 | Frontend (Docker host mapping) | `docker-compose.yml` |
| 5432 | PostgreSQL (inside Docker) | Internal only |

## Generating Secrets

```bash
# JWT secret (64 hex chars = 32 bytes)
openssl rand -hex 32

# Database password
openssl rand -hex 24

# Agent token
openssl rand -hex 16
```

## Systemd Deployments

For non-Docker API deployments, create `/etc/dockpanel/api.env`:

```bash
DATABASE_URL=postgresql://user:password@127.0.0.1:5432/dockpanel
JWT_SECRET=your_64_char_hex_secret_here
AGENT_SOCKET=/var/run/dockpanel/agent.sock
AGENT_TOKEN=your_agent_token_here
LISTEN_ADDR=127.0.0.1:3080
```

Set permissions: `chmod 600 /etc/dockpanel/api.env`

Reference it in the systemd service:
```ini
[Service]
EnvironmentFile=/etc/dockpanel/api.env
```
