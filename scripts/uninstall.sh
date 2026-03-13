#!/usr/bin/env bash
#
# DockPanel Uninstaller
# Removes DockPanel completely from the server.
#
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BOLD='\033[1m'
NC='\033[0m'

if [ "$EUID" -ne 0 ]; then
    echo -e "${RED}Run as root${NC}"
    exit 1
fi

echo ""
echo -e "${YELLOW}${BOLD}DockPanel Uninstaller${NC}"
echo ""
echo "This will remove:"
echo "  - Agent and API binaries and systemd services"
echo "  - PostgreSQL container and data volume"
echo "  - Nginx panel config"
echo "  - Config directory (/etc/dockpanel)"
echo "  - Source directory (/opt/dockpanel, if present)"
echo ""
echo -e "${YELLOW}Database and backup data will be DELETED.${NC}"
echo ""

if [ -t 0 ]; then
    echo -n "Continue? [y/N] "
    read -r REPLY
    if [[ ! "$REPLY" =~ ^[Yy]$ ]]; then
        echo "Aborted."
        exit 0
    fi
fi

echo ""

# Stop and remove agent service
echo -e "${GREEN}[+]${NC} Removing agent service..."
systemctl stop dockpanel-agent 2>/dev/null || true
systemctl disable dockpanel-agent 2>/dev/null || true
rm -f /etc/systemd/system/dockpanel-agent.service
rm -f /usr/local/bin/dockpanel-agent

# Stop and remove API service
echo -e "${GREEN}[+]${NC} Removing API service..."
systemctl stop dockpanel-api 2>/dev/null || true
systemctl disable dockpanel-api 2>/dev/null || true
rm -f /etc/systemd/system/dockpanel-api.service
rm -f /usr/local/bin/dockpanel-api

systemctl daemon-reload 2>/dev/null || true

# Remove PostgreSQL container and volume
if docker ps -a --format '{{.Names}}' 2>/dev/null | grep -q "^dockpanel-postgres$"; then
    echo -e "${GREEN}[+]${NC} Removing PostgreSQL container..."
    docker stop dockpanel-postgres 2>/dev/null || true
    docker rm dockpanel-postgres 2>/dev/null || true
fi

if docker volume ls --format '{{.Name}}' 2>/dev/null | grep -q "^dockpanel-pgdata$"; then
    echo -e "${GREEN}[+]${NC} Removing PostgreSQL data volume..."
    docker volume rm dockpanel-pgdata 2>/dev/null || true
fi

# Also handle old Docker Compose deployments
for DIR in /opt/dockpanel/panel /home/*/dockpanel/panel; do
    if [ -f "$DIR/docker-compose.yml" ]; then
        echo -e "${GREEN}[+]${NC} Stopping old Docker Compose deployment at $DIR..."
        (cd "$DIR" && docker compose down -v 2>/dev/null) || true
        break
    fi
done

# Remove nginx config
echo -e "${GREEN}[+]${NC} Removing nginx config..."
rm -f /etc/nginx/sites-enabled/dockpanel-panel.conf
rm -f /etc/nginx/conf.d/dockpanel-panel.conf
nginx -t > /dev/null 2>&1 && (nginx -s reload 2>/dev/null || systemctl reload nginx 2>/dev/null) || true

# Remove directories
echo -e "${GREEN}[+]${NC} Removing data directories..."
rm -rf /etc/dockpanel
rm -rf /var/run/dockpanel
rm -rf /var/backups/dockpanel

# Remove source (if installed to /opt/dockpanel by install.sh)
if [ -d /opt/dockpanel ]; then
    echo -e "${GREEN}[+]${NC} Removing source directory..."
    rm -rf /opt/dockpanel
fi

echo ""
echo -e "${GREEN}${BOLD}DockPanel removed.${NC}"
echo -e "Note: Docker, Nginx, and Node.js were NOT uninstalled (they may be used by other services)."
echo ""
