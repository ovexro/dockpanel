#!/usr/bin/env bash
#
# DockPanel Setup
# Installs DockPanel on a fresh server.
# Supports: Ubuntu 20+, Debian 11+, CentOS 9+, Rocky 9+, Fedora 39+, Amazon Linux 2023
# Architectures: x86_64, ARM64 (aarch64)
#
# Architecture:
#   - PostgreSQL 16 (Docker container on port 5450)
#   - Agent (Rust binary, systemd, Unix socket)
#   - API (Rust binary, systemd, port 3080)
#   - CLI (Rust binary, /usr/local/bin/dockpanel)
#   - Frontend (Vite build, served by nginx)
#   - Nginx (reverse proxy + static files)
#
# Usage:
#   bash scripts/setup.sh                         # Interactive (asks for domain)
#   PANEL_DOMAIN=panel.example.com bash scripts/setup.sh  # Non-interactive with domain
#   INSTALL_FROM_RELEASE=1 bash scripts/setup.sh  # Download pre-built binaries
#   PANEL_PORT=9090 bash scripts/setup.sh         # Custom port (no domain)
#
set -euo pipefail

# ── Configuration (override with env vars) ──────────────────────────────
PANEL_DOMAIN="${PANEL_DOMAIN:-}"
PANEL_PORT="${PANEL_PORT:-8443}"
CONFIG_DIR="/etc/dockpanel"
AGENT_BIN="/usr/local/bin/dockpanel-agent"
API_BIN="/usr/local/bin/dockpanel-api"
CLI_BIN="/usr/local/bin/dockpanel"
DB_PORT=5450
DB_CONTAINER="dockpanel-postgres"
INSTALL_FROM_RELEASE="${INSTALL_FROM_RELEASE:-0}"
GITHUB_REPO="ovexro/dockpanel"

# ── Resolve repo root ───────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
FRONTEND_DIR="$REPO_DIR/panel/frontend"
AGENT_SRC="$REPO_DIR/panel/agent"
API_SRC="$REPO_DIR/panel/backend"
CLI_SRC="$REPO_DIR/panel/cli"

# ── Colors ───────────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

DIM='\033[2m'
WHITE='\033[1;37m'

log()    { echo -e "  ${GREEN}✓${NC} $1"; }
warn()   { echo -e "  ${YELLOW}⚠${NC} $1"; }
error()  { echo -e "  ${RED}✗${NC} $1" >&2; }
info()   { echo -e "  ${CYAN}→${NC} $1"; }

# ── Progress tracking ─────────────────────────────────────────────────
TOTAL_STEPS=15
CURRENT_STEP=0
SETUP_START=0

progress_bar() {
    local pct=$1
    local width=40
    local filled=$((pct * width / 100))
    local empty=$((width - filled))
    local bar=""
    for ((i=0; i<filled; i++)); do bar+="█"; done
    for ((i=0; i<empty; i++)); do bar+="░"; done
    echo -n "$bar"
}

step() {
    CURRENT_STEP=$((CURRENT_STEP + 1))
    local pct=$((CURRENT_STEP * 100 / TOTAL_STEPS))
    local elapsed=""
    if [ "$SETUP_START" -gt 0 ]; then
        local now
        now=$(date +%s)
        local secs=$((now - SETUP_START))
        elapsed=" ${DIM}${secs}s${NC}"
    fi
    echo ""
    echo -e "  ${DIM}[${CURRENT_STEP}/${TOTAL_STEPS}]${NC} ${CYAN}$(progress_bar $pct)${NC} ${WHITE}${pct}%${NC}${elapsed}"
    echo -e "  ${BOLD}$1${NC}"
    echo ""
}

header() { step "$1"; }

# ── Pre-flight Checks ───────────────────────────────────────────────────
preflight_checks() {
    info "Running pre-flight checks..."

    # Check disk space (need at least 3GB)
    FREE_KB=$(df /opt 2>/dev/null | awk 'NR==2 {print $4}')
    if [ -n "$FREE_KB" ] && [ "$FREE_KB" -lt 3145728 ]; then
        error "Less than 3GB free disk space. Need at least 3GB."
        exit 1
    fi

    # Check available memory (warn if very low)
    FREE_MEM=$(free -m | awk '/^Mem:/ {print $7}')
    if [ -n "$FREE_MEM" ] && [ "$FREE_MEM" -lt 256 ]; then
        warn "Less than 256MB available memory. Performance may be degraded."
    fi

    info "Pre-flight checks passed."
}

# ── Package manager ──────────────────────────────────────────────────────
detect_pkg_manager() {
    if command -v apt-get &> /dev/null; then
        PKG_MGR="apt"
    elif command -v dnf &> /dev/null; then
        PKG_MGR="dnf"
    elif command -v yum &> /dev/null; then
        PKG_MGR="yum"
    else
        error "No supported package manager found (apt/dnf/yum)"
        exit 1
    fi
}

pkg_install() {
    local output
    case "$PKG_MGR" in
        apt) output=$(apt-get install -y "$@" 2>&1) ;;
        dnf) output=$(dnf install -y "$@" 2>&1) ;;
        yum) output=$(yum install -y "$@" 2>&1) ;;
    esac
    local rc=$?
    if [ $rc -ne 0 ]; then
        warn "Failed to install: $* (exit code $rc)"
        echo "$output" | tail -5 >&2
        return $rc
    fi
}

pkg_update() {
    case "$PKG_MGR" in
        apt) apt-get update -y > /dev/null 2>&1 ;;
        dnf) dnf check-update > /dev/null 2>&1 || true ;;
        yum) yum check-update > /dev/null 2>&1 || true ;;
    esac
}

