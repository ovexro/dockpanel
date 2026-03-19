#!/usr/bin/env bash
# DockPanel Remote Agent Installer
# Usage: curl -sSL https://panel.example.com/install-agent.sh | sudo bash -s -- \
#   --panel-url https://panel.example.com \
#   --token <agent_token> \
#   --server-id <server_uuid>
#
# This installs ONLY the DockPanel agent binary (no database, no API, no frontend).
# The agent connects back to the panel via HTTPS on port 9443.

set -euo pipefail

PANEL_URL=""
TOKEN=""
SERVER_ID=""
AGENT_PORT="9443"

# Parse arguments
while [[ $# -gt 0 ]]; do
    case "$1" in
        --panel-url) PANEL_URL="$2"; shift 2 ;;
        --token) TOKEN="$2"; shift 2 ;;
        --server-id) SERVER_ID="$2"; shift 2 ;;
        --port) AGENT_PORT="$2"; shift 2 ;;
        *) echo "Unknown argument: $1"; exit 1 ;;
    esac
done

if [[ -z "$TOKEN" ]]; then
    echo "Error: --token is required"
    echo "Usage: $0 --panel-url <url> --token <token> --server-id <uuid>"
    exit 1
fi

echo "======================================"
echo "  DockPanel Agent Installer (Remote)"
echo "======================================"
echo ""

# Detect architecture
ARCH=$(uname -m)
case "$ARCH" in
    x86_64)  ARCH_LABEL="x86_64" ;;
    aarch64) ARCH_LABEL="aarch64" ;;
    arm64)   ARCH_LABEL="aarch64" ;;
    *) echo "Unsupported architecture: $ARCH"; exit 1 ;;
esac
echo "[1/7] Architecture: $ARCH_LABEL"

# Install dependencies
echo "[2/7] Installing dependencies..."
apt-get update -qq > /dev/null 2>&1
apt-get install -y -qq curl docker.io > /dev/null 2>&1
systemctl enable --now docker > /dev/null 2>&1 || true

# Create directories
echo "[3/7] Creating directories..."
mkdir -p /etc/dockpanel/ssl
mkdir -p /var/run/dockpanel
mkdir -p /var/www
mkdir -p /var/backups/dockpanel
mkdir -p /var/lib/dockpanel/git

# Save agent token
echo "[4/7] Saving agent token..."
echo "$TOKEN" > /etc/dockpanel/agent.token
chmod 600 /etc/dockpanel/agent.token

# Download agent binary
echo "[5/7] Downloading agent binary..."
DOWNLOAD_URL="https://github.com/ovexro/dockpanel/releases/latest/download/dockpanel-agent-${ARCH_LABEL}"
if ! curl -fsSL "$DOWNLOAD_URL" -o /usr/local/bin/dockpanel-agent; then
    echo "  Release download failed. Trying panel download..."
    if [[ -n "$PANEL_URL" ]]; then
        curl -fsSL "${PANEL_URL}/api/agent/download?arch=${ARCH_LABEL}" -o /usr/local/bin/dockpanel-agent || {
            echo "Error: Could not download agent binary"
            exit 1
        }
    else
        echo "Error: Could not download agent binary (no --panel-url provided)"
        exit 1
    fi
fi
chmod +x /usr/local/bin/dockpanel-agent

# Generate self-signed TLS cert for agent HTTPS
echo "[6/7] Generating TLS certificate..."
if [[ ! -f /etc/dockpanel/ssl/agent.crt ]]; then
    openssl req -x509 -newkey rsa:2048 -keyout /etc/dockpanel/ssl/agent.key \
        -out /etc/dockpanel/ssl/agent.crt -days 3650 -nodes \
        -subj "/CN=dockpanel-agent" > /dev/null 2>&1
    chmod 600 /etc/dockpanel/ssl/agent.key
fi

# Create systemd service
echo "[7/7] Creating systemd service..."
cat > /etc/systemd/system/dockpanel-agent.service << 'UNIT'
[Unit]
Description=DockPanel Agent
After=network.target docker.service
Wants=docker.service

[Service]
Type=simple
ExecStart=/usr/local/bin/dockpanel-agent
Environment=RUST_LOG=info
Environment=AGENT_LISTEN_TCP=0.0.0.0:9443
Restart=always
RestartSec=5

# Permissions
ReadWritePaths=/var/www /var/run/dockpanel /etc/dockpanel /var/backups/dockpanel /var/lib/dockpanel
ReadWritePaths=/etc/nginx /var/log/nginx /run/nginx.pid /var/lib/nginx
ReadWritePaths=/etc/letsencrypt /etc/postfix /etc/dovecot /etc/opendkim /var/vmail
ReadWritePaths=/var/spool/cron /tmp

[Install]
WantedBy=multi-user.target
UNIT

# Allow agent port through firewall
if command -v ufw &> /dev/null; then
    ufw allow ${AGENT_PORT}/tcp > /dev/null 2>&1 || true
fi

# Start agent
systemctl daemon-reload
systemctl enable dockpanel-agent
systemctl start dockpanel-agent

echo ""
echo "======================================"
echo "  DockPanel Agent installed!"
echo "======================================"
echo ""
echo "  Agent listening on: 0.0.0.0:${AGENT_PORT}"
echo "  Token: ${TOKEN:0:12}..."
echo "  Server ID: ${SERVER_ID}"
echo ""
echo "  Return to your DockPanel and click"
echo "  'Test Connection' to verify."
echo ""
