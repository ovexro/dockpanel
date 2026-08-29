# API Reference

DockPanel exposes 840 REST endpoints (546 backend + 294 agent) across 50+ categories — the figure derived from the two routers in `FEATURES.md` §Verified Metrics, which owns it. The tables below document the commonly used subset, not every route. All endpoints except `/api/health`, `/api/branding`, `/api/auth/setup-status`, and `/api/auth/login` require authentication.

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
| POST | `/api/auth/2fa/disable` | Disable 2FA (TOTP or recovery code) |
| POST | `/api/auth/2fa/recovery-codes` | Issue a fresh set of recovery codes |
| GET | `/api/auth/2fa/status` | 2FA state, enforcement, recovery codes remaining |
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
| POST | `/api/sites/{id}/files/upload` | Upload a file — JSON body, `content` base64-encoded, 1.5 MB limit |
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
| GET | `/api/sites/{id}/ssl` | Get SSL status, including who issued the certificate |
| POST | `/api/sites/{id}/ssl/upload` | Upload custom certificate |

`GET /api/sites/{id}/ssl` returns `provenance`, one of:

| Value | Meaning |
|-------|---------|
| `dockpanel` | Issued by Let's Encrypt, so DockPanel issued it and renews it |
| `foreign` | Issued by somebody else; `issuer` names them, and DockPanel will not renew it |
| `unknown` | No answer to act on — the agent was unreachable, or the site is served by a certificate stored under its zone's name rather than its own |

⛔ `unknown` must never be rendered as a CA name. It is what an unreachable agent
returns, and a certificate that is real and healthy sits behind it.

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

## Databases (14 endpoints)

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
| GET | `/api/databases/{id}/indexes/{table}` | Table indexes |
| GET | `/api/databases/{id}/foreign-keys` | Foreign-key map |
| GET | `/api/databases/{id}/schema-overview` | Tables + relationships in one call |
| GET/PUT | `/api/databases/{id}/pitr` | Read/store a PITR retention preference. Point-in-time recovery is **not implemented** — nothing is archived and `POST /pitr/restore` answers 501 |
| POST | `/api/databases/{id}/reset-password` | Rotate the database password |
| GET | `/api/databases/{id}/dumps` | **Admin only.** List dumps the operator placed in this database's backup directory, including files that cannot be imported and why |
| POST | `/api/databases/{id}/import` | **Admin only.** Import one of those dumps. Body: `{"filename": "mydb-20260821-120000.sql.gz"}`. Answers `504` with an explanation if the import outlasts the panel's 270s wait — that is not a failure |

---

## Docker Apps (25 endpoints)

### `GET /api/apps/templates`
List available app templates (146 templates across 14 categories). **Admin only.**

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
| POST | `/api/apps/{container_id}/update` | Pull latest image + redeploy. Refuses, leaving the container running, when the image cannot be pulled and the local copy is either absent or already the one in use |
| DELETE | `/api/apps/{container_id}` | Remove |
| GET | `/api/apps/{container_id}/logs` | Container logs |
| GET | `/api/apps/{container_id}/stats` | CPU/memory/network stats |
| GET | `/api/apps/{container_id}/env` | Environment variables |
| PUT | `/api/apps/{container_id}/env` | Update env vars (recreates container) |
| POST | `/api/apps/{container_id}/exec` | Execute command in container |
| GET | `/api/apps/{container_id}/volumes` | Volume mounts |
| POST | `/api/apps/{container_id}/snapshot` | Create backup image |

`PUT /api/apps/{container_id}/env` replaces the container's environment with the
object you send: a name you omit is removed, and a value sent back as `********`
means "leave this one alone" (the container is the only place an app's
environment is stored, so the masked read is what the panel has to reconcile
against). Names may be added freely — the same bounds the deploy endpoint
enforces apply here: at most 50 variables, names up to 255 characters, values up
to 4KB, and a name may not contain `=` or a null byte.