# ── Banner ───────────────────────────────────────────────────────────────
print_banner() {
    echo ""
    echo -e "${CYAN}${BOLD}"
    cat << 'BANNER'
    ____             __   ____                  __
   / __ \____  _____/ /__/ __ \____ _____  ___  / /
  / / / / __ \/ ___/ //_/ /_/ / __ `/ __ \/ _ \/ /
 / /_/ / /_/ / /__/ ,< / ____/ /_/ / / / /  __/ /
/_____/\____/\___/_/|_/_/    \__,_/_/ /_/\___/_/
BANNER
    echo -e "${NC}"
    echo -e "  ${BOLD}Your server. Your rules. Your panel.${NC}"
    echo -e "  Free & open source — https://dockpanel.dev"
    echo ""
}

# ── Checks ───────────────────────────────────────────────────────────────
check_root() {
    if [ "$EUID" -ne 0 ]; then
        error "This script must be run as root (or with sudo)"
        exit 1
    fi
}

check_source() {
    # Source check only needed if building from source
    if [ "$INSTALL_FROM_RELEASE" = "1" ]; then
        return
    fi
    if [ ! -d "$AGENT_SRC/src" ]; then
        error "Cannot find agent source at $AGENT_SRC"
        error "Run this script from the DockPanel repository root,"
        error "or set INSTALL_FROM_RELEASE=1 to download pre-built binaries."
        exit 1
    fi
}

detect_os() {
    header "Detecting OS"

    if [ ! -f /etc/os-release ]; then
        error "Cannot detect OS — /etc/os-release not found"
        exit 1
    fi

    . /etc/os-release

    case "${ID:-}" in
        ubuntu|debian)
            log "Detected: $PRETTY_NAME"
            ;;
        centos|rocky|almalinux|fedora)
            log "Detected: $PRETTY_NAME"
            ;;
        amzn)
            log "Detected: $PRETTY_NAME (Amazon Linux)"
            ;;
        *)
            warn "Untested OS: ${ID:-unknown} — proceeding anyway"
            ;;
    esac

    # Architecture check
    ARCH=$(uname -m)
    case "$ARCH" in
        x86_64)  DL_ARCH="amd64"; log "Architecture: x86_64" ;;
        aarch64) DL_ARCH="arm64"; log "Architecture: ARM64 (homelab ready)" ;;
        *) error "Unsupported architecture: $ARCH"; exit 1 ;;
    esac

    # Check for swap on low-memory systems (Rust compilation needs ~1.5GB RAM)
    if [ "$INSTALL_FROM_RELEASE" != "1" ]; then
        local total_mem
        total_mem=$(awk '/MemTotal/ {print int($2/1024)}' /proc/meminfo 2>/dev/null || echo "0")
        local swap_total
        swap_total=$(awk '/SwapTotal/ {print int($2/1024)}' /proc/meminfo 2>/dev/null || echo "0")

        if [ "$total_mem" -lt 1500 ] && [ "$swap_total" -lt 512 ]; then
            warn "Low memory detected (${total_mem}MB RAM, ${swap_total}MB swap)"
            warn "Rust compilation may fail. Creating 2GB swap file..."
            if [ ! -f /swapfile ]; then
                dd if=/dev/zero of=/swapfile bs=1M count=2048 status=none
                chmod 600 /swapfile
                mkswap /swapfile > /dev/null 2>&1
                swapon /swapfile
                log "Temporary 2GB swap file created"
            else
                log "Swap file already exists"
            fi
        fi
    fi
}

# ── Install Dependencies ────────────────────────────────────────────────
install_dependencies() {
    header "Installing Dependencies"

    pkg_update

    # EPEL for RHEL-family (needed for certbot, fail2ban, etc.)
    if [ "$PKG_MGR" != "apt" ]; then
        pkg_install epel-release || true
    fi

    pkg_install curl openssl ca-certificates

    # lsb-release only on Debian-based
    if [ "$PKG_MGR" = "apt" ]; then
        pkg_install gnupg lsb-release
    fi

    # Build tools required for Rust compilation (cmake for aws-lc-sys, gcc for ring)
    if [ "$INSTALL_FROM_RELEASE" != "1" ]; then
        log "Installing build tools for Rust compilation..."
        if [ "$PKG_MGR" = "apt" ]; then
            pkg_install build-essential cmake pkg-config libssl-dev
        else
            pkg_install gcc gcc-c++ cmake make pkg-config openssl-devel
        fi
        log "Build tools installed"
    fi

    log "Base packages installed"
}

install_docker() {
    header "Docker"

    if command -v docker &> /dev/null; then
        log "Docker already installed: $(docker --version | head -1)"
    else
        log "Installing Docker..."
        curl -fsSL https://get.docker.com | sh > /dev/null 2>&1
        systemctl enable --now docker > /dev/null 2>&1
        log "Docker installed: $(docker --version | head -1)"
    fi
}

install_nginx() {
    header "Nginx"

    if command -v nginx &> /dev/null; then
        log "Nginx already installed"
    else
        log "Installing Nginx..."
        if [ "$PKG_MGR" = "apt" ]; then
            pkg_install nginx
        else
            pkg_install nginx
        fi
        systemctl enable --now nginx > /dev/null 2>&1
        log "Nginx installed"
    fi
}

