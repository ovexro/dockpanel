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
#   bash scripts/setup.sh                # Build from source
#   INSTALL_FROM_RELEASE=1 bash scripts/setup.sh  # Download pre-built binaries
#   PANEL_PORT=9090 bash scripts/setup.sh
#
set -euo pipefail

# ── Configuration (override with env vars) ──────────────────────────────
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

log()    { echo -e "${GREEN}[+]${NC} $1"; }
warn()   { echo -e "${YELLOW}[!]${NC} $1"; }
error()  { echo -e "${RED}[x]${NC} $1" >&2; }
info()   { echo -e "${CYAN}[i]${NC} $1"; }
header() { echo -e "\n${CYAN}${BOLD}── $1 ──${NC}\n"; }

# ── Pre-flight Checks ───────────────────────────────────────────────────
preflight_checks() {
    info "Running pre-flight checks..."

    # Check disk space (need at least 3GB)
    FREE_KB=$(df /opt 2>/dev/null | awk 'NR==2 {print $4}')
    if [ -n "$FREE_KB" ] && [ "$FREE_KB" -lt 3145728 ]; then
        error "Less than 3GB free disk space. Need at least 3GB."
        exit 1
    fi

    # Check available memory (need at least 512MB)
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
    case "$PKG_MGR" in
        apt) apt-get install -y "$@" > /dev/null 2>&1 ;;
        dnf) dnf install -y "$@" > /dev/null 2>&1 ;;
        yum) yum install -y "$@" > /dev/null 2>&1 ;;
    esac
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

    # ARM: check for swap (compilation needs ~1GB RAM)
    if [ "$ARCH" = "aarch64" ] && [ "$INSTALL_FROM_RELEASE" != "1" ]; then
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
    pkg_install curl openssl ca-certificates

    # lsb-release only on Debian-based
    if [ "$PKG_MGR" = "apt" ]; then
        pkg_install gnupg lsb-release
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
    echo "d /var/run/dockpanel 0755 root root -" > /etc/tmpfiles.d/dockpanel.conf

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

    # Build agent
    log "Building agent (this may take a few minutes)..."
    (cd "$AGENT_SRC" && $CARGO_CMD build --release 2>&1 | tail -1)
    cp "$AGENT_SRC/target/release/dockpanel-agent" "$AGENT_BIN"
    chmod +x "$AGENT_BIN"
    log "Agent built ($(du -h "$AGENT_BIN" | cut -f1))"

    # Build API
    log "Building API (this may take a few minutes)..."
    (cd "$API_SRC" && $CARGO_CMD build --release 2>&1 | tail -1)
    cp "$API_SRC/target/release/dockpanel-api" "$API_BIN"
    chmod +x "$API_BIN"
    log "API built ($(du -h "$API_BIN" | cut -f1))"

    # Build CLI
    log "Building CLI..."
    (cd "$CLI_SRC" && $CARGO_CMD build --release 2>&1 | tail -1)
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

[Service]
Type=simple
ExecStart=/usr/local/bin/dockpanel-agent
ExecStartPost=/bin/sh -c 'sleep 1 && chgrp www-data /var/run/dockpanel/agent.sock 2>/dev/null; chmod 660 /var/run/dockpanel/agent.sock 2>/dev/null; true'
Restart=always
RestartSec=5
StartLimitBurst=5
StartLimitIntervalSec=60
Environment=RUST_LOG=info
RuntimeDirectory=dockpanel
ReadWritePaths=/etc/nginx /etc/dockpanel /var/run/dockpanel /var/backups/dockpanel /var/www /var/log /etc/letsencrypt /var/lib/nginx /run/nginx.pid
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=read-only
PrivateTmp=yes
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

[Service]
Type=simple
ExecStart=/usr/local/bin/dockpanel-api
Restart=always
RestartSec=5
StartLimitBurst=5
StartLimitIntervalSec=60
Environment=RUST_LOG=info
EnvironmentFile=/etc/dockpanel/api.env
NoNewPrivileges=yes
PrivateTmp=yes
ProtectHome=yes
ProtectKernelLogs=yes
ProtectKernelModules=yes
ProtectSystem=strict
RuntimeDirectory=dockpanel
ReadWritePaths=/var/run/dockpanel /tmp
MemoryMax=1G
LimitNOFILE=65535

[Install]
WantedBy=multi-user.target
EOF

    # API environment
    cat > "$CONFIG_DIR/api.env" << EOF
DATABASE_URL=postgresql://dockpanel:${DB_PASSWORD}@127.0.0.1:${DB_PORT}/dockpanel
JWT_SECRET=${JWT_SECRET}
AGENT_SOCKET=/var/run/dockpanel/agent.sock
AGENT_TOKEN=${AGENT_TOKEN}
LISTEN_ADDR=127.0.0.1:3080
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

    cat > "$NGINX_CONF" << NGINXEOF
server {
    listen ${PANEL_PORT};
    server_name _;

    client_max_body_size 100M;

    # API
    location /api/ {
        proxy_pass http://127.0.0.1:3080;
        proxy_http_version 1.1;
        proxy_set_header Host \$host;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
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

    # Test and reload
    if nginx -t > /dev/null 2>&1; then
        nginx -s reload 2>/dev/null || systemctl reload nginx
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
print_summary() {
    local SERVER_IP
    SERVER_IP=$(curl -sf --max-time 5 https://api.ipify.org 2>/dev/null || \
                hostname -I 2>/dev/null | awk '{print $1}' || \
                echo "YOUR_SERVER_IP")

    echo ""
    echo -e "${GREEN}${BOLD}╔══════════════════════════════════════════════════════╗${NC}"
    echo -e "${GREEN}${BOLD}║         DockPanel installed successfully!            ║${NC}"
    echo -e "${GREEN}${BOLD}╚══════════════════════════════════════════════════════╝${NC}"
    echo ""
    echo -e "  ${BOLD}Panel URL:${NC}      http://${SERVER_IP}:${PANEL_PORT}"
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
    echo -e "  ${YELLOW}Security:${NC} Restrict port ${PANEL_PORT} with a firewall (ufw/iptables)."
    echo -e "  ${YELLOW}SSL:${NC}      Set up HTTPS with: certbot --nginx"
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

    create_services
    configure_nginx
    wait_for_health
    setup_db_backup
    print_summary
}

main "$@"
