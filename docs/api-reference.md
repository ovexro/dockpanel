# API Reference

DockPanel exposes 371 REST endpoints across 37 categories. All endpoints except `/api/health`, `/api/branding`, `/api/auth/setup-status`, and `/api/auth/login` require authentication.

## Authentication

All authenticated requests require either:
- **Cookie**: `token=<JWT>` (set by login response)
- **Bearer**: `Authorization: Bearer <JWT>`

JWTs expire after 2 hours. Obtain one via `POST /api/auth/login`.

### Multi-server

Include `X-Server-Id: <uuid>` header to target a specific server. Omit for the local server.

---

## Auth (18 endpoints)

### `GET /api/auth/setup-status`
Check if initial admin setup is needed. **No auth required.**

**Response**: `{ "needs_setup": true }`

### `POST /api/auth/setup`
Create the initial admin account. Only works once.

```json
{ "email": "admin@example.com", "password": "SecurePass123!" }
```

### `POST /api/auth/login`
Authenticate and receive a JWT cookie.

```json
{ "email": "admin@example.com", "password": "SecurePass123!" }
```

**Response**: `{ "user": { "id": "uuid", "email": "...", "role": "admin" } }`
If 2FA enabled: `{ "requires_2fa": true, "temp_token": "..." }`

### `POST /api/auth/2fa/verify`
Complete 2FA challenge.

```json
{ "temp_token": "...", "code": "123456" }
```

### `POST /api/auth/logout`
Invalidate the current JWT.

### `GET /api/auth/me`
Get the authenticated user's profile.

### Other auth endpoints
| Method | Path | Purpose |
|--------|------|---------|
| POST | `/api/auth/register` | Create user account |
| POST | `/api/auth/verify-email` | Verify email token |
| POST | `/api/auth/forgot-password` | Request password reset |
| POST | `/api/auth/reset-password` | Reset password with token |
| POST | `/api/auth/change-password` | Change password (authenticated) |
| POST | `/api/auth/revoke-all` | Revoke all sessions |
| POST | `/api/auth/2fa/setup` | Get TOTP QR code |
| POST | `/api/auth/2fa/enable` | Enable 2FA with verification code |
| POST | `/api/auth/2fa/disable` | Disable 2FA |
| GET | `/api/auth/2fa/status` | Check 2FA status |
| GET | `/api/auth/oauth/{provider}` | Start OAuth flow (google/github/gitlab) |
| GET | `/api/auth/oauth/{provider}/callback` | OAuth callback |

---

## Sites (70 endpoints)

### `POST /api/sites`
Create a new site.

```json
{
  "domain": "example.com",
  "runtime": "php",
  "php_version": "8.3",
  "proxy_port": null,
  "app_command": null,
  "cms": "wordpress",
  "site_title": "My Site",
  "admin_email": "admin@example.com",
  "admin_user": "admin",
  "admin_password": "WpPass123!"
}
```

**Runtimes**: `static`, `php`, `proxy`, `node`, `python`
**CMS options**: `wordpress`, `laravel`, `drupal`, `joomla`, `symfony`, `codeigniter`

### `GET /api/sites`
List all sites for the authenticated user.

### `GET /api/sites/{id}`
Get site details.

### `DELETE /api/sites/{id}`
Delete site and all associated resources (database containers, nginx config, SSL, crons, backups).

### Files

| Method | Path | Body |
|--------|------|------|
| GET | `/api/sites/{id}/files?path=.` | List directory |
| GET | `/api/sites/{id}/files/read?path=index.html` | Read file content |
| PUT | `/api/sites/{id}/files/write` | `{ "path": "file.txt", "content": "..." }` |
| POST | `/api/sites/{id}/files/create` | `{ "path": "dir", "is_dir": true }` |
| POST | `/api/sites/{id}/files/rename` | `{ "from": "old.txt", "to": "new.txt" }` |
| DELETE | `/api/sites/{id}/files?path=file.txt` | Delete file |
| POST | `/api/sites/{id}/files/upload` | Multipart file upload |
| GET | `/api/sites/{id}/files/download?path=file.txt` | Download file |

### Backups