install_node() {
    header "Node.js (for frontend build)"

    # Skip if using pre-built release (frontend comes as tarball)
    if [ "$INSTALL_FROM_RELEASE" = "1" ]; then
        log "Skipping Node.js (using pre-built frontend)"
        return
    fi

    if command -v node &> /dev/null; then
        log "Node.js already installed: $(node --version)"
    else
        log "Installing Node.js 20 LTS..."
        if [ "$PKG_MGR" = "apt" ]; then
            curl -fsSL https://deb.nodesource.com/setup_20.x | bash - > /dev/null 2>&1
            apt-get install -y nodejs > /dev/null 2>&1
        else
            curl -fsSL https://rpm.nodesource.com/setup_20.x | bash - > /dev/null 2>&1
            $PKG_MGR install -y nodejs > /dev/null 2>&1
        fi
        log "Node.js installed: $(node --version)"
    fi
}

# ── Directories ──────────────────────────────────────────────────────────
create_directories() {
    header "Creating Directories"

    mkdir -p -m 0700 "$CONFIG_DIR"
    mkdir -p /var/run/dockpanel
    mkdir -p /etc/dockpanel/ssl
    mkdir -p /var/backups/dockpanel
    mkdir -p /var/www/acme

    # Ensure socket directory persists across tmpfiles cleanup/reboot
    echo "d /run/dockpanel 0755 root root -" > /etc/tmpfiles.d/dockpanel.conf

    # Create all directories/files required by systemd ReadWritePaths
    # (these may not exist yet on a fresh install — services are installed later)
    mkdir -p /etc/postfix /etc/dovecot /var/vmail /var/spool/postfix /run/opendkim
    mkdir -p /var/lib/nginx /etc/letsencrypt /var/lib/dpkg /var/cache/apt /var/lib/apt
    mkdir -p /etc/php /var/spool/cron /var/lib/dockpanel/git /var/lib/dockpanel/recordings
    touch /etc/opendkim.conf /run/nginx.pid 2>/dev/null || true

    log "Directories created"
}

# ── Secrets ──────────────────────────────────────────────────────────────
generate_secrets() {
    header "Generating Secrets"

    # Agent token (persistent — reuse if exists)
    if [ -f "$CONFIG_DIR/agent.token" ]; then
        AGENT_TOKEN=$(cat "$CONFIG_DIR/agent.token")
        log "Agent token: reusing existing"
    else
        AGENT_TOKEN=$(openssl rand -hex 16)
        echo "$AGENT_TOKEN" > "$CONFIG_DIR/agent.token"
        chmod 600 "$CONFIG_DIR/agent.token"
        log "Agent token: generated"
    fi

    # Reuse from existing api.env if present (idempotent reinstall)
    if [ -f "$CONFIG_DIR/api.env" ]; then
        EXISTING_DB_PW=$(grep '^DATABASE_URL=' "$CONFIG_DIR/api.env" 2>/dev/null | sed 's|.*://dockpanel:\(.*\)@.*|\1|' || true)
        EXISTING_JWT=$(grep '^JWT_SECRET=' "$CONFIG_DIR/api.env" 2>/dev/null | cut -d= -f2- || true)
    fi

    if [ -n "${EXISTING_DB_PW:-}" ] && [ -n "${EXISTING_JWT:-}" ]; then
        DB_PASSWORD="$EXISTING_DB_PW"
        JWT_SECRET="$EXISTING_JWT"
        log "DB password: reusing existing"
        log "JWT secret: reusing existing"
    else
        DB_PASSWORD=$(openssl rand -hex 24)
        JWT_SECRET=$(openssl rand -hex 32)
        log "DB password: generated"
        log "JWT secret: generated"
    fi
}

# ── PostgreSQL ───────────────────────────────────────────────────────────
setup_database() {
    header "PostgreSQL Database"

    if docker ps --format '{{.Names}}' | grep -q "^${DB_CONTAINER}$"; then
        log "PostgreSQL container already running"
    elif docker ps -a --format '{{.Names}}' | grep -q "^${DB_CONTAINER}$"; then
        log "Starting existing PostgreSQL container..."
        docker start "$DB_CONTAINER" > /dev/null 2>&1
    else
        # Remove stale volume from previous failed install (PostgreSQL ignores
        # POSTGRES_PASSWORD when an existing data directory is found, causing
        # password mismatch if the password was regenerated)
        if docker volume inspect dockpanel-pgdata > /dev/null 2>&1; then
            warn "Removing stale database volume from previous install..."
            docker volume rm dockpanel-pgdata > /dev/null 2>&1 || true
        fi

        log "Creating PostgreSQL 16 container..."
        docker run -d \
            --name "$DB_CONTAINER" \
            --restart unless-stopped \
            -e POSTGRES_DB=dockpanel \
            -e POSTGRES_USER=dockpanel \
            -e "POSTGRES_PASSWORD=$DB_PASSWORD" \
            -p "127.0.0.1:${DB_PORT}:5432" \
            -v dockpanel-pgdata:/var/lib/postgresql/data \
            postgres:16-alpine > /dev/null 2>&1
        log "PostgreSQL container created (port $DB_PORT)"
    fi

    # Wait for PostgreSQL to be ready
    log "Waiting for PostgreSQL..."
    local WAITED=0
    while [ "$WAITED" -lt 30 ]; do
        if docker exec "$DB_CONTAINER" pg_isready -U dockpanel > /dev/null 2>&1; then
            log "PostgreSQL ready"
            return
        fi
        sleep 2
        WAITED=$((WAITED + 2))
    done
    error "PostgreSQL did not become ready within 30s"
    exit 1
}