**Domain-derived variables.** When a template declares a variable holding the
app's own public address and you claim a domain at deploy time, `POST
/api/apps/deploy` fills that variable in from the domain rather than leaving the
template's `localhost` default. It applies to `n8n`, `ghost`, `plausible`,
`drone`, `photoprism` and `graylog`; a value you send explicitly always wins.
The template objects returned by `GET /api/apps/templates` mark such variables
with `domain_derived: true`.

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
Create a named stack from compose YAML, optionally fronted by a domain.

```json
{
  "name": "my-stack",
  "yaml": "version: \"3\"\nservices:\n  web:\n    image: nginx:alpine",
  "domain": "app.example.com",
  "tls_mode": "provided",
  "tls_certificate": "example-com-wildcard"
}
```

`tls_mode` is how the domain is served and is **stored on the stack**: `none`
(plain HTTP, or TLS terminated upstream), `acme` (a Let's Encrypt certificate
ordered under `ssl_email`) or `provided` (a certificate registered under the alias
in `tls_certificate` — see the registry below). Omitted, it is derived from
`ssl_email` exactly as every client before v2.160.0 meant it. A `provided` claim is
refused before any row is written when the alias is unknown on the stack's server,
when that server's agent predates v2.160.0, or when the certificate does not name
the domain — the refusal says which names it does cover.

`PUT /api/stacks/{id}` treats an **absent** field as *keep*: the mode, `ssl_email`,
the alias and `domain` itself stay as stored unless the request names them. An
explicit `"domain": null` vacates the domain (and a vacated domain has no mode).
Before v2.160.0 the request's `ssl_email` was forwarded verbatim, so an edit that
omitted it rewrote the vhost without TLS.

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

## TLS certificate registry (4 endpoints)

A certificate registered once, under an alias, and served by any number of Compose
stacks that claim a domain it covers (#104). The pair lives on that server's agent,
beside the per-domain tree; the panel keeps the alias and the metadata. Administrators only.

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/tls-certificates` | List the certificates registered on the scoped server, with names, issuer, expiry, status and the stacks using each |
| POST | `/api/tls-certificates` | Register `{alias, certificate, private_key}` — the alias is 1–64 lowercase letters, digits or hyphens; the key must belong to the certificate; 409 if the alias is taken |
| PUT | `/api/tls-certificates/{id}` | Replace the pair behind an alias; refused if the new certificate stops covering a domain a stack serves under it |
| DELETE | `/api/tls-certificates/{id}` | Remove it; 409 naming the stacks while any still uses it |

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
Trigger a build + deploy. Returns `202` in **two different shapes**, and a client that
reads only `deploy_id` will treat the second as a silent success:

```json
{ "deploy_id": "…", "message": "Deployment started" }
```
The build has started. Stream it via `GET /api/git-deploys/deploy/{deploy_id}/log` (SSE).

```json
{ "status": "pending_approval", "message": "Deploy requires approval from another admin …" }
```
The deployment has `deploy_protected` set, so **nothing was built**. A request was filed
and a different administrator must resolve it — see *Deploy Approvals* below. There is no
`deploy_id`, and none exists until the approval is granted. Filing a request is
idempotent: a deployment may have only one waiting at a time, and a repeat call says so
rather than queueing a second.

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

## Security (23 endpoints)

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
| GET | `/api/security/canary-status` | Canary tripwire state — which paths are watched, absent or masked |
| POST | `/api/security/canary/arm` | Plant the canary files the agent can write |

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

**Account passwords** are hashed with **Argon2id** and stored in Dovecot's
`{ARGON2ID}` scheme (since v2.13.1). Accounts created before v2.13.1 were stored
with a hash Dovecot could not verify — **reset each mailbox's password once**
after upgrading so login works. Address fields (e-mail, alias source/destination,
catch-all, forward-to) are validated against a strict character set and return
`400` on invalid input.

---

## Servers (8 endpoints)

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
| POST | `/api/users/{id}/reset-password` | Set a new password for a user |
| POST | `/api/users/{id}/toggle-suspend` | Suspend or restore a user |
| POST | `/api/users/{id}/reset-2fa` | Clear a user's 2FA enrolment |

### Teams (7)

