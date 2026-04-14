#!/bin/bash
# DockPanel post-deploy health check
# Run after deploying new binaries to verify no runtime errors
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'

echo "=== Post-deploy health check ==="

# Check services are active
echo -n "Services: "
AGENT=$(systemctl is-active dockpanel-agent 2>/dev/null)
API=$(systemctl is-active dockpanel-api 2>/dev/null)

if [ "$AGENT" = "active" ] && [ "$API" = "active" ]; then
  echo -e "${GREEN}agent=active api=active${NC}"
else
  echo -e "${RED}agent=$AGENT api=$API${NC}"
  exit 1
fi

# Wait for startup
sleep 2

# Check for errors/panics in logs
echo -n "Runtime errors: "
ERRORS=$(journalctl -u dockpanel-agent -u dockpanel-api --since "30 sec ago" 2>/dev/null | grep -ciE "error|panic|fatal" || true)

if [ "$ERRORS" -eq 0 ]; then
  echo -e "${GREEN}none${NC}"
else
  echo -e "${RED}$ERRORS error(s) found:${NC}"
  journalctl -u dockpanel-agent -u dockpanel-api --since "30 sec ago" 2>/dev/null | grep -iE "error|panic|fatal" | head -5
  exit 1
fi

# Quick API health check
echo -n "API health: "
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:3080/api/settings/health 2>/dev/null || echo "000")

if [ "$HTTP_CODE" = "200" ]; then
  echo -e "${GREEN}200 OK${NC}"
elif [ "$HTTP_CODE" = "401" ]; then
  echo -e "${GREEN}401 (auth required — API is responding)${NC}"
else
  echo -e "${RED}HTTP $HTTP_CODE${NC}"
  exit 1
fi

echo -e "${GREEN}Deploy verified.${NC}"