# ── Download Pre-built Binaries ──────────────────────────────────────────
download_binaries() {
    header "Downloading Pre-built Binaries"

    # Get latest release tag
    local RELEASE_TAG
    RELEASE_TAG=$(curl -sf "https://api.github.com/repos/${GITHUB_REPO}/releases/latest" | grep '"tag_name"' | head -1 | cut -d'"' -f4)

    if [ -z "$RELEASE_TAG" ]; then
        error "Could not determine latest release. Check https://github.com/${GITHUB_REPO}/releases"
        exit 1
    fi

    log "Latest release: $RELEASE_TAG"
    local BASE_URL="https://github.com/${GITHUB_REPO}/releases/download/${RELEASE_TAG}"

    # Stop running services before overwriting their binaries. Re-running the
    # installer with services active causes `curl -o` to fail with "Text file
    # busy" (exit 23) because Linux refuses to overwrite a running executable.
    # systemctl stop is a no-op if the unit is inactive or missing.
    if command -v systemctl >/dev/null 2>&1; then
        systemctl stop dockpanel-api dockpanel-agent 2>/dev/null || true
    fi

    # Download agent
    log "Downloading agent (${DL_ARCH})..."
    curl -sfL "${BASE_URL}/dockpanel-agent-linux-${DL_ARCH}" -o "$AGENT_BIN"
    chmod +x "$AGENT_BIN"
    log "Agent downloaded ($(du -h "$AGENT_BIN" | cut -f1))"

    # Download API
    log "Downloading API (${DL_ARCH})..."
    curl -sfL "${BASE_URL}/dockpanel-api-linux-${DL_ARCH}" -o "$API_BIN"
    chmod +x "$API_BIN"
    log "API downloaded ($(du -h "$API_BIN" | cut -f1))"

    # Download CLI
    log "Downloading CLI (${DL_ARCH})..."
    curl -sfL "${BASE_URL}/dockpanel-cli-linux-${DL_ARCH}" -o "$CLI_BIN"
    chmod +x "$CLI_BIN"
    log "CLI downloaded ($(du -h "$CLI_BIN" | cut -f1))"

    # Download frontend
    log "Downloading frontend..."
    local FE_TARBALL="/tmp/dockpanel-frontend.tar.gz"
    curl -sfL "${BASE_URL}/dockpanel-frontend.tar.gz" -o "$FE_TARBALL"

    # Extract frontend — need a target directory
    local FE_DIR="/opt/dockpanel/frontend"
    mkdir -p "$FE_DIR"
    tar xzf "$FE_TARBALL" -C "$FE_DIR"
    rm -f "$FE_TARBALL"

    # If dist/ is nested inside, flatten it
    if [ -d "$FE_DIR/dist" ]; then
        FRONTEND_DIST="$FE_DIR/dist"
    else
        FRONTEND_DIST="$FE_DIR"
    fi

    log "Frontend extracted to $FRONTEND_DIST"
}

# ── Cargo Build with Progress ────────────────────────────────────────────
cargo_build_with_progress() {
    local src_dir="$1"
    local label="$2"
    local count=0
    local start_time
    start_time=$(date +%s)

    (cd "$src_dir" && $CARGO_CMD build --release 2>&1) | while IFS= read -r line; do
        if echo "$line" | grep -qE '^\s*Compiling '; then
            count=$((count + 1))
            local crate_name
            crate_name=$(echo "$line" | sed 's/.*Compiling \([^ ]*\).*/\1/')
            local elapsed=$(( $(date +%s) - start_time ))
            printf "\r    ${DIM}%s: %d crates (%ds) → %s${NC}                    " "$label" "$count" "$elapsed" "$crate_name" >&2
        elif echo "$line" | grep -qE '^\s*Finished '; then
            local elapsed=$(( $(date +%s) - start_time ))
            printf "\r    ${DIM}%s: %d crates compiled in %ds${NC}                              \n" "$label" "$count" "$elapsed" >&2
        fi
    done
}

# ── Build Binaries ───────────────────────────────────────────────────────
build_binaries() {
    header "Building Binaries"

    # Check for Rust toolchain
    if command -v cargo &> /dev/null; then
        CARGO_CMD="cargo"
    elif [ -f "$HOME/.cargo/bin/cargo" ]; then
        CARGO_CMD="$HOME/.cargo/bin/cargo"
    else
        log "Installing Rust toolchain..."
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y > /dev/null 2>&1
        CARGO_CMD="$HOME/.cargo/bin/cargo"
    fi

    # Stop running services so cp can overwrite their binaries (see note in
    # download_binaries — same "Text file busy" trap).
    if command -v systemctl >/dev/null 2>&1; then
        systemctl stop dockpanel-api dockpanel-agent 2>/dev/null || true
    fi

    # Build agent
    log "Building agent..."
    cargo_build_with_progress "$AGENT_SRC" "Agent"
    cp "$AGENT_SRC/target/release/dockpanel-agent" "$AGENT_BIN"
    chmod +x "$AGENT_BIN"
    log "Agent built ($(du -h "$AGENT_BIN" | cut -f1))"

    # Build API
    log "Building API..."
    cargo_build_with_progress "$API_SRC" "API"
    cp "$API_SRC/target/release/dockpanel-api" "$API_BIN"
    chmod +x "$API_BIN"
    log "API built ($(du -h "$API_BIN" | cut -f1))"

    # Build CLI
    log "Building CLI..."
    cargo_build_with_progress "$CLI_SRC" "CLI"
    cp "$CLI_SRC/target/release/dockpanel" "$CLI_BIN"
    chmod +x "$CLI_BIN"
    log "CLI built ($(du -h "$CLI_BIN" | cut -f1))"
}