> ⚠ **These endpoints grant no access.** They create teams, invite members and
> validate roles, but `team_members` is read by `routes/teams.rs` and by nothing
> else, so no authorization path in the panel consults team membership — the
> `admin` / `developer` / `viewer` roles have no effect on any resource. There is
> also no Teams UI, and the invite email links to an SPA route that does not
> exist. Documented because the endpoints are real and routed; do not build
> against them expecting them to authorize anything. See `FEATURES.md`
> §Withdrawn Claims.

| Method | Path | Purpose |
|--------|------|---------|
| GET/POST | `/api/teams` | List/create teams |
| DELETE | `/api/teams/{id}` | Delete team |
| POST | `/api/teams/{id}/invite` | Invite member |
| POST | `/api/teams/accept` | Accept invitation |
| PUT/DELETE | `/api/teams/{id}/members/{member_id}` | Update/remove member |

### Resellers (14) — Admin creates, reseller manages
The `/api/resellers` family is admin-only and is driven by **Admin → Resellers**.
The `/api/reseller/*` family answers for the calling reseller's own tenant and
refuses an administrator, who has no tenant — use `/api/resellers/{id}` instead.

| Method | Path | Purpose |
|--------|------|---------|
| GET/POST | `/api/resellers` | List/create reseller profiles |
| GET/PUT/DELETE | `/api/resellers/{id}` | Manage profile |
| GET/POST/DELETE | `/api/resellers/{id}/servers` | Server allocation |
| GET | `/api/reseller/dashboard` | Reseller's dashboard |
| GET/POST/PUT/DELETE | `/api/reseller/users` | Reseller's sub-users |

`POST /api/resellers` promotes an **existing** account and takes its `user_id`. It
refuses an administrator (400) and an account that already has a profile (409). An
account that kept a profile through an earlier demotion is restored with its stored
quotas rather than re-created. `PUT` is COALESCE on every column, so a `null` field
leaves the stored value unchanged — there is no way to clear one through this API.

### API Keys (4)

> ⚠ **A key issued here authenticates nothing.** The endpoints below generate,
> hash, store and rotate keys, and no code path ever reads a stored hash back to
> authenticate a request — the only bearer-token extractor JWT-decodes the value
> and nothing else. Authenticate with the session cookie or a JWT. See
> `FEATURES.md` §Withdrawn Claims.

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
| POST | `/api/backup-orchestrator/db-backup` | Create DB backup |
| GET | `/api/backup-orchestrator/db-backups` | List DB backups |
| POST | `/api/backup-orchestrator/db-backups/{id}/restore` | Restore DB backup |
| DELETE | `/api/backup-orchestrator/db-backups/{id}` | Delete DB backup |
| POST | `/api/backup-orchestrator/volume-backup` | Create volume backup |
| GET | `/api/backup-orchestrator/volume-backups` | List volume backups |
| POST | `/api/backup-orchestrator/volume-backups/{id}/restore` | Restore volume backup |
| GET | `/api/backup-orchestrator/verifications` | List verifications |
| POST | `/api/backup-orchestrator/verify` | Verify the most recent backups |
| POST | `/api/backup-orchestrator/drill` | Trigger end-to-end drill (site/db/volume) — async, returns 202 |
| GET | `/api/backup-orchestrator/drills` | List drills (paginated `{items, total}`) |

---

## Incident Management

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/incidents` | List incidents |
| GET | `/api/incidents/summary` | Counts by status, plus `open` — every status except `resolved` and `postmortem` |
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
| DELETE | `/api/status-page/components/{id}` | Delete component |
| POST | `/api/status-page/subscribe` | Subscribe email (public, no auth) |
| POST | `/api/status-page/unsubscribe` | Unsubscribe (public, no auth; DELETE also accepted) |
| GET | `/api/status-page/subscribers` | List subscribers (admin) |
| GET | `/api/status-page/public` | Public status payload (no auth) |

---

## Secrets Manager

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/secrets/vaults` | List vaults |
| POST | `/api/secrets/vaults` | Create vault |
| PUT | `/api/secrets/vaults/{id}` | Update vault |
| DELETE | `/api/secrets/vaults/{id}` | Delete vault |
| GET | `/api/secrets/vaults/{id}/secrets` | List secrets |
| POST | `/api/secrets/vaults/{id}/secrets` | Create secret |
| PUT | `/api/secrets/vaults/{id}/secrets/{sid}` | Update secret |
| DELETE | `/api/secrets/vaults/{id}/secrets/{sid}` | Delete secret |
| GET | `/api/secrets/vaults/{id}/secrets/{sid}/versions` | Version history |
| POST | `/api/secrets/vaults/{id}/inject/{site_id}` | Inject a vault's secrets into a site's env |
| GET | `/api/secrets/vaults/{id}/pull` | Pull secrets (CLI) |
| GET | `/api/secrets/vaults/{id}/export` | Export vault |
| POST | `/api/secrets/vaults/{id}/import` | Import vault |

