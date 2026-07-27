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
# Set to 1 by configure_nginx when the panel is served over a self-signed
# certificate (no domain → no Let's Encrypt). print_summary reads it to print
# the right scheme and explain the browser warning before it happens.
PANEL_SELF_SIGNED=0
# Set to 1 by provision_panel_ssl only when certbot actually issued a
# certificate. print_summary reads it so a failed issuance can never be
# reported as an https:// panel URL.
PANEL_SSL_OK=0
# Which firewall this box is enforcing with: firewalld | ufw | none.
# Resolved by detect_firewall in main().
FW_MGR="none"

# ── Resolve repo root ───────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
FRONTEND_DIR="$REPO_DIR/panel/frontend"
AGENT_SRC="$REPO_DIR/panel/agent"
API_SRC="$REPO_DIR/panel/backend"
CLI_SRC="$REPO_DIR/panel/cli"

# Directories the agent unit declares in ReadWritePaths, with any `-` prefix
# stripped. The unit (panel/agent/dockpanel-agent.service) is the single source
# of truth, so this can never fall behind it the way a hand-copied list does.
# Prints nothing if no unit is readable; callers must tolerate an empty result.
agent_rwp_dirs() {
    local unit="$AGENT_SRC/dockpanel-agent.service"
    [ -f "$unit" ] || unit="/etc/systemd/system/dockpanel-agent.service"
    [ -f "$unit" ] || return 0
    sed -n 's/^ReadWritePaths=//p' "$unit" | tr ' ' '\n' | sed 's/^-//' | grep '^/' || true
}

# ── Colors ───────────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

DIM='\033[2m'
WHITE='\033[1;37m'

# ── Install log + terminal detection ─────────────────────────────────────
# Everything a wrapped command prints is captured here, so a failure at any
# step leaves full forensics behind instead of vanishing into >/dev/null.
INSTALL_LOG="/var/log/dockpanel-install.log"
IS_TTY=0
if [ -t 1 ]; then IS_TTY=1; fi

# Spinner frames: braille on UTF-8 terminals, plain ASCII everywhere else.
case "${LC_ALL:-${LC_CTYPE:-${LANG:-}}}" in
    *[Uu][Tt][Ff]*8*) SPIN_FRAMES='⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏' ;;
    *)                SPIN_FRAMES='|/-\' ;;
esac
SPIN_NFRAMES=${#SPIN_FRAMES}

plog() {  # append a plain line to the install log; never fails, never echoes
    printf '%s\n' "$*" >> "$INSTALL_LOG" 2>/dev/null || true
}

log()    { echo -e "  ${GREEN}✓${NC} $1";   plog "  [ok]   $1"; }
warn()   { echo -e "  ${YELLOW}⚠${NC} $1";  plog "  [warn] $1"; }
error()  { echo -e "  ${RED}✗${NC} $1" >&2; plog "  [FAIL] $1"; }
info()   { echo -e "  ${CYAN}→${NC} $1";    plog "  [..]   $1"; }

# ── Spinner-wrapped command runner ───────────────────────────────────────
# run "Label" cmd [args...] — runs cmd with all output captured to the
# install log, animating a spinner + live elapsed seconds while it works
# (falls back to a static line when stdout isn't a terminal). Returns the
# command's exit code; the caller decides whether that's fatal.
SPIN_PID=""
run() {
    local label="$1"; shift
    plog "→ RUN: $label"
    plog "  \$ $*"
    local rc=0
    if [ "$IS_TTY" = "1" ]; then
        "$@" >> "$INSTALL_LOG" 2>&1 &
        SPIN_PID=$!
        local i=0 start now frame
        start=$(date +%s)
        while kill -0 "$SPIN_PID" 2>/dev/null; do
            frame="${SPIN_FRAMES:$((i % SPIN_NFRAMES)):1}"
            now=$(( $(date +%s) - start ))
            # Colours live in the FORMAT string as literal escapes; the frame
            # char goes through %s so a backslash frame (ASCII set) can't merge
            # with a following escape and print literal garbage.
            printf '\r\033[2K  \033[0;36m%s\033[0m %s \033[2m%ss\033[0m' "$frame" "$label" "$now"
            i=$((i + 1))
            sleep 0.12 2>/dev/null || sleep 1
        done
        if wait "$SPIN_PID"; then rc=0; else rc=$?; fi
        SPIN_PID=""
        printf '\r\033[2K'
    else
        info "$label"
        if "$@" >> "$INSTALL_LOG" 2>&1; then rc=0; else rc=$?; fi
    fi
    plog "← DONE rc=$rc: $label"
    return $rc
}

# ── Failure box (EXIT trap) ──────────────────────────────────────────────
# If the installer dies anywhere before the summary, tell the user exactly
# where it stopped, show the tail of the log, and how to retry — instead of
# cutting off mid-paint.
CURRENT_STEP_NAME="starting up"
INSTALL_DONE=0
on_exit() {
    local rc=$?
    if [ -n "$SPIN_PID" ]; then kill "$SPIN_PID" 2>/dev/null || true; fi
    if [ "$INSTALL_DONE" = "1" ]; then return 0; fi
    echo ""
    echo -e "${RED}${BOLD}╔══════════════════════════════════════════════════════╗${NC}"
    echo -e "${RED}${BOLD}║           DockPanel install did not finish           ║${NC}"
    echo -e "${RED}${BOLD}╚══════════════════════════════════════════════════════╝${NC}"
    echo ""
    echo -e "  Stopped during: ${BOLD}${CURRENT_STEP_NAME}${NC} (exit code ${rc})"
    if [ -s "$INSTALL_LOG" ]; then
        echo ""
        echo -e "  Last lines of the install log:"
        tail -n 8 "$INSTALL_LOG" 2>/dev/null | sed 's/^/    /' || true
        echo ""
        echo -e "  Full log:  ${BOLD}${INSTALL_LOG}${NC}"
    fi
    echo ""
    echo -e "  Re-running the installer is safe — it picks up where it left off:"
    echo -e "    ${BOLD}curl -sL https://dockpanel.dev/install.sh | sudo bash${NC}"
    echo ""
    echo -e "  Stuck? Open an issue: https://github.com/ovexro/dockpanel/issues"
    echo ""
}
trap on_exit EXIT

# ── Service outcome tracking ─────────────────────────────────────────────
# The final summary prints what ACTUALLY happened, not a wishlist.
SVC_OK=()
SVC_FAIL=()
svc_ok()   { SVC_OK+=("$1"); }
svc_fail() { SVC_FAIL+=("$1"); }

# Wait for a unit to come up, and do not confuse "not running" with "could not ask".
#
# `systemctl is-active` answers three different questions with two exit codes.
# During install, dbus can be momentarily unavailable — the call then prints
#   Failed to retrieve unit state: Message recipient disconnected from message
#   bus without replying
# to stderr, exits non-zero, and says nothing on stdout. A caller that reads only
# the exit code concludes the service failed. Observed on a genuinely fresh
# Ubuntu 24.04 box (s258): the installer aborted at step 11/15 with "Agent failed
# to start" while its own journal excerpt, printed directly underneath, showed the
# agent started and listening on its socket. The install stopped four steps from
# the end over a question that was never answered.
#
# So read the ANSWER, not the exit code: `active` succeeds, `failed` gives up
# immediately (waiting cannot help a unit that already failed), and anything else
# — `activating`, `inactive`, or the empty string we get when the bus is out — is
# treated as "not yet" and retried inside a bounded window.
wait_for_unit() {
    local unit="$1" tries="${2:-20}" state
    for _ in $(seq 1 "$tries"); do
        state=$(systemctl is-active "$unit" 2>/dev/null || true)
        case "$state" in
            active) return 0 ;;
            failed) return 1 ;;
            *)      sleep 1 ;;
        esac
    done
    [ "$(systemctl is-active "$unit" 2>/dev/null || true)" = "active" ]
}

