#!/usr/bin/env bash
#
# DockPanel Updater
# Pulls latest code, rebuilds binaries + frontend, restarts services.
# Preserves database, secrets, and configuration.
#
# Usage: bash scripts/update.sh
#
set -euo pipefail

# ── Colors ────────────────────────────────────────────────────────────────
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
BOLD='\033[1m'
NC='\033[0m'

log()    { echo -e "${GREEN}[+]${NC} $1"; }
warn()   { echo -e "${YELLOW}[!]${NC} $1"; }
error()  { echo -e "${RED}[x]${NC} $1" >&2; }

# ── Checks ────────────────────────────────────────────────────────────────
if [ "$EUID" -ne 0 ]; then
    error "Run as root"
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
AGENT_SRC="$REPO_DIR/panel/agent"
API_SRC="$REPO_DIR/panel/backend"
CLI_SRC="$REPO_DIR/panel/cli"
FRONTEND_DIR="$REPO_DIR/panel/frontend"
AGENT_BIN="/usr/local/bin/dockpanel-agent"
API_BIN="/usr/local/bin/dockpanel-api"
CLI_BIN="/usr/local/bin/dockpanel"

if [ ! -d "$AGENT_SRC/src" ]; then
    error "Cannot find agent source at $AGENT_SRC"
    exit 1
fi

echo ""
echo -e "${GREEN}${BOLD}DockPanel Updater${NC}"
echo ""

# ── Pull latest code ──────────────────────────────────────────────────────
if [ -d "$REPO_DIR/.git" ]; then
    log "Pulling latest changes..."
    (cd "$REPO_DIR" && git pull --ff-only) || {
        error "Git pull failed. Resolve conflicts manually."
        exit 1
    }
fi

# ── Detect Rust toolchain ────────────────────────────────────────────────
if command -v cargo &> /dev/null; then
    CARGO_CMD="cargo"
elif [ -f "$HOME/.cargo/bin/cargo" ]; then
    CARGO_CMD="$HOME/.cargo/bin/cargo"
else
    error "Rust toolchain not found. Install with: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    exit 1
fi

# ── Build agent ───────────────────────────────────────────────────────────
log "Building agent..."
(cd "$AGENT_SRC" && $CARGO_CMD build --release 2>&1 | tail -1)

# ── Build API ─────────────────────────────────────────────────────────────
log "Building API..."
(cd "$API_SRC" && $CARGO_CMD build --release 2>&1 | tail -1)

# ── Build CLI ─────────────────────────────────────────────────────────────
log "Building CLI..."
(cd "$CLI_SRC" && $CARGO_CMD build --release 2>&1 | tail -1)

# ── Build frontend (before service restart to minimize downtime) ──────────
if [ -d "$FRONTEND_DIR" ]; then
    log "Building frontend..."
    (cd "$FRONTEND_DIR" && npm ci --silent 2>/dev/null || npm install --silent 2>/dev/null)
    (cd "$FRONTEND_DIR" && npx vite build 2>&1 | tail -3)
    log "Frontend rebuilt"
fi

# ── Backup database before upgrade ────────────────────────────────────────
BACKUP_DIR="/var/backups/dockpanel/db"
mkdir -p "$BACKUP_DIR"
log "Backing up database..."
if docker exec dockpanel-postgres pg_dump -U dockpanel dockpanel | gzip > "$BACKUP_DIR/pre-upgrade-$(date +%Y%m%d%H%M%S).sql.gz"; then
    log "Database backup saved to $BACKUP_DIR/"
else
    error "Database backup failed, aborting upgrade"
    exit 1
fi

# ── Deploy binaries ───────────────────────────────────────────────────────
# Note: ~2-5s downtime during binary swap is expected for self-hosted deployments.
# Zero-downtime upgrades would require load balancer or socket activation.
log "Backing up current binaries..."
cp "$AGENT_BIN" "${AGENT_BIN}.bak" 2>/dev/null || true
cp "$API_BIN" "${API_BIN}.bak" 2>/dev/null || true
cp "$CLI_BIN" "${CLI_BIN}.bak" 2>/dev/null || true

log "Stopping services..."
systemctl stop dockpanel-agent dockpanel-api 2>/dev/null || true

cp "$AGENT_SRC/target/release/dockpanel-agent" "$AGENT_BIN"
cp "$API_SRC/target/release/dockpanel-api" "$API_BIN"
cp "$CLI_SRC/target/release/dockpanel" "$CLI_BIN"
chmod +x "$AGENT_BIN" "$API_BIN" "$CLI_BIN"
log "Binaries updated (agent: $(du -h "$AGENT_BIN" | cut -f1), api: $(du -h "$API_BIN" | cut -f1), cli: $(du -h "$CLI_BIN" | cut -f1))"

systemctl daemon-reload
systemctl start dockpanel-agent
sleep 1
systemctl start dockpanel-api
log "Services restarted"

# ── Health check with rollback ────────────────────────────────────────────
rollback() {
    error "Health check failed, rolling back..."
    cp "${AGENT_BIN}.bak" "$AGENT_BIN" 2>/dev/null || true
    cp "${API_BIN}.bak" "$API_BIN" 2>/dev/null || true
    cp "${CLI_BIN}.bak" "$CLI_BIN" 2>/dev/null || true
    systemctl daemon-reload
    systemctl restart dockpanel-agent dockpanel-api
    warn "Rolled back to previous binaries"
    exit 1
}

log "Running post-deploy health check..."
sleep 5

# Basic health endpoint
if ! curl -sf --max-time 15 http://127.0.0.1:3080/api/health > /dev/null 2>&1; then
    rollback
fi
log "Health check: /api/health OK"

# Auth subsystem (setup-status is unauthenticated, tests DB connectivity)
if ! curl -sf --max-time 15 -X POST http://127.0.0.1:3080/api/auth/setup-status \
    -H "Content-Type: application/json" > /dev/null 2>&1; then
    rollback
fi
log "Health check: /api/auth/setup-status OK"

# Agent reachable (non-fatal — agent may start slower)
if ! curl -sf --max-time 15 http://127.0.0.1:3080/api/system/info > /dev/null 2>&1; then
    warn "Agent connectivity check failed (non-fatal, agent may still be starting)"
fi

# CLI health check (non-fatal)
if ! dockpanel --version > /dev/null 2>&1; then
    warn "CLI health check failed (non-fatal)"
fi

log "Health checks passed"
# Clean up backups
rm -f "${AGENT_BIN}.bak" "${API_BIN}.bak" "${CLI_BIN}.bak"

# ── Wait for health ──────────────────────────────────────────────────────
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
