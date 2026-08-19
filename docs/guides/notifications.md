# Notifications Guide

DockPanel provides an in-app notification system with real-time delivery via Server-Sent Events (SSE), unread badges, and bulk management.

## Notification Sources

Notifications are generated automatically by the panel for events such as:

- Monitor down/recovery alerts
- Backup completion or failure
- Deploy started/completed/failed
- SSL certificate expiring
- Incident created or updated
- Security scan findings
- System alerts (disk full, high CPU)

## Viewing Notifications

### In the Panel

1. The bell icon in the top navigation shows the unread count
2. Click it to open the notification dropdown
3. Click any notification to navigate to the related resource
4. Click **Mark all read** to clear the unread count

### Real-Time Updates

Notifications are delivered in real-time via SSE (Server-Sent Events). The panel automatically connects to the notification stream when you log in. New notifications appear instantly without refreshing the page.

## Managing Notifications

### Mark as Read

- Click a notification to mark it as read
- Or use **Mark all read** to clear all at once

### Notification Preferences

The in-panel notification feed has no per-type preferences -- every notification
the panel raises appears in it.

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

Events are delivered as SSE with `data:` containing the JSON notification object.