join_comma() {
    local out="" item
    for item in "$@"; do
        if [ -z "$out" ]; then out="$item"; else out="$out, $item"; fi
    done
    printf '%s' "$out"
}

# ── Progress tracking ─────────────────────────────────────────────────
# TOTAL_STEPS is recomputed in main() once the install mode (release vs
# source) and domain are known, so the bar reaches 100% exactly at the end
# instead of jumping from 93%.
TOTAL_STEPS=14
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
    CURRENT_STEP_NAME="$1"
    local pct=$((CURRENT_STEP * 100 / TOTAL_STEPS))
    if [ "$pct" -gt 100 ]; then pct=100; fi
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
    plog ""
    plog "== STEP ${CURRENT_STEP}/${TOTAL_STEPS}: $1 =="
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
        # Tell apt to wait up to 5 min for the dpkg lock instead of failing
        # immediately. Without this, agent installers (PHP, services, updates)
        # fail with "Could not get lock /var/lib/dpkg/lock-frontend" whenever
        # unattended-upgrades is running in the background — common on fresh
        # Debian 13 boots, where the auto-update kicks off right after install.
        mkdir -p /etc/apt/apt.conf.d
        cat > /etc/apt/apt.conf.d/99-dockpanel-lock-wait.conf << 'APT_EOF'
DPkg::Lock::Timeout "300";
APT_EOF
    elif command -v dnf &> /dev/null; then
        PKG_MGR="dnf"
    elif command -v yum &> /dev/null; then
        PKG_MGR="yum"
    else
        error "No supported package manager found (apt/dnf/yum)"
        exit 1
    fi
}

# Plain installers — meant to be wrapped in `run`, which captures their full
# output into $INSTALL_LOG (the old versions swallowed output, so failures
# left nothing to debug with).
pkg_install() {
    case "$PKG_MGR" in
        apt) apt-get install -y "$@" ;;
        dnf) dnf install -y "$@" ;;
        yum) yum install -y "$@" ;;
    esac
}

pkg_update() {
    case "$PKG_MGR" in
        apt) apt-get update -y ;;
        dnf) dnf check-update || true ;;
        yum) yum check-update || true ;;
    esac
}

# ── Firewall ─────────────────────────────────────────────────────────────
# Sibling of detect_pkg_manager: find the firewall the box is ALREADY
# enforcing with, so we configure that one instead of installing a second.
# Debian/Ubuntu images normally ship neither -> we install UFW. RHEL-family
# images ship firewalld running -> we use it. Never both (see the s265 note
# in install_recommended_services).
detect_firewall() {
    if command -v firewall-cmd &> /dev/null && firewall-cmd --state &> /dev/null; then
        FW_MGR="firewalld"
    elif command -v ufw &> /dev/null; then
        FW_MGR="ufw"
    else
        FW_MGR="none"
    fi
}

# Allow a port, e.g. `fw_allow 443/tcp`. Returns non-zero when there is no
# firewall to configure or the rule could not be added — callers must not
# assume success, which is exactly what the old code did.
fw_allow() {
    local port="$1"
    case "$FW_MGR" in
        firewalld) firewall-cmd --permanent --add-port="$port" > /dev/null 2>&1 ;;
        ufw)       ufw allow "$port" > /dev/null 2>&1 ;;
        *)         return 1 ;;
    esac
}

# firewalld stages --permanent rules; they do nothing until reloaded. UFW
# applies immediately, so this is a no-op there.
fw_reload() {
    case "$FW_MGR" in
        firewalld) firewall-cmd --reload > /dev/null 2>&1 || true ;;
    esac
}

# ── SELinux ──────────────────────────────────────────────────────────────
# On Enforcing systems (the RHEL family default) nginx may not open outbound
# TCP connections unless httpd_can_network_connect is set. The panel vhost
# proxies to the API on 127.0.0.1:3080 and every site vhost proxies to PHP-FPM
# or an app container, so without this EVERY request returns 502 — including
# from the box itself. s265 measured it: the boolean alone flipped 502 -> 200.
# The denial is dontaudit'ed, so nothing appears in ausearch or the journal;
# there is no breadcrumb to follow, which is why this must be set up front.
configure_selinux() {
    command -v getenforce &> /dev/null || return 0
    [ "$(getenforce 2>/dev/null)" = "Enforcing" ] || return 0

    header "SELinux"
    if command -v setsebool &> /dev/null && \
       run "Allowing nginx to reach the panel API (httpd_can_network_connect)" \
           setsebool -P httpd_can_network_connect on; then
        log "SELinux: nginx may now proxy to the API and to site backends"
        svc_ok "SELinux policy"
    else
        warn "SELinux is Enforcing and httpd_can_network_connect could not be set —"
        warn "the panel will answer 502 until you run: setsebool -P httpd_can_network_connect on"
        svc_fail "SELinux policy"
    fi
}

# True when apt has a real installation candidate for a package. `apt-cache
# show` is NOT enough — on Ubuntu 26.04 `php-opcache` still has a stanza but
# `Candidate: (none)` (OPcache became built-in in PHP 8.5), and one dead
# package fails an entire apt transaction.
apt_has_candidate() {
    local c
    # LC_ALL=C: apt-cache localises the "Candidate:" label (fr "Candidat :",
    # de "Installationskandidat:"), which would make the sed match nothing on
    # a non-English box and silently block the whole PHP install.
    c=$(LC_ALL=C apt-cache policy "$1" 2>/dev/null | sed -n 's/^  Candidate: //p')
    [ -n "$c" ] && [ "$c" != "(none)" ]
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

    run "Refreshing package index" pkg_update || true

    # EPEL for RHEL-family (needed for certbot, fail2ban, etc.)
    #
    # CRB ("CodeReady Builder", called PowerTools on EL8) must be enabled
    # ALONGSIDE it. EPEL's own installation instructions require it, because
    # EPEL packages routinely link against libraries EL keeps there — and a
    # missing CRB does not fail loudly, it fails at the point some LATER package
    # is installed. Measured on Rocky 9.8 (s268): `opendkim` and
    # `opendkim-tools` need `libmilter.so.1.0` (sendmail-milter) and
    # `libmemcached.so.11` (libmemcached-awesome), both CRB-only, so the mail
    # installer died on "nothing provides …" — while a name-availability probe
    # said all four packages were available, which is how the product came to
    # claim the packages resolved here.
    if [ "$PKG_MGR" != "apt" ]; then
        run "Enabling EPEL repository" pkg_install epel-release || true
        # The repo id is `crb` on EL9+/Rocky/Alma and `powertools` on EL8.
        # Try both; neither existing is not fatal on its own.
        if command -v dnf >/dev/null 2>&1; then
            run "Enabling CRB repository (required by EPEL)" \
                sh -c 'dnf -y config-manager --set-enabled crb 2>/dev/null \
                    || dnf -y config-manager --set-enabled powertools 2>/dev/null \
                    || dnf -y install dnf-plugins-core >/dev/null 2>&1 && \
                       { dnf -y config-manager --set-enabled crb 2>/dev/null \
                      || dnf -y config-manager --set-enabled powertools 2>/dev/null; }' || true
        fi
    fi

    local BASE_PKGS="curl, openssl, ca-certificates"
    if [ "$PKG_MGR" = "apt" ]; then
        # gnupg + lsb-release only exist/matter on Debian-based
        run "Installing base packages (${BASE_PKGS}, gnupg, lsb-release)" \
            pkg_install curl openssl ca-certificates gnupg lsb-release
    else
        run "Installing base packages (${BASE_PKGS})" \
            pkg_install curl openssl ca-certificates
    fi

    # Build tools required for Rust compilation (cmake for aws-lc-sys, gcc for ring)
    if [ "$INSTALL_FROM_RELEASE" != "1" ]; then
        if [ "$PKG_MGR" = "apt" ]; then
            run "Installing build tools (build-essential, cmake, pkg-config, libssl-dev)" \
                pkg_install build-essential cmake pkg-config libssl-dev
        else
            run "Installing build tools (gcc, cmake, make, pkg-config, openssl-devel)" \
                pkg_install gcc gcc-c++ cmake make pkg-config openssl-devel
        fi
        log "Build tools installed"
    fi

    log "Base packages installed"
}