---

## Webhook Gateway

| Method | Path | Description |
|--------|------|-------------|
| POST | `/api/webhooks/gateway/{token}` | **Public inbound receiver.** The only unauthenticated route here — the token in the path is the credential |
| GET | `/api/webhook-gateway/endpoints` | List endpoints |
| POST | `/api/webhook-gateway/endpoints` | Create endpoint |
| PUT | `/api/webhook-gateway/endpoints/{id}` | Pause or resume an endpoint — body `{"enabled": bool}` |
| DELETE | `/api/webhook-gateway/endpoints/{id}` | Delete endpoint, and every delivery and route recorded against it |
| GET | `/api/webhook-gateway/endpoints/{id}/deliveries` | List deliveries |
| POST | `/api/webhook-gateway/deliveries/{delivery_id}/replay` | Replay a delivery to its matching routes |
| GET | `/api/webhook-gateway/endpoints/{id}/routes` | List routes |
| POST | `/api/webhook-gateway/endpoints/{id}/routes` | Create route |
| PUT | `/api/webhook-gateway/routes/{route_id}` | Pause or resume a route — body `{"enabled": bool}` |
| DELETE | `/api/webhook-gateway/routes/{id}` | Delete route |

---

## Notifications

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/notifications` | List notifications (`limit`, `before`, `category`, `unread`) |
| GET | `/api/notifications/summary` | Totals and per-category counts |
| GET | `/api/notifications/unread-count` | Unread badge count |
| POST | `/api/notifications/{id}/read` | Mark as read |
| POST | `/api/notifications/read-all` | Mark all read |
| DELETE | `/api/notifications/{id}` | Delete one notification |
| DELETE | `/api/notifications/read` | Delete every notification already read |
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

A Git deployment with `deploy_protected` set will not build when its owner asks. The
request is filed here and a **different** administrator has to resolve it — the panel
surfaces this as *Pending Approvals* on the Git Deploy page.

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/deploy-approvals` | List pending approvals |
| POST | `/api/deploy-approvals/{id}/approve` | Approve, and start the deploy |
| POST | `/api/deploy-approvals/{id}/reject` | Reject, or withdraw your own |

`GET /api/deploy-approvals` returns only requests whose deployment is **still** protected,
on machines this administrator operates — this box, or a server they registered
themselves. Each row carries `id`, `deploy_id`, `deploy_name`, `repo_url`, `branch`,
`requested_by`, `requested_by_email`, `status` and `created_at`.

`approve` answers `202` with a fresh `deploy_id` you can stream, and refuses with:

| Code | When |
|---|---|
| `403` | you filed the request yourself |
| `409` | the request is already resolved, the deployment is no longer protected, or a build is already running |
| `404` | the request is not on a machine you operate |

`reject` is terminal — the requester has to ask again. It carries **no** self-rejection
rule, deliberately: withdrawing your own request is the only exit on an install with a
single administrator, where nobody else can ever approve it.

---

## Panel Self-Update (v2.10.0+, Phase 4 W4)

Admin-only endpoints to drive panel updates from the UI instead of SSH.
The orchestrator shells out to `scripts/update.sh` under
`DOCKPANEL_NO_SELF_REFRESH=1 + DOCKPANEL_VERSION=<target>` rather than
reimplementing binary swap — every bug fix in update.sh keeps working.