# ── Build Frontend ───────────────────────────────────────────────────────
build_frontend() {
    header "Building Frontend"

    if [ ! -d "$FRONTEND_DIR" ]; then
        warn "Frontend source not found at $FRONTEND_DIR — skipping"
        return
    fi

    log "Installing npm dependencies..."
    (cd "$FRONTEND_DIR" && npm ci --silent 2>/dev/null || npm install --silent 2>/dev/null)

    log "Building frontend..."
    (cd "$FRONTEND_DIR" && npx vite build 2>&1 | tail -3)
    log "Frontend built at $FRONTEND_DIR/dist/"
}

# ── Systemd Services ─────────────────────────────────────────────────────
create_services() {
    header "Systemd Services"

    # Agent service
    cat > /etc/systemd/system/dockpanel-agent.service << 'EOF'
[Unit]
Description=DockPanel Agent
After=network.target nginx.service
Wants=nginx.service
StartLimitBurst=5
StartLimitIntervalSec=60

[Service]
Type=simple
ExecStartPre=/bin/sh -c 'mkdir -p /run/dockpanel /var/lib/dockpanel/git'
ExecStart=/usr/local/bin/dockpanel-agent
ExecStartPost=/bin/sh -c 'sleep 1 && chgrp $(getent group www-data >/dev/null 2>&1 && echo www-data || echo nginx) /var/run/dockpanel/agent.sock 2>/dev/null; chmod 660 /var/run/dockpanel/agent.sock 2>/dev/null; true'
Restart=always
RestartSec=5
Environment=RUST_LOG=info
NoNewPrivileges=no
ProtectSystem=no
ProtectHome=no
PrivateTmp=no
ProtectKernelLogs=yes
ProtectKernelModules=yes
MemoryMax=512M
LimitNOFILE=65535

[Install]
WantedBy=multi-user.target
EOF

    # API service
    cat > /etc/systemd/system/dockpanel-api.service << 'EOF'
[Unit]
Description=DockPanel API
After=network.target docker.service dockpanel-agent.service
Wants=dockpanel-agent.service
StartLimitBurst=5
StartLimitIntervalSec=60

[Service]
Type=simple
ExecStart=/usr/local/bin/dockpanel-api
Restart=always
RestartSec=5
Environment=RUST_LOG=info
EnvironmentFile=/etc/dockpanel/api.env
NoNewPrivileges=yes
PrivateTmp=yes
ProtectHome=yes
ProtectKernelLogs=yes
ProtectKernelModules=yes
ProtectSystem=no
ReadWritePaths=/var/run/dockpanel /tmp
MemoryMax=1G
LimitNOFILE=65535

[Install]
WantedBy=multi-user.target
EOF

    # API environment — determine BASE_URL from domain or leave empty for IP access
    local API_BASE_URL=""
    if [ -n "$PANEL_DOMAIN" ]; then
        API_BASE_URL="https://${PANEL_DOMAIN}"
    fi

    cat > "$CONFIG_DIR/api.env" << EOF
DATABASE_URL=postgresql://dockpanel:${DB_PASSWORD}@127.0.0.1:${DB_PORT}/dockpanel
JWT_SECRET=${JWT_SECRET}
AGENT_SOCKET=/var/run/dockpanel/agent.sock
AGENT_TOKEN=${AGENT_TOKEN}
LISTEN_ADDR=127.0.0.1:3080
BASE_URL=${API_BASE_URL}
EOF
    chmod 600 "$CONFIG_DIR/api.env"

    systemctl daemon-reload

    # Start agent
    systemctl enable dockpanel-agent > /dev/null 2>&1
    systemctl restart dockpanel-agent
    sleep 2

    if systemctl is-active --quiet dockpanel-agent; then
        log "Agent service running"
    else
        error "Agent failed to start"
        journalctl -u dockpanel-agent --no-pager -n 10
        exit 1
    fi

    # Start API
    systemctl enable dockpanel-api > /dev/null 2>&1
    systemctl restart dockpanel-api
    sleep 2

    if systemctl is-active --quiet dockpanel-api; then
        log "API service running"
    else
        error "API failed to start"
        journalctl -u dockpanel-api --no-pager -n 10
        exit 1
    fi
}

