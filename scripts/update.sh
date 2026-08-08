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

# ── Escape the caller's service cgroup ────────────────────────────────────
# When the panel triggers an update it spawns this script from inside
# dockpanel-api.service. That unit is KillMode=control-group (the systemd
# default), so the `systemctl stop dockpanel-agent dockpanel-api` further down
# SIGTERMs every process in the cgroup — including this script, one line before
# it swaps the binaries. The box is then left with both services stopped and the
# old binaries still in place. Setting a POSIX process group (which the
# orchestrator does) does not help: systemd kills by cgroup, not process group.
#
# Re-exec into a transient scope so we live outside the unit being stopped.
# Harmless for the normal SSH invocation — that cgroup never matches.
# Must be a transient *service* (PID1-owned), not `--scope`: a scope is created
# in the caller's context, so it lands in the invoking session's user-0.slice and
# dies with it ("Removed slice user-0.slice" — observed killing an update
# mid-swap). A transient service is owned by PID1 and outlives both the unit
# being stopped and any session.
if [ -z "${DOCKPANEL_UPDATE_DETACHED:-}" ] && command -v systemd-run >/dev/null 2>&1; then
    if grep -qE 'dockpanel-(api|agent)\.service' /proc/self/cgroup 2>/dev/null; then
        echo "[+] Re-executing outside the panel's service cgroup..."
        exec systemd-run --quiet --collect \
            --unit="dockpanel-self-update-$$" \
            --setenv=DOCKPANEL_UPDATE_DETACHED=1 \
            --setenv=INSTALL_FROM_RELEASE="${INSTALL_FROM_RELEASE:-}" \
            --setenv=DOCKPANEL_VERSION="${DOCKPANEL_VERSION:-}" \
            --setenv=DOCKPANEL_NO_SELF_REFRESH="${DOCKPANEL_NO_SELF_REFRESH:-}" \
            bash "$0" "$@"
    fi
fi

# ── The verdict file ──────────────────────────────────────────────────────
# Why this exists (F1, s231→s282): the panel orchestrator spawns this script and
# waits on the child. But the block above `exec systemd-run`s without `--wait`,
# and systemd-run returns 0 the instant PID1 ACCEPTS the job — measured at ~29ms
# against a real update that runs ~54s. So the orchestrator's child exits 0
# almost immediately, its stdout pipe hits EOF one line in, and the exit status
# it observes belongs to systemd-run rather than to the update.
#
# For a SUCCESSFUL update that does not matter much: this script stops
# dockpanel-api, the orchestrator dies with it, and the next boot works out what
# happened. The case that was broken is a failure BEFORE the services are
# stopped — a bad download, a missing file, a failed database backup. The api is
# never stopped, so no restart ever comes to carry a verdict; the orchestrator
# saw exit 0 and finalises nothing; and the operator watches an in-flight update
# frozen on "Re-executing outside the panel's service cgroup…" until the
# 15-minute window lapses, with the real error only in the transient unit's
# journal.
#
# The fix is the one the agent side already uses (`last-agent-update.json`) and
# the restore already uses (`last-restore.json`): write the outcome to a file
# that outlives the process. `--pipe` is NOT the alternative — it implies
# `--wait` and wires the unit's stdout to a caller that is about to be killed, so
# the updater would take SIGPIPE on its next log line, mid binary swap.
DOCKPANEL_STATE_DIR="${DOCKPANEL_STATE_DIR:-/var/lib/dockpanel}"
_dockpanel_result="$DOCKPANEL_STATE_DIR/last-panel-update.json"
_dockpanel_stage="preflight"
_dockpanel_finished=0

_dockpanel_json_escape() {
    printf '%s' "$1" | tr -d '\000-\010\013\014\016-\037' \
        | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' | tr '\n' ' '
}

_dockpanel_write_result() {
    local ok="$1" stage="$2" detail="$3"
    mkdir -p "$DOCKPANEL_STATE_DIR" 2>/dev/null || return 0
    local tmp="$_dockpanel_result.tmp"
    printf '{"target_version":"%s","ok":%s,"stage":"%s","detail":"%s","finished_at":"%s"}\n' \
        "$(_dockpanel_json_escape "${DOCKPANEL_VERSION:-}")" "$ok" "$stage" \
        "$(_dockpanel_json_escape "$detail")" \
        "$(date -u +%Y-%m-%dT%H:%M:%SZ)" > "$tmp" 2>/dev/null || return 0
    chmod 0600 "$tmp" 2>/dev/null || true
    mv "$tmp" "$_dockpanel_result" 2>/dev/null || true
}

# Safety net: if we get killed or fail after the services are stopped but before
# they are started again, bring them back rather than leaving the box dark.
_dockpanel_services_stopped=0
_dockpanel_on_exit() {
    local code=$?
    if [ "$_dockpanel_services_stopped" = "1" ]; then
        systemctl start dockpanel-agent dockpanel-api 2>/dev/null || true
    fi
    # Every exit path leaves a verdict, including the ones nobody wrote by hand:
    # a `set -e` abort, a SIGTERM, a failed curl. Without this the only paths
    # that reported anything were the ones that happened to be instrumented.
    if [ "$_dockpanel_finished" != "1" ]; then
        _dockpanel_write_result false "$_dockpanel_stage" \
            "update aborted at stage '$_dockpanel_stage' with exit code $code"
    fi
}
trap _dockpanel_on_exit EXIT INT TERM

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

# Directories the agent unit declares in ReadWritePaths, with any `-` prefix
# stripped. Derived from the unit that is about to be deployed (or, on a layout
# with no repo tree, the one already installed) so this can never fall behind it.
# Prints nothing if no unit is readable; callers must tolerate an empty result.
agent_rwp_dirs() {
    local unit="$AGENT_SRC/dockpanel-agent.service"
    [ -f "$unit" ] || unit="/etc/systemd/system/dockpanel-agent.service"
    [ -f "$unit" ] || return 0
    sed -n 's/^ReadWritePaths=//p' "$unit" | tr ' ' '\n' | sed 's/^-//' | grep '^/' || true
}
FRONTEND_DIR="$REPO_DIR/panel/frontend"
AGENT_BIN="/usr/local/bin/dockpanel-agent"
API_BIN="/usr/local/bin/dockpanel-api"
CLI_BIN="/usr/local/bin/dockpanel"
INSTALL_FROM_RELEASE="${INSTALL_FROM_RELEASE:-0}"
GITHUB_REPO="ovexro/dockpanel"

# ── Mode detection (must run BEFORE self-refresh) ─────────────────────────
# Self-refresh is gated on INSTALL_FROM_RELEASE=1, so the auto-detect that
# flips it to 1 has to happen first. v2.8.15 and earlier had this in the
# wrong order: a user running `bash update.sh` (no env vars) entered with
# INSTALL_FROM_RELEASE=0, failed the self-refresh check, then got bumped
# to 1 by auto-detect — but with the stale local script still running.
# Result: binaries upgrade fine, but script-side fixes (unit files, nginx
# tweaks, install-agent.sh deploy) never reach pre-v2.8.16 panels.
if [ "$INSTALL_FROM_RELEASE" != "1" ] && [ ! -d "$AGENT_SRC/src" ]; then
    log "No source found — switching to pre-built binary download"
    INSTALL_FROM_RELEASE=1
