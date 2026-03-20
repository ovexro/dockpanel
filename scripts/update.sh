#!/usr/bin/env bash
#
# DockPanel Updater
# Pulls latest code, rebuilds binaries + frontend, restarts services.
# Preserves database, secrets, and configuration.
#
# Usage: bash scripts/update.sh
#        INSTALL_FROM_RELEASE=1 bash scripts/update.sh  # Download pre-built binaries
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
INSTALL_FROM_RELEASE="${INSTALL_FROM_RELEASE:-0}"
GITHUB_REPO="ovexro/dockpanel"

# Auto-detect: if no source available, use release binaries
if [ "$INSTALL_FROM_RELEASE" != "1" ] && [ ! -d "$AGENT_SRC/src" ]; then
    log "No source found — switching to pre-built binary download"
    INSTALL_FROM_RELEASE=1
fi

# For source builds, verify source exists
if [ "$INSTALL_FROM_RELEASE" != "1" ] && [ ! -d "$AGENT_SRC/src" ]; then
    error "Cannot find agent source at $AGENT_SRC"
    exit 1
fi

echo ""
echo -e "${GREEN}${BOLD}DockPanel Updater${NC}"
echo ""

# ── Pull latest code (only for source builds) ────────────────────────────
if [ "$INSTALL_FROM_RELEASE" != "1" ] && [ -d "$REPO_DIR/.git" ]; then
    log "Pulling latest changes..."
    (cd "$REPO_DIR" && git pull --ff-only) || {
        error "Git pull failed. Resolve conflicts manually."
        exit 1
    }
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

# ── Build or download binaries ────────────────────────────────────────────
if [ "$INSTALL_FROM_RELEASE" = "1" ]; then
    # Download pre-built binaries from GitHub Releases
    ARCH=$(uname -m)
    case "$ARCH" in
        x86_64)  DL_ARCH="amd64" ;;
        aarch64) DL_ARCH="arm64" ;;
        *) error "Unsupported architecture: $ARCH"; exit 1 ;;
    esac

    log "Fetching latest release..."
    RELEASE_TAG=$(curl -sf "https://api.github.com/repos/${GITHUB_REPO}/releases/latest" | grep '"tag_name"' | head -1 | cut -d'"' -f4)
    if [ -z "$RELEASE_TAG" ]; then
        error "Could not determine latest release. Check https://github.com/${GITHUB_REPO}/releases"
        exit 1
    fi
    log "Latest release: $RELEASE_TAG"
    BASE_URL="https://github.com/${GITHUB_REPO}/releases/download/${RELEASE_TAG}"

    log "Downloading agent (${DL_ARCH})..."
    curl -sfL "${BASE_URL}/dockpanel-agent-linux-${DL_ARCH}" -o /tmp/dockpanel-agent-new
    chmod +x /tmp/dockpanel-agent-new

    log "Downloading API (${DL_ARCH})..."
    curl -sfL "${BASE_URL}/dockpanel-api-linux-${DL_ARCH}" -o /tmp/dockpanel-api-new
    chmod +x /tmp/dockpanel-api-new

    log "Downloading CLI (${DL_ARCH})..."
    curl -sfL "${BASE_URL}/dockpanel-cli-linux-${DL_ARCH}" -o /tmp/dockpanel-cli-new
    chmod +x /tmp/dockpanel-cli-new

    # Download and extract frontend
    log "Downloading frontend..."
    curl -sfL "${BASE_URL}/dockpanel-frontend.tar.gz" -o /tmp/dockpanel-frontend.tar.gz
    FE_DIR="/opt/dockpanel/frontend"
    mkdir -p "$FE_DIR"
    tar xzf /tmp/dockpanel-frontend.tar.gz -C "$FE_DIR"
    rm -f /tmp/dockpanel-frontend.tar.gz
    log "Frontend updated"
else
    # Build from source
    # Detect Rust toolchain
    if command -v cargo &> /dev/null; then
        CARGO_CMD="cargo"
    elif [ -f "$HOME/.cargo/bin/cargo" ]; then
        CARGO_CMD="$HOME/.cargo/bin/cargo"
    else
        error "Rust toolchain not found. Install with: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
        error "Or use: INSTALL_FROM_RELEASE=1 bash scripts/update.sh"
        exit 1
    fi

    log "Building agent..."
    (cd "$AGENT_SRC" && $CARGO_CMD build --release 2>&1 | tail -1)

    log "Building API..."
    (cd "$API_SRC" && $CARGO_CMD build --release 2>&1 | tail -1)

    log "Building CLI..."
    (cd "$CLI_SRC" && $CARGO_CMD build --release 2>&1 | tail -1)

    if [ -d "$FRONTEND_DIR" ]; then
        log "Building frontend..."
        (cd "$FRONTEND_DIR" && npm ci --silent 2>/dev/null || npm install --silent 2>/dev/null)
        (cd "$FRONTEND_DIR" && npx vite build 2>&1 | tail -3)
        log "Frontend rebuilt"
    fi
fi

# ── Deploy binaries ───────────────────────────────────────────────────────
# Note: ~2-5s downtime during binary swap is expected for self-hosted deployments.
log "Backing up current binaries..."
cp "$AGENT_BIN" "${AGENT_BIN}.bak" 2>/dev/null || true
cp "$API_BIN" "${API_BIN}.bak" 2>/dev/null || true
cp "$CLI_BIN" "${CLI_BIN}.bak" 2>/dev/null || true

log "Stopping services..."
systemctl stop dockpanel-agent dockpanel-api 2>/dev/null || true

if [ "$INSTALL_FROM_RELEASE" = "1" ]; then
    mv /tmp/dockpanel-agent-new "$AGENT_BIN"
    mv /tmp/dockpanel-api-new "$API_BIN"
    mv /tmp/dockpanel-cli-new "$CLI_BIN"
else
    cp "$AGENT_SRC/target/release/dockpanel-agent" "$AGENT_BIN"
    cp "$API_SRC/target/release/dockpanel-api" "$API_BIN"
    cp "$CLI_SRC/target/release/dockpanel" "$CLI_BIN"
fi
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

echo ""
echo -e "${GREEN}${BOLD}Update complete!${NC}"
echo ""
echo -e "  Agent: $(systemctl is-active dockpanel-agent)"
echo -e "  API:   $(systemctl is-active dockpanel-api)"
echo -e "  Version: $($CLI_BIN --version 2>/dev/null || echo 'unknown')"
echo ""