# ── Nginx for Panel ──────────────────────────────────────────────────────
configure_nginx() {
    header "Configuring Nginx"

    # Determine nginx group (www-data on Debian, nginx on RHEL)
    if id -g www-data &> /dev/null; then
        NGINX_GROUP="www-data"
    elif id -g nginx &> /dev/null; then
        NGINX_GROUP="nginx"
    else
        NGINX_GROUP="root"
    fi

    # Determine config directory
    if [ -d /etc/nginx/sites-enabled ]; then
        NGINX_CONF="/etc/nginx/sites-enabled/dockpanel-panel.conf"
    elif [ -d /etc/nginx/conf.d ]; then
        NGINX_CONF="/etc/nginx/conf.d/dockpanel-panel.conf"
    else
        NGINX_CONF="/etc/nginx/conf.d/dockpanel-panel.conf"
        mkdir -p /etc/nginx/conf.d
    fi

    # Determine frontend dist path
    local FE_ROOT
    if [ "$INSTALL_FROM_RELEASE" = "1" ] && [ -n "${FRONTEND_DIST:-}" ]; then
        FE_ROOT="$FRONTEND_DIST"
    else
        FE_ROOT="${FRONTEND_DIR}/dist"
    fi

    local SERVER_NAME="_"
    local LISTEN_DIRECTIVE="listen ${PANEL_PORT};"
    if [ -n "$PANEL_DOMAIN" ]; then
        SERVER_NAME="$PANEL_DOMAIN"
        # Use interface IP to match agent-generated site configs (prevents nginx routing conflicts)
        local BIND_IP
        BIND_IP=$(ip route get 8.8.8.8 2>/dev/null | grep -oP 'src \K\S+' || true)
        if [ -n "$BIND_IP" ]; then
            LISTEN_DIRECTIVE="listen ${BIND_IP}:80;"
        else
            LISTEN_DIRECTIVE="listen 80;"
        fi
    fi

    cat > "$NGINX_CONF" << NGINXEOF
server {
    ${LISTEN_DIRECTIVE}
    server_name ${SERVER_NAME};

    client_max_body_size 100M;

    # API
    location /api/ {
        proxy_pass http://127.0.0.1:3080;
        proxy_http_version 1.1;
        proxy_set_header Upgrade \$http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host \$host;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
        proxy_read_timeout 300s;
    }

    # Agent proxy (for frontend /agent/* calls)
    location /agent/ {
        proxy_pass http://unix:/var/run/dockpanel/agent.sock:/;
        proxy_http_version 1.1;
        proxy_set_header Host \$host;
        proxy_set_header X-Real-IP \$remote_addr;
    }

    # Agent WebSocket terminal
    location /agent/terminal/ws {
        proxy_pass http://unix:/var/run/dockpanel/agent.sock:/terminal/ws;
        proxy_http_version 1.1;
        proxy_set_header Upgrade \$http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host \$host;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_read_timeout 3600s;
        proxy_send_timeout 3600s;
    }

    # Agent WebSocket log stream
    location /agent/logs/stream {
        proxy_pass http://unix:/var/run/dockpanel/agent.sock:/logs/stream;
        proxy_http_version 1.1;
        proxy_set_header Upgrade \$http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host \$host;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_read_timeout 3600s;
        proxy_send_timeout 3600s;
    }

    # Frontend static files
    root ${FE_ROOT};
    index index.html;

    location / {
        try_files \$uri \$uri/ /index.html;
    }

    location /assets/ {
        expires 1y;
        add_header Cache-Control "public, immutable";
    }

    # Hide nginx version
    server_tokens off;

    # Security headers
    add_header X-Content-Type-Options "nosniff" always;
    add_header X-Frame-Options "DENY" always;
    add_header Referrer-Policy "strict-origin-when-cross-origin" always;
    add_header Permissions-Policy "camera=(), microphone=(), geolocation=()" always;
    add_header Strict-Transport-Security "max-age=31536000; includeSubDomains" always;
    add_header Content-Security-Policy "default-src 'self'; script-src 'self' 'unsafe-inline' 'unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self' wss:; frame-ancestors 'none';" always;
    add_header X-XSS-Protection "1; mode=block" always;
}
NGINXEOF

    # Test and restart (full restart needed to release port bindings from removed default site)
    if nginx -t > /dev/null 2>&1; then
        systemctl restart nginx
        log "Nginx configured — panel on port $PANEL_PORT"
    else
        error "Nginx config test failed"
        nginx -t 2>&1
        exit 1
    fi
}

# ── Health Check ─────────────────────────────────────────────────────────
wait_for_health() {
    header "Health Check"

    log "Waiting for API..."
    local WAITED=0
    while [ "$WAITED" -lt 30 ]; do
        if curl -sf http://127.0.0.1:3080/api/health > /dev/null 2>&1; then
            log "API healthy"
            return
        fi
        sleep 2
        WAITED=$((WAITED + 2))
    done

    warn "API not responding on port 3080 yet — check: journalctl -u dockpanel-api -n 20"
}