| Method | Path | Purpose |
|--------|------|---------|
| POST | `/api/sites/{id}/backups` | Create backup |
| GET | `/api/sites/{id}/backups` | List backups |
| POST | `/api/sites/{id}/backups/{backup_id}/restore` | Restore backup |
| DELETE | `/api/sites/{id}/backups/{backup_id}` | Delete backup |
| GET | `/api/sites/{id}/backup-schedule` | Get schedule |
| PUT | `/api/sites/{id}/backup-schedule` | Set schedule |

### Crons

| Method | Path | Body |
|--------|------|------|
| POST | `/api/sites/{id}/crons` | `{ "schedule": "*/5 * * * *", "command": "echo hi" }` |
| GET | `/api/sites/{id}/crons` | List crons |
| PUT | `/api/sites/{id}/crons/{cron_id}` | Update cron |
| DELETE | `/api/sites/{id}/crons/{cron_id}` | Delete cron |
| POST | `/api/sites/{id}/crons/{cron_id}/run` | Run immediately |

### SSL

| Method | Path | Purpose |
|--------|------|---------|
| POST | `/api/sites/{id}/ssl` | Provision Let's Encrypt cert |
| GET | `/api/sites/{id}/ssl` | Get SSL status |
| POST | `/api/sites/{id}/ssl/upload` | Upload custom certificate |

### Other site endpoints

| Method | Path | Purpose |
|--------|------|---------|
| PUT | `/api/sites/{id}/php` | Switch PHP version |
| PUT | `/api/sites/{id}/limits` | Set rate limits, upload size, PHP workers |
| GET | `/api/sites/{id}/provision-log` | SSE stream of provisioning progress |
| POST | `/api/sites/{id}/clone` | Clone site |
| GET/PUT | `/api/sites/{id}/env` | Environment variables |
| GET | `/api/sites/{id}/health` | HTTP health check |
| GET | `/api/sites/{id}/stats` | Bandwidth/traffic stats |
| GET | `/api/sites/{id}/access-logs` | Nginx access logs |
| GET | `/api/sites/{id}/php-errors` | PHP error log |
| POST/GET | `/api/sites/{id}/redirects` | URL redirects |
| POST/GET | `/api/sites/{id}/password-protect` | HTTP basic auth |
| POST/GET | `/api/sites/{id}/aliases` | Domain aliases |
| POST/GET/DELETE | `/api/sites/{id}/staging` | Staging environments |
| GET/POST | `/api/sites/{id}/wordpress/*` | WordPress management |

---

## Databases (7 endpoints)

### `POST /api/databases`
Create a MySQL or PostgreSQL database in a Docker container.

```json
{
  "site_id": "uuid",
  "name": "mydb",
  "engine": "postgres"
}
```

**Engines**: `postgres`, `mysql`, `mariadb`

### `GET /api/databases`
List all databases.

### `POST /api/databases/{id}/query`
Execute SQL query.

```json
{ "sql": "SELECT * FROM users LIMIT 10" }
```

### Other database endpoints

| Method | Path | Purpose |
|--------|------|---------|
| DELETE | `/api/databases/{id}` | Delete database + container |
| GET | `/api/databases/{id}/credentials` | Connection string |
| GET | `/api/databases/{id}/tables` | List tables |
| GET | `/api/databases/{id}/tables/{table}` | Table schema |

---

## Docker Apps (25 endpoints)

### `GET /api/apps/templates`
List available app templates (54 templates). **Admin only.**

### `POST /api/apps/deploy`
Deploy a Docker app. **Admin only.**

```json
{
  "template_id": "redis",
  "name": "my-redis",
  "port": 6379,
  "env": { "REDIS_PASSWORD": "secret" },
  "domain": "redis.example.com",
  "ssl_email": "admin@example.com",
  "memory_mb": 256,
  "cpu_percent": 50
}
```

Returns `202` with `deploy_id`. Stream progress via `GET /api/apps/deploy/{deploy_id}/log` (SSE).

### `GET /api/apps`
List running containers.

### Container lifecycle

