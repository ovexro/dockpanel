# Multi-Server Management Guide

## Overview

DockPanel lets you manage unlimited remote servers from a single panel. One server runs the full panel (API + frontend + database), and remote servers run only the lightweight agent binary (~20MB, ~30MB RAM). All communication between the panel and remote agents uses HTTPS with token-based authentication.

## Architecture

```
┌──────────────────────────┐
│     Panel Server         │
│  (API + Frontend + DB)   │
│                          │
│  Agent (local, Unix sock)│
└─────────┬────────────────┘
          │ HTTPS (port 9443)
          │
    ┌─────┴──────┐
    │            │
┌───▼───┐  ┌───▼───┐
│Remote 1│  │Remote 2│
│ Agent  │  │ Agent  │
└────────┘  └────────┘
```

## Install the Remote Agent

On the remote server, run the agent install script with your panel's URL and authentication token:

```bash
curl -sSL https://your-panel.example.com/install-agent.sh | sudo bash -s -- \
  --panel-url https://your-panel.example.com \
  --token YOUR_AGENT_TOKEN \
  --server-id SERVER_UUID
```

Where:
- `--panel-url` -- The URL of your main DockPanel instance
- `--token` -- The agent authentication token (found in Settings > API or `/etc/dockpanel/api.env` on the panel server)
- `--server-id` -- The server UUID (generated when you add the server in the panel)

The install script:

1. Detects the OS and architecture (x86_64 or ARM64)
2. Downloads the pre-built agent binary from GitHub Releases
3. Installs Docker if not present
4. Writes the agent config to `/etc/dockpanel/api.env`
5. Creates a systemd service and starts the agent
6. Opens port 9443 in the firewall

The agent supports Ubuntu 20+, Debian 11+, CentOS 9+, Rocky Linux 9+, Fedora 39+, and Amazon Linux 2023.

## Add a Server in the Panel

1. Go to **Servers** in the sidebar
2. Click **Add Server**
3. Enter:
   - **Name**: A label for the server (e.g., `web-2`, `eu-prod`)
   - **Hostname / IP**: The remote server's public IP or hostname
   - **Port**: `9443` (default)
4. Click **Add**
5. The panel generates a **Server UUID** and **Token** -- use these in the agent install command

## Test Connection

After installing the remote agent and adding the server in the panel:

1. Go to **Servers**
2. Find your server in the list
3. Click **Test Connection**

A successful test confirms that:
- The agent is running and reachable
- The authentication token matches
- The HTTPS connection is established

If the test fails, see [Network Requirements](#network-requirements) below.

## Network Requirements

The remote agent listens on **port 9443** (TCP) for incoming connections from the panel.

### Firewall rules

On the **remote server**, port 9443 must be open:

```bash
# UFW (Ubuntu/Debian)
ufw allow 9443/tcp

# firewalld (CentOS/Rocky)
firewall-cmd --permanent --add-port=9443/tcp
firewall-cmd --reload
```

### Security group (cloud providers)

If the remote server is on AWS, GCP, Azure, or Oracle Cloud, add an inbound rule in the security group:
- **Protocol**: TCP
- **Port**: 9443
- **Source**: The panel server's public IP (for best security), or `0.0.0.0/0`

### TLS

All communication between the panel and remote agents uses HTTPS (TLS). The agent generates a self-signed certificate on first start. No manual certificate management is needed.

## Managing Resources Across Servers

Once connected, you can manage remote servers exactly like the local server.

### Server Selector

The sidebar shows a server selector at the top. Choose a server to scope all operations (Sites, Databases, Docker Apps, Backups, etc.) to that server.

### What you can do on remote servers

- Create and manage sites (static, PHP, Node.js, Python, proxy)
- Create and manage databases (MySQL, PostgreSQL)
- Deploy Docker apps from templates
- Set up Git Deploy with webhooks
- Manage SSL certificates
- Create and restore backups
- Run diagnostics
- View logs and metrics
- Manage firewall rules
- Access the web terminal

### Metrics and monitoring

The remote agent periodically reports metrics (CPU, RAM, disk, network) to the central panel. You can see per-server dashboards, set up alerts, and monitor uptime -- all from the single panel.

### Reseller server allocation

If you use reseller accounts, you can allocate specific servers to specific resellers. Reseller users only see and manage resources on their allocated servers.
