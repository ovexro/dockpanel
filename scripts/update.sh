#!/usr/bin/env bash
#
# DockPanel Updater
# Pulls latest code, rebuilds binaries + frontend, restarts services.
# Preserves database, secrets, and configuration.
#
# Usage: bash scripts/update.sh
#
set -euo pipefail

# ── Colors ────────────────────────────────────────────────────────────────────
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
BOLD='\033[1m'
NC='\033[0m'

log()    { echo -e "${GREEN}[+]${NC} $1"; }
warn()   { echo -e "${YELLOW}[!]${NC} $1"; }
error()  { echo -e "${RED}[x]${NC} $1" >&2; }

# ── Checks ────────────────────────────────────────────────────────────────────
if [ "$EUID" -ne 0 ]; then
    error "Run as root"
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
AGENT_SRC="$REPO_DIR/panel/agent"
API_SRC="$REPO_DIR/panel/backend"
FRONTEND_DIR="$REPO_DIR/panel/frontend"
AGENT_BIN="/usr/local/bin/dockpanel-agent"
API_BIN="/usr/local/bin/dockpanel-api"

if [ ! -d "$AGENT_SRC/src" ]; then
    error "Cannot find agent source at $AGENT_SRC"
    exit 1
fi

echo ""
echo -e "${GREEN}${BOLD}DockPanel Updater${NC}"
echo ""

# ── Pull latest code ──────────────────────────────────────────────────────────
if [ -d "$REPO_DIR/.git" ]; then
    log "Pulling latest changes..."
    (cd "$REPO_DIR" && git pull --ff-only) || {
        error "Git pull failed. Resolve conflicts manually."
        exit 1
    }
fi

# ── Detect Rust toolchain ────────────────────────────────────────────────────
if command -v cargo &> /dev/null; then
    CARGO_CMD="cargo"
elif [ -f "$HOME/.cargo/bin/cargo" ]; then
    CARGO_CMD="$HOME/.cargo/bin/cargo"
else
    error "Rust toolchain not found. Install with: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    exit 1
fi

# ── Build agent ───────────────────────────────────────────────────────────────
log "Building agent..."
(cd "$AGENT_SRC" && $CARGO_CMD build --release 2>&1 | tail -1)

# ── Build API ─────────────────────────────────────────────────────────────────
log "Building API..."
(cd "$API_SRC" && $CARGO_CMD build --release 2>&1 | tail -1)

# ── Deploy binaries ───────────────────────────────────────────────────────────
log "Stopping services..."
systemctl stop dockpanel-agent dockpanel-api 2>/dev/null || true

cp "$AGENT_SRC/target/release/dockpanel-agent" "$AGENT_BIN"
cp "$API_SRC/target/release/dockpanel-api" "$API_BIN"
chmod +x "$AGENT_BIN" "$API_BIN"
log "Binaries updated"

systemctl start dockpanel-agent
sleep 1
systemctl start dockpanel-api
log "Services restarted"

# ── Build frontend ────────────────────────────────────────────────────────────
if [ -d "$FRONTEND_DIR" ]; then
    log "Building frontend..."
    (cd "$FRONTEND_DIR" && npm ci --silent 2>/dev/null || npm install --silent 2>/dev/null)
    (cd "$FRONTEND_DIR" && npx vite build 2>&1 | tail -3)
    log "Frontend rebuilt"
fi

# ── Wait for health ──────────────────────────────────────────────────────────
log "Waiting for API..."
WAITED=0
while [ "$WAITED" -lt 30 ]; do
    if curl -sf http://127.0.0.1:3080/api/health > /dev/null 2>&1; then
        break
    fi
    sleep 2
    WAITED=$((WAITED + 2))
done

if curl -sf http://127.0.0.1:3080/api/health > /dev/null 2>&1; then
    echo ""
    echo -e "${GREEN}${BOLD}Update complete!${NC}"
    echo ""
    echo -e "  Agent: $(systemctl is-active dockpanel-agent)"
    echo -e "  API:   $(systemctl is-active dockpanel-api)"
    echo ""
else
    warn "API not responding yet. Check: journalctl -u dockpanel-api -n 20"
fi
