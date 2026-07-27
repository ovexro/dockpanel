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

# An agent with no central URL never phones home, so the panel never records a
# `last_seen_at` for it — and the fleet rolling update only considers servers
# seen in the last 5 minutes. The box would install fine and then be invisible
# to every fleet operation, with nothing anywhere saying why. Fail loudly here
# instead. (The panel used to hand out a copy-paste command with an empty
# --panel-url whenever it was installed without a domain; that is fixed too.)
if [[ -z "$PANEL_URL" || -z "$SERVER_ID" ]]; then
    echo "Error: --panel-url and --server-id are required"
    echo "  Without them the agent cannot check in, and a server that never"
    echo "  checks in can never be selected by a fleet update."
    echo "Usage: $0 --panel-url <url> --token <token> --server-id <uuid>"
    exit 1
fi
if [[ "$PANEL_URL" == --* || "$TOKEN" == --* || "$SERVER_ID" == --* ]]; then
    echo "Error: an option value looks like another flag (--panel-url '$PANEL_URL',"
    echo "  --server-id '$SERVER_ID'). One of the values is probably missing."
    exit 1
fi

echo "======================================"
echo "  DockPanel Agent Installer (Remote)"
echo "======================================"
echo ""

# Detect architecture
ARCH=$(uname -m)
case "$ARCH" in
    x86_64)  ARCH_LABEL="amd64" ;;
    aarch64) ARCH_LABEL="arm64" ;;
    arm64)   ARCH_LABEL="arm64" ;;
    *) echo "Unsupported architecture: $ARCH"; exit 1 ;;
esac
echo "[1/7] Architecture: $ARCH_LABEL"

# Detect package manager
detect_pkg_manager() {
    if command -v apt-get &> /dev/null; then
        PKG_MGR="apt"
    elif command -v dnf &> /dev/null; then
        PKG_MGR="dnf"
    elif command -v yum &> /dev/null; then
        PKG_MGR="yum"
    else
        echo "Error: No supported package manager found (apt/dnf/yum)"
        exit 1
    fi
}

pkg_install() {
    case "$PKG_MGR" in
        apt)
            apt-get update -qq > /dev/null 2>&1
            apt-get install -y -qq "$@" > /dev/null 2>&1
            ;;
        dnf) dnf install -y -q "$@" > /dev/null 2>&1 ;;
        yum) yum install -y -q "$@" > /dev/null 2>&1 ;;
    esac
}

# Install dependencies
echo "[2/7] Installing dependencies..."
detect_pkg_manager

# Install Docker.
#
# `get.docker.com` points the RHEL clones at
# download.docker.com/linux/<id>/… — and `linux/rocky` carries no `docker-ce`
# at all, while there is no `almalinux` branch to point at. So on those
# families the script adds a repo, refreshes the cache, and ends with
# `Error: Unable to find a match: docker-ce docker-ce-cli`. s264 found this and
# fixed it in setup.sh (v2.37.0) with an el-clone repo aimed at `linux/centos`;
# the fix never reached THIS installer, so adding a remote RHEL server has been
# impossible the whole time — measured on a stock Rocky 9, s271.
#
# The panel's own installer is the source of the shape below; keep them the same.
if ! command -v docker &> /dev/null; then
    DOCKER_OS_ID=""
    [ -f /etc/os-release ] && DOCKER_OS_ID=$(. /etc/os-release && echo "${ID:-}")
    case "$DOCKER_OS_ID" in
        rocky|almalinux|centos|rhel|ol)
            cat > /etc/yum.repos.d/docker-ce.repo << 'REPOEOF'
[docker-ce-stable]
name=Docker CE Stable - $basearch
baseurl=https://download.docker.com/linux/centos/$releasever/$basearch/stable
enabled=1
gpgcheck=1
gpgkey=https://download.docker.com/linux/centos/gpg
REPOEOF
            # --allowerasing because RHEL-family cloud images commonly preinstall
            # podman/runc, which containerd.io obsoletes, and dnf aborts the whole
            # transaction on a conflict rather than substituting.
            dnf install -y -q --allowerasing docker-ce docker-ce-cli containerd.io \
                docker-buildx-plugin docker-compose-plugin
            ;;
        *)
            curl -fsSL https://get.docker.com | sh > /dev/null 2>&1
            ;;
    esac