# Docker's convenience script points each distro at its own repo path under
# download.docker.com/linux/<distro>. For the RHEL rebuilds that path is a trap:
# `linux/rocky/9/` EXISTS and serves valid metadata, but upstream fills it with
# containerd.io and the plugins only — no docker-ce, no docker-ce-cli. So on
# Rocky the script cheerfully adds the repo and the very next command dies with
# `Error: Unable to find a match: docker-ce docker-ce-cli`, aborting the install
# at step 3 of 15. AlmaLinux fares worse still: it is not in the script's distro
# list at all, so it never reaches a repo.
#
# The packages under `linux/centos/$releasever` are plain el$releasever builds
# that install cleanly on every RHEL rebuild (that path carried 198 docker-ce
# RPMs when this was written, against rocky's 0), so point the clones there
# ourselves rather than at a directory upstream does not fill.
docker_repo_rhel_clone() {
    cat > /etc/yum.repos.d/docker-ce.repo << 'REPOEOF'
[docker-ce-stable]
name=Docker CE Stable - $basearch
baseurl=https://download.docker.com/linux/centos/$releasever/$basearch/stable
enabled=1
gpgcheck=1
gpgkey=https://download.docker.com/linux/centos/gpg
REPOEOF
}

install_docker() {
    header "Docker"

    if command -v docker &> /dev/null; then
        log "Docker already installed: $(docker --version | head -1)"
    else
        local DOCKER_OS_ID=""
        [ -f /etc/os-release ] && DOCKER_OS_ID=$(. /etc/os-release && echo "${ID:-}")

        case "$DOCKER_OS_ID" in
            rocky|almalinux|centos|rhel|ol)
                docker_repo_rhel_clone
                # --allowerasing is precautionary, not load-bearing: a stock
                # rockylinux:9 has no conflicting runtime and installs fine
                # without it (checked). It is here because RHEL-family CLOUD
                # images commonly preinstall podman/runc, which containerd.io
                # obsoletes — and dnf aborts the whole transaction on a conflict
                # rather than substituting.
                run "Installing Docker (docker-ce, el-clone repo)" \
                    bash -c 'dnf install -y --allowerasing docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin'
                ;;
            *)
                run "Installing Docker (get.docker.com — takes a minute)" \
                    bash -c 'curl -fsSL https://get.docker.com | sh'
                ;;
        esac
        systemctl enable --now docker > /dev/null 2>&1
        if ! command -v docker &> /dev/null; then
            error "Docker installation failed — see $INSTALL_LOG"
            exit 1
        fi
        log "Docker installed: $(docker --version | head -1)"
    fi
    local dv
    dv=$(docker --version 2>/dev/null | sed -n 's/^Docker version \([0-9.]*\).*/\1/p')
    svc_ok "Docker${dv:+ $dv}"
}

install_nginx() {
    header "Nginx"

    if command -v nginx &> /dev/null; then
        log "Nginx already installed"
    else
        run "Installing Nginx" pkg_install nginx
        systemctl enable --now nginx > /dev/null 2>&1
        if ! command -v nginx &> /dev/null; then
            error "Nginx installation failed — see $INSTALL_LOG"
            exit 1
        fi
        log "Nginx installed"
    fi
    local nv
    nv=$(nginx -v 2>&1 | sed -n 's|^nginx version: nginx/\([0-9.]*\).*|\1|p')
    svc_ok "Nginx${nv:+ $nv}"
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
        # `set -o pipefail` inside the wrapper is load-bearing: without it a
        # failed nodesource curl pipes nothing into `bash -` (which exits 0),
        # the `&&` then installs the distro's much-older Node, and the build
        # later dies cryptically. With pipefail the curl failure aborts here.
        if [ "$PKG_MGR" = "apt" ]; then
            run "Installing Node.js 20 LTS (nodesource)" \
                bash -c 'set -o pipefail; curl -fsSL https://deb.nodesource.com/setup_20.x | bash - && apt-get install -y nodejs'
        else
            run "Installing Node.js 20 LTS (nodesource)" \
                bash -c "set -o pipefail; curl -fsSL https://rpm.nodesource.com/setup_20.x | bash - && $PKG_MGR install -y nodejs"
        fi
        if ! command -v node &> /dev/null; then
            error "Node.js installation failed — see $INSTALL_LOG"
            exit 1
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

    # Create all directories required by the agent unit's ReadWritePaths, DERIVED
    # FROM THE UNIT rather than hand-mirrored here.
    #
    # Two different failure modes make this load-bearing, and the `-` prefix is
    # what separates them:
    #   * an UNPREFIXED entry that does not exist fails the namespace mount, and
    #     the agent does not start at all — loud, and therefore self-correcting.
    #   * a `-`-prefixed entry means "bind IF IT EXISTS". When it is absent
    #     systemd skips it silently, the unit starts and reports success, and
    #     every write under that path fails with `Read-only file system` for the
    #     lifetime of the namespace. Creating the directory afterwards does not
    #     help: the mount namespace is fixed at start, so only a restart recovers.
    #
    # This list was hand-copied into setup.sh AND update.sh and drifted (s269:
    # /var/spool/cron reached the unit and setup.sh but never update.sh, so an
    # upgraded box without a cron spool got a silently unwritable one — the exact
    # defect s268 had just fixed, re-entering through the upgrade door).
    # Deriving it from the single-source unit removes the drift, not one instance.
    _rwp="$(agent_rwp_dirs)"
    [ -n "$_rwp" ] && mkdir -p $_rwp
    # Paths the agent reaches through an escape hatch or that are not RWP entries.
    mkdir -p /run/opendkim /var/lib/nginx /var/lib/dpkg /var/cache/apt /var/lib/apt
    mkdir -p /var/lib/dockpanel/git /var/lib/dockpanel/recordings /var/cache/nginx/fastcgi
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

        # POSTGRES_PASSWORD is passed via the environment (bare `-e` inherits),
        # so the secret never appears in the logged command line.
        POSTGRES_PASSWORD="$DB_PASSWORD" \
        run "Creating PostgreSQL 16 container (pulls the image on first install)" \
            docker run -d \
            --name "$DB_CONTAINER" \
            --restart unless-stopped \
            -e POSTGRES_DB=dockpanel \
            -e POSTGRES_USER=dockpanel \
            -e POSTGRES_PASSWORD \
            -p "127.0.0.1:${DB_PORT}:5432" \
            -v dockpanel-pgdata:/var/lib/postgresql/data \
            postgres:16-alpine
        log "PostgreSQL container created (port $DB_PORT)"
    fi

    # Wait for PostgreSQL to be ready
    if run "Waiting for PostgreSQL to accept connections" bash -c \
        "for i in \$(seq 1 15); do docker exec $DB_CONTAINER pg_isready -U dockpanel && exit 0; sleep 2; done; exit 1"; then
        log "PostgreSQL ready"
    else
        error "PostgreSQL did not become ready within 30s"
        exit 1
    fi
}

# ── Download Pre-built Binaries ──────────────────────────────────────────
INSTALLED_VERSION=""