| Method | Path | Purpose |
|--------|------|---------|
| POST | `/api/apps/{container_id}/start` | Start |
| POST | `/api/apps/{container_id}/stop` | Stop |
| POST | `/api/apps/{container_id}/restart` | Restart |
| POST | `/api/apps/{container_id}/update` | Pull latest image + redeploy |
| DELETE | `/api/apps/{container_id}` | Remove |
| GET | `/api/apps/{container_id}/logs` | Container logs |
| GET | `/api/apps/{container_id}/stats` | CPU/memory/network stats |
| GET | `/api/apps/{container_id}/env` | Environment variables |
| PUT | `/api/apps/{container_id}/env` | Update env vars (recreates container) |
| POST | `/api/apps/{container_id}/exec` | Execute command in container |
| GET | `/api/apps/{container_id}/volumes` | Volume mounts |
| POST | `/api/apps/{container_id}/snapshot` | Create backup image |

### Docker Compose

| Method | Path | Purpose |
|--------|------|---------|
| POST | `/api/apps/compose/parse` | Validate compose YAML |
| POST | `/api/apps/compose/deploy` | Deploy compose stack |

### Images & Registries

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/apps/images` | List images |
| POST | `/api/apps/images/prune` | Remove dangling images |
| DELETE | `/api/apps/images/{id}` | Remove image |
| GET | `/api/apps/registries` | List registries |
| POST | `/api/apps/registry-login` | Login to registry |
| POST | `/api/apps/registry-logout` | Logout |

---

## Docker Compose Stacks (8 endpoints)

### `POST /api/stacks`
Create a named stack from compose YAML.

```json
{
  "name": "my-stack",
  "yaml": "version: \"3\"\nservices:\n  web:\n    image: nginx:alpine"
}
```

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/stacks` | List stacks |
| GET | `/api/stacks/{id}` | Stack details |
| PUT | `/api/stacks/{id}` | Update YAML + redeploy |
| POST | `/api/stacks/{id}/start` | Start all services |
| POST | `/api/stacks/{id}/stop` | Stop all services |
| POST | `/api/stacks/{id}/restart` | Restart |
| DELETE | `/api/stacks/{id}` | Remove stack |

---

## Git Deploy (16 endpoints)

### `POST /api/git-deploys`
Create a git deployment.

```json
{
  "name": "my-app",
  "repo_url": "https://github.com/user/repo.git",
  "branch": "main",
  "domain": "app.example.com",
  "container_port": 3000,
  "auto_deploy": true,
  "build_context": ".",
  "preview_ttl_hours": 24
}
```

### `POST /api/git-deploys/{id}/deploy`
Trigger a build + deploy. Returns `202`. Stream via `GET /api/git-deploys/deploy/{deploy_id}/log` (SSE).

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/git-deploys` | List deployments |
| GET | `/api/git-deploys/{id}` | Details |
| PUT | `/api/git-deploys/{id}` | Update config |
| DELETE | `/api/git-deploys/{id}` | Remove |
| POST | `/api/git-deploys/{id}/keygen` | Generate SSH deploy key |
| GET | `/api/git-deploys/{id}/history` | Deploy history |
| POST | `/api/git-deploys/{id}/rollback/{history_id}` | Rollback to version |
| GET | `/api/git-deploys/{id}/logs` | Container logs |
| POST | `/api/git-deploys/{id}/start` | Start container |
| POST | `/api/git-deploys/{id}/stop` | Stop |
| POST | `/api/git-deploys/{id}/restart` | Restart |
| GET | `/api/git-deploys/{id}/previews` | Preview environments |
| DELETE | `/api/git-deploys/{id}/previews/{preview_id}` | Delete preview |

---

## Monitoring (13 endpoints)

### `POST /api/monitors`
Create an uptime monitor.

```json
{
  "name": "Google",
  "url": "https://google.com",
  "check_interval": 300,
  "monitor_type": "http",
  "alert_email": true,
  "alert_slack_url": "https://hooks.slack.com/...",
  "keyword": "OK"
}
```

**Types**: `http`, `https`, `tcp`, `ping`, `dns`

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/monitors` | List monitors |
| PUT | `/api/monitors/{id}` | Update |
| DELETE | `/api/monitors/{id}` | Delete |
| POST | `/api/monitors/{id}/check` | Force check now |
| GET | `/api/monitors/{id}/checks` | Check history |
| GET | `/api/monitors/{id}/incidents` | Downtime incidents |
| GET | `/api/monitors/{id}/uptime` | Uptime percentage |
| GET | `/api/monitors/{id}/chart` | Response time chart |
| GET | `/api/monitors/certificates` | SSL certificate dashboard |
| GET/POST | `/api/monitors/maintenance` | Maintenance windows |
| GET | `/api/status-page` | Public status page |
| POST | `/api/heartbeat/{monitor_id}/{token}` | Dead man's switch (no auth) |

