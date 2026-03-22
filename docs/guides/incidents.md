# Incident Management Guide

DockPanel's incident management gives you a structured way to track outages, communicate with your team, notify subscribers, and produce postmortems. Incidents appear on your Public Status Page automatically.

## Incident Lifecycle

Every incident moves through a defined lifecycle:

```
investigating --> identified --> monitoring --> resolved --> postmortem
```

| Status | Meaning |
|--------|---------|
| **investigating** | Something is wrong, you're looking into it |
| **identified** | Root cause found, working on a fix |
| **monitoring** | Fix deployed, watching for recurrence |
| **resolved** | Incident is over, service restored |
| **postmortem** | Post-incident review in progress |

## Severity Levels

| Severity | Use when |
|----------|----------|
| **minor** | Small degradation, most users unaffected |
| **major** | Significant impact, core functionality impaired |
| **critical** | Full outage, service completely unavailable |
| **maintenance** | Planned work, not an actual outage |

## Creating an Incident

### From the Panel

1. Go to **Incidents** in the sidebar
2. Click **Create Incident**
3. Fill in:
   - **Title**: Short description (e.g., "API response times elevated")
   - **Severity**: minor, major, critical, or maintenance
   - **Description**: What users are experiencing
   - **Affected components**: Select which status page components are impacted
   - **Visible on status page**: Toggle on to show publicly (default: on)
4. Click **Create**

An initial timeline entry is created automatically, and email subscribers are notified.

### Auto-Created Incidents

When a monitor detects downtime, DockPanel creates an auto-incident automatically. These appear on the status page alongside manually created incidents. When the monitor recovers, the auto-incident is resolved.

## Incident Timeline and Updates

The timeline is a chronological log of everything that happened during an incident. Each update records the status at that moment, a message, and who posted it.

### Post an Update

1. Open the incident
2. Click **Post Update**
3. Select the new status (e.g., "identified")
4. Write a message (e.g., "Root cause is a failed database migration. Rolling back now.")
5. Click **Post**

The incident status changes, the update appears in the timeline, and subscribers are notified by email.

You can post as many updates as needed. Each one is timestamped and attributed to the author.

### Example Timeline

```
14:02 UTC [investigating] API response times elevated across all endpoints.
14:15 UTC [identified]    Root cause: database connection pool exhausted after deploy.
14:22 UTC [monitoring]    Connection pool increased, deploying fix now.
14:35 UTC [resolved]      Service fully restored. Response times normal.
```

## Affected Components

Components are status page entries that represent your services (e.g., "API", "Website", "Database"). Linking an incident to components:

- Marks those components as degraded or down on the status page
- Automatically clears the override when the incident is resolved

### Link Components

When creating or editing an incident, select one or more components from the dropdown. You can manage components under **Status Page** > **Components**.

## Postmortem

When you move an incident to the **postmortem** status, DockPanel auto-generates a template pre-populated with your timeline:

```markdown
## Incident Postmortem

### Summary
[Describe the incident]

### Timeline
- **14:02 UTC** [investigating]: API response times elevated
- **14:15 UTC** [identified]: Database connection pool exhausted
- **14:22 UTC** [monitoring]: Deploying fix
- **14:35 UTC** [resolved]: Service fully restored

### Root Cause
[What caused this?]

### Resolution
[How was it fixed?]

### Action Items
- [ ]
```

Edit the template to fill in root cause, resolution, and action items. Toggle **Publish postmortem** to make it visible on the public status page.

## Email Subscribers

Subscribers receive email notifications for:

- New incidents created
- Status changes (investigating -> identified -> resolved, etc.)
- Update messages posted to the timeline

Notifications are sent through your configured SMTP settings (see **Settings** > **Email**). If SMTP is not configured, subscriber notifications are silently skipped.

Subscribers can sign up on the public status page (if the subscribe option is enabled) or be managed from **Status Page** > **Subscribers** in the admin panel.

## How Incidents Appear on the Status Page

Active incidents (any status except resolved) appear at the top of the public status page with their severity badge and latest update. Resolved incidents appear in the incident history section for the configured number of days (default: 90).

Incidents marked as **not visible on status page** are hidden from public view but still tracked internally.

## API Reference

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/incidents` | List incidents (supports `?status=` filter) |
| `POST` | `/api/incidents` | Create an incident |
| `GET` | `/api/incidents/{id}` | Get incident with timeline and components |
| `PUT` | `/api/incidents/{id}` | Update incident (status, severity, postmortem) |
| `DELETE` | `/api/incidents/{id}` | Delete an incident |
| `POST` | `/api/incidents/{id}/updates` | Post a timeline update |
| `GET` | `/api/incidents/{id}/updates` | List timeline updates |

### Create Incident Example

```bash
curl -X POST https://panel.example.com/api/incidents \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "title": "Database connectivity issues",
    "severity": "major",
    "description": "Users experiencing timeouts on write operations",
    "component_ids": ["uuid-of-api-component"],
    "visible_on_status_page": true
  }'
```

### Post Update Example

```bash
curl -X POST https://panel.example.com/api/incidents/INCIDENT_ID/updates \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "status": "identified",
    "message": "Root cause identified: disk full on primary database server"
  }'
```

### Resolve an Incident

```bash
curl -X POST https://panel.example.com/api/incidents/INCIDENT_ID/updates \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "status": "resolved",
    "message": "Disk space freed, database connections restored"
  }'
```

When an incident is resolved via the API, linked alerts are auto-resolved and component status overrides are cleared.