fi

# Auto-detect: if Rust toolchain isn't available, use release binaries.
# Production VPS installs typically don't have cargo on PATH (and usually
# don't have enough RAM to compile rustc's dep tree — proc-macro2 OOMs at
# ~1-2 GB). Fall back to the pre-built artifacts on the matching tag rather
# than asking the operator to install rustup just to update.
if [ "$INSTALL_FROM_RELEASE" != "1" ] \
   && ! command -v cargo > /dev/null 2>&1 \
   && [ ! -x "$HOME/.cargo/bin/cargo" ]; then
    log "Rust toolchain not found — switching to pre-built binary download"
    log "(set BUILD_FROM_SOURCE=1 to force compile-from-source instead)"
    INSTALL_FROM_RELEASE=1
fi

# Explicit opt-in to keep compile-from-source behaviour even when cargo
# is on PATH (e.g. for developers iterating on a checkout).
if [ "${BUILD_FROM_SOURCE:-0}" = "1" ]; then
    INSTALL_FROM_RELEASE=0
fi

# ── Self-refresh ──────────────────────────────────────────────────────────
# In binary-release mode, the on-disk copy of this script can lag the
# repo by several releases (it's only refreshed by re-running install.sh).
# That means a bug in update.sh — like the 405-rollback bug fixed in
# v2.7.13 — strands operators unable to upgrade. Pull the latest script
# from the latest release tag and re-exec ourselves before running any
# update logic. SELF_REFRESHED=1 prevents an infinite re-exec loop.
# DOCKPANEL_NO_SELF_REFRESH=1 — set by the panel-internal orchestrator
# (services/panel_update.rs) so it can stream a single update.sh invocation's
# stdout into its state machine without a mid-flight re-exec breaking the pipe.
# SSH-operator flow keeps self-refresh on by default.
if [ "${SELF_REFRESHED:-0}" != "1" ] && [ "$INSTALL_FROM_RELEASE" = "1" ] \
   && [ "${DOCKPANEL_NO_SELF_REFRESH:-0}" != "1" ]; then
    LATEST_TAG=$(curl -fsSL "https://api.github.com/repos/${GITHUB_REPO}/releases/latest" 2>/dev/null \
        | grep -m1 '"tag_name"' | cut -d'"' -f4 || true)
    if [ -n "$LATEST_TAG" ]; then
        REMOTE_URL="https://raw.githubusercontent.com/${GITHUB_REPO}/${LATEST_TAG}/scripts/update.sh"
        TMP=$(mktemp)
        if curl -fsSL "$REMOTE_URL" -o "$TMP" 2>/dev/null && [ -s "$TMP" ]; then
            # Compare to current to avoid an unnecessary re-exec on every run
            if ! cmp -s "$TMP" "${BASH_SOURCE[0]}"; then
                log "Refreshing update.sh from $LATEST_TAG (current copy is stale)"
                cp "$TMP" "${BASH_SOURCE[0]}" 2>/dev/null || true
                rm -f "$TMP"
                export SELF_REFRESHED=1
                exec bash "${BASH_SOURCE[0]}" "$@"
            fi
            rm -f "$TMP"
        else
            rm -f "$TMP"
        fi
    fi
fi

# For source builds, verify source exists
if [ "$INSTALL_FROM_RELEASE" != "1" ] && [ ! -d "$AGENT_SRC/src" ]; then
    error "Cannot find agent source at $AGENT_SRC"
    exit 1
fi

echo ""
echo -e "${GREEN}${BOLD}DockPanel Updater${NC}"
echo ""

# ── Sync repo to origin/main ──────────────────────────────────────────────
_dockpanel_stage="sync-repo"
# Both modes need a fresh tree: the canonical systemd unit
# (panel/agent/dockpanel-agent.service), nginx templates, install-agent.sh,
# and a few other repo-resident files are deployed from $REPO_DIR. Without a
# pull, `bash /opt/dockpanel/scripts/update.sh` would download new binaries
# but redeploy the OLD canonical unit — exactly what stranded the v2.8.13 →
# v2.8.14 upgrade-path test (RuntimeDirectory=dockpanel and /var/cache/nginx
# in ReadWritePaths never reached the deployed unit). v2.8.15.
#
# `git pull --ff-only` doesn't cover installs cloned with `-b vX.Y.Z` (those
# end up on a detached HEAD with no `main` known locally), so the sync uses
# `git fetch origin main` + `git reset --hard FETCH_HEAD` to forcibly track
# main. Local edits to /opt/dockpanel are unsupported (it's a deploy
# artifact, not a working tree) — `git stash` captures any incidental drift
# in case anyone wants to inspect it post-upgrade.
if [ -d "$REPO_DIR/.git" ]; then
    log "Syncing repo to latest origin/main..."
    (cd "$REPO_DIR" && {
        git stash -q 2>/dev/null || true
        if git fetch --depth=1 origin main 2>/dev/null; then
            git reset --hard FETCH_HEAD 2>&1 | tail -1 >/dev/null || true
        else
            log "Warning: git fetch failed — deploying from existing on-disk source"
        fi
    }) || log "Warning: repo sync failed — deploying from existing on-disk source"
fi

# ── Backup database before upgrade ────────────────────────────────────────
_dockpanel_stage="db-backup"
BACKUP_DIR="/var/backups/dockpanel/db"
mkdir -p "$BACKUP_DIR"
# `>` creates 0666 & ~umask. This dump carries `servers.agent_token` in
# cleartext — the Bearer credential for every agent endpoint and the key the
# agent signs its own root-shell tickets with — so it is root-only. The two
# chmods repair a tree an older installer already left 0755/0644; the agent
# does the same at startup, but update.sh runs on panel boxes that may not
# restart the agent for hours.
umask 077
chmod 700 "$BACKUP_DIR" /var/backups/dockpanel 2>/dev/null || true
log "Backing up database..."
if docker exec dockpanel-postgres pg_dump -U dockpanel dockpanel | gzip > "$BACKUP_DIR/pre-upgrade-$(date +%Y%m%d%H%M%S).sql.gz"; then
    log "Database backup saved to $BACKUP_DIR/"
else
    error "Database backup failed, aborting upgrade"
    exit 1
fi