| Method | Path | Description |
|--------|------|-------------|
| GET    | `/api/update/status` | Current update state + version + channel |
| POST   | `/api/update/manual-check` | Force a `/releases` poll now (bypasses `hold`) |
| POST   | `/api/update/apply` | Snapshot + invoke update.sh. `{target_version: "v2.10.0"}` must match advertised version |
| POST   | `/api/update/rollback` | Restore from snapshot. `{snapshot_id: "<uuid>"}`. Destructive — DB is also restored. Returns `202` immediately; see below |
| GET    | `/api/update/channel` | Read current channel (`stable` \| `candidate` \| `hold`) |
| PUT    | `/api/update/channel` | Set channel. `{channel: "candidate"}` |
| GET    | `/api/snapshots` | List panel snapshots (newest first) |
| POST   | `/api/snapshots` | Create a manual snapshot. Empty body. Returns the new snapshot row |
| DELETE | `/api/snapshots/{id}` | Delete a snapshot (file + DB row) |
| GET    | `/api/update/fleet` | List recent fleet update runs |
| POST   | `/api/update/fleet` | Start a fleet rolling update. `{target_version, halt_on_failure?, include_panel?}` |
| GET    | `/api/update/fleet/{id}` | Fleet run detail (plan + per-server progress) |

#### Rollback is asynchronous, and how you learn the outcome

`POST /api/update/rollback` returns `202` as soon as the restore has been handed
to a detached systemd unit. It cannot return the result: the first thing a
restore does is stop `dockpanel-api`, so the process handling your request no
longer exists by the time there is an outcome to report. Validation that *can*
be done up front (snapshot exists, file present, sha256 matches) still happens
synchronously, so a bad request is still a `404`/`410`/`5xx` immediately.

Poll `GET /api/update/status` afterwards and read `last_restore`:

```json
{
  "snapshot_id": "…",
  "ok": true,
  "stage": "complete",
  "detail": "restored snapshot …; health: {\"status\":\"ok\"}",
  "finished_at": "2026-07-19T18:16:34Z"
}
```

`stage` names the step that was running when the restore stopped, so a failure
says where it stopped and what was left untouched. The database is applied in a
single transaction, so a rollback that fails during the database stage changes
nothing at all. The same document is written to
`/var/lib/dockpanel/last-restore.json` on the box.

**What a rollback does and does not rewind.** It restores what the snapshot
contains: the three binaries, `/etc/dockpanel`, and the database. The database is
a true point-in-time revert — the `public` schema is dropped and rebuilt from the
dump in a single transaction — so database objects created *after* the snapshot
do **not** survive it, and neither does data written into them. The database as it
stood immediately before the rollback is dumped first to
`/var/lib/dockpanel/pre-rollback-<id>.sql.gz` (mode `0600`), which is the way
back from a rollback you did not mean; the three most recent are kept and older
ones are pruned. Nothing outside the snapshot — nginx vhosts, Let's Encrypt
certificates, site files, docker volumes — is touched. A rollback restores the
panel, not the machine.

Up to and including v2.11.6 the database stage applied only the dump's own `DROP`
statements, which merged the snapshot into whatever was already there: tables
added by a newer version's migration outlived the rollback while
`_sqlx_migrations` was rewound past them, and a subsequent update back to that
version re-ran the migration against objects that already existed, leaving the
panel in a startup crash loop. Rolling back with v2.11.7 or newer replaces the
schema outright and is unaffected.

#### What a fleet rolling update actually does

`POST /api/update/fleet` builds an ordered plan of your remote servers — oldest
agent version first, skipping any that are already at the target and any that
have not checked in for five minutes — and returns `202` with a `run_id`. Poll
`GET /api/update/fleet/{id}` for per-server progress; each entry carries the
outcome and, on failure, the reason.

Each member updates **its own agent binary**: the agent fetches
`dockpanel-agent-linux-<arch>` for the requested release tag, verifies it
against that release's `checksums.txt` *before* installing it, swaps it in by
rename, restarts itself, and rolls back to the previous binary if the new one
does not come up. The result is written to
`/var/lib/dockpanel/last-agent-update.json` on the member. A server that is
itself a full DockPanel install runs the ordinary `update.sh` flow instead.

A member counts as updated only when its `/health` reports the target version.
Nothing infers success from an exit status or from the agent's own claim — up to
v2.11.7 it did, and a fleet run could report success on a box whose update had
aborted a second earlier.

