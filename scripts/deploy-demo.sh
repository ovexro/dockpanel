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

# The published assets, named ONCE. The download loop, the checksum check, the
# linkage guard and the swap all iterate this list, so a fourth asset cannot be
# added and silently skip a check. That is exactly how the linkage guard came to
# cover one binary of three: it named `dockpanel-api-linux-amd64` literally while
# the loop above it fetched three and the swap below it installed three (s336).
ASSETS=(dockpanel-agent-linux-amd64 dockpanel-api-linux-amd64 dockpanel-cli-linux-amd64)
declare -A ASSET_DEST=(
    [dockpanel-agent-linux-amd64]=/usr/local/bin/dockpanel-agent
    [dockpanel-api-linux-amd64]=/usr/local/bin/dockpanel-api
    [dockpanel-cli-linux-amd64]=/usr/local/bin/dockpanel
)

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
for a in "${ASSETS[@]}"; do
    curl -fsSL -o "$TMP/$a" "$BASE/$a"
    echo "  $a $(stat -c%s "$TMP/$a") bytes"
done

# Verify every asset against the release's own checksums.txt before anything is
# installed. The other three paths that consume a published release have always
# done this — update.sh:351, agent-self-update.sh:181, setup.sh:905 — and this
# one, the path that deploys the PUBLIC demo, was the only one that verified no
# checksum at all. Fail closed like update.sh: this deploys a named, already
# published tag, so a manifest that will not fetch is a reason to stop rather
# than a reason to shrug.
CHECKSUMS="$TMP/checksums.txt"
if ! curl -fsSL -o "$CHECKSUMS" "$BASE/checksums.txt"; then
    echo "REFUSING: could not fetch $BASE/checksums.txt — will not install unverified binaries"; exit 1
fi
for a in "${ASSETS[@]}"; do
    want=$(awk -v n="$a" '$2 == n {print $1; exit}' "$CHECKSUMS")
    if [ -z "$want" ]; then
        echo "REFUSING: checksums.txt for $TAG has no entry for $a"; exit 1
    fi
    got=$(sha256sum "$TMP/$a" | awk '{print $1}')
    if [ "$want" != "$got" ]; then
        echo "REFUSING: sha256 mismatch for $a"; echo "  expected $want"; echo "  got      $got"; exit 1
    fi
    echo "  $a: sha256 verified"
done

# Prove they are the static musl binaries users get, not something else. A
# dev-built gnu binary reproduced issue #70's exact GLIBC failure at s228, and
# this refusal is why a hand deploy — not this script — is what put a stale
# glibc agent on the demo at s271.
#
# Until s336 this checked the API asset ALONE — so the guard whose reason for
# existing is the s271 stale-glibc incident skipped the agent, the binary that
# incident was about. It also read any readelf failure as proof of static
# linkage: the old form ended in a grep, and under `pipefail` a pipeline keeps
# the RIGHTMOST non-zero status, so readelf's 127 was overwritten by grep's
# no-match 1 and a missing binutils, a non-ELF file or a zero-byte download all
# printed "confirmed: no DT_NEEDED (static)". Measured, all three.
require_static_musl() {
    local path="$1" name dyn
    name=$(basename "$path")
    if [ ! -s "$path" ]; then
        echo "REFUSING: $name is missing or zero bytes"; exit 1
    fi
    if ! command -v readelf >/dev/null 2>&1; then
        echo "REFUSING: readelf is not installed, so $name's linkage cannot be proven (apt-get install binutils)"; exit 1
    fi
    if ! dyn=$(readelf -d "$path" 2>&1); then
        echo "REFUSING: readelf could not read $name — ${dyn%%$'\n'*}"; exit 1
    fi
    case "$dyn" in
        *NEEDED*) echo "REFUSING: $name is dynamically linked"; exit 1 ;;
    esac
    echo "  $name: confirmed no DT_NEEDED (static)"
}
for a in "${ASSETS[@]}"; do
    file "$TMP/$a" | cut -c1-100
    require_static_musl "$TMP/$a"
