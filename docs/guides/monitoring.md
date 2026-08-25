# Monitoring & Alerting

## Monitors

Monitors check whether your services are reachable. DockPanel supports four active monitor types and a passive heartbeat type.

| Type | How it works |
|------|-------------|
| **HTTP** | Sends a GET request and checks the status code. Follows up to 5 redirects. 30s timeout. |
| **TCP** | Opens a TCP connection to a host and port. 10s timeout. |
| **Ping** | Sends an ICMP ping to a hostname or IP. |
| **Keyword** | HTTP check that also verifies the response body contains (or does not contain) a specific string. |
| **Heartbeat** | Dead man's switch -- expects periodic pings from your app. If a ping is missed, the monitor goes down. |

### Creating a Monitor

1. Go to **Monitors** in the sidebar
2. Click **Create Monitor**
3. Fill in:
   - **Name**: A descriptive label (e.g., `Production API`)
   - **URL**: The endpoint to check (e.g., `https://api.example.com/health`)
   - **Type**: HTTP, TCP, Ping, Keyword, or Heartbeat
   - **Check Interval**: How often to check, in seconds (default: 60)
   - **Port** (TCP only): The port number to connect to
   - **Keyword** (Keyword only): The string to match, and whether it must or must not be present
   - **Custom Headers** (HTTP/Keyword): Optional JSON object of headers to include in requests
   - **Alert Email**: Toggle email notifications on down/recovery
   - **Slack/Discord URL**: Webhook URL for chat notifications
4. Click **Create**

### From the CLI

```bash
dockpanel monitors create --name "My Site" --url https://example.com --interval 300
```

### Monitor Status

Each monitor tracks:

- **Status**: `up` or `down`
- **Response Time**: Milliseconds for the last check
- **Last Status Code**: HTTP status code (HTTP/Keyword monitors)
- **Last Checked**: Timestamp of the most recent check

Check history is kept for 24 hours and then purged automatically.

### Maintenance Windows

When a maintenance window is active for a user, their uptime monitors are skipped and the alert engine stops paging them. CPU, memory, disk, disk forecast, memory leak, server offline, service health, SSL expiry, GPU and container alerts are all held for the duration.

Nothing is lost by holding them. Each describes a standing condition that is re-evaluated every 60 seconds, so anything still true when the window closes fires on the next check. The window is applied before any alert state is recorded, precisely so that a held alert is deferred rather than marked as already sent.

Alerts that report a **one-off event** are deliberately not suppressed, because nothing would raise them again afterwards: backup failure, backup verification failure, cron failure, security findings and SSL renewal failures still arrive during a window.

### Heartbeat Monitors (Dead Man's Switch)

1. Create a heartbeat monitor in the panel
2. Copy the unique URL: `POST /api/heartbeat/{monitor_id}/{token}`
3. Add a cron job or scheduled task in your application to ping this URL
4. If the expected interval passes without a ping, the monitor goes down

No authentication is required for heartbeat pings.

## Alert Rules

Alert rules define thresholds for server-level conditions. Configure them under **Settings** > **Alert Channels**.

### Available Alert Types

| Alert | What it watches | Default threshold |
|-------|----------------|-------------------|
| **CPU** | CPU usage exceeds threshold for N minutes | 90% for 5 min |
| **Memory** | Memory usage exceeds threshold for N minutes | 90% for 5 min |
| **Disk** | Disk usage exceeds threshold | 85% |
| **Server Offline** | Server stops reporting metrics | 2 min heartbeat timeout |
| **SSL Expiry** | SSL certificate approaching expiration | Warns at 30, 14, 7, 3, 1 days |
| **Service Health** | Nginx, PHP-FPM, MySQL, PostgreSQL down | Checked every 2 min |
| **Backup Failure** | A scheduled backup fails | Immediate |
| **Container Health** | Docker container unhealthy or crash-looping | Checked every 2 min |

You can set rules globally (apply to all servers) or per-server to override global defaults.

### Cooldown

After an alert fires, it will not fire again for the configured cooldown period (default: 60 minutes). This prevents notification spam during sustained incidents.

### Muted Types

You can mute specific alert types from external notifications (Slack, Discord, etc.) while still recording them in the panel. Set a comma-separated list of types to suppress in the alert rules.

## Notification Channels

Configure where alerts are sent under **Settings** > **Alert Channels**.

| Channel | Configuration |
|---------|---------------|
| **Email** | Toggle on/off. Uses the SMTP settings from Settings. |
| **Slack** | Paste a Slack incoming webhook URL. |
| **Discord** | Paste a Discord webhook URL. |
| **PagerDuty** | Enter your PagerDuty Events API v2 integration key. |
| **Generic Webhook** | Any URL. Receives a JSON POST with alert details. |

All webhook URLs are validated against SSRF -- internal/private IP addresses are rejected.

## Alert Escalation

If a firing alert is not acknowledged within 15 minutes, the alert engine re-sends notifications. Escalation repeats every 30 minutes until the alert is acknowledged or resolved.