With `halt_on_failure` (the default) the run stops at the first failure and the
remaining members are marked `skipped`. `include_panel` updates the panel itself
after the fleet, and only if every member succeeded.

**Upgrading a fleet that is on 2.11.7 or older:** the mechanism lives in the
agent, so those members cannot be rolled from the panel. They report that they
are too old and name the remedy — re-run `install-agent.sh` on them once to
reach 2.11.8, after which fleet updates work normally.

Agent-side (called by the orchestrator over the agent's bearer-auth API):

| Method | Path | Description |
|--------|------|-------------|
| POST   | `/panel/update` | Start the update on the agent host. `{target_version}`. Returns immediately |
| GET    | `/panel/update/status` | Progress window + failure reason. Reports `running_version`; success is confirmed from `/health`, not from here |

#### Agent auto-update (2.12.0)

Instead of you starting a fleet run, each agent can ask the panel whether it
should move. This is **off by default**, including on upgrade.

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| GET    | `/api/agent/version` | Agent bearer token (the `servers` row token, **not** a user session) | What version the calling agent should be running |

```json
{ "target_version": "v2.12.0" }     // move to this
{ "target_version": null }          // do nothing
```

`null` is the answer whenever **Settings → Telemetry → Updates → Agent
Auto-Update** is off, or the update channel is on **Hold**. Hold wins over the
switch. The decision is made by the panel, not the agent: an agent only ever
learns things by asking, so the panel is the only place a switch can actually
take effect.

The target is always **the release the panel itself is running** — an agent is
never pushed ahead of its panel, and the release that built the panel also built
that agent asset, so it is guaranteed to exist.

An agent that is behind checks in on its own schedule (first check an hour after
start, then roughly every 6 hours with jitter so a fleet does not arrive at
once) and runs the same updater a fleet rolling update runs: the download is
verified against the release's own `checksums.txt` before anything is installed,
the swap is atomic within the target directory, and the new agent must answer
`/health` reporting the expected version or it is rolled back. The outcome is
written to `/var/lib/dockpanel/last-agent-update.json` on every exit path.

Rate limit: 120 requests/minute per server, as for the other agent endpoints.

**Agents older than 2.12.0 ignore this.** They read a different field, so they
see nothing to do and carry on — the response deliberately carries no `version`,
`download_url` or `checksum` key, so an old agent cannot be walked into an
update it has no working code to perform. Bring them forward with a fleet
rolling update.

Note: distinct from `/system/updates/*` (OS-package apt-get mgmt).

## Configuration Drift (v2.13.0, Phase 4 W5)

Read-only report answering "is my fleet's operational posture consistent?".
Because DockPanel is a single hub DB + thin agents, every server's declared
config already lives centrally keyed by `server_id`, so this is a **local
cross-server diff** — no remote agent call, and an offline member is still
comparable. Computed on demand (no background scan). Admin only.

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/drift/servers` | Comparable servers (local first) — reference/target picker source |
| GET | `/api/drift?reference=<uuid>&targets=<uuid,uuid>` | Drift report against the reference server |

`reference` defaults to the local server; `targets` defaults to every other
server. Four entities are compared, each in its most meaningful form:

- **alert_rules** — one row per server, whole-row posture diff (the flagship
  signal: "monitoring is not identical").
- **sites** — per-domain inventory asymmetry (present on one server, not the
  other) plus per-site config diffs (`waf_mode`, `ssl_enabled`, `php_version`,
  caches, limits, …) for domains present on both.
- **crons** — per `(domain, command)` job parity.
- **backup_coverage** — per-server summary: how many sites have an enabled
  backup schedule, how many are unprotected, how many destinations exist.

Secret-bearing fields (`notify_slack_url`, `notify_discord_url`) are compared by
**presence only** — the report shows `set`/`unset`, never the value.

**Report only.** Reconcile (push a source-of-truth server's config to the
others) is deliberately not in this release — it is cross-server mutation with no
existing transport, and DockPanel keeps that surface explicit. Comparing a
member's live on-box state against its declared config is likewise a later leg.