# fetch_asset <asset-name> <dest> — download to a QUARANTINE temp file next to
# dest, verify its sha256 against the release checksums.txt, and only move it
# onto the live path AFTER it verifies — so the live executable path never
# holds unverified bytes (a mismatch aborts without touching the old binary).
# A checksum MISMATCH is fatal; a missing manifest only warns (availability
# must not brick an install, integrity must). Sets FETCH_VERIFIED=1/0 so the
# caller can report honestly whether a checksum was actually compared.
FETCH_VERIFIED=0
fetch_asset() {
    local asset="$1" dest="$2"
    local tmp="${dest}.dpdl.$$"
    FETCH_VERIFIED=0
    if ! run "Downloading ${asset}" \
        curl --retry 3 --retry-delay 2 -sfL "${BASE_URL}/${asset}" -o "$tmp"; then
        rm -f "$tmp"
        error "Download failed: ${asset} — see $INSTALL_LOG"
        exit 1
    fi
    if [ -s "$CHECKSUMS_FILE" ]; then
        local want got
        want=$(awk -v n="$asset" '$2 == n {print $1; exit}' "$CHECKSUMS_FILE")
        if [ -n "$want" ]; then
            got=$(sha256sum "$tmp" | awk '{print $1}')
            if [ "$want" != "$got" ]; then
                rm -f "$tmp"
                error "Checksum MISMATCH for ${asset} — refusing to install a corrupt binary"
                plog "expected $want got $got"
                exit 1
            fi
            FETCH_VERIFIED=1
        fi
    fi
    # Atomic swap into the live path (never `cp` onto a running binary).
    mv -f "$tmp" "$dest"
}

# Truthful post-download line — "verified" ONLY when a checksum was compared.
fetched_msg() {
    local what="$1" path="$2"
    if [ "$FETCH_VERIFIED" = "1" ]; then
        log "${what} downloaded + verified ($(du -h "$path" | cut -f1))"
    else
        log "${what} downloaded ($(du -h "$path" | cut -f1)) — checksum not verified"
    fi
}

download_binaries() {
    header "Downloading Pre-built Binaries"

    # Which release to install.
    #
    # install.sh already reads DOCKPANEL_VERSION and clones that ref — but this,
    # its only consumer, used to always fetch releases/latest. So
    # `DOCKPANEL_VERSION=v2.31.2 install.sh` produced a v2.31.2 tree running
    # v2.34.2 binaries and printed "Version: v2.34.2" at the end (s261). Unit
    # files, nginx templates and install-agent.sh are deployed from the tree,
    # so that skew is the same class that stranded the v2.8.13 -> v2.8.14
    # upgrade path. Honour the pin here; `main` (the default) means "latest".
    local RELEASE_TAG
    if [ -n "${DOCKPANEL_VERSION:-}" ] && [ "${DOCKPANEL_VERSION}" != "main" ]; then
        RELEASE_TAG="$DOCKPANEL_VERSION"
        case "$RELEASE_TAG" in
            [0-9]*) RELEASE_TAG="v$RELEASE_TAG" ;;
        esac
        log "Pinned release: $RELEASE_TAG (DOCKPANEL_VERSION)"
    else
        RELEASE_TAG=$(curl --retry 3 --retry-delay 2 -sf "https://api.github.com/repos/${GITHUB_REPO}/releases/latest" | grep '"tag_name"' | head -1 | cut -d'"' -f4)
        log "Latest release: $RELEASE_TAG"
    fi

    if [ -z "$RELEASE_TAG" ]; then
        error "Could not determine latest release. Check https://github.com/${GITHUB_REPO}/releases"
        exit 1
    fi
    INSTALLED_VERSION="$RELEASE_TAG"
    BASE_URL="https://github.com/${GITHUB_REPO}/releases/download/${RELEASE_TAG}"

    # Checksum manifest first — every later download is verified against it.
    # mktemp (root-owned, O_EXCL, unpredictable) so a local user can't pre-seed
    # a manifest that would make a tampered binary "verify".
    CHECKSUMS_FILE=$(umask 077; mktemp /tmp/dockpanel-checksums.XXXXXX 2>/dev/null || echo "")
    if [ -z "$CHECKSUMS_FILE" ] || ! curl --retry 3 --retry-delay 2 -sfL "${BASE_URL}/checksums.txt" -o "$CHECKSUMS_FILE" 2>/dev/null; then
        warn "checksums.txt not available — skipping integrity verification"
        : > "$CHECKSUMS_FILE" 2>/dev/null || CHECKSUMS_FILE=""
    fi

    # Stop running services before overwriting their binaries. Re-running the
    # installer with services active causes `curl -o` to fail with "Text file
    # busy" (exit 23) because Linux refuses to overwrite a running executable.
    # systemctl stop is a no-op if the unit is inactive or missing.
    if command -v systemctl >/dev/null 2>&1; then
        systemctl stop dockpanel-api dockpanel-agent 2>/dev/null || true
    fi

    fetch_asset "dockpanel-agent-linux-${DL_ARCH}" "$AGENT_BIN"
    chmod +x "$AGENT_BIN"
    fetched_msg "Agent" "$AGENT_BIN"

    fetch_asset "dockpanel-api-linux-${DL_ARCH}" "$API_BIN"
    chmod +x "$API_BIN"
    fetched_msg "API" "$API_BIN"

    fetch_asset "dockpanel-cli-linux-${DL_ARCH}" "$CLI_BIN"
    chmod +x "$CLI_BIN"
    fetched_msg "CLI" "$CLI_BIN"

    local FE_TARBALL
    FE_TARBALL=$(umask 077; mktemp /tmp/dockpanel-frontend.XXXXXX.tar.gz 2>/dev/null || echo /tmp/dockpanel-frontend.tar.gz)
    fetch_asset "dockpanel-frontend.tar.gz" "$FE_TARBALL"

    # Extract frontend — need a target directory
    local FE_DIR="/opt/dockpanel/frontend"
    mkdir -p "$FE_DIR"
    tar xzf "$FE_TARBALL" -C "$FE_DIR"
    rm -f "$FE_TARBALL"
    [ -n "$CHECKSUMS_FILE" ] && rm -f "$CHECKSUMS_FILE"

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

    INSTALLED_VERSION=$(git -C "$REPO_DIR" describe --tags --always 2>/dev/null || echo "")
    INSTALLED_VERSION="${INSTALLED_VERSION:+${INSTALLED_VERSION} (built from source)}"
}

# ── Build Frontend ───────────────────────────────────────────────────────
build_frontend() {
    header "Building Frontend"

    if [ ! -d "$FRONTEND_DIR" ]; then
        warn "Frontend source not found at $FRONTEND_DIR — skipping"
        return
    fi

    log "Installing npm dependencies..."
    # `npm ci` installs EXACTLY the committed package-lock.json — the tree the
    # audit gates actually scanned. `npm install` re-resolves and can pull
    # versions no scanner here has ever seen, which silently un-ships a
    # dependency patch. Keep the fallback (a box whose lock has drifted should
    # still install) but never let it happen quietly, and never discard the
    # reason: sending both streams to /dev/null is how the RHEL agent install
    # failed for a year printing nothing at all.
    if ! (cd "$FRONTEND_DIR" && npm ci --silent); then
        warn "npm ci failed (reason above) — falling back to 'npm install'"
        warn "Dependencies will be RE-RESOLVED; the tree may differ from the audited lockfile"
        if ! (cd "$FRONTEND_DIR" && npm install --silent); then
            error "npm install also failed — the frontend build below will likely fail too"
        fi
    fi

    log "Building frontend..."
    (cd "$FRONTEND_DIR" && npx vite build 2>&1 | tail -3)
    log "Frontend built at $FRONTEND_DIR/dist/"
}

