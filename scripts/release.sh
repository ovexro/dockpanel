#!/bin/bash
# DockPanel Release Script
# Builds x86_64 + ARM64 binaries, packages, creates GitHub Release
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

cd "$(dirname "$0")/.."

# ─── Determine version ───
VERSION=$(grep '^version' panel/agent/Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
TAG="v${VERSION}"
LATEST_TAG=$(git describe --tags --abbrev=0 2>/dev/null || echo "none")

if [ "$TAG" = "$LATEST_TAG" ]; then
  echo -e "${RED}Version $VERSION already has a tag ($TAG). Bump version first.${NC}"
  echo "  Edit: panel/agent/Cargo.toml, panel/backend/Cargo.toml, panel/cli/Cargo.toml, panel/frontend/package.json"
  exit 1
fi

echo -e "${GREEN}Building DockPanel $TAG${NC}"
echo "  Previous tag: $LATEST_TAG"
echo ""

# ─── Version consistency check ───
V_BACKEND=$(grep '^version' panel/backend/Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
V_CLI=$(grep '^version' panel/cli/Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
V_FRONTEND=$(grep '"version"' panel/frontend/package.json | head -1 | sed 's/.*: *"\(.*\)".*/\1/')

if [ "$VERSION" != "$V_BACKEND" ] || [ "$VERSION" != "$V_CLI" ] || [ "$VERSION" != "$V_FRONTEND" ]; then
  echo -e "${RED}Version mismatch: agent=$VERSION backend=$V_BACKEND cli=$V_CLI frontend=$V_FRONTEND${NC}"
  exit 1
fi

# ─── Source cargo env ───
source /root/.cargo/env 2>/dev/null || true

# ─── Build directory ───
DIST="dist/release-${VERSION}"
rm -rf "$DIST"
mkdir -p "$DIST"

# ─── Build x86_64 (native) ───
echo -e "${YELLOW}Building x86_64...${NC}"
cd panel/agent && cargo build --release 2>&1 | tail -1 && cd ../..
cd panel/backend && cargo build --release 2>&1 | tail -1 && cd ../..
cd panel/cli && cargo build --release 2>&1 | tail -1 && cd ../..

cp panel/agent/target/release/dockpanel-agent "$DIST/dockpanel-agent-linux-amd64"
cp panel/backend/target/release/dockpanel-api "$DIST/dockpanel-api-linux-amd64"
cp panel/cli/target/release/dockpanel "$DIST/dockpanel-cli-linux-amd64"

# ─── Build ARM64 (cross-compile) ───
echo -e "${YELLOW}Building aarch64...${NC}"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc

cd panel/agent && cargo build --release --target aarch64-unknown-linux-gnu 2>&1 | tail -1 && cd ../..
cd panel/backend && cargo build --release --target aarch64-unknown-linux-gnu 2>&1 | tail -1 && cd ../..
cd panel/cli && cargo build --release --target aarch64-unknown-linux-gnu 2>&1 | tail -1 && cd ../..

cp panel/agent/target/aarch64-unknown-linux-gnu/release/dockpanel-agent "$DIST/dockpanel-agent-linux-arm64"
cp panel/backend/target/aarch64-unknown-linux-gnu/release/dockpanel-api "$DIST/dockpanel-api-linux-arm64"
cp panel/cli/target/aarch64-unknown-linux-gnu/release/dockpanel "$DIST/dockpanel-cli-linux-arm64"

# ─── Build frontend ───
echo -e "${YELLOW}Building frontend...${NC}"
cd panel/frontend && npm run build 2>&1 | tail -1 && cd ../..
tar czf "$DIST/dockpanel-frontend.tar.gz" -C panel/frontend dist/

# ─── Generate checksums ───
echo -e "${YELLOW}Generating checksums...${NC}"
cd "$DIST" && sha256sum * > SHA256SUMS && cd ../..

# ─── Show results ───
echo ""
echo -e "${GREEN}Release artifacts in $DIST/:${NC}"
ls -lh "$DIST/"
echo ""

# ─── Docs audit ───
echo -e "${YELLOW}Running docs audit...${NC}"
bash scripts/docs-audit.sh || true
echo ""

# ─── Create release ───
echo -e "${YELLOW}Ready to create GitHub Release $TAG${NC}"
echo "To create the release, run:"
echo ""
echo "  git tag $TAG && git push origin $TAG"
echo "  gh release create $TAG --title 'DockPanel $TAG' --generate-notes $DIST/*"
echo ""
echo "Or pass --publish to this script to do it automatically (requires gh auth + git push access)."

if [ "${1:-}" = "--publish" ]; then
  echo ""
  echo -e "${YELLOW}Publishing...${NC}"
  git tag "$TAG"
  echo "  Tagged $TAG"
  # Note: user must provide PAT for push
  echo -e "${RED}Push the tag manually: git push origin $TAG${NC}"
  echo "  Then: gh release create $TAG --title 'DockPanel $TAG' --generate-notes $DIST/*"
fi
