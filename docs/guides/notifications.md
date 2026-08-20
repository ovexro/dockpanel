# Notifications Guide

DockPanel keeps an in-app notification feed — the bell in the panel chrome and
the **Notifications** screen behind it. Notifications arrive in real time over
Server-Sent Events (SSE), carry a link to whatever they are about, and can be
filtered, marked read and deleted.

The feed is separate from **external** delivery (email, Slack, Discord,
PagerDuty). Anything the panel raises appears in the feed whether or not you
have configured an external channel; see
[Alerts & Monitoring](./monitoring.md) for the external side.

## What raises a notification

Each notification carries a **category**, which is what the filter row on the
Notifications screen is built from. The categories the panel raises today:

| Category | Raised when |
|---|---|
| `alert` | A threshold alert fires or resolves — disk, CPU, memory, a server going offline, a service stopping, a container crash-looping |
| `monitor` | An uptime monitor goes down, comes back up, or misses a heartbeat |
| `security` | A login from an address not seen before, a canary file being touched, an emergency lockdown, a password reset, a completed security scan |
| `deploy` | A deploy finishes, fails, or succeeds while the site answers 5xx; a Git deploy, rollback or auto-rollback |
| `site` | A site is created, deleted, renamed, enabled or disabled, or its cache/WAF settings change |
| `incident` | An incident is created or resolved |
| `backup` | Sites go 48 hours or more without a backup |
| `ssl` | A certificate is renewed automatically |
| `auto_heal` | The auto-healer restarts a service, exhausts its retries, or reclaims disk |
| `system` | A container is put to sleep for being idle |

Every category above is raised by the panel itself. Nothing needs configuring
for the feed to work.

## Reading notifications

The bell shows how many are unread. It saturates at `99+`; hover it to read the
exact count. Clicking it opens the **Notifications** screen.

On that screen:

- **A notification's title is a link** when the panel knows where the event
  happened — a failed deploy opens the site, a firing alert opens the Alerts
  tab, a canary trip opens the security audit log. Following the link marks the
  notification read. Notifications with nowhere sensible to go — a *site
  deleted*, whose subject no longer exists — render as plain text.
- **Filter** by unread, or by any category the feed actually contains. The
  filter row is built from your own notifications, so a category appears there
  the first time it is raised.
- **Mark all read** clears the unread count, and the bell updates immediately.
- **Delete** removes a single notification; **Clear read** removes every
  notification you have already read and never touches an unread one.
- **Load older** pages back through the feed 50 at a time.

New notifications appear at the top of the list as they happen, without a
refresh — the screen is on the same SSE stream as the bell.

### Retention

Notifications are deleted after `retention_notification_days` (default **30**).
This is a database setting with no screen; it applies to read and unread alike,
so treat the feed as a 30-day window rather than an archive. The permanent
record of a security event is the audit log under **Security > Audit**, which is
never pruned.

## Notification preferences

The in-panel feed has no per-type preferences — every notification the panel
raises appears in it.

What *is* configurable is which alert types are sent to your **external**
channels (email, Slack, Discord, PagerDuty): see
**Settings** > **Alert Channels** > *Suppress External Notifications*.
Suppressing a type there still records it in the panel feed.

## API Reference

See the [Notifications API](../api-reference.md#notifications) for all endpoints.

### SSE Stream

Connect to the real-time notification stream:

```bash
curl -N -H "Authorization: Bearer $TOKEN" \
  https://panel.example.com/api/notifications/stream
```

Each event's `data:` is the complete notification object — `id`, `title`,
`message`, `severity`, `category`, `link`, `read_at` and `created_at` — so a
client can render an arriving notification without asking for it again.
