#!/usr/bin/env bash
#
# Deploy THIS box to a published release, from the PUBLISHED static-musl assets.
#
# This is the third path that installs DockPanel onto a running machine. The
# other two are scripts/setup.sh (fresh install) and scripts/update.sh (upgrade
# in place). It exists because the demo box is deployed from published artefacts
# rather than from a git checkout, and it is the path used to prove that what
# was released is what runs.
#
# It lived OUTSIDE this repository until s275, and that is the whole reason it is
# here now. Between v2.37.0 and s274 it could not run at all — its ReadWritePaths
# pre-create loop never stripped systemd's optional-path "-" prefix, so it ran
# `mkdir -p -/etc/letsencrypt` and died on `mkdir: invalid option -- '/'`. s269
# had fixed exactly that in setup.sh and update.sh BY DERIVATION; this third copy
# never learned it, because no pin, no CI job and no docs check could see a file
# that was not in the tree. Eight releases. It failed SAFE — aborting before
# `systemctl stop` — which is precisely why nobody noticed: the manual fallback
# worked, and hand deploys are how a stale glibc agent reached the demo at s271.
#
# The lesson, and the reason for this header: a derivation fix reaches the
# mirrors in the tree. The out-of-repo copy is the one that rots silently.
#
# Usage:
#   DEMO_HOST=panel.example.com bash scripts/deploy-demo.sh v2.44.1
#
# DEMO_HOST is required and has no default: the public hostname is a property of
# the box you are deploying, not of this repository.
#
set -euo pipefail

TAG="${1:?usage: DEMO_HOST=<panel-host> deploy-demo.sh vX.Y.Z}"
DEMO_HOST="${DEMO_HOST:?DEMO_HOST must name the public hostname of the panel being deployed (no default)}"
GITHUB_REPO="${GITHUB_REPO:-ovexro/dockpanel}"
BASE="https://github.com/$GITHUB_REPO/releases/download/$TAG"

# ── Resolve repo root — same idiom as setup.sh/update.sh ─────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
AGENT_SRC="$REPO_DIR/panel/agent"
FE_DIST="$REPO_DIR/panel/frontend/dist"

# Directories the agent unit declares in ReadWritePaths, with any `-` prefix
# stripped. The unit is the single source of truth, so this can never fall
# behind it the way a hand-copied list does. Byte-identical to
# setup.sh::agent_rwp_dirs and update.sh::agent_rwp_dirs — tests/sandbox-paths-
# pin-e2e.sh asserts all three agree, and discovers this file rather than naming
# it, so a fourth copy joins the check by existing.
agent_rwp_dirs() {
    local unit="$AGENT_SRC/dockpanel-agent.service"
    [ -f "$unit" ] || unit="/etc/systemd/system/dockpanel-agent.service"
    [ -f "$unit" ] || return 0
    sed -n 's/^ReadWritePaths=//p' "$unit" | tr ' ' '\n' | sed 's/^-//' | grep '^/' || true
}

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

echo "=== BEFORE ==="
curl -s -m 8 "https://$DEMO_HOST/api/health"; echo
dockpanel --version || true

echo "=== download $TAG assets ==="
for a in dockpanel-agent-linux-amd64 dockpanel-api-linux-amd64 dockpanel-cli-linux-amd64; do
    curl -fsSL -o "$TMP/$a" "$BASE/$a"
    echo "  $a $(stat -c%s "$TMP/$a") bytes"
done

# Prove they are the static musl binaries users get, not something else. A
# dev-built gnu binary reproduced issue #70's exact GLIBC failure at s228, and
# this refusal is why a hand deploy — not this script — is what put a stale
# glibc agent on the demo at s271.
file "$TMP/dockpanel-api-linux-amd64" | cut -c1-100
if readelf -d "$TMP/dockpanel-api-linux-amd64" 2>/dev/null | grep -c NEEDED >/dev/null; then
    echo "REFUSING: api asset is dynamically linked"; exit 1
fi
echo "  confirmed: no DT_NEEDED (static)"