# ── Systemd Services ─────────────────────────────────────────────────────
create_services() {
    header "Systemd Services"

    # Agent service — deploy from repo (single source of truth: panel/agent/dockpanel-agent.service)
    cp "$AGENT_SRC/dockpanel-agent.service" /etc/systemd/system/dockpanel-agent.service
    chmod 644 /etc/systemd/system/dockpanel-agent.service

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

    if wait_for_unit dockpanel-agent; then
        log "Agent service running"
    else
        error "Agent failed to start"
        journalctl -u dockpanel-agent --no-pager -n 10
        exit 1
    fi

    # Start API
    systemctl enable dockpanel-api > /dev/null 2>&1
    systemctl restart dockpanel-api

    if wait_for_unit dockpanel-api; then
        log "API service running"
    else
        error "API failed to start"
        journalctl -u dockpanel-api --no-pager -n 10
        exit 1
    fi
}

# ── Panel TLS ────────────────────────────────────────────────────────────
# The panel's very first screen asks the operator to CREATE an admin password.
# Without a domain there is no Let's Encrypt certificate, and the panel used to
# serve that screen over plain HTTP — putting the credential on the wire in
# cleartext on the exact path the README advertises. A self-signed certificate
# is not a trusted certificate, but a browser warning the operator can reason
# about beats a password nobody can see leaving the machine.
detect_server_ip() {
    curl -sf --max-time 5 https://api.ipify.org 2>/dev/null || \
    hostname -I 2>/dev/null | awk '{print $1}' || \
    echo ""
}

generate_self_signed_panel_cert() {
    local ip="$1"
    local dir="${CONFIG_DIR}/ssl"

    mkdir -p "$dir"
    chmod 700 "$dir"

    # Idempotent — never clobber a certificate the operator may have replaced
    # with their own, and never regenerate on a re-run (that would invalidate
    # the fingerprint anyone has already accepted in their browser).
    if [ -s "$dir/panel.crt" ] && [ -s "$dir/panel.key" ]; then
        return 0
    fi

    # Name every address the panel can actually be reached on, not just the
    # public one: on a NAT'd or LAN box the operator types the local address,
    # and a certificate that doesn't name it adds a second, avoidable warning
    # on top of the untrusted-issuer one.
    local san="DNS:localhost,IP:127.0.0.1"
    [ -n "$ip" ] && san="${san},IP:${ip}"
    local local_ip
    for local_ip in $(hostname -I 2>/dev/null || true); do
        # IPv4/IPv6 literals only, and never repeat one already listed.
        case ",${san}," in
            *",IP:${local_ip},"*) continue ;;
        esac
        san="${san},IP:${local_ip}"
    done

    # -addext needs OpenSSL 1.1.1+. Every distro we support ships 3.x, but fall
    # back to a SAN-less certificate rather than dropping to plain HTTP.
    if openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
            -subj "/CN=${ip:-dockpanel}" -addext "subjectAltName=${san}" \
            -keyout "$dir/panel.key" -out "$dir/panel.crt" > /dev/null 2>&1 ||
       openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
            -subj "/CN=${ip:-dockpanel}" \
            -keyout "$dir/panel.key" -out "$dir/panel.crt" > /dev/null 2>&1; then
        chmod 600 "$dir/panel.key"
        chmod 644 "$dir/panel.crt"
        return 0
    fi

    rm -f "$dir/panel.key" "$dir/panel.crt"
    return 1
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

    # Drop install-agent.sh into FE_ROOT so the panel's SPA-fallback nginx config
    # serves it via `try_files $uri` (instead of returning the SPA index.html).
    # Backend at panel/backend/src/routes/servers.rs prints `curl … {panel_url}/install-agent.sh`
    # in the multi-server install command — this is what makes that URL resolve. (#56, v2.8.14)
    if [ -f "$REPO_DIR/scripts/install-agent.sh" ] && [ -d "$FE_ROOT" ]; then
        cp "$REPO_DIR/scripts/install-agent.sh" "$FE_ROOT/install-agent.sh"
        chmod 644 "$FE_ROOT/install-agent.sh"
    fi

    local SERVER_NAME="_"
    local LISTEN_DIRECTIVE="listen ${PANEL_PORT};"
    local PANEL_TLS_BLOCK=""
    if [ -n "$PANEL_DOMAIN" ]; then
        SERVER_NAME="$PANEL_DOMAIN"
        # Use interface IP to match agent-generated site configs (prevents nginx routing conflicts).
        # Always pair the IPv4 listen with a plain `[::]:80` IPv6 listen so the panel is
        # reachable via IPv6 too. Without it, agent-managed site vhosts (which bind
        # `[::]:443 ssl`) become the de-facto default for IPv6 traffic and serve their own
        # WP/canonical-redirect responses for the panel domain.
        local BIND_IP
        BIND_IP=$(ip route get 8.8.8.8 2>/dev/null | grep -oP 'src \K\S+' || true)
        if [ -n "$BIND_IP" ]; then
            LISTEN_DIRECTIVE="listen ${BIND_IP}:80;
    listen [::]:80;"
        else
            LISTEN_DIRECTIVE="listen 80;
    listen [::]:80;"
        fi
    else
        # No domain → certbot can't issue. Terminate TLS anyway with a
        # self-signed certificate so first-boot credentials are encrypted.
        local IP_FOR_CERT
        IP_FOR_CERT=$(detect_server_ip)
        if generate_self_signed_panel_cert "$IP_FOR_CERT"; then
            PANEL_SELF_SIGNED=1
            # Same binding as before, now with TLS. Deliberately NOT adding an
            # [::] listen here: this is the default install path, and a box with
            # IPv6 disabled would fail to bind and come up with no panel at all.
            LISTEN_DIRECTIVE="listen ${PANEL_PORT} ssl;"
            PANEL_TLS_BLOCK="
    ssl_certificate ${CONFIG_DIR}/ssl/panel.crt;
    ssl_certificate_key ${CONFIG_DIR}/ssl/panel.key;
    ssl_protocols TLSv1.2 TLSv1.3;
    ssl_session_cache shared:PanelSSL:1m;
    ssl_session_timeout 10m;
"
        else
            warn "Could not generate a self-signed certificate — the panel will serve plain HTTP on ${PANEL_PORT}"
            info "The admin password you create will NOT be encrypted in transit; prefer PANEL_DOMAIN=..."
        fi
    fi

    # Drop-in dir for path-mounted tool reverse-proxies (webmail in v2.8.22+, etc.)
    # Agent writes fragment files here on tool install/remove; setup.sh + update.sh
    # only ensure the include directive is present in the panel vhost.
    mkdir -p /etc/nginx/conf.d/dockpanel-panel.locations

    cat > "$NGINX_CONF" << NGINXEOF