If you attach an **escalation policy** to an alert rule, that chain drives the escalation instead: its first step fires the moment the alert is created, and each later step fires once the alert has gone unacknowledged past that step's threshold. When the chain runs out, the alert falls back to the same 30-minute cadence above, re-paging the last step's route -- an unacknowledged alert never stops paging on its own.

To stop escalation, go to **Alerts** and click **Acknowledge** on the alert.

## Alert Lifecycle

An alert moves through three states:

1. **Firing** -- The condition is active. Notifications are sent.
2. **Acknowledged** -- A user acknowledged it. Escalation stops.
3. **Resolved** -- The condition cleared (automatically or manually).

Resolved alerts are purged after 30 days.

## Smart Alerting

### Response Time Degradation

If an HTTP monitor returns a successful status but the response time exceeds 5000ms, a `slow_response` warning alert is created. This catches performance degradation before full downtime.

### Auto-Incidents

When a monitor goes down, DockPanel automatically creates a managed incident for the public status page. When the monitor recovers, the incident is auto-resolved. Status page subscribers are notified of both transitions.

### Disk-Full Forecast

The alert engine tracks disk usage over time. If the trend projects the disk will be full within 48 hours, a warning alert fires -- giving you time to act before it becomes critical.

### Memory Leak Detection

If memory usage has risen more than 10% over the past hour and is currently above 60%, a `memory_leak` warning alert fires. This is based on comparing recent 10-minute averages against 20-minute-old averages from the metrics history.

### Docker Container Monitoring

Every 2 minutes, the alert engine checks all Docker containers on the server:

- **Unhealthy**: Containers with a failing health check trigger an alert.
- **Crash-loop**: If the auto-healer restarts a service 3 times in 30 minutes and it keeps failing, it stops retrying and creates an incident.

## SSL Certificate Dashboard

The certificate dashboard shows the SSL certificates on your sites with their expiry
dates. Certificates expiring within the configured warning window (default: 30, 14, 7,
3, 1 days) trigger alerts.

**A status of `Unknown` means the panel does not know when the certificate expires**,
not that it is healthy. That happens for certificates installed before v2.139.0 through
**SSL → Upload certificate**, which recorded only that SSL was enabled; re-uploading the
certificate records its expiry.

**An uploaded certificate must name the site it secures.** Since v2.148.0 the upload is
refused, before anything is written, if the certificate's subjectAltName entries (or its
Common Name, when it carries no SAN) do not cover the site's domain — and the refusal
tells you which names it does cover. A wildcard counts for exactly one level, so
`*.example.com` covers `app.example.com` and not `app.staging.example.com`. Before that
check existed, pasting the wrong `fullchain.pem` installed cleanly: nginx does not
compare a certificate against `server_name`, so the panel showed a padlock and only the
browser disagreed, with `ERR_CERT_COMMON_NAME_INVALID`. The same check refuses a private
key pasted into the certificate field — which used to be stored with the certificate's
permissions rather than the key's, and left the site with no recorded expiry at all.

**DockPanel renews only the Let's Encrypt certificates it issued itself.** Its ACME
client can produce nothing else, so a certificate from any other issuer — a commercial
wildcard, a Cloudflare Origin CA certificate, a corporate PKI — is left alone by the
automatic renewal loops, and the Renew button refuses it with the issuer named. Renew
those wherever they were issued and upload the replacement. (Before v2.139.0 the weekly
security scan replaced them instead, which is why this is stated so plainly.)

### All certificates on this server (administrators)

Administrators get an **All certificates on this server** switch. It lists every site's
certificate on the current server together with its owner, and — because the agent scans
the host, not the database — the certificates the machine holds that belong to no site
at all: DNS-01 wildcards, certificates issued for Docker apps, anything installed by
hand. Those appear as **Not a site** and offer no Renew or Delete control, because both
act on a site record and there is none.

This exists because the diagnostics scanner reports findings about the whole host, so an
administrator could be told a certificate was expiring and then find it on no screen.
If the server's agent is older than the panel it cannot enumerate certificates, and the
page says so rather than presenting a partial list as a complete one.

## API Reference

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/monitors` | List all monitors |
| `POST` | `/api/monitors` | Create a monitor |
| `PUT` | `/api/monitors/{id}` | Update a monitor |
| `DELETE` | `/api/monitors/{id}` | Delete a monitor |
| `GET` | `/api/alerts` | List alerts (filterable by `status`, `alert_type`, `limit`) |
| `GET` | `/api/alerts/summary` | Alert counts by status (firing, acknowledged, resolved) |
| `PUT` | `/api/alerts/{id}/acknowledge` | Acknowledge an alert |
| `PUT` | `/api/alerts/{id}/resolve` | Manually resolve an alert |
| `GET` | `/api/alert-rules` | Get alert rules |
| `PUT` | `/api/alert-rules` | Create or update global alert rules |
| `PUT` | `/api/alert-rules/{server_id}` | Create or update per-server rules |
| `DELETE` | `/api/alert-rules/{server_id}` | Remove per-server override |