---

## DNS (12 endpoints)

### `POST /api/dns/zones`
Create a DNS zone.

```json
{
  "domain": "example.com",
  "provider": "cloudflare",
  "cf_zone_id": "...",
  "cf_api_token": "..."
}
```

**Providers**: `cloudflare`, `powerdns`

### `POST /api/dns/zones/{id}/records`
Add a DNS record.

```json
{
  "type": "A",
  "name": "@",
  "content": "1.2.3.4",
  "ttl": 3600,
  "proxied": true
}
```

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/dns/zones` | List zones |
| DELETE | `/api/dns/zones/{id}` | Delete zone |
| GET | `/api/dns/zones/{id}/records` | List records |
| PUT | `/api/dns/zones/{id}/records/{record_id}` | Update record |
| DELETE | `/api/dns/zones/{id}/records/{record_id}` | Delete record |
| POST | `/api/dns/propagation` | Check propagation |
| POST | `/api/dns/health-check` | DNS health check |
| GET | `/api/dns/zones/{id}/dnssec` | DNSSEC status |
| GET | `/api/dns/zones/{id}/changelog` | Record change history |
| GET | `/api/dns/zones/{id}/analytics` | Query volume |

---

## Security (21 endpoints)

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/security/overview` | Security posture summary |
| POST | `/api/security/scan` | Run security scan |
| GET | `/api/security/scans` | Scan history |
| GET | `/api/security/scans/{id}` | Scan details |
| GET | `/api/security/posture` | Security score |
| GET | `/api/security/report` | Compliance report (HTML) |
| GET | `/api/security/firewall` | UFW rules |
| POST | `/api/security/firewall/rules` | Add rule |
| DELETE | `/api/security/firewall/rules/{number}` | Delete rule |
| GET | `/api/security/fail2ban` | Fail2Ban status |
| POST | `/api/security/fail2ban/ban` | Ban IP |
| POST | `/api/security/fail2ban/unban` | Unban IP |
| GET | `/api/security/fail2ban/{jail}/banned` | List banned |
| POST | `/api/security/ssh/disable-password` | Disable SSH password auth |
| POST | `/api/security/ssh/enable-password` | Enable SSH password auth |
| POST | `/api/security/ssh/disable-root` | Disable root login |
| POST | `/api/security/ssh/change-port` | Change SSH port |
| GET | `/api/security/login-audit` | Login history |
| POST | `/api/security/fix` | Apply security fix |
| POST | `/api/security/panel-jail/setup` | Create Fail2Ban jail for panel |
| GET | `/api/security/panel-jail/status` | Panel jail status |

---