fi
systemctl enable --now docker > /dev/null 2>&1 || true

# Say so, rather than dying at "[2/7] Installing dependencies..." with every
# stream sent to /dev/null. That is exactly how the RHEL failure above stayed
# invisible: `set -e` aborted the script mid-step and printed nothing at all.
if ! command -v docker &> /dev/null; then
    echo "Error: Docker could not be installed on this server (${DOCKER_OS_ID:-unknown})."
    echo "  The agent manages containers, so the install cannot continue without it."
    echo "  Install Docker by hand and re-run this script."
    exit 1
fi

# Install curl and openssl if missing
pkg_install curl openssl

# Create directories
echo "[3/7] Creating directories..."
mkdir -p /etc/dockpanel/ssl
mkdir -p /var/run/dockpanel
mkdir -p /var/www
mkdir -p /var/backups/dockpanel
mkdir -p /var/lib/dockpanel/git

# Ensure socket directory persists across reboots
echo "d /run/dockpanel 0755 root root -" > /etc/tmpfiles.d/dockpanel.conf

# Save agent token and server ID
echo "[4/7] Saving configuration..."
echo "$TOKEN" > /etc/dockpanel/agent.token
chmod 600 /etc/dockpanel/agent.token

# Persist agent configuration
# AGENT_TOKEN + AGENT_LISTEN_TCP = direct multi-server TCP access
# DOCKPANEL_* vars = phone-home mode (agent checks in with central panel)
cat > /etc/dockpanel/agent.env << ENVEOF
AGENT_TOKEN=$TOKEN
AGENT_LISTEN_TCP=0.0.0.0:$AGENT_PORT
DOCKPANEL_SERVER_TOKEN=$TOKEN
DOCKPANEL_SERVER_ID=$SERVER_ID
DOCKPANEL_CENTRAL_URL=$PANEL_URL
ENVEOF
chmod 600 /etc/dockpanel/agent.env

# Download agent binary (naming matches GitHub release assets)
echo "[5/7] Downloading agent binary..."
DOWNLOAD_URL="https://github.com/ovexro/dockpanel/releases/latest/download/dockpanel-agent-linux-${ARCH_LABEL}"
if ! curl -fsSL "$DOWNLOAD_URL" -o /usr/local/bin/dockpanel-agent; then
    echo "Error: Could not download the agent binary from $DOWNLOAD_URL"
    echo "  Check connectivity to github.com and that a release asset exists for arch '${ARCH_LABEL}'."
    exit 1
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

# Install the systemd unit — TAKEN FROM THE BINARY, not written here.
#
# This step used to hand-write a third copy of the unit into a heredoc, and that
# copy switched every one of systemd's protection directives off, listed no
# writable paths at all, and omitted eight more hardening directives the real
# unit sets — under a comment claiming it matched the local agent's hardening.
# It matched nothing: every server the panel manages remotely ran the agent with
# no sandbox, from s253 to s271. The directives are deliberately not spelled out
# in this comment: tests/sandbox-paths-pin-e2e.sh greps this file for them, and
# a pin that matches its own explanation cannot fail.
#
# The panel's own installers (setup.sh, update.sh) deploy
# panel/agent/dockpanel-agent.service by copying that file. This script has no
# repo tree — it is fetched from the panel and downloads a release binary — so
# it asks the binary it just downloaded, which carries the unit via include_str!
# for exactly the reason agent-self-update.sh does: it cannot then drift from
# the binary that runs it. One unit, three installers, no mirrors.
#
# The `timeout` is load-bearing, not caution. A binary that predates the flag
# does not reject it — it ignores every argument and starts the DAEMON, which
# binds the agent socket and never returns. Without the bound, this installer
# hangs for ever at "[7/7]" against an older release instead of refusing, which
# is what it did when first driven on a real box (s271).
echo "[7/7] Installing systemd service..."
if ! timeout 15 /usr/local/bin/dockpanel-agent --print-unit > /tmp/dockpanel-agent.service.$$ 2>/dev/null \
   || ! grep -q '^ExecStart=/usr/local/bin/dockpanel-agent' /tmp/dockpanel-agent.service.$$; then
    rm -f /tmp/dockpanel-agent.service.$$
    echo "Error: the downloaded agent could not emit its systemd unit."
    echo "  '/usr/local/bin/dockpanel-agent --print-unit' produced nothing usable, which"
    echo "  means the release binary predates this installer. Writing a unit here instead"
    echo "  is what shipped an unsandboxed agent to every remote server for eighteen"
    echo "  releases, so this installer refuses to guess one."
    exit 1