# ── Summary ──────────────────────────────────────────────────────────────
install_recommended_services() {
    header "Recommended Services"

    # PHP-FPM (needed for WordPress, PHP sites)
    if ! command -v php &> /dev/null; then
        log "Installing PHP-FPM..."
        local PHP_VER="8.3"
        if [ "$PKG_MGR" = "apt" ]; then
            # Add ondrej PPA for PHP 8.3 on older distros
            if ! apt-cache show php8.3 > /dev/null 2>&1; then
                apt-get install -y software-properties-common > /dev/null 2>&1
                add-apt-repository -y ppa:ondrej/php > /dev/null 2>&1 || true
                apt-get update -y > /dev/null 2>&1
            fi
            if apt-get install -y php${PHP_VER}-fpm php${PHP_VER}-cli php${PHP_VER}-mysql \
                php${PHP_VER}-pgsql php${PHP_VER}-curl php${PHP_VER}-gd php${PHP_VER}-mbstring \
                php${PHP_VER}-xml php${PHP_VER}-zip php${PHP_VER}-bcmath php${PHP_VER}-intl \
                php${PHP_VER}-readline php${PHP_VER}-opcache > /dev/null 2>&1; then
                systemctl enable --now php${PHP_VER}-fpm > /dev/null 2>&1
                log "PHP ${PHP_VER} installed with FPM"
            else
                warn "PHP ${PHP_VER} installation failed — install manually later from Settings → Services"
            fi
        else
            # RHEL/Rocky/Fedora
            if pkg_install php-fpm php-cli php-common php-mysqlnd php-pgsql php-xml php-mbstring php-curl php-zip php-gd; then
                systemctl enable --now php-fpm > /dev/null 2>&1
                log "PHP installed with FPM"
            else
                warn "PHP installation failed — install manually later from Settings → Services"
            fi
        fi
    else
        log "PHP already installed: $(php -v | head -1 | awk '{print $2}')"
    fi

    # Certbot (needed for SSL certificates)
    if ! command -v certbot &> /dev/null; then
        log "Installing Certbot..."
        if pkg_install certbot python3-certbot-nginx; then
            systemctl enable --now certbot.timer > /dev/null 2>&1
            log "Certbot installed with auto-renewal"
        else
            warn "Certbot installation failed — SSL provisioning will not work until installed"
        fi
    else
        log "Certbot already installed"
    fi

    # UFW (firewall)
    if ! command -v ufw &> /dev/null; then
        log "Installing UFW firewall..."
        pkg_install ufw
        ufw default deny incoming > /dev/null 2>&1
        ufw default allow outgoing > /dev/null 2>&1
        ufw allow 22/tcp > /dev/null 2>&1
        ufw --force enable > /dev/null 2>&1
        log "UFW installed and enabled"
    else
        log "UFW already installed"
    fi

    # Ensure panel ports are always open (even if UFW was pre-existing)
    if command -v ufw &> /dev/null; then
        ufw allow 80/tcp > /dev/null 2>&1
        ufw allow 443/tcp > /dev/null 2>&1
        if [ -n "$PANEL_PORT" ] && [ "$PANEL_PORT" != "80" ] && [ "$PANEL_PORT" != "443" ]; then
            ufw allow "${PANEL_PORT}/tcp" > /dev/null 2>&1
        fi
        log "Firewall: ports 80, 443${PANEL_PORT:+, $PANEL_PORT} allowed"
    fi

    # Fail2Ban (intrusion prevention)
    if ! command -v fail2ban-client &> /dev/null; then
        log "Installing Fail2Ban..."
        if ! pkg_install fail2ban; then
            warn "Fail2Ban installation failed — install manually later from Settings → Services"
            log "All recommended services ready"
            return
        fi
        cat > /etc/fail2ban/jail.local << 'F2BEOF'
[DEFAULT]
bantime = 3600
findtime = 600
maxretry = 5

[sshd]
enabled = true

[nginx-http-auth]
enabled = true

[nginx-limit-req]
enabled = true
F2BEOF
        systemctl enable --now fail2ban > /dev/null 2>&1
        log "Fail2Ban installed with SSH + Nginx jails"
    else
        log "Fail2Ban already installed"
    fi

    log "All recommended services ready"
}

provision_panel_ssl() {
    if [ -z "$PANEL_DOMAIN" ]; then
        log "No domain set — skipping SSL (access via IP:${PANEL_PORT})"
        return
    fi

    header "Panel SSL Certificate"

    if ! command -v certbot &> /dev/null; then
        log "Certbot not found — skipping SSL"
        return
    fi

    log "Provisioning Let's Encrypt certificate for $PANEL_DOMAIN..."
    if certbot --nginx -d "$PANEL_DOMAIN" --non-interactive --agree-tos --register-unsafely-without-email --redirect 2>/dev/null; then
        log "SSL certificate provisioned for $PANEL_DOMAIN"
    else
        log "SSL provisioning failed — you can retry manually: certbot --nginx -d $PANEL_DOMAIN"
        log "If using Cloudflare proxy, set SSL mode to 'Full' and try again"
    fi
}

print_summary() {
    local SERVER_IP
    SERVER_IP=$(curl -sf --max-time 5 https://api.ipify.org 2>/dev/null || \
                hostname -I 2>/dev/null | awk '{print $1}' || \
                echo "YOUR_SERVER_IP")

    local elapsed_total=$(( $(date +%s) - SETUP_START ))
    local mins=$((elapsed_total / 60))
    local secs=$((elapsed_total % 60))

    echo ""
    echo -e "  ${CYAN}$(progress_bar 100)${NC} ${WHITE}100%${NC} ${DIM}${mins}m ${secs}s${NC}"
    echo ""
    echo -e "${GREEN}${BOLD}╔══════════════════════════════════════════════════════╗${NC}"
    echo -e "${GREEN}${BOLD}║         DockPanel installed successfully!            ║${NC}"
    echo -e "${GREEN}${BOLD}╚══════════════════════════════════════════════════════╝${NC}"
    echo ""
    if [ -n "$PANEL_DOMAIN" ]; then
        echo -e "  ${BOLD}Panel URL:${NC}      https://${PANEL_DOMAIN}"
    else
        echo -e "  ${BOLD}Panel URL:${NC}      http://${SERVER_IP}:${PANEL_PORT}"
    fi
    echo ""
    echo -e "  ${BOLD}First step:${NC}     Open the URL and create your admin account"
    echo ""
    echo -e "  ${BOLD}CLI:${NC}            dockpanel status"
    echo -e "                  dockpanel diagnose"
    echo -e "                  dockpanel --help"
    echo ""
    echo -e "  ${BOLD}Service commands:${NC}"
    echo -e "    Agent status:   systemctl status dockpanel-agent"
    echo -e "    API status:     systemctl status dockpanel-api"
    echo -e "    Agent logs:     journalctl -u dockpanel-agent -f"
    echo -e "    API logs:       journalctl -u dockpanel-api -f"
    echo -e "    Restart all:    systemctl restart dockpanel-agent dockpanel-api"
    echo ""
    echo -e "  ${BOLD}Paths:${NC}"
    echo -e "    Config:         ${CONFIG_DIR}/"
    echo -e "    Agent token:    ${CONFIG_DIR}/agent.token"
    echo -e "    API env:        ${CONFIG_DIR}/api.env"
    echo -e "    Backups:        /var/backups/dockpanel/"
    echo ""
    echo -e "  ${BOLD}Database:${NC}"
    echo -e "    Container:      ${DB_CONTAINER} (port ${DB_PORT})"
    echo -e "    Connect:        docker exec -it ${DB_CONTAINER} psql -U dockpanel -d dockpanel"
    echo ""
    echo -e "  ${BOLD}Installed services:${NC}"
    echo -e "    Docker, Nginx, PHP-FPM, Certbot, UFW, Fail2Ban"
    echo ""
    echo -e "  ${BOLD}Optional (install from panel):${NC}"
    echo -e "    Mail server:    Settings → Services or Mail page → Install"
    echo -e "    Webmail:        Apps → Deploy → Roundcube"
    echo -e "    Spam filter:    Apps → Deploy → Rspamd"
    echo ""
    echo -e "  ${YELLOW}Next steps:${NC}"
    echo -e "    1. Open the panel URL and create your admin account"
    echo -e "    2. Add your first site (Sites → Create Site)"
    echo -e "    3. Provision SSL (click the lock icon on any site)"
    echo -e "    4. Run diagnostics (Diagnostics → Run Scan)"
    echo ""
    echo -e "  ${YELLOW}Update:${NC}   Run: bash /opt/dockpanel/scripts/update.sh"
    echo ""
}