## Alerts (8 endpoints)

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/alerts` | List alerts (filter by status/type) |
| GET | `/api/alerts/summary` | Count by severity |
| PUT | `/api/alerts/{id}/acknowledge` | Mark as seen |
| PUT | `/api/alerts/{id}/resolve` | Close alert |
| GET | `/api/alert-rules` | Get thresholds |
| PUT | `/api/alert-rules` | Update global rules |
| PUT | `/api/alert-rules/{server_id}` | Per-server rules |
| DELETE | `/api/alert-rules/{server_id}` | Remove server overrides |

---

## Mail (39 endpoints)

### `POST /api/mail/install`
Install Postfix + Dovecot + OpenDKIM. **Admin only.**

### `POST /api/mail/domains`
Add a mail domain.

### Key mail endpoints

| Category | Endpoints |
|----------|-----------|
| Domains | CRUD `/api/mail/domains`, `/api/mail/domains/{id}` |
| Accounts | CRUD `/api/mail/domains/{id}/accounts` |
| Aliases | CRUD `/api/mail/domains/{id}/aliases` |
| DNS | `/api/mail/domains/{id}/dns`, `/api/mail/domains/{id}/dns-check` |
| Queue | GET `/api/mail/queue`, POST `flush`, DELETE `{queue_id}` |
| Spam | `/api/mail/rspamd/install`, `status`, `toggle` |
| Webmail | `/api/mail/webmail/install`, `status`, `remove` |
| Relay | `/api/mail/relay/configure`, `status`, `remove` |
| TLS | `/api/mail/tls/status`, `enforce` |
| Rate limit | `/api/mail/rate-limit/set`, `status`, `remove` |
| Logs | GET `/api/mail/logs` |
| Storage | GET `/api/mail/storage` |
| Backup | POST `/api/mail/backup`, GET `backups`, POST `restore` |
| Reputation | GET `/api/mail/blacklist-check` |

---

## Servers (10 endpoints)

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/servers` | List servers |
| POST | `/api/servers` | Add remote server |
| GET | `/api/servers/{id}` | Server details + metrics |
| PUT | `/api/servers/{id}` | Update name/IP/URL |
| DELETE | `/api/servers/{id}` | Remove server |
| POST | `/api/servers/{id}/test` | Test agent connectivity |
| GET | `/api/servers/{id}/metrics` | Historical metrics |
| POST | `/api/servers/{id}/rotate-token` | Rotate agent token |
| GET | `/api/servers/{id}/commands` | List dispatched commands |
| POST | `/api/servers/{id}/commands` | Dispatch command to agent |

---

## Extensions (7 endpoints)

### `POST /api/extensions`
Create a webhook integration.

```json
{
  "name": "My Webhook",
  "webhook_url": "https://example.com/hook",
  "event_subscriptions": "site.created,site.deleted,backup.completed",
  "api_scopes": "sites:read,monitors:read"
}
```

**Events**: `site.created`, `site.deleted`, `backup.completed`, `deploy.started`, `deploy.completed`, `app.deployed`, `auth.login`, `ssl.provisioned`

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/extensions` | List |
| PUT | `/api/extensions/{id}` | Update |
| DELETE | `/api/extensions/{id}` | Delete |
| POST | `/api/extensions/{id}/test` | Send test event |
| POST | `/api/extensions/{id}/rotate-secret` | Rotate HMAC secret |
| GET | `/api/extensions/{id}/events` | Delivery log |

---

## Other Endpoints

### Users (4) — Admin only
| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/users` | List users |
| POST | `/api/users` | Create user |
| PUT | `/api/users/{id}` | Update role |
| DELETE | `/api/users/{id}` | Delete user |

### Teams (7)
| Method | Path | Purpose |
|--------|------|---------|
| GET/POST | `/api/teams` | List/create teams |
| DELETE | `/api/teams/{id}` | Delete team |
| POST | `/api/teams/{id}/invite` | Invite member |
| POST | `/api/teams/accept` | Accept invitation |
| PUT/DELETE | `/api/teams/{id}/members/{member_id}` | Update/remove member |

### Resellers (14) — Admin creates, reseller manages
| Method | Path | Purpose |
|--------|------|---------|
| GET/POST | `/api/resellers` | List/create reseller profiles |
| GET/PUT/DELETE | `/api/resellers/{id}` | Manage profile |
| GET/POST/DELETE | `/api/resellers/{id}/servers` | Server allocation |
| GET | `/api/reseller/dashboard` | Reseller's dashboard |
| GET/POST/PUT/DELETE | `/api/reseller/users` | Reseller's sub-users |

### API Keys (4)
| Method | Path | Purpose |
|--------|------|---------|
| GET/POST | `/api/api-keys` | List/create |
| DELETE | `/api/api-keys/{id}` | Revoke |
| POST | `/api/api-keys/{id}/rotate` | Rotate key |