fi
install -m 644 /tmp/dockpanel-agent.service.$$ /etc/systemd/system/dockpanel-agent.service
rm -f /tmp/dockpanel-agent.service.$$

# Every directory the unit's ReadWritePaths names, DERIVED FROM THE UNIT.
#
# An UNPREFIXED entry that does not exist fails the namespace mount and the
# agent does not start at all — /etc/apt did this to every RPM-family box
# (s264), and /etc/nginx would do it here, because an agent-only server has no
# nginx and nothing above creates that path. A `-`-prefixed entry is worse when
# missing, not better: systemd skips it silently, the unit starts, reports
# success, and every write beneath it fails read-only until the next restart
# (s269). So both halves are created, and the `-` is stripped rather than
# filtered on.
AGENT_RWP="$(sed -n 's/^ReadWritePaths=//p' /etc/systemd/system/dockpanel-agent.service \
             | tr ' ' '\n' | sed 's/^-//' | { grep '^/' || true; })"
for d in $AGENT_RWP; do
    [ -d "$d" ] || mkdir -p "$d" 2>/dev/null || true
done

# Allow agent port through firewall
if command -v ufw &> /dev/null; then
    ufw allow ${AGENT_PORT}/tcp > /dev/null 2>&1 || true
elif command -v firewall-cmd &> /dev/null; then
    firewall-cmd --permanent --add-port=${AGENT_PORT}/tcp > /dev/null 2>&1 || true
    firewall-cmd --reload > /dev/null 2>&1 || true
fi

# Start agent. "The command ran" is not the success condition — "the unit is
# active" is. install_powerdns ended in `let _ = systemctl restart` and hid three
# separate bugs behind that silence for two releases (lesson #45), so this
# installer polls and shows the failing journal line rather than printing a
# success banner over a crash loop. `activating` is tolerated because a unit
# with Restart=always can be caught mid-cycle by a single probe.
systemctl daemon-reload
systemctl enable dockpanel-agent >/dev/null 2>&1 || true
systemctl start dockpanel-agent || true

AGENT_OK=0
for _ in $(seq 1 15); do
    STATE=$(systemctl is-active dockpanel-agent 2>/dev/null || true)
    if [[ "$STATE" == "active" ]]; then AGENT_OK=1; break; fi
    [[ "$STATE" == "failed" ]] && break
    sleep 1
done

if [[ "$AGENT_OK" != "1" ]]; then
    echo ""
    echo "Error: dockpanel-agent did not start (systemctl is-active: $(systemctl is-active dockpanel-agent 2>/dev/null || echo unknown))"
    echo "Last journal lines:"
    journalctl -u dockpanel-agent -n 15 --no-pager 2>/dev/null || true
    exit 1
fi

echo ""
echo "======================================"
echo "  DockPanel Agent installed!"
echo "======================================"
echo ""
echo "  Agent listening on: 0.0.0.0:${AGENT_PORT}"
echo "  Token: ${TOKEN:0:12}..."
echo "  Server ID: ${SERVER_ID}"
echo "  Config: /etc/dockpanel/agent.env"
echo ""
echo "  Return to your DockPanel and click"
echo "  'Test Connection' to verify."
echo ""