# ── Build or download binaries ────────────────────────────────────────────
_dockpanel_stage="binaries"
if [ "$INSTALL_FROM_RELEASE" = "1" ]; then
    # Download pre-built binaries from GitHub Releases
    ARCH=$(uname -m)
    case "$ARCH" in
        x86_64)  DL_ARCH="amd64" ;;
        aarch64) DL_ARCH="arm64" ;;
        *) error "Unsupported architecture: $ARCH"; exit 1 ;;
    esac

    # DOCKPANEL_VERSION=vX.Y.Z (or vX.Y.Z-rc.N) — pin to a specific release
    # tag instead of /releases/latest. Lets the panel-internal orchestrator
    # honour the operator's candidate-channel pick and the in-process
    # "available version" snapshot, instead of racing /releases/latest mid-
    # apply (which could shift to a newer GA between poll and click).
    if [ -n "${DOCKPANEL_VERSION:-}" ]; then
        RELEASE_TAG="$DOCKPANEL_VERSION"
        # Accept a bare semver too. Panels up to v2.11.2 passed the version with
        # the `v` already stripped (their poller normalises it for display), which
        # built releases/download/2.11.2/... — a 404 — so their self-update could
        # never complete. Those panels ship a fixed binary only by updating, so
        # the only thing that can heal them is this script: the repo sync above
        # pulls this file before the download runs, meaning a second attempt on
        # an old panel now succeeds.
        case "$RELEASE_TAG" in
            [0-9]*) RELEASE_TAG="v$RELEASE_TAG" ;;
        esac
        log "Pinned release: $RELEASE_TAG (DOCKPANEL_VERSION)"
    else
        log "Fetching latest release..."
        RELEASE_TAG=$(curl -sf "https://api.github.com/repos/${GITHUB_REPO}/releases/latest" | grep '"tag_name"' | head -1 | cut -d'"' -f4)
        if [ -z "$RELEASE_TAG" ]; then
            error "Could not determine latest release. Check https://github.com/${GITHUB_REPO}/releases"
            exit 1
        fi
        log "Latest release: $RELEASE_TAG"
    fi
    BASE_URL="https://github.com/${GITHUB_REPO}/releases/download/${RELEASE_TAG}"

    # Verify every downloaded asset against the release's checksums.txt before
    # installing it — parity with scripts/agent-self-update.sh. The release ships
    # checksums.txt (.github/workflows/release.yml: `sha256sum dockpanel-* >
    # checksums.txt`; entries are bare basenames). Fail closed: a missing
    # checksums.txt, a missing entry, or a hash mismatch aborts the upgrade —
    # never install unverified bytes (lesson #25/#48).
    CHECKSUMS=/tmp/dockpanel-checksums.txt
    if ! curl -sfL "${BASE_URL}/checksums.txt" -o "$CHECKSUMS"; then
        error "Could not download ${BASE_URL}/checksums.txt — refusing to install unverified binaries"
        exit 1
    fi
    verify_checksum() {
        # $1 = downloaded file, $2 = asset name as it appears in checksums.txt
        local file="$1" asset="$2" expect actual
        expect=$(awk -v a="$asset" '$2 == a {print $1}' "$CHECKSUMS" | head -1)
        if [ -z "$expect" ]; then
            error "checksums.txt for $RELEASE_TAG has no entry for $asset — refusing to install"
            exit 1
        fi
        actual=$(sha256sum "$file" | awk '{print $1}')
        if [ "$actual" != "$expect" ]; then
            error "sha256 mismatch for $asset: got $actual, expected $expect — refusing to install"
            exit 1
        fi
        log "Verified $asset (sha256)"
    }

    log "Downloading agent (${DL_ARCH})..."
    curl -sfL "${BASE_URL}/dockpanel-agent-linux-${DL_ARCH}" -o /tmp/dockpanel-agent-new
    verify_checksum /tmp/dockpanel-agent-new "dockpanel-agent-linux-${DL_ARCH}"
    chmod +x /tmp/dockpanel-agent-new

    log "Downloading API (${DL_ARCH})..."
    curl -sfL "${BASE_URL}/dockpanel-api-linux-${DL_ARCH}" -o /tmp/dockpanel-api-new
    verify_checksum /tmp/dockpanel-api-new "dockpanel-api-linux-${DL_ARCH}"
    chmod +x /tmp/dockpanel-api-new

    log "Downloading CLI (${DL_ARCH})..."
    curl -sfL "${BASE_URL}/dockpanel-cli-linux-${DL_ARCH}" -o /tmp/dockpanel-cli-new
    verify_checksum /tmp/dockpanel-cli-new "dockpanel-cli-linux-${DL_ARCH}"
    chmod +x /tmp/dockpanel-cli-new

    # Download and extract frontend
    log "Downloading frontend..."
    curl -sfL "${BASE_URL}/dockpanel-frontend.tar.gz" -o /tmp/dockpanel-frontend.tar.gz
    verify_checksum /tmp/dockpanel-frontend.tar.gz "dockpanel-frontend.tar.gz"
    FE_DIR="/opt/dockpanel/frontend"
    mkdir -p "$FE_DIR"
    tar xzf /tmp/dockpanel-frontend.tar.gz -C "$FE_DIR"
    rm -f /tmp/dockpanel-frontend.tar.gz
    rm -f "$CHECKSUMS"
    log "Frontend updated"
else
    # Build from source
    # Detect Rust toolchain
    if command -v cargo &> /dev/null; then
        CARGO_CMD="cargo"
    elif [ -f "$HOME/.cargo/bin/cargo" ]; then
        CARGO_CMD="$HOME/.cargo/bin/cargo"
    else
        error "Rust toolchain not found, but BUILD_FROM_SOURCE=1 was requested."
        error "Recommended: drop BUILD_FROM_SOURCE=1 — update.sh will auto-fetch pre-built binaries."
        error "If you really want to compile from source: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
        error "(note: building from source needs ~4 GB RAM — most production VPSes won't have it)"
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
        # `npm ci` installs EXACTLY the committed package-lock.json — the tree the
        # audit gates actually scanned. `npm install` re-resolves and can pull
        # versions no scanner here has ever seen, which silently un-ships a
        # dependency patch. Keep the fallback (a box whose lock has drifted should
        # still update) but never let it happen quietly, and never discard the
        # reason: sending both streams to /dev/null is how the RHEL agent install
        # failed for a year printing nothing at all.
        if ! (cd "$FRONTEND_DIR" && npm ci --silent); then
            warn "npm ci failed (reason above) — falling back to 'npm install'"
            warn "Dependencies will be RE-RESOLVED; the tree may differ from the audited lockfile"
            if ! (cd "$FRONTEND_DIR" && npm install --silent); then
                error "npm install also failed — the frontend build below will likely fail too"
            fi
        fi
        (cd "$FRONTEND_DIR" && npx vite build 2>&1 | tail -3)
        log "Frontend rebuilt"
    fi
fi

