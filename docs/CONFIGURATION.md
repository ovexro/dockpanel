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
| `BASE_URL` | No | `https://panel.example.com` | Panel base URL (used for links in emails, webhooks) |
| `CORS_ORIGINS` | No | `https://panel.example.com` | Comma-separated list of allowed CORS origins |
| `LOG_FORMAT` | No | `text` | Set to `json` for JSON structured logging |
| `SECRETS_ENCRYPTION_KEY` | No | derived from `JWT_SECRET` | Dedicated key for encrypting stored credentials and Secrets Manager values. **Requires v2.112.0 or later — see the warning below before setting it on an existing install.** |
| `RUST_LOG` | No | `info` | Log level (`error`, `warn`, `info`, `debug`, `trace`) |

### `SECRETS_ENCRYPTION_KEY` — separating credential encryption from `JWT_SECRET`

By default the panel derives its credential-encryption key from `JWT_SECRET`.
Setting `SECRETS_ENCRYPTION_KEY` gives credentials their own key, so rotating the
JWT signing secret no longer touches encrypted data.

It governs two stores:

- stored credentials — database passwords, SMTP passwords, DKIM private keys,
  Cloudflare API tokens, TOTP secrets, backup-destination secrets, the WHMCS API
  secret, BunnyCDN provider keys, GitHub deploy tokens, and the per-server agent
  tokens the panel dials its fleet with
- the Secrets Manager vault, including the CMS admin password site creation
  writes there and any value marked for injection into a site's environment

> ⚠ **On v2.111.0 and earlier, setting this variable on an install that already
> holds data was silent, irreversible data loss**, and it is worth stating plainly
> because nothing documented the variable at the time. Those versions swapped the
> key rather than adding it: the key the panel had actually encrypted with was
> left out of the decrypt chain entirely, credentials came back as base64 text
> that every remote service then rejected, and TOTP verification failed closed —
> locking out every account with 2FA enabled. **If you run v2.111.0 or earlier, do
> not set this variable. Upgrade first.**
>
> From **v2.112.0** the panel tries every key derivation a value could have been
> written under, so adding, changing or removing the variable is survivable and
> reversible: put the old value back and the data reads again.

**Adding it to an existing install (v2.112.0+):**

1. Generate a key: `openssl rand -hex 32`
2. Add `SECRETS_ENCRYPTION_KEY=...` to `/etc/dockpanel/api.env` and restart the API.
   Existing data keeps working — it is read through the previous derivation.
3. Rewrite everything under the new key:
   `POST /api/settings/credentials/reencrypt` (admin only).
   The response reports, per store, how many values were `examined`, `rewritten`,
   `already_current` and `unreadable`. It is idempotent and safe to re-run.
4. Confirm `unreadable` is `0` before you discard the old key. Any value the panel
   could not open is reported and **left untouched** rather than overwritten.

**Changing or removing it** follows the same shape — change the value, restart,
re-run the re-encryption. Until step 3 completes the install is still relying on
the previous key being derivable, so do not rotate `JWT_SECRET` in the same
maintenance window.

Set it once, keep it backed up with your database, and treat losing it the same
way you would treat losing the database: without it, or without `JWT_SECRET`, the
encrypted values cannot be recovered.

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