### System (10)
| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/system/info` | CPU, RAM, disk, OS |
| GET | `/api/system/processes` | Running processes |
| GET | `/api/system/network` | Network stats |
| GET | `/api/system/disk-io` | Disk I/O |
| POST | `/api/system/cleanup` | Clean temp files |
| POST | `/api/system/hostname` | Change hostname |
| GET | `/api/system/updates` | Available updates |
| GET | `/api/system/updates/count` | Update count |
| POST | `/api/system/updates/apply` | Apply updates |
| POST | `/api/system/reboot` | Reboot server |

### Settings (7)
| Method | Path | Purpose |
|--------|------|---------|
| GET/PUT | `/api/settings` | Panel settings |
| GET | `/api/settings/health` | Health check (DB + agent) |
| POST | `/api/settings/smtp/test` | Test email delivery |
| POST | `/api/settings/test-webhook` | Test Slack/Discord webhook |
| GET | `/api/settings/export` | Export config as JSON |
| POST | `/api/settings/import` | Import config |

### Logs (10)
| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/logs` | System logs |
| GET | `/api/logs/search` | Search logs |
| GET | `/api/logs/stats` | Log statistics |
| GET | `/api/logs/sizes` | Log file sizes |
| POST | `/api/logs/truncate` | Truncate log |
| GET | `/api/logs/docker` | List Docker containers |
| GET | `/api/logs/docker/{container}` | Container logs |
| GET | `/api/logs/service/{service}` | Service logs |
| POST | `/api/logs/check-errors` | Find error patterns |
| GET | `/api/logs/stream/token` | Get WebSocket token for live streaming |

### Diagnostics (2)
| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/agent/diagnostics` | Run diagnostics (6 categories) |
| POST | `/api/agent/diagnostics/fix` | Apply one-click fix |

### Terminal (3)
| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/terminal/token` | Get WebSocket ticket |
| POST | `/api/terminal/share` | Create shareable terminal link |
| GET | `/api/terminal/shared/{id}` | View shared terminal |

### Migration (6)
| Method | Path | Purpose |
|--------|------|---------|
| POST | `/api/migration/analyze` | Analyze cPanel/Plesk/HestiaCP backup |
| GET | `/api/migration` | List migrations |
| GET | `/api/migration/{id}` | Migration details |
| POST | `/api/migration/{id}/import` | Start import |
| GET | `/api/migration/{id}/progress` | Import progress (SSE) |
| DELETE | `/api/migration/{id}` | Delete migration |

### WordPress Toolkit (2 global)
| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/wordpress/sites` | Scan all sites for WordPress |
| POST | `/api/wordpress/bulk-update` | Update plugins/themes across sites |

### Other
| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/health` | API health (no auth) |
| GET | `/api/branding` | Panel branding (no auth) |
| GET | `/api/dashboard/intelligence` | Health score + issues |
| GET | `/api/dashboard/metrics-history` | Historical charts |
| GET | `/api/dashboard/docker` | Docker summary |
| GET/POST | `/api/ssh-keys` | SSH key management |
| DELETE | `/api/ssh-keys/{fingerprint}` | Remove SSH key |
| GET/POST | `/api/panel-whitelist` | IP whitelist |
| GET/POST/POST | `/api/auto-updates/*` | Auto-update management |
| GET/POST | `/api/backup-destinations` | Remote backup targets |
| POST | `/api/traefik/install` | Install Traefik |
| GET | `/api/traefik/status` | Traefik status |
| POST | `/api/traefik/uninstall` | Remove Traefik |
| GET | `/api/ws/metrics` | WebSocket live metrics |
| GET | `/api/activity` | Activity audit log |
| GET | `/api/system-logs` | System event log |
| GET/POST | `/api/services/*` | Service installers (PHP, Certbot, UFW, Fail2Ban) |

---