# ── Ensure required directories exist (may be new in this version) ────────
_dockpanel_stage="directories"
log "Ensuring required directories exist..."
mkdir -p /etc/dockpanel/ssl /var/run/dockpanel /var/backups/dockpanel
mkdir -p /var/www/acme/.well-known/acme-challenge
mkdir -p /var/lib/dockpanel/git
# Directories needed by agent ReadWritePaths (created only if missing), DERIVED
# FROM THE UNIT ABOUT TO BE DEPLOYED rather than hand-copied.
#
# systemd fails the namespace mount on a missing UNPREFIXED entry, so the agent
# would not start at all — loud. The dangerous half is the `-`-prefixed entries,
# which mean "bind IF IT EXISTS": when absent, systemd skips them silently, the
# unit starts and reports success, and every write beneath that path fails with
# `Read-only file system` until the next restart. The namespace is fixed at
# start, so creating the directory later does not rescue a running agent.
#
# This ran BEFORE the unit is refreshed and reloaded below, which is what makes
# the ordering work: the directories exist by the time the agent restarts.
#
# It used to be a literal list, hand-mirrored from setup.sh, and it drifted —
# /var/spool/cron reached the unit and setup.sh at s268 but never this loop, so
# an upgraded box with no cron spool got a silently unwritable one and the cron
# fix s268 shipped "so existing installs recover on upgrade" did not fully land
# on exactly those installs. Deriving the list closes the class, not the case.
for d in $(agent_rwp_dirs); do
    [ -d "$d" ] || mkdir -p "$d" 2>/dev/null || true
done
# Paths the agent reaches through an escape hatch, or that are not RWP entries.
for d in /run/opendkim /var/lib/nginx /var/cache/nginx/fastcgi; do
    [ -d "$d" ] || mkdir -p "$d" 2>/dev/null || true
done
echo "d /run/dockpanel 0755 root root -" > /etc/tmpfiles.d/dockpanel.conf 2>/dev/null || true

# v2.8.17: drop apt lock-wait config so agent's apt-get install/update/purge
# waits up to 5 min for the dpkg lock instead of failing immediately when
# unattended-upgrades is running in the background (common on fresh Debian).
# Idempotent — overwrites on every update.sh run, no apt operation needed.
if command -v apt-get &> /dev/null; then
    mkdir -p /etc/apt/apt.conf.d
    cat > /etc/apt/apt.conf.d/99-dockpanel-lock-wait.conf << 'APT_EOF'
DPkg::Lock::Timeout "300";
APT_EOF

    # v2.48.1: exclude the agent from needrestart's auto-restart list. A
    # panel-driven update runs apt from inside dockpanel-agent; because the
    # agent links libc, an ordinary libc6 upgrade made needrestart restart
    # dockpanel-agent.service mid-run and kill the process streaming the
    # update's own progress. apt itself survived (it runs in a systemd-run
    # transient scope), but the NDJSON stream died before its final
    # {"type":"done"} line, so the panel reported a clean 16-package upgrade
    # as "Update completed with errors" (measured on this box, s286).
    # Everything else still restarts — only the reporting channel is spared.
    if [ -d /etc/needrestart ]; then
        mkdir -p /etc/needrestart/conf.d
        # Subscript assignment ADDS to the hash the main needrestart.conf
        # already filled in; `$nrconf{override_rc} = {...}` would replace it
        # and drop the distro's own exclusions.
        cat > /etc/needrestart/conf.d/99-dockpanel.conf << 'NR_EOF'
# Managed by DockPanel — do not edit; rewritten by setup.sh/update.sh.
$nrconf{override_rc}{qr(^dockpanel-agent)} = 0;
1;
NR_EOF
    fi

    # v2.8.19: pre-v2.8.19 cloudflared installs wrote a literal
    # `$(lsb_release -cs)` into /etc/apt/sources.list.d/cloudflared.list
    # (single-quoted bash didn't expand the substitution). Once landed, the
    # broken file made every subsequent `apt-get update` on the box fail —
    # blocking unrelated installs (Redis, WAF, …). Repair it here so
    # operators upgrading via update.sh get an unblocked apt without manual
    # intervention.
    if [ -f /etc/apt/sources.list.d/cloudflared.list ] && \
       grep -qF '$(lsb_release' /etc/apt/sources.list.d/cloudflared.list; then
        log "Removing broken cloudflared apt source (literal \$(lsb_release ...) — fixed in v2.8.19)"
        rm -f /etc/apt/sources.list.d/cloudflared.list
    fi
fi

# ── v2.38.0: heal RHEL-family installs the old installer left unreachable ──
# Every install on Rocky/Alma/CentOS/Fedora before v2.38.0 finished in one of
# two broken states, and an operator cannot click their way out of either
# because both make the panel unreachable — so the repair has to happen here.
#
#   1. setup.sh installed UFW next to the firewalld the distro was already
#      running, then opened 80/443 in UFW only. firewalld kept dropping them.
#   2. SELinux (Enforcing by default) blocks nginx from opening a socket to
#      the API, so every request answered 502 — even from the box itself. The
#      denial is dontaudit'ed, so nothing shows up in the journal or ausearch.
#
# Both repairs are idempotent and no-ops on Debian/Ubuntu.
if command -v firewall-cmd &> /dev/null && firewall-cmd --state &> /dev/null; then
    fw_changed=0
    for svc in http https; do
        if ! firewall-cmd --query-service="$svc" &> /dev/null; then
            firewall-cmd --permanent --add-service="$svc" &> /dev/null && fw_changed=1
        fi
    done
    if [ "$fw_changed" = "1" ]; then
        firewall-cmd --reload &> /dev/null || true
        log "firewalld: opened 80/443 (this box was serving behind a closed firewall)"
    fi
fi

if command -v getenforce &> /dev/null && [ "$(getenforce 2>/dev/null)" = "Enforcing" ] \
   && command -v getsebool &> /dev/null; then
    if getsebool httpd_can_network_connect 2>/dev/null | grep -q -- "--> off"; then
        if setsebool -P httpd_can_network_connect on 2>/dev/null; then
            log "SELinux: allowed nginx to reach the API (the panel was answering 502)"
        else
            warn "SELinux is Enforcing and httpd_can_network_connect is off — the panel will"
            warn "answer 502. Run: setsebool -P httpd_can_network_connect on"
        fi
    fi
fi

# ── Refresh systemd service files (may have changed between versions) ─────
_dockpanel_stage="systemd-units"
log "Updating systemd service files..."
# Agent unit — deploy from repo (single source of truth: panel/agent/dockpanel-agent.service)
# v2.8.13: existing installs upgrading from v2.8.12 or earlier get the strict sandbox here.
if [ -f "$AGENT_SRC/dockpanel-agent.service" ]; then
    cp "$AGENT_SRC/dockpanel-agent.service" /etc/systemd/system/dockpanel-agent.service
    chmod 644 /etc/systemd/system/dockpanel-agent.service
else
    warn "Agent unit source not found at $AGENT_SRC/dockpanel-agent.service — keeping existing on-disk unit (no repo tree on this layout)"
fi

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

# ── Update nginx frontend path if needed ──────────────────────────────────
if [ "$INSTALL_FROM_RELEASE" = "1" ]; then
    FE_DIST="/opt/dockpanel/frontend/dist"
    for conf in /etc/nginx/sites-enabled/dockpanel-panel.conf /etc/nginx/conf.d/dockpanel-panel.conf; do
        if [ -f "$conf" ] && grep -q "panel/frontend/dist" "$conf" 2>/dev/null; then
            sed -i "s|/opt/dockpanel/panel/frontend/dist|${FE_DIST}|g" "$conf"
            log "Updated nginx frontend path in $conf"
            nginx -t > /dev/null 2>&1 && nginx -s reload > /dev/null 2>&1
        fi
    done