done

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
for a in "${ASSETS[@]}"; do
    install -m 0755 "$TMP/$a" "${ASSET_DEST[$a]}.new"
    mv "${ASSET_DEST[$a]}.new" "${ASSET_DEST[$a]}"
done
systemctl start dockpanel-agent
sleep 2
systemctl start dockpanel-api

echo "=== AFTER ==="
# This block used to REPORT and never ASSERT. The health loop fell out silently
# after 40 seconds, the public curl carried no -f so a 502 printed as an empty
# string, every probe sat inside a command substitution inside an echo where it
# could not trip errexit, and nothing compared any of the three version reads to
# $TAG — the script's one required argument. So a run that left the API dead
# still exited 0, and anything reading that status recorded a successful deploy
# of the box we prove releases on (s336).
WANT="${TAG#v}"
DEPLOY_OK=1
fail() { echo "  FAILED: $1"; DEPLOY_OK=0; }

LOCAL_HEALTH=""
for i in $(seq 1 20); do
    LOCAL_HEALTH=$(curl -fsS -m 4 http://127.0.0.1:3080/api/health 2>/dev/null) && break
    sleep 2
done
echo "local:  ${LOCAL_HEALTH:-(no answer)}"
[ -n "$LOCAL_HEALTH" ] || fail "the local /api/health never answered in 40s"

PUBLIC_HEALTH=$(curl -fsS -m 8 "https://$DEMO_HOST/api/health" 2>/dev/null) || PUBLIC_HEALTH=""
echo "public: ${PUBLIC_HEALTH:-(no answer)}"
[ -n "$PUBLIC_HEALTH" ] || fail "https://$DEMO_HOST/api/health returned no body"

CLI_VERSION=$(dockpanel --version 2>/dev/null) || CLI_VERSION=""
echo "cli:    ${CLI_VERSION:-(no answer)}"
[ -n "$CLI_VERSION" ] || fail "the CLI did not report a version"

# The agent's OWN socket, which the reads above cannot see. /api/health describes
# the API binary; a stale AGENT is invisible to it, and that is exactly what s271
# shipped and only this read caught. An unreadable socket is a FAILURE, not a
# footnote: it means the one probe that would catch a stale agent did not run.
AGENT_HEALTH=""
if [ -S /run/dockpanel/agent.sock ] && [ -r /etc/dockpanel/agent.token ]; then
    AGENT_HEALTH=$(curl -fsS -m 8 --unix-socket /run/dockpanel/agent.sock \
        -H "Authorization: Bearer $(cat /etc/dockpanel/agent.token)" \
        http://localhost/health 2>/dev/null) || AGENT_HEALTH=""
    echo "agent:  ${AGENT_HEALTH:-(no answer)}"
    [ -n "$AGENT_HEALTH" ] || fail "the agent socket did not answer /health"
else
    echo "agent:  (socket or token not readable)"
    fail "the agent socket or token could not be read, so the agent version was never checked"
fi

# Every surface that answered must report the tag we were asked to deploy.
for probe in "local|$LOCAL_HEALTH" "public|$PUBLIC_HEALTH" "cli|$CLI_VERSION" "agent|$AGENT_HEALTH"; do
    name=${probe%%|*}; body=${probe#*|}
    [ -n "$body" ] || continue
    case "$body" in
        *"$WANT"*) ;;
        *) fail "$name does not report $WANT" ;;
    esac
done

API_STATE=$(systemctl is-active dockpanel-api || true)
AGENT_STATE=$(systemctl is-active dockpanel-agent || true)
echo "units:  $API_STATE $AGENT_STATE"
[ "$API_STATE" = active ] || fail "dockpanel-api is $API_STATE"
[ "$AGENT_STATE" = active ] || fail "dockpanel-agent is $AGENT_STATE"

echo "=== journal errors (2 min) ==="
journalctl -u dockpanel-api -u dockpanel-agent --since "-2 min" -p err --no-pager | tail -10 || true

if [ "$DEPLOY_OK" -ne 1 ]; then
    echo "=== DEPLOY FAILED — see the FAILED lines above ==="
    exit 1
fi
echo "=== deploy verified: every surface reports $WANT ==="