## Backup Orchestrator

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/backup-orchestrator/health` | Health dashboard |
| GET | `/api/backup-orchestrator/storage-history` | Storage growth (30 days) |
| POST | `/api/backup-orchestrator/policies` | Create backup policy |
| POST | `/api/backup-orchestrator/policies/protect-all` | One-click protect-all |
| GET | `/api/backup-orchestrator/policies` | List policies |
| PUT | `/api/backup-orchestrator/policies/{id}` | Update policy |
| DELETE | `/api/backup-orchestrator/policies/{id}` | Delete policy |
| POST | `/api/backup-orchestrator/db-backups/{db_name}` | Create DB backup |
| GET | `/api/backup-orchestrator/db-backups` | List DB backups |
| POST | `/api/backup-orchestrator/db-backups/{id}/restore` | Restore DB backup |
| DELETE | `/api/backup-orchestrator/db-backups/{id}` | Delete DB backup |
| POST | `/api/backup-orchestrator/vol-backups` | Create volume backup |
| GET | `/api/backup-orchestrator/vol-backups` | List volume backups |
| POST | `/api/backup-orchestrator/volume-backups/{id}/restore` | Restore volume backup |
| DELETE | `/api/backup-orchestrator/vol-backups/{id}` | Delete volume backup |
| GET | `/api/backup-orchestrator/verifications` | List verifications |
| POST | `/api/backup-orchestrator/verify/{id}` | Verify a backup |

---

## Incident Management

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/incidents` | List incidents |
| POST | `/api/incidents` | Create incident |
| GET | `/api/incidents/{id}` | Get incident |
| PUT | `/api/incidents/{id}` | Update incident |
| POST | `/api/incidents/{id}/updates` | Post timeline update |
| GET | `/api/incidents/{id}/updates` | Get timeline |
| DELETE | `/api/incidents/{id}` | Delete incident |

---

## Status Page

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/status-page/config` | Get status page config |
| PUT | `/api/status-page/config` | Update config |
| GET | `/api/status-page/components` | List components |
| POST | `/api/status-page/components` | Create component |
| PUT | `/api/status-page/components/{id}` | Update component |
| DELETE | `/api/status-page/components/{id}` | Delete component |
| POST | `/api/status-page/subscribers` | Subscribe email |
| DELETE | `/api/status-page/subscribers/{token}` | Unsubscribe |
| GET | `/status` | Public status page (no auth) |

---

## Secrets Manager

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/secrets/vaults` | List vaults |
| POST | `/api/secrets/vaults` | Create vault |
| DELETE | `/api/secrets/vaults/{id}` | Delete vault |
| GET | `/api/secrets/vaults/{id}/secrets` | List secrets |
| POST | `/api/secrets/vaults/{id}/secrets` | Create secret |
| PUT | `/api/secrets/vaults/{id}/secrets/{sid}` | Update secret |
| DELETE | `/api/secrets/vaults/{id}/secrets/{sid}` | Delete secret |
| GET | `/api/secrets/vaults/{id}/secrets/{sid}/versions` | Version history |
| POST | `/api/secrets/vaults/{id}/inject` | Inject secrets to .env |
| GET | `/api/secrets/vaults/{id}/pull` | Pull secrets (CLI) |
| GET | `/api/secrets/vaults/{id}/export` | Export vault |
| POST | `/api/secrets/vaults/{id}/import` | Import vault |

---

## Webhook Gateway

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/webhook-gateway/endpoints` | List endpoints |
| POST | `/api/webhook-gateway/endpoints` | Create endpoint |
| DELETE | `/api/webhook-gateway/endpoints/{id}` | Delete endpoint |
| GET | `/api/webhook-gateway/endpoints/{id}/deliveries` | List deliveries |
| POST | `/api/webhook-gateway/endpoints/{id}/replay/{did}` | Replay delivery |
| GET | `/api/webhook-gateway/endpoints/{id}/routes` | List routes |
| POST | `/api/webhook-gateway/endpoints/{id}/routes` | Create route |
| DELETE | `/api/webhook-gateway/routes/{id}` | Delete route |

---

## Notifications

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/notifications` | List notifications |
| GET | `/api/notifications/unread-count` | Unread badge count |
| POST | `/api/notifications/{id}/read` | Mark as read |
| POST | `/api/notifications/read-all` | Mark all read |
| GET | `/api/notifications/stream` | SSE real-time stream |

---

## Sessions

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/auth/sessions` | List active sessions |
| DELETE | `/api/auth/sessions/{id}` | Revoke session |
| GET | `/api/auth/export-my-data` | GDPR data export |

---

## Deploy Approvals

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/deploy-approvals` | List pending approvals |
| POST | `/api/deploy-approvals/{id}/approve` | Approve deploy |
| POST | `/api/deploy-approvals/{id}/reject` | Reject deploy |