else
    FE_DIST="${REPO_DIR}/panel/frontend/dist"
fi

# ── Drop install-agent.sh into FE_ROOT (#56, v2.8.14) ─────────────────────
# Panel SPA-fallback nginx serves $uri before falling back to index.html.
# Without the script present in FE_ROOT, `curl {panel}/install-agent.sh | bash`
# returns the SPA HTML and fails with 'syntax error near unexpected token'.
if [ -f "${REPO_DIR}/scripts/install-agent.sh" ] && [ -d "$FE_DIST" ]; then
    cp "${REPO_DIR}/scripts/install-agent.sh" "$FE_DIST/install-agent.sh"
    chmod 644 "$FE_DIST/install-agent.sh"
    log "Refreshed install-agent.sh in $FE_DIST"
fi

# ── Migrate panel nginx config to bind IPv6 (fixes site-vhost dual-stack hijack) ──
_dockpanel_stage="nginx-migrations"
# v2.8.3: agent site templates declare `listen [::]:443 ssl;` (dual-stack), but
# pre-v2.8.3 setup.sh bound the panel to IPv4 only. The first SSL site to be
# provisioned then became the de-facto default for IPv6 traffic — WordPress
# would 301 panel-domain requests to its canonical home_url. Fix: ensure the
# panel vhost also has `listen [::]:80;` and `listen [::]:443 ssl;`. Plain
# (no `ipv6only=on`) so the panel listens dual-stack on the same socket as
# every site, and nginx routes by server_name across the shared listener.
# Strip any `ipv6only=on` certbot may have added on prior installs to keep
# the listener options consistent across vhosts (otherwise nginx errors with
# "duplicate listen options" when sites add their own [::]:443 ssl block).
NGINX_NEEDS_RELOAD=0
NGINX_NEEDS_RESTART=0
for conf in /etc/nginx/sites-enabled/dockpanel-panel.conf /etc/nginx/conf.d/dockpanel-panel.conf; do
    [ -f "$conf" ] || continue
    # certbot --nginx writes a WILDCARD `listen 443 ssl;`, while agent-generated
    # site vhosts bind `<ip>:443 ssl`. nginx treats those as different listen
    # sockets, the explicit-IP one wins every connection to that address, and the
    # panel's server_name is never consulted — so on a domain install the first
    # site to get a certificate silently takes over the panel's own hostname.
    # The IPv6 pairing step below cannot repair this shape: its rewrite needs a
    # colon before the port, so it slides straight past a wildcard `listen 443
    # ssl;`. (Its grep does fire — but on the [::]:443 line — which is why the
    # wildcard survived unnoticed.)
    if grep -qE '^[[:space:]]*listen 443 ssl;' "$conf"; then
        PANEL_BIND_IP=$(ip route get 8.8.8.8 2>/dev/null | grep -oP 'src \K\S+' || true)
        if [ -n "$PANEL_BIND_IP" ]; then
            sed -i -E "s|^([[:space:]]*)listen 443 ssl;|\1listen ${PANEL_BIND_IP}:443 ssl;|" "$conf"
            log "Pinned panel :443 listen to ${PANEL_BIND_IP} in $conf (was wildcard; site vhosts could shadow the panel)"
            # A reload cannot move an already-bound 0.0.0.0:443 listener to a
            # specific address — nginx inherits the old socket and the rewrite
            # silently no-ops. This one needs a restart.
            NGINX_NEEDS_RESTART=1
        fi
    fi
    if ! grep -qE 'listen \[::\]:80' "$conf"; then
        sed -i -E '0,/^([[:space:]]*)listen ([^;]*):80;[[:space:]]*$/{s||\1listen \2:80;\n\1listen [::]:80;|}' "$conf"
        sed -i -E '0,/^([[:space:]]*)listen 80;[[:space:]]*$/{s||\1listen 80;\n\1listen [::]:80;|}' "$conf"
        log "Added IPv6 :80 listen to $conf"
        NGINX_NEEDS_RELOAD=1
    fi
    if grep -qE 'listen [^;]*:443 ssl' "$conf" && ! grep -qE 'listen \[::\]:443' "$conf"; then
        sed -i -E '0,/^([[:space:]]*)listen ([^;]*):443 ssl;[[:space:]]*$/{s||\1listen \2:443 ssl;\n\1listen [::]:443 ssl;|}' "$conf"
        log "Added IPv6 :443 ssl listen to $conf"
        NGINX_NEEDS_RELOAD=1
    fi
    # Strip `ipv6only=on` from panel listens if previously added — site vhosts
    # use plain `[::]:443 ssl;` and nginx rejects mixing the two on a shared socket.
    if grep -qE 'listen \[::\]:(80|443 ssl) ipv6only=on' "$conf"; then
        sed -i -E 's|^([[:space:]]*)listen \[::\]:80 ipv6only=on;|\1listen [::]:80;|' "$conf"
        sed -i -E 's|^([[:space:]]*)listen \[::\]:443 ssl ipv6only=on;|\1listen [::]:443 ssl;|' "$conf"
        log "Stripped ipv6only=on from $conf for shared-socket compatibility"
        NGINX_NEEDS_RELOAD=1
    fi
