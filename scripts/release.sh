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

# ─── Generate SBOMs (SPDX JSON) ───
echo -e "${YELLOW}Generating SBOMs...${NC}"
if ! command -v cargo-sbom >/dev/null 2>&1; then
  echo "  Installing cargo-sbom..."
  cargo install cargo-sbom --locked --version ^0.10 >/dev/null
fi
cargo sbom --project-directory panel/agent   --output-format spdx_json_2_3 > "$DIST/dockpanel-agent.spdx.json"
cargo sbom --project-directory panel/backend --output-format spdx_json_2_3 > "$DIST/dockpanel-api.spdx.json"
cargo sbom --project-directory panel/cli     --output-format spdx_json_2_3 > "$DIST/dockpanel-cli.spdx.json"

# ─── Generate checksums ───
echo -e "${YELLOW}Generating checksums...${NC}"
cd "$DIST" && sha256sum * > SHA256SUMS && cd ../..

# ─── Note on signatures ───
echo -e "${YELLOW}Note:${NC} local builds are unsigned. Signed releases are produced by GitHub Actions (.github/workflows/release.yml) using cosign keyless via Sigstore."

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
#
# THE ARTIFACTS IN $DIST ARE NOT WHAT SHIPS. They are native `cargo build`
# output: dynamically linked against this box's glibc. Every published release
# is a STATIC MUSL build produced by .github/workflows/release.yml with
# `cargo zigbuild`, signed with Sigstore, precisely so the agent runs on
# Ubuntu 20.04+, Rocky 9 and Amazon Linux 2023 (#70) — and a glibc agent
# reaching a box is a failure this project has already had (s271).
#
# So this script BUILDS and AUDITS; the tag PUBLISHES. Uploading $DIST/* by
# hand would replace 33 signed static assets with 12 unsigned dynamic ones.
# Until s288 the instructions printed here said to do exactly that.
echo -e "${YELLOW}Local build complete — these artifacts are for smoke-testing only.${NC}"
echo "  $DIST"
echo "  ($(file "$DIST/dockpanel-agent-linux-amd64" | grep -oE 'statically linked|dynamically linked') — the published assets are static musl)"
echo ""
echo -e "${GREEN}To publish $TAG, push the tag and let CI build it:${NC}"
echo ""
echo "  git tag $TAG && git push origin $TAG"
echo ""
echo "  → .github/workflows/release.yml builds x86_64 + aarch64 static musl,"
echo "    generates SBOMs + checksums, signs with Sigstore, and creates the Release."
echo "  → Then verify per [[feedback_ship_verification_protocol]]: gh release view,"
echo "    and fetch a published binary to confirm 0 DT_NEEDED."
echo ""
echo "Do NOT run 'gh release create' with $DIST/* — see the comment above this block."

if [ "${1:-}" = "--publish" ]; then
  echo ""
  echo -e "${YELLOW}Tagging...${NC}"
  git tag "$TAG"
  echo "  Tagged $TAG locally."
  echo -e "${RED}Now push it: git push origin $TAG${NC}"
  echo "  CI does the rest. Nothing from $DIST is uploaded."
fi