echo "=== refresh the agent systemd unit ==="
# This script used to swap BINARIES ONLY, so every unit change since the box was
# installed silently never landed here — update.sh refreshes the unit, this
# didn't. Found at s253 with the demo still missing /var/vmail (an s236 fix) and
# /etc/php (v2.28.0's), which made the per-site PHP-FPM pool fix inert on the one
# box we demo from.
#
# Pre-create every ReadWritePaths entry FIRST: systemd fails a unit outright when
# an unprefixed path is missing, and "the agent won't start" is a far worse
# outcome than a stale sandbox. Order matters — the namespace is fixed at start,
# so a directory created afterwards does not rescue a running service.
UNIT_SRC="$AGENT_SRC/dockpanel-agent.service"
if [ -f "$UNIT_SRC" ]; then
    for d in $(agent_rwp_dirs); do
        [ -e "$d" ] || mkdir -p "$d"
    done
    if ! cmp -s "$UNIT_SRC" /etc/systemd/system/dockpanel-agent.service; then
        cp "$UNIT_SRC" /etc/systemd/system/dockpanel-agent.service
        chmod 644 /etc/systemd/system/dockpanel-agent.service
        systemctl daemon-reload
        echo "  agent unit refreshed from the repo"
    else
        echo "  agent unit already current"
    fi
else
    echo "  WARNING: $UNIT_SRC not found — leaving the on-disk unit alone"
fi

echo "=== drop install-agent.sh into the frontend dist (#56, v2.8.14) ==="
# The panel's Add-Server dialog prints `curl -sSL {panel}/install-agent.sh | sudo
# bash -s -- …`. nginx serves the SPA with a try_files fallback, so when that file
# is absent the URL returns HTTP 200 WITH index.html and the operator pipes an
# HTML document into bash. Not a 404 — a 200 of SPA fallback HTML, the same shape
# as issue #56 and as the failure mode the live-surfaces check names for
# install.sh. setup.sh and update.sh both deploy it; this path never did, because
# it only ever swapped binaries. Found at s274 by fetching the published
# installer the way an operator does and getting 643 bytes of <!DOCTYPE html>.
INSTALL_AGENT_SRC="$REPO_DIR/scripts/install-agent.sh"
if [ -f "$INSTALL_AGENT_SRC" ] && [ -d "$FE_DIST" ]; then
    install -m 0644 "$INSTALL_AGENT_SRC" "$FE_DIST/install-agent.sh"
    echo "  install-agent.sh deployed to $FE_DIST"
else
    echo "  WARNING: cannot deploy install-agent.sh (src or dist missing)"
fi

echo "=== stop, swap, start ==="
# `mv`, never `cp` onto a running binary: cp fails ETXTBSY (lesson #48).
systemctl stop dockpanel-api dockpanel-agent
install -m 0755 "$TMP/dockpanel-agent-linux-amd64" /usr/local/bin/dockpanel-agent.new
install -m 0755 "$TMP/dockpanel-api-linux-amd64"   /usr/local/bin/dockpanel-api.new
install -m 0755 "$TMP/dockpanel-cli-linux-amd64"   /usr/local/bin/dockpanel.new
mv /usr/local/bin/dockpanel-agent.new /usr/local/bin/dockpanel-agent
mv /usr/local/bin/dockpanel-api.new   /usr/local/bin/dockpanel-api
mv /usr/local/bin/dockpanel.new       /usr/local/bin/dockpanel
systemctl start dockpanel-agent
sleep 2
systemctl start dockpanel-api

echo "=== AFTER ==="
for i in $(seq 1 20); do
    H=$(curl -fsS -m 4 http://127.0.0.1:3080/api/health 2>/dev/null) && { echo "local:  $H"; break; }
    sleep 2
done
echo "public: $(curl -s -m 8 "https://$DEMO_HOST/api/health")"
echo "cli:    $(dockpanel --version)"

# The agent's OWN socket, which the two lines above cannot see. /api/health
# describes the API binary; a stale AGENT is invisible to it, and that is exactly
# what s271 shipped and only this read caught.
if [ -S /run/dockpanel/agent.sock ] && [ -r /etc/dockpanel/agent.token ]; then
    echo "agent:  $(curl -s -m 8 --unix-socket /run/dockpanel/agent.sock \
        -H "Authorization: Bearer $(cat /etc/dockpanel/agent.token)" \
        http://localhost/health 2>/dev/null || echo '(unreadable)')"
else
    echo "agent:  (socket or token not readable — read the agent version by hand)"
fi

echo "units:  $(systemctl is-active dockpanel-api) $(systemctl is-active dockpanel-agent)"
echo "=== journal errors (2 min) ==="
journalctl -u dockpanel-api -u dockpanel-agent --since "-2 min" -p err --no-pager | tail -10 || true