done
# Strip `ipv6only=on` from site vhosts left over from v2.8.3.
# v2.8.3 baked the option into agent templates AND added it via update.sh;
# v2.8.4 reverted the template but dropped this site-vhost cleanup. Result:
# v2.8.3-installed sites kept `[::]:443 ssl ipv6only=on` while the panel vhost
# (cleaned by the loop above) used plain `[::]:443 ssl` — nginx rejects the
# mix as "duplicate listen options" on the shared socket. Bringing them back
# in line restores reload-ability without touching site config in any other way.
if [ -d /etc/nginx/sites-enabled ]; then
    for site_conf in /etc/nginx/sites-enabled/*.conf; do
        [ -f "$site_conf" ] || continue
        case "$(basename "$site_conf")" in
            dockpanel-panel.conf) continue ;;
        esac
        if grep -qE 'listen \[::\]:(80|443 ssl) ipv6only=on' "$site_conf"; then
            sed -i -E 's|^([[:space:]]*)listen \[::\]:80 ipv6only=on;|\1listen [::]:80;|' "$site_conf"
            sed -i -E 's|^([[:space:]]*)listen \[::\]:443 ssl ipv6only=on;|\1listen [::]:443 ssl;|' "$site_conf"
            log "Stripped ipv6only=on from $site_conf for shared-socket compatibility"
            NGINX_NEEDS_RELOAD=1
        fi
    done
fi

# ── v2.68.1: retrofit the static branch's denies onto EXISTING site vhosts ───
# v2.68.0 fixed the TEMPLATE, and a template only reaches a site when something
# re-renders its vhost — a runtime switch, an SSL issuance, a settings change.
# Nothing re-renders on upgrade. So the site that most needs this fix, one already
# switched from php to static under v2.67.0, keeps serving wp-config.php, .env and
# .git/config as plain text after the operator has updated and been told it is
# fixed. Same shape as the v2.47.1 index.html migration below: a fix that reaches
# only new renders is a fix nobody in the field receives.
#
# Scoped by what the vhost IS, not by what we guess it was: a static vhost has the
# static branch's `index index.html index.htm;` and no fastcgi handler. A php vhost
# already carries these denies inside its preset branch and is skipped. Additive,
# idempotent (guarded on the deny already being present), and every edit is
# validated by `nginx -t` with a per-file rollback — a security retrofit must never
# be the thing that takes a customer's site down.
if [ -d /etc/nginx/sites-enabled ]; then
    for site_conf in /etc/nginx/sites-enabled/*.conf; do
        [ -f "$site_conf" ] || continue
        case "$(basename "$site_conf")" in
            dockpanel-panel.conf) continue ;;
        esac
        grep -q 'index index.html index.htm;' "$site_conf" || continue
        grep -q 'fastcgi_pass' "$site_conf" && continue
        grep -qF 'location ~ /\.(?!well-known)' "$site_conf" && continue

        cp -p "$site_conf" "$site_conf.pre-2680" 2>/dev/null || true
        if awk '
            { print }
            /^[[:space:]]*index index\.html index\.htm;[[:space:]]*$/ {
                print "";
                print "    # v2.68.1: a site switched from php to static keeps its application";
                print "    # source in the same docroot. Both denies exist in every php preset.";
                print "    location ~ /\\.(?!well-known) {";
                print "        deny all;";
                print "    }";
                print "";
                print "    location ~ \\.php$ {";
                print "        deny all;";
                print "    }";
            }
        ' "$site_conf.pre-2680" > "$site_conf.new" 2>/dev/null && [ -s "$site_conf.new" ]; then
            mv "$site_conf.new" "$site_conf"
            if nginx -t > /dev/null 2>&1; then
                rm -f "$site_conf.pre-2680"
                log "Retrofitted dotfile/.php denies onto static vhost $site_conf"
                NGINX_NEEDS_RELOAD=1
            else
                mv "$site_conf.pre-2680" "$site_conf"
                log "WARN: denies rejected by nginx -t for $site_conf — reverted, site untouched"
            fi
        else
            rm -f "$site_conf.new" "$site_conf.pre-2680"
            log "WARN: could not retrofit denies onto $site_conf — skipped"
        fi
    done
fi
# ── v2.8.22: ensure panel vhost includes dockpanel-panel.locations/*.conf ─
# Drop-in dir for path-mounted tool reverse-proxies. Webmail uses this in
# v2.8.22+ (writes webmail.conf on install, deletes on remove). Pre-v2.8.22
# panel vhosts lack the include directive — inject it just before the
# server-block closing brace via awk (sed nested-brace handling is fragile).
mkdir -p /etc/nginx/conf.d/dockpanel-panel.locations
for conf in /etc/nginx/sites-enabled/dockpanel-panel.conf /etc/nginx/conf.d/dockpanel-panel.conf; do
    [ -f "$conf" ] || continue
    if ! grep -q "dockpanel-panel.locations" "$conf"; then
        # Inject before the FIRST top-level `}` (the server block close).
        # Panel vhost has exactly one top-level `}`, so single-match is safe.
        if awk '
            /^}/ && !done {
                print "    # Drop-in location blocks for path-mounted tools (webmail, etc.)";
                print "    include /etc/nginx/conf.d/dockpanel-panel.locations/*.conf;";
                print "";
                done=1
            }
            { print }
        ' "$conf" > "$conf.new" && [ -s "$conf.new" ]; then
            mv "$conf.new" "$conf"
            log "Added dockpanel-panel.locations include to $conf"
            NGINX_NEEDS_RELOAD=1
        else
            rm -f "$conf.new"
            log "WARN: failed to inject panel-locations include into $conf — skipped"
        fi
    fi
done

# ── v2.47.1: give index.html a cache directive on EXISTING panel vhosts ───
# setup.sh started writing `location = /index.html { Cache-Control: no-cache }`
# in v2.47.1, but setup.sh only ever runs at INSTALL time and this script never
# re-runs it. Without a migration the fix therefore reaches new boxes only, and
# every panel already in the field keeps serving index.html with no cache
# directive at all: the browser falls back to HEURISTIC freshness — roughly a
# tenth of the file's age, so days on a panel that has been up a month — and in
# that window keeps naming the PREVIOUS hashed bundle, which is still on disk
# because the frontend untars OVER the directory rather than replacing it.
# Nothing 404s. The operator updates, is told it worked, and goes on running the
# old frontend against the new backend. That is s270's delivery class: a fix
# written into the install-time template only is a fix nobody who already
# installed will ever receive.
#
# The repeated add_headers are copied FROM THE VHOST BEING MIGRATED, not from
# this script. A location block's add_headers REPLACE the server block's set
# rather than merging with it, so the block has to re-state whatever that box
# already sends — otherwise this migration would strip the CSP off the one
# response that carries it, a worse bug than the one being fixed. Copying the
# box's own set (rather than today's) also keeps an old vhost from being
# silently given a different CSP on a single response. If no server-level
# add_header is found the migration is skipped rather than guessed at.
for conf in /etc/nginx/sites-enabled/dockpanel-panel.conf /etc/nginx/conf.d/dockpanel-panel.conf; do
    [ -f "$conf" ] || continue
    grep -q "location = /index.html" "$conf" && continue
    # Server-level headers are indented 4; location-level ones 8, so an exact
    # four-space prefix selects the parent set without matching nested blocks.
    if ! grep -qE '^    add_header ' "$conf"; then
        log "WARN: $conf has no server-level add_header — skipped index.html cache migration (injecting it would strip the security headers)"
        continue
    fi
    # Inject before the FIRST top-level `}`. The panel vhost has exactly one
    # (verified: one `server {` at column 0), the same assumption the
    # panel-locations migration above already relies on.
    if awk '
        /^    add_header / { hdr[++n] = $0 }
        /^}/ && !done {
            print "    # Without a cache directive the browser applies heuristic freshness and";
            print "    # serves a stale index.html, naming an older hashed bundle that is still";
            print "    # on disk — so an updated panel goes on running the previous frontend.";
            print "    # Every add_header here is a REPEAT: a location block replaces the";
            print "    # server block'"'"'s set rather than merging with it.";
            print "    location = /index.html {";
            print "        add_header Cache-Control \"no-cache\" always;";
            for (i = 1; i <= n; i++) { sub(/^    /, "        ", hdr[i]); print hdr[i] }
            print "    }";
            print "";
            done = 1
        }
        { print }
    ' "$conf" > "$conf.new" && [ -s "$conf.new" ] && grep -q "location = /index.html" "$conf.new"; then
        mv "$conf.new" "$conf"
        log "Added index.html cache directive to $conf"
        NGINX_NEEDS_RELOAD=1
    else
        rm -f "$conf.new"
        log "WARN: failed to inject index.html cache block into $conf — skipped"
    fi
done

# ── v2.47.2: repeat the security headers on /assets/ on EXISTING panel vhosts ──
# Same delivery class as the migration above, and this session's own lesson
# applied to this session's own change: setup.sh started repeating the server
# header set inside `location /assets/` in v2.47.2, and setup.sh runs only at
# install time. Without a migration, every panel already in the field goes on
# serving its JS and CSS bundles with NO X-Content-Type-Options — the one header
# that actually matters for a script response, missing from the only responses
# that are scripts — because a location block's add_headers REPLACE the server
# block's set rather than merging with it, and this block set only Cache-Control.
#
# It also emitted two Cache-Control lines saying different things (max-age from
# `expires 1y`, `public, immutable` from the add_header). One directive replaces
# both. Deliberately NOT `always` on it: an error response must not be cached,
# or a 404 during an update is remembered for a year.
#
# As above, the repeated headers are copied FROM THE VHOST BEING MIGRATED rather
# than written from this script, so an older install keeps its own policy instead
# of being silently handed today's on one class of response; and a vhost with no
# server-level add_header is skipped rather than guessed at.
#
# Two passes over the file (awk NR==FNR), unlike the single pass above: the
# server-level add_headers are written AFTER the /assets/ block, so they are not
# yet known at the point the block has to be rewritten.
for conf in /etc/nginx/sites-enabled/dockpanel-panel.conf /etc/nginx/conf.d/dockpanel-panel.conf; do
    [ -f "$conf" ] || continue
    grep -q "location /assets/" "$conf" || continue
    grep -q 'max-age=31536000, immutable' "$conf" && continue
    if ! grep -qE '^    add_header ' "$conf"; then
        log "WARN: $conf has no server-level add_header — skipped /assets/ header migration (rewriting it would strip the security headers)"
        continue
    fi
    if awk '
        NR == FNR { if (/^    add_header /) hdr[++n] = $0; next }
        /^    location \/assets\/ \{/ && !done {
            print "    # Hashed filenames, so a year is safe. ONE Cache-Control: the expires";
            print "    # directive emitted its own on top of the add_header and the two went out";
            print "    # as separate lines saying different things. Not \"always\" — an error";
            print "    # response must not be cached, or a 404 mid-update is kept for a year.";
            print "    # Every add_header below REPEATS this vhost'"'"'s own server-level set: a";
            print "    # location block replaces that set rather than merging with it, so without";
            print "    # them every script this panel serves goes out with no nosniff.";
            print "    location /assets/ {";
            print "        add_header Cache-Control \"public, max-age=31536000, immutable\";";
            for (i = 1; i <= n; i++) { h = hdr[i]; sub(/^    /, "        ", h); print h }
            print "    }";
            skip = 1; done = 1; next
        }
        skip && /^    \}/ { skip = 0; next }
        skip { next }
        { print }
    ' "$conf" "$conf" > "$conf.new" && [ -s "$conf.new" ] \
        && grep -q 'max-age=31536000, immutable' "$conf.new" \
        && ! awk '/^    location \/assets\/ \{/ {f=1} f {print} f && /^    \}/ {exit}' \
             "$conf.new" | grep -q 'expires '; then
        mv "$conf.new" "$conf"
        log "Repeated the security headers on /assets/ in $conf"
        NGINX_NEEDS_RELOAD=1
    else
        rm -f "$conf.new"
        log "WARN: failed to rewrite the /assets/ block in $conf — skipped"
    fi
done

if [ "$NGINX_NEEDS_RELOAD" = "1" ] || [ "$NGINX_NEEDS_RESTART" = "1" ]; then
    if nginx -t > /dev/null 2>&1; then
        if [ "$NGINX_NEEDS_RESTART" = "1" ]; then
            systemctl restart nginx > /dev/null 2>&1 && log "Nginx restarted after panel :443 listen migration"
            # Verify the socket actually moved — the whole point of restarting.
            if command -v ss > /dev/null 2>&1 && [ -n "${PANEL_BIND_IP:-}" ] \
               && ! ss -ltn "( sport = :443 )" 2>/dev/null | grep -q "${PANEL_BIND_IP}:443"; then
                log "WARN: panel is still not listening on ${PANEL_BIND_IP}:443 — a site vhost may shadow it"
            fi
        else
            nginx -s reload > /dev/null 2>&1 && log "Nginx reloaded after config migrations"
        fi
    else
        log "WARN: nginx -t failed after config migrations; not reloading. Check sites-enabled/."
    fi
fi

# ── webmail nginx fragment: healed by the AGENT, not here ─────────
# This used to hold a hand-copied mirror of the fragment and rewrite it when
# `sub_filter` was missing. The mirror froze at the v2.10.1 shape and never
# learned the header set v2.36.0 added, so its "heal" produced a fragment whose
# inherited `frame-ancestors 'none'` makes Roundcube render an empty inbox —
# a repair that wrote the defect. The agent now owns the template outright
# (`routes::mail::{webmail_nginx_block, heal_webmail_nginx}`) and reconciles the
# file at startup, which this script triggers when it restarts the agent.
# Do not reintroduce a copy of the fragment here; one writer is the fix.

# Ensure BASE_URL is set in api.env for CORS.
# The test MUST stay anchored to the start of the line. An unanchored substring
# search also hits the DATABASE_URL= line that setup.sh writes first into every
# api.env (that key ends with the same eight characters), so before v2.31.2 this
# repair was skipped on every install that has ever existed. Require a real key
# with a non-empty value — setup.sh writes a bare, valueless key on domainless
# installs, and that counts as unset here.
if [ -f /etc/dockpanel/api.env ] && ! grep -qE '^BASE_URL=.+' /etc/dockpanel/api.env; then
    # Detect panel URL from nginx config
    PANEL_DOMAIN=""
    for conf in /etc/nginx/sites-enabled/dockpanel-panel.conf /etc/nginx/conf.d/dockpanel-panel.conf; do
        if [ -f "$conf" ]; then
            PANEL_DOMAIN=$(grep "server_name" "$conf" | head -1 | awk '{print $2}' | tr -d ';')
            break
        fi
    done
    if [ -n "$PANEL_DOMAIN" ] && [ "$PANEL_DOMAIN" != "_" ]; then
        if grep -qE '^BASE_URL=' /etc/dockpanel/api.env; then
            # A bare `BASE_URL=` is already there (domainless install later moved
            # onto a domain). Rewrite it in place — appending a second key would
            # leave two BASE_URL lines and make the effective value depend on
            # which one the env parser happens to keep.
            sed -i -E "s|^BASE_URL=.*|BASE_URL=https://${PANEL_DOMAIN}|" /etc/dockpanel/api.env
            log "Set empty BASE_URL to https://${PANEL_DOMAIN} in api.env"
        else
            echo "BASE_URL=https://${PANEL_DOMAIN}" >> /etc/dockpanel/api.env
            log "Added BASE_URL=https://${PANEL_DOMAIN} to api.env"
        fi
    fi
fi

# ── Deploy binaries ───────────────────────────────────────────────────────
_dockpanel_stage="deploy"
# Note: ~2-5s downtime during binary swap is expected for self-hosted deployments.
log "Backing up current binaries..."
cp "$AGENT_BIN" "${AGENT_BIN}.bak" 2>/dev/null || true
cp "$API_BIN" "${API_BIN}.bak" 2>/dev/null || true
cp "$CLI_BIN" "${CLI_BIN}.bak" 2>/dev/null || true

log "Stopping services..."
_dockpanel_services_stopped=1
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

# SELinux: restore the binaries' security context after the swap.
#
# A rename within one filesystem PRESERVES the source label, and the release
# path above moves these in from /tmp — so each binary arrives carrying
# `user_tmp_t` instead of `bin_t`, and on an Enforcing box systemd then refuses
# to execute it:
#
#     Failed at step EXEC spawning /usr/local/bin/dockpanel-agent: Permission denied
#     dockpanel-agent.service: Main process exited, code=exited, status=203/EXEC
#
# Measured on Rocky 9.8 at s266. The failure is silent in the worst way: the
# update reports success, and the service is dead until someone relabels it by
# hand. Note the `cp` branch does NOT have this problem — copying ONTO an
# existing file keeps the target's label — so this only ever bit the
# release-binary path, which is the one every real install uses.
#
# No-op on Debian/Ubuntu, where restorecon is usually absent.
if command -v restorecon > /dev/null 2>&1; then
    restorecon -F "$AGENT_BIN" "$API_BIN" "$CLI_BIN" 2>/dev/null || true
fi

log "Binaries updated (agent: $(du -h "$AGENT_BIN" | cut -f1), api: $(du -h "$API_BIN" | cut -f1), cli: $(du -h "$CLI_BIN" | cut -f1))"

systemctl daemon-reload
systemctl start dockpanel-agent
sleep 1
systemctl start dockpanel-api
_dockpanel_services_stopped=0
log "Services restarted"

# ── Health check with rollback ────────────────────────────────────────────
rollback() {
    error "Health check failed, rolling back..."

    # Restore with `mv`, not `cp`, and stop the services first.
    #
    # This path had never executed in production. `cp` writes into the existing
    # inode, so restoring over the RUNNING dockpanel-api/dockpanel-agent failed
    # ETXTBSY ("Text file busy") — and because each cp was suffixed
    # `2>/dev/null || true`, the failure was discarded and the script printed
    # "Rolled back to previous binaries" while the box kept running the binary
    # that had just failed its health check. Only the CLI (not running) was
    # actually restored, so the box then disagreed with itself:
    # `dockpanel --version` reported the old version while /api/health reported
    # the new one. rename(2) replaces the directory entry and is unaffected by a
    # busy inode — it is what the forward swap above already uses.
    _dockpanel_services_stopped=1
    systemctl stop dockpanel-agent dockpanel-api 2>/dev/null || true

    local restore_failed=0
    local pair
    for pair in "$AGENT_BIN" "$API_BIN" "$CLI_BIN"; do
        if [ -f "${pair}.bak" ]; then
            if mv "${pair}.bak" "$pair"; then
                log "Restored $pair"
            else
                error "FAILED to restore $pair from ${pair}.bak"
                restore_failed=1
            fi
        else
            error "No backup at ${pair}.bak — cannot restore $pair"
            restore_failed=1
        fi
    done

    # Tolerate a non-zero start here rather than letting `set -e` abort the
    # function before the diagnosis below is printed — the EXIT trap is a safety
    # net for the process dying, not a substitute for telling the operator what
    # happened.
    systemctl daemon-reload
    systemctl start dockpanel-agent || error "dockpanel-agent did not start after rollback"
    sleep 1
    systemctl start dockpanel-api || error "dockpanel-api did not start after rollback"
    _dockpanel_services_stopped=0

    if [ "$restore_failed" = "1" ]; then
        error "ROLLBACK INCOMPLETE — the panel may still be running the failed build."
        error "Inspect /usr/local/bin/dockpanel-{api,agent} and restore manually."
        _dockpanel_finished=1
        _dockpanel_write_result false "rollback" \
            "health check failed after the swap AND the rollback did not complete — the panel may still be running the failed build; inspect /usr/local/bin/dockpanel-{api,agent}"
        exit 1
    fi

    warn "Rolled back to previous binaries"
    # A completed rollback is a DIFFERENT outcome from a failed one, and the
    # orchestrator has to be able to tell them apart: this box is healthy and
    # running its previous version, which is not something that needs an
    # operator at 3am.
    _dockpanel_finished=1
    _dockpanel_write_result false "rollback" \
        "health check failed after the swap; rolled back to the previous binaries and the panel is running again"
    exit 1
}

log "Running post-deploy health check..."
sleep 20

# Basic health endpoint
if ! curl -sf --max-time 30 http://127.0.0.1:3080/api/health > /dev/null 2>&1; then
    rollback
fi
log "Health check: /api/health OK"

# Auth subsystem (setup-status is unauthenticated, tests DB connectivity).
# Note: this endpoint is GET-only — using POST returns 405 and triggered an
# unconditional rollback on every update before this fix.
if ! curl -sf --max-time 30 http://127.0.0.1:3080/api/auth/setup-status > /dev/null 2>&1; then
    rollback
fi
log "Health check: /api/auth/setup-status OK"

# Agent reachable (non-fatal — agent may start slower).
#
# Ask the AGENT, not the panel. `/api/system/info` is behind auth, and this
# check sent no token — so it returned 401, `curl -sf` treated that as failure,
# and every update on every install printed "Agent connectivity check failed"
# whether the agent was healthy or dead. A check that fails identically in both
# states carries no signal; it only taught operators to ignore the warning.
# The agent's own /health is explicitly exempt from auth
# (panel/agent/src/routes/mod.rs), which makes it the honest probe.
AGENT_SOCK=/run/dockpanel/agent.sock
[ -S "$AGENT_SOCK" ] || AGENT_SOCK=/var/run/dockpanel/agent.sock
if ! curl -sf --max-time 30 --unix-socket "$AGENT_SOCK" http://localhost/health > /dev/null 2>&1; then
    warn "Agent connectivity check failed (non-fatal, agent may still be starting)"
else
    log "Health check: agent /health OK"
fi

# CLI health check (non-fatal)
if ! dockpanel --version > /dev/null 2>&1; then
    warn "CLI health check failed (non-fatal)"
fi

log "Health checks passed"
_dockpanel_stage="complete"
_dockpanel_finished=1
_dockpanel_write_result true "complete" \
    "updated to ${DOCKPANEL_VERSION:-latest} and passed the post-deploy health checks"

# Clean up backups
rm -f "${AGENT_BIN}.bak" "${API_BIN}.bak" "${CLI_BIN}.bak"

echo ""
echo -e "${GREEN}${BOLD}Update complete!${NC}"
echo ""
echo -e "  Agent: $(systemctl is-active dockpanel-agent)"
echo -e "  API:   $(systemctl is-active dockpanel-api)"
echo -e "  Version: $($CLI_BIN --version 2>/dev/null || echo 'unknown')"
echo ""
