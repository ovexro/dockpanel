#!/usr/bin/env bash
#
# DockPanel Quick Installer
# Usage: curl -sL https://dockpanel.dev/install.sh | bash
#
# Modes:
#   Default:              Clone repo, build from source
#   INSTALL_FROM_RELEASE=1: Clone repo, download pre-built binaries (faster, no Rust needed)
#
set -euo pipefail

VERSION="${DOCKPANEL_VERSION:-main}"
INSTALL_DIR="/opt/dockpanel"

RED='\033[0;31m'
GREEN='\033[0;32m'
BOLD='\033[1m'
NC='\033[0m'

echo ""
echo -e "${GREEN}${BOLD}DockPanel Installer${NC}"
echo -e "  Free, self-hosted server management panel"
echo ""

# Check root
if [ "$EUID" -ne 0 ]; then
    echo -e "${RED}Error: Run as root — sudo bash or pipe to sudo bash${NC}"
    exit 1
fi

# Detect package manager
if command -v apt-get &> /dev/null; then
    PKG_INSTALL="apt-get install -y"
    PKG_UPDATE="apt-get update -y"
elif command -v dnf &> /dev/null; then
    PKG_INSTALL="dnf install -y"
    PKG_UPDATE="dnf check-update || true"
elif command -v yum &> /dev/null; then
    PKG_INSTALL="yum install -y"
    PKG_UPDATE="yum check-update || true"
else
    echo -e "${RED}Error: No supported package manager found (apt/dnf/yum)${NC}"
    exit 1
fi

# Install git if needed
if ! command -v git &> /dev/null; then
    echo -e "${GREEN}[+]${NC} Installing git..."
    $PKG_UPDATE > /dev/null 2>&1
    $PKG_INSTALL git > /dev/null 2>&1
fi

# Clone or update repo
if [ -d "$INSTALL_DIR/.git" ]; then
    echo -e "${GREEN}[+]${NC} Updating existing installation..."
    if ! (cd "$INSTALL_DIR" && git pull --ff-only); then
        echo -e "${RED}[x] Git update failed (local changes?). Run: cd $INSTALL_DIR && git stash && git pull${NC}" >&2
        exit 1
    fi
else
    echo -e "${GREEN}[+]${NC} Downloading DockPanel..."
    rm -rf "$INSTALL_DIR"
    git clone --depth 1 -b "$VERSION" https://github.com/ovexro/dockpanel.git "$INSTALL_DIR"
fi

# On ARM with <2GB RAM, auto-use pre-built binaries (compilation would OOM)
ARCH=$(uname -m)
if [ "$ARCH" = "aarch64" ]; then
    TOTAL_MEM=$(awk '/MemTotal/ {print int($2/1024)}' /proc/meminfo 2>/dev/null || echo "0")
    if [ "$TOTAL_MEM" -lt 2000 ] && [ "${INSTALL_FROM_RELEASE:-0}" != "1" ]; then
        echo -e "${GREEN}[+]${NC} ARM64 with ${TOTAL_MEM}MB RAM — using pre-built binaries for faster install"
        export INSTALL_FROM_RELEASE=1
    fi
fi

# Run setup
echo -e "${GREEN}[+]${NC} Running setup..."
exec bash "$INSTALL_DIR/scripts/setup.sh"