# ── PostgreSQL Backup ────────────────────────────────────────────────────
setup_db_backup() {
    header "PostgreSQL Backup"

    local BACKUP_SCRIPT="/opt/dockpanel/scripts/db-backup.sh"
    mkdir -p /opt/dockpanel/scripts

    cat > "$BACKUP_SCRIPT" << 'BKEOF'
#!/bin/bash
BACKUP_DIR="/var/backups/dockpanel/db"
mkdir -p "$BACKUP_DIR"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
docker exec dockpanel-postgres pg_dump -U dockpanel -d dockpanel | gzip > "$BACKUP_DIR/dockpanel_$TIMESTAMP.sql.gz"
# Keep last 7 days
find "$BACKUP_DIR" -name "*.sql.gz" -mtime +7 -delete
BKEOF
    chmod +x "$BACKUP_SCRIPT"

    # Install cron job (daily at 3 AM)
    (crontab -l 2>/dev/null | grep -v "$BACKUP_SCRIPT"; echo "0 3 * * * $BACKUP_SCRIPT") | crontab -

    log "Database backup script installed ($BACKUP_SCRIPT)"
    log "Cron job: daily at 3:00 AM, 7-day retention"
}

# ── Main ─────────────────────────────────────────────────────────────────
main() {
    SETUP_START=$(date +%s)
    print_banner
    check_root
    detect_pkg_manager
    detect_os
    preflight_checks

    # Auto-detect: if no source available, use release binaries
    if [ "$INSTALL_FROM_RELEASE" != "1" ] && [ ! -d "$AGENT_SRC/src" ]; then
        log "No source found — switching to pre-built binary download"
        INSTALL_FROM_RELEASE=1
    fi

    # Ask for domain if not set via env
    if [ -z "$PANEL_DOMAIN" ]; then
        echo ""
        echo -e "${BOLD}Enter your panel domain (e.g. panel.example.com)${NC}"
        echo -e "Leave blank to access via IP:${PANEL_PORT} instead"
        echo -e "${BOLD}Tip:${NC} set PANEL_DOMAIN=... in the environment to skip this prompt"
        echo -n "> "
        if [ -t 0 ]; then
            read -r PANEL_DOMAIN
        # `[ -r /dev/tty ]` returns true on Linux even when the process has no
        # controlling tty. Probe with an actual open so we don't print a confusing
        # "No such device or address" error to stderr.
        elif { : </dev/tty; } 2>/dev/null; then
            # Piped via curl but an interactive terminal is reachable
            read -r PANEL_DOMAIN < /dev/tty || PANEL_DOMAIN=""
        else
            # Fully non-interactive (e.g. piped through SSH without tty).
            # Skip the prompt — caller should have set PANEL_DOMAIN already.
            echo "(no tty — continuing without a panel domain; set PANEL_DOMAIN to configure)"
            PANEL_DOMAIN=""
        fi
        PANEL_DOMAIN=$(echo "$PANEL_DOMAIN" | tr -d ' ')
    fi

    if [ -n "$PANEL_DOMAIN" ]; then
        log "Panel domain: $PANEL_DOMAIN"
        PANEL_PORT="80"  # Will be upgraded to 443 by certbot
    fi

    check_source
    install_dependencies
    install_docker
    install_nginx
    install_node
    create_directories
    generate_secrets
    setup_database

    if [ "$INSTALL_FROM_RELEASE" = "1" ]; then
        download_binaries
    else
        build_binaries
        build_frontend
    fi

    # Remove default server block that conflicts
    if [ -f /etc/nginx/sites-enabled/default ]; then
        rm -f /etc/nginx/sites-enabled/default
    fi
    # RHEL: comment out default server block in nginx.conf
    if [ "$PKG_MGR" != "apt" ] && grep -q "server {" /etc/nginx/nginx.conf 2>/dev/null; then
        sed -i '/^[[:space:]]*server {/,/^[[:space:]]*}/s/^/#/' /etc/nginx/nginx.conf 2>/dev/null || true
    fi

    # These steps should continue even if one fails
    set +e
    configure_nginx
    create_services

    # Wait for services to start
    sleep 3

    # Start services (may already be started by create_services)
    systemctl start dockpanel-agent 2>/dev/null
    systemctl start dockpanel-api 2>/dev/null
    sleep 2

    install_recommended_services
    provision_panel_ssl
    wait_for_health
    setup_db_backup
    set -e

    print_summary
}

main "$@"