server {
    ${LISTEN_DIRECTIVE}
    server_name ${SERVER_NAME};
${PANEL_TLS_BLOCK}
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

    # Drop-in location blocks for path-mounted tools (webmail, etc.)
    include /etc/nginx/conf.d/dockpanel-panel.locations/*.conf;

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

    if run "Waiting for the API to answer on port 3080" bash -c \
        'for i in $(seq 1 15); do curl -sf http://127.0.0.1:3080/api/health && exit 0; sleep 2; done; exit 1'; then
        log "API healthy"
    else
        warn "API not responding on port 3080 yet — check: journalctl -u dockpanel-api -n 20"
    fi
}

# ── Recommended Services ─────────────────────────────────────────────────

# install_php_set <prefix> — install <prefix>-fpm + <prefix>-cli, then every
# extension metapackage this apt source actually has a candidate for.
# A fixed list would fail the whole transaction over one obsolete name:
# PHP 8.5 (Ubuntu 26.04) absorbed OPcache, so `php-opcache` no longer has an
# installation candidate there — that single dead package is what broke the
# old all-or-nothing install. Skipped names are reported, not fatal.
install_php_set() {
    local prefix="$1"
    local exts="" skipped="" e
    if ! run "Installing ${prefix}-fpm + ${prefix}-cli" apt-get install -y "${prefix}-fpm" "${prefix}-cli"; then
        return 1
    fi
    for e in mysql pgsql curl gd mbstring xml zip bcmath intl readline opcache; do
        if apt_has_candidate "${prefix}-${e}"; then
            exts="$exts ${prefix}-${e}"
        else
            skipped="$skipped $e"
        fi
    done
    if [ -n "$exts" ]; then
        # shellcheck disable=SC2086
        if ! run "Installing PHP extensions ($(echo $exts | wc -w) packages)" apt-get install -y $exts; then
            warn "Some PHP extensions failed to install — see $INSTALL_LOG"
        fi
    fi
    if [ -n "$skipped" ]; then
        info "Skipped:${skipped} (built into this PHP version or not packaged)"
    fi
    return 0
}

# cleanup_php_repo — remove a 3rd-party PHP source that turned out not to
# serve this release. Leaving it behind breaks EVERY later `apt-get update`
# on the user's box with a 404 (this exact wreckage was found on a fresh
# Ubuntu 26.04 install after the old installer's failed PPA attempt).
cleanup_php_repo() {
    rm -f /etc/apt/sources.list.d/sury-php.list \
          /etc/apt/sources.list.d/ondrej-ubuntu-php-*.sources \
          /etc/apt/sources.list.d/ondrej-ubuntu-php-*.list 2>/dev/null || true
    run "Refreshing package index" apt-get update -y || true
}

install_php() {
    if command -v php &> /dev/null; then
        log "PHP already installed: $(php -v | head -1 | awk '{print $2}')"
        svc_ok "PHP $(php -r 'echo PHP_MAJOR_VERSION.".".PHP_MINOR_VERSION;' 2>/dev/null) (FPM)"
        return
    fi

    if [ "$PKG_MGR" = "apt" ]; then
        # Re-source /etc/os-release — detect_os' copy is local to that function.
        local OS_ID="" OS_CODENAME=""
        if [ -f /etc/os-release ]; then
            OS_ID=$(. /etc/os-release && echo "${ID:-}")
            OS_CODENAME=$(. /etc/os-release && echo "${VERSION_CODENAME:-}")
        fi

        # Step 1: the distro's own PHP. Covers Ubuntu 24.04 (8.3), Ubuntu
        # 26.04 (8.5), Debian 12 (8.2), Debian 13 (8.4) — modern distros ship
        # a usable PHP and need no 3rd-party repo at all.
        local PHP_VER=""
        if apt_has_candidate php-fpm && install_php_set php; then
            PHP_VER=$(php -r 'echo PHP_MAJOR_VERSION.".".PHP_MINOR_VERSION;' 2>/dev/null || true)
        fi

        # Step 2: releases without a usable distro PHP fall back to a
        # 3rd-party repo (Debian → deb.sury.org, Ubuntu → ppa:ondrej/php) —
        # but ONLY after confirming that repo actually publishes for this
        # release (brand-new releases lag), and with cleanup on failure so a
        # dead source can never poison the box's apt state.
        if [ -z "$PHP_VER" ]; then
            local FALLBACK_VER="8.3" repo_added=0
            if [ "$OS_ID" = "debian" ] && [ -n "$OS_CODENAME" ]; then
                if curl -sfI --max-time 15 "https://packages.sury.org/php/dists/${OS_CODENAME}/Release" > /dev/null 2>&1; then
                    if run "Adding deb.sury.org PHP repo (${OS_CODENAME})" bash -c "
                        apt-get install -y apt-transport-https lsb-release ca-certificates curl gnupg &&
                        curl -sSLo /usr/share/keyrings/deb.sury.org-php.gpg https://packages.sury.org/php/apt.gpg &&
                        echo 'deb [signed-by=/usr/share/keyrings/deb.sury.org-php.gpg] https://packages.sury.org/php/ ${OS_CODENAME} main' > /etc/apt/sources.list.d/sury-php.list &&
                        apt-get update -y"; then
                        repo_added=1
                    fi
                else
                    warn "deb.sury.org publishes no PHP packages for Debian ${OS_CODENAME} yet"
                fi
            elif [ "$OS_ID" = "ubuntu" ] && [ -n "$OS_CODENAME" ]; then
                if curl -sfI --max-time 15 "https://ppa.launchpadcontent.net/ondrej/php/ubuntu/dists/${OS_CODENAME}/Release" > /dev/null 2>&1; then
                    if run "Adding ppa:ondrej/php (${OS_CODENAME})" bash -c "
                        apt-get install -y software-properties-common &&
                        add-apt-repository -y ppa:ondrej/php &&
                        apt-get update -y"; then
                        repo_added=1
                    fi
                else
                    warn "ppa:ondrej/php publishes no packages for Ubuntu ${OS_CODENAME} yet"
                fi
            fi

            if [ "$repo_added" = "1" ] && apt_has_candidate "php${FALLBACK_VER}-fpm"; then
                if install_php_set "php${FALLBACK_VER}"; then
                    PHP_VER="$FALLBACK_VER"
                fi
            fi
            if [ -z "$PHP_VER" ] && [ "$repo_added" = "1" ]; then
                cleanup_php_repo
            fi
        fi

        if [ -n "$PHP_VER" ]; then
            systemctl enable --now "php${PHP_VER}-fpm" > /dev/null 2>&1
            log "PHP ${PHP_VER} installed with FPM"
            svc_ok "PHP ${PHP_VER} (FPM)"
        else
            warn "PHP could not be installed — WordPress/PHP sites need it; retry later from Settings → Services"
            svc_fail "PHP-FPM"
        fi
    else
        # RHEL/Rocky/Fedora.
        #
        # SELECT THE MODULE STREAM FIRST. `dnf install php-fpm` with no stream
        # enabled resolves to the NON-MODULAR base package — PHP 8.0.30 on
        # Rocky 9, older than every stream the box offers (8.1/8.2/8.3) and
        # end-of-life since November 2023 — and the install summary then
        # cheerfully prints "PHP 8.0 (FPM)". Measured on a real box at s266,
        # where this installer did exactly that.
        #
        # It also made the panel's own PHP installer unreachable: it checks
        # whether PHP is already present, finds 8.0, and reports "already
        # installed", so nothing ever offers the operator a supported version.
        local php_stream
        php_stream=$(dnf -q module list php 2>/dev/null \
                     | awk '$1=="php" && $2 ~ /^[0-9]/ {print $2}' | sort -Vr | head -1)
        if [ -n "$php_stream" ]; then
            if dnf -y module enable "php:${php_stream}" > /dev/null 2>&1; then
                info "Selected PHP ${php_stream} (module stream)"
            else
                warn "Could not enable the php:${php_stream} module stream — falling back to the distro default, which may be end-of-life"
            fi
        fi

        # The extension names are passed as-is: several of them (php-curl,
        # php-zip, php-sqlite3) are VIRTUAL provides satisfied by php-common
        # rather than packages of their own, and dnf resolves those, so this
        # list is not the all-or-nothing hazard the apt side has to guard
        # against. Verified against a real dnf at s266.
        if run "Installing PHP (distro packages)" pkg_install php-fpm php-cli php-common php-mysqlnd php-pgsql php-xml php-mbstring php-curl php-zip php-gd; then
            systemctl enable --now php-fpm > /dev/null 2>&1
            log "PHP installed with FPM"
            svc_ok "PHP $(php -r 'echo PHP_MAJOR_VERSION.".".PHP_MINOR_VERSION;' 2>/dev/null) (FPM)"
        else
            warn "PHP could not be installed — retry later from Settings → Services"
            svc_fail "PHP-FPM"
        fi
    fi
}

install_recommended_services() {
    header "Recommended Services"

    # PHP-FPM (needed for WordPress, PHP sites)
    install_php

    # Certbot (needed for SSL certificates)
    if ! command -v certbot &> /dev/null; then
        if run "Installing Certbot" pkg_install certbot python3-certbot-nginx; then
            systemctl enable --now certbot.timer > /dev/null 2>&1
            log "Certbot installed with auto-renewal"
            svc_ok "Certbot"
        else
            warn "Certbot failed to install — SSL provisioning will not work until it is"
            svc_fail "Certbot"
        fi
    else
        log "Certbot already installed"
        svc_ok "Certbot"
    fi

    # Firewall. Install one only if the box has none — never a second one.
    #
    # s265: this used to install UFW unconditionally. On the RHEL family that
    # put UFW's iptables rules alongside the firewalld nftables rules the
    # distro already had running, and then opened 80/443 in UFW *only*. The
    # packets kept dying at firewalld, so Let's Encrypt could not reach
    # /.well-known/acme-challenge, the panel got no certificate, and it was
    # unreachable from any browser — while the installer printed
    # "installed successfully" and an https:// URL. Two firewalls is not a
    # hardening measure, it is an outage nobody can see.
    case "$FW_MGR" in
        firewalld)
            log "Firewall: firewalld is already active — using it (not installing UFW)"
            svc_ok "firewalld"
            ;;
        ufw)
            log "UFW already installed"
            svc_ok "UFW"
            ;;
        none)
            if run "Installing UFW firewall" pkg_install ufw; then
                ufw default deny incoming > /dev/null 2>&1
                ufw default allow outgoing > /dev/null 2>&1
                ufw allow 22/tcp > /dev/null 2>&1
                ufw --force enable > /dev/null 2>&1
                FW_MGR="ufw"
                log "UFW installed and enabled"
                svc_ok "UFW"
            else
                warn "UFW failed to install — no firewall is active"
                svc_fail "UFW"
            fi
            ;;
    esac

    # Ensure panel ports are open in whichever firewall is actually enforcing.
    if fw_allow 80/tcp && fw_allow 443/tcp; then
        local opened="80, 443"
        if [ -n "$PANEL_PORT" ] && [ "$PANEL_PORT" != "80" ] && [ "$PANEL_PORT" != "443" ]; then
            fw_allow "${PANEL_PORT}/tcp" && opened="$opened, $PANEL_PORT"
        fi
        fw_reload
        log "Firewall ($FW_MGR): ports $opened allowed"
    elif [ "$FW_MGR" = "none" ]; then
        log "Firewall: none active — nothing to open"
    else
        warn "Firewall ($FW_MGR): could not open 80/443 — the panel may be unreachable"
    fi

    # Fail2Ban (intrusion prevention)
    if ! command -v fail2ban-client &> /dev/null; then
        if run "Installing Fail2Ban" pkg_install fail2ban; then
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
            svc_ok "Fail2Ban"
        else
            warn "Fail2Ban failed to install — retry later from Settings → Services"
            svc_fail "Fail2Ban"
        fi
    else
        log "Fail2Ban already installed"
        svc_ok "Fail2Ban"
    fi

    # Honest section verdict — never claim "all ready" over a failure.
    if [ ${#SVC_FAIL[@]} -eq 0 ]; then
        log "All recommended services ready"
    else
        warn "$(join_comma "${SVC_FAIL[@]}") did not install — the panel still works; retry from Settings → Services"
    fi
}

# Bind the panel's TLS listener the same way its neighbours are bound.
#
# certbot --nginx writes a WILDCARD `listen 443 ssl;` onto the vhost it
# manages, but agent-generated site vhosts bind `<ip>:443 ssl`
# (agent/src/templates/nginx/https.conf). nginx treats those as two different
# listen sockets and the explicit-IP one wins every connection to that
# address, so the panel's server_name is never consulted: the FIRST site to
# get a certificate becomes the de-facto server for the panel's own domain and
# the panel goes dark from outside. configure_nginx already avoids exactly
# this on :80 by pinning it to the interface IP — certbot then reintroduces it
# on :443, which is why the guard there was not enough.
normalize_panel_listen() {
    local conf="$NGINX_CONF"
    [ -f "$conf" ] || return 0

    local bind_ip
    bind_ip=$(ip route get 8.8.8.8 2>/dev/null | grep -oP 'src \K\S+' || true)
    # No default route: every vhost falls back to wildcard, so they already agree.
    [ -n "$bind_ip" ] || return 0

    sed -i -E "s|^([[:space:]]*)listen 443 ssl;|\1listen ${bind_ip}:443 ssl;|" "$conf"
    # Site vhosts declare a plain `[::]:443 ssl`; nginx rejects mixing
    # ipv6only=on with it on the same socket.
    sed -i -E 's|^([[:space:]]*)listen \[::\]:443 ssl ipv6only=on;|\1listen [::]:443 ssl;|' "$conf"

    if ! nginx -t > /dev/null 2>&1; then
        warn "nginx config test failed after adjusting the panel listen directives"
        return 0
    fi

    # A RELOAD cannot move an already-bound listener from 0.0.0.0:443 to
    # <ip>:443 — nginx inherits the old socket and the change silently
    # no-ops, leaving a config on disk that disagrees with what is running.
    # Only a restart rebinds.
    systemctl restart nginx > /dev/null 2>&1 || true

    # Trust the socket, not the restart's exit code.
    if command -v ss > /dev/null 2>&1 && ! ss -ltn "( sport = :443 )" 2>/dev/null | grep -q "${bind_ip}:443"; then
        warn "panel is not listening on ${bind_ip}:443 — a site vhost may shadow it"
    fi
}

provision_panel_ssl() {
    if [ -z "$PANEL_DOMAIN" ]; then
        if [ "$PANEL_SELF_SIGNED" = "1" ]; then
            log "No domain set — serving HTTPS on IP:${PANEL_PORT} with a self-signed certificate"
            info "Let's Encrypt needs a domain; re-run with PANEL_DOMAIN=... for a trusted certificate"
        else
            log "No domain set — panel on IP:${PANEL_PORT} (plain HTTP)"
        fi
        return
    fi

    header "Panel SSL Certificate"

    if ! command -v certbot &> /dev/null; then
        log "Certbot not found — skipping SSL"
        return
    fi

    if run "Provisioning Let's Encrypt certificate for ${PANEL_DOMAIN}" \
        certbot --nginx -d "$PANEL_DOMAIN" --non-interactive --agree-tos --register-unsafely-without-email --redirect; then
        log "SSL certificate provisioned for $PANEL_DOMAIN"
        PANEL_SSL_OK=1
        normalize_panel_listen
    else
        # Order these by what actually causes it. s265: a firewall the
        # installer did not configure blocked :80, so the ACME challenge could
        # never be fetched — and the old hint blamed Cloudflare, which was not
        # even in the path. Reachability first, proxies last.
        warn "SSL provisioning failed — the panel is served over plain HTTP for now"
        info "Let's Encrypt must reach http://${PANEL_DOMAIN}/.well-known/acme-challenge/ from the internet. Check, in order:"
        info "  1. ${PANEL_DOMAIN} resolves to this server's public IP"
        info "  2. ports 80 and 443 are open — in the OS firewall (${FW_MGR}) AND in any provider firewall"
        info "  3. if the domain is proxied by Cloudflare, set SSL mode to 'Full'"
        info "Then retry: certbot --nginx -d $PANEL_DOMAIN"
    fi
}

print_summary() {
    INSTALL_DONE=1

    local SERVER_IP
    SERVER_IP=$(detect_server_ip)
    SERVER_IP="${SERVER_IP:-YOUR_SERVER_IP}"

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
    if [ -n "$INSTALLED_VERSION" ]; then
        echo -e "  ${BOLD}Version:${NC}        ${INSTALLED_VERSION}"
    fi
    if [ -n "$PANEL_DOMAIN" ] && [ "$PANEL_SSL_OK" = "1" ]; then
        echo -e "  ${BOLD}Panel URL:${NC}      https://${PANEL_DOMAIN}"
    elif [ -n "$PANEL_DOMAIN" ]; then
        # No certificate was issued. Printing https:// here sends the operator
        # to a URL that cannot answer, and it reads as if the install worked.
        echo -e "  ${BOLD}Panel URL:${NC}      http://${PANEL_DOMAIN}  ${YELLOW}(no certificate — see below)${NC}"
    elif [ "$PANEL_SELF_SIGNED" = "1" ]; then
        echo -e "  ${BOLD}Panel URL:${NC}      https://${SERVER_IP}:${PANEL_PORT}"
    else
        echo -e "  ${BOLD}Panel URL:${NC}      http://${SERVER_IP}:${PANEL_PORT}"
    fi
    echo ""
    echo -e "  ${BOLD}First step:${NC}     Open the URL and create your admin account"
    if [ "$PANEL_SELF_SIGNED" = "1" ]; then
        echo ""
        echo -e "  ${DIM}Your browser will warn once about the certificate — that is expected${NC}"
        echo -e "  ${DIM}for an IP address. It is self-signed, and it is there so the admin${NC}"
        echo -e "  ${DIM}password you are about to create is encrypted on the way to the server.${NC}"
        echo -e "  ${DIM}For a trusted certificate: point a domain here, then re-run with${NC}"
        echo -e "  ${DIM}PANEL_DOMAIN=your.domain bash /opt/dockpanel/scripts/setup.sh${NC}"
    fi
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
    echo -e "    Install log:    ${INSTALL_LOG}"
    echo ""
    echo -e "  ${BOLD}Database:${NC}"
    echo -e "    Container:      ${DB_CONTAINER} (port ${DB_PORT})"
    echo -e "    Connect:        docker exec -it ${DB_CONTAINER} psql -U dockpanel -d dockpanel"
    echo ""
    # What ACTUALLY installed — never a hardcoded wishlist.
    if [ ${#SVC_OK[@]} -gt 0 ]; then
        echo -e "  ${BOLD}Installed services:${NC}"
        echo -e "    $(join_comma "${SVC_OK[@]}")"
    fi
    if [ ${#SVC_FAIL[@]} -gt 0 ]; then
        echo ""
        echo -e "  ${YELLOW}${BOLD}⚠ Not installed:${NC}"
        echo -e "    $(join_comma "${SVC_FAIL[@]}")"
        echo -e "    ${DIM}Retry from the panel: Settings → Services · details: ${INSTALL_LOG}${NC}"
    fi
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
    echo -e "  ${BOLD}Docs:${NC}     https://docs.dockpanel.dev"
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
# pipefail is load-bearing: the exit status of `pg_dump | gzip` is *gzip's*, and
# gzip compresses a truncated stream and exits 0. Without it a pg_dump that died
# halfway was written out as the day's backup, the retention sweep below then
# deleted a good older one to make room, and nothing anywhere said a word.
set -o pipefail
BACKUP_DIR="/var/backups/dockpanel/db"
mkdir -p "$BACKUP_DIR"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
OUT="$BACKUP_DIR/dockpanel_$TIMESTAMP.sql.gz"
if ! docker exec dockpanel-postgres pg_dump -U dockpanel -d dockpanel | gzip > "$OUT"; then
    echo "dockpanel db-backup: pg_dump failed, discarding $OUT" >&2
    rm -f "$OUT"
    exit 1
fi
# A zero exit is not the success condition — a whole dump is. pg_dump emits this
# marker near the end; its absence means the file is short whatever exited 0.
if ! gunzip -c "$OUT" | tail -20 | grep -q 'PostgreSQL database dump complete'; then
    echo "dockpanel db-backup: $OUT is incomplete, discarding" >&2
    rm -f "$OUT"
    exit 1
fi
# Keep last 7 days — only ever reached once today's backup is known good, so a
# bad run can never evict a good one.
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

    # Start the install log. Prefer /var/log; if it's read-only (hardened /
    # containerized host) fall back to a root-owned, unpredictable, O_EXCL temp
    # file via mktemp — never a fixed /tmp path a local user could pre-create
    # or symlink (the installer runs as root and logs config paths).
    if : > "$INSTALL_LOG" 2>/dev/null; then
        chmod 600 "$INSTALL_LOG" 2>/dev/null || true
    else
        INSTALL_LOG=$(umask 077; mktemp /tmp/dockpanel-install.XXXXXX 2>/dev/null || echo /dev/null)
    fi
    plog "DockPanel installer started: $(date 2>/dev/null || true)"

    # No apt/debconf prompt may ever block a piped installer
    export DEBIAN_FRONTEND=noninteractive

    detect_pkg_manager
    detect_firewall

    # Auto-detect: if no source available, use release binaries
    if [ "$INSTALL_FROM_RELEASE" != "1" ] && [ ! -d "$AGENT_SRC/src" ]; then
        INSTALL_FROM_RELEASE=1
    fi

    # Ask for the domain UP FRONT — all input happens before the first step,
    # so the install never stops to wait for a human once it starts moving.
    if [ -z "$PANEL_DOMAIN" ]; then
        echo ""
        echo -e "${BOLD}Enter your panel domain (e.g. panel.example.com)${NC}"
        echo -e "Leave blank to use IP:${PANEL_PORT} with a self-signed certificate instead"
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

    # Every conditional step is now known — size the progress bar so it hits
    # 100% exactly at the end (no phantom steps, no 93% → 100% jump).
    TOTAL_STEPS=14
    if [ "$INSTALL_FROM_RELEASE" != "1" ]; then
        TOTAL_STEPS=$((TOTAL_STEPS + 1))   # build binaries + frontend = 2 steps vs 1 download step
    fi
    if [ -n "$PANEL_DOMAIN" ]; then
        TOTAL_STEPS=$((TOTAL_STEPS + 1))   # panel SSL certificate step
    fi

    detect_os
    preflight_checks
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
    # RHEL: comment out the default server block in nginx.conf, which binds :80
    # and would fight the panel vhost.
    #
    # This used to be a sed range — `/server {/,/^[[:space:]]*}/` — and that range
    # ends at the FIRST line that is just a closing brace, which inside a server
    # block is the end of its first nested `location`, not the end of the server.
    # So it commented the opening third of the block and left the remainder at
    # http level, and nginx -t died with `"location" directive is not allowed
    # here in /etc/nginx/nginx.conf:52`. Every RHEL-family install failed there.
    # Counting braces is the only way to find the block's real end.
    if [ "$PKG_MGR" != "apt" ] && [ -f /etc/nginx/nginx.conf ]; then
        awk '
          # Only an UNcommented top-level server block; an already-commented one
          # starts with # and must not match, so re-running stays a no-op.
          !c && /^[[:space:]]*server[[:space:]]*\{/ { c = 1; d = 0 }
          c {
            d += gsub(/\{/, "{") - gsub(/\}/, "}")
            print "#" $0
            if (d <= 0) c = 0
            next
          }
          { print }
        ' /etc/nginx/nginx.conf > /etc/nginx/nginx.conf.dockpanel-new 2>/dev/null &&
          mv /etc/nginx/nginx.conf.dockpanel-new /etc/nginx/nginx.conf
        rm -f /etc/nginx/nginx.conf.dockpanel-new
    fi

    # These steps should continue even if one fails
    set +e
    # Before nginx is asked to proxy anything: on Enforcing SELinux it may not
    # open the socket to the API at all, and the denial is silent.
    configure_selinux
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
