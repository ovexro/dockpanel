# Public Status Page Guide

DockPanel includes a public status page that shows the real-time health of your services. It requires no authentication -- anyone with the link can see it. Ideal for keeping users, clients, and team members informed during outages and maintenance.

## Enabling the Status Page

1. Go to **Settings** > **General**
2. Toggle **Public Status Page** to enabled
3. The status page is now live at `/status` on your panel URL

Example: if your panel is at `https://panel.example.com`, the status page is at `https://panel.example.com/status`.

When disabled, visiting `/status` returns a 404.

## Setting Up Components

Components represent the services your users care about (e.g., "Website", "API", "Database", "Email"). Each component shows its current status on the public page.

### Create a Component

1. Go to **Status Page** > **Components**
2. Click **Create Component**
3. Fill in:
   - **Name**: What users see (e.g., "Payment API")
   - **Description**: Optional detail (e.g., "Handles all payment processing")
   - **Group**: Optional group name for organizing (e.g., "Core Services")
   - **Sort order**: Controls display order (lower = higher on page)
   - **Linked monitors**: Select one or more uptime monitors to auto-determine status
4. Click **Save**

### Component Status

Each component has one of three statuses:

| Status | Meaning | How it's set |
|--------|---------|-------------|
| **Operational** | Everything working | All linked monitors are "up" |
| **Degraded** | Partial issues | Some linked monitors are not "up" |
| **Major Outage** | Service down | Any linked monitor is "down" |

Status is computed automatically from linked monitors. If no monitors are linked, the component defaults to "operational".

You can also override the status manually by linking an incident to the component. The override clears automatically when the incident is resolved.

## Component Groups

Group related components together for a cleaner layout. For example:

```
Core Services
  - Website ............. Operational
  - API ................. Operational
  - Database ............ Operational

Communication
  - Email ............... Operational
  - Push Notifications .. Degraded
```

Set the group name when creating or editing a component. Components without a group appear ungrouped at the top.

## Overall Status

The page header shows an overall status computed from all components:

| Overall | Condition |
|---------|-----------|
| **All Systems Operational** | Every component is operational |
| **Partial System Outage** | At least one component is degraded |
| **Major System Outage** | At least one component has a major outage |

This updates in real time as monitors check your services.

## Incidents on the Status Page

Active incidents appear prominently at the top of the status page with:

- Title and severity badge (minor/major/critical/maintenance)
- Current status (investigating, identified, monitoring, etc.)
- Full timeline of updates

Resolved incidents move to the **Incident History** section, which shows incidents from the last N days (configurable, default 90).

Incidents must have **Visible on status page** enabled to appear. See the [Incident Management Guide](incidents.md) for details on creating and managing incidents.

Auto-detected incidents from uptime monitors also appear, showing which monitor went down and when it recovered.

## Email Subscribers

Visitors can subscribe to status updates by entering their email on the status page. Subscribers receive email notifications when:

- A new incident is created
- An incident status changes
- An update is posted to an incident timeline

### Managing Subscribers

1. Go to **Status Page** > **Subscribers**
2. View all subscribed emails and their verification status
3. Subscribers self-manage via the public subscribe/unsubscribe endpoints

### Unsubscribe

Subscribers can unsubscribe by:
- Using the unsubscribe link in notification emails
- Sending a DELETE request to `/api/status-page/unsubscribe`

To hide the subscribe form from the public page, disable **Show Subscribe** in the status page config.

## Customizing the Status Page

### From the Panel

1. Go to **Status Page** > **Settings**
2. Configure:
   - **Title**: Page heading (default: "Service Status")
   - **Description**: Subtitle text (default: "Current status of our services")
   - **Logo URL**: URL to your logo image
   - **Accent color**: Hex color for the theme (default: `#22c55e` green)
   - **Show subscribe**: Show/hide the email subscription form
   - **Show incident history**: Show/hide the resolved incidents section
   - **History days**: How many days of resolved incidents to display (default: 90)
3. Click **Save**

### Via API

```bash
curl -X PUT https://panel.example.com/api/status-page/config \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "title": "Acme Corp Status",
    "description": "Real-time status of Acme services",
    "accent_color": "#3b82f6",
    "show_subscribe": true,
    "history_days": 30
  }'
```

## Embedding and Linking

### Link from Your Website

Add a "System Status" link in your site footer or navigation pointing to your status page URL:

```html
<a href="https://panel.example.com/status">System Status</a>
```

### Custom Domain

To serve the status page on a custom domain (e.g., `status.example.com`), set up a reverse proxy or CNAME that points to your panel, and configure your web server to route `/status` requests.

## API Reference

| Method | Endpoint | Auth | Description |
|--------|----------|------|-------------|
| `GET` | `/api/status-page/public` | No | Full public status page data (components, incidents, config) |
| `GET` | `/api/status-page/config` | Admin | Get status page configuration |
| `PUT` | `/api/status-page/config` | Admin | Update status page configuration |
| `GET` | `/api/status-page/components` | Admin | List components with linked monitors |
| `POST` | `/api/status-page/components` | Admin | Create a component |
| `DELETE` | `/api/status-page/components/{id}` | Admin | Delete a component |
| `POST` | `/api/status-page/subscribe` | No | Subscribe an email to updates |
| `DELETE` | `/api/status-page/unsubscribe` | No | Unsubscribe an email |
| `GET` | `/api/status-page/subscribers` | Admin | List all subscribers |

The `/api/status-page/public` endpoint returns everything needed to render the status page: config, components with computed status, active/recent incidents with timelines, and auto-detected monitor incidents.
