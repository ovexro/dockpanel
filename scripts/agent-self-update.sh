#!/usr/bin/env bash
#
# DockPanel — update the AGENT binary on a remote (agent-only) server.
#
# This is what a fleet rolling update actually runs on each member box. It is
# compiled into dockpanel-agent (`include_str!`) and written to
# /var/lib/dockpanel/agent-self-update.sh at update time, so it cannot drift
# from the binary that invokes it and needs nothing from /opt/dockpanel
# (lessons #35/#38 — a repo-side file no installer deploys is dev fiction).
#
# WHY THIS EXISTS AT ALL — measured on a two-box lab, s232:
#   The agent used to shell out to /opt/dockpanel/scripts/update.sh. Two things
#   were wrong with that, and the second is why the first could not simply be
#   "fixed" by shipping the repo to agent boxes:
#
#   1. `scripts/install-agent.sh` never creates /opt/dockpanel. On a real
#      agent-only box `/opt` holds nothing but containerd. So every fleet update
#      died instantly with `500: update script not found at
#      /opt/dockpanel/scripts/update.sh` — and install-agent.sh is the only
#      documented way to add a remote server, so the feature's success rate on
#      its entire target population was zero.
#
#   2. update.sh is the PANEL updater. It syncs a git repo, dumps a postgres
#      container, replaces the API and the frontend, rewrites nginx. An
#      agent-only box has none of that. Planting the repo and re-running the
#      fleet update produced, on the lab: `Error response from daemon: No such
#      container: dockpanel-postgres` → `[x] Database backup failed, aborting
#      upgrade` → exit 1, one second after the panel had already recorded the
#      server as **succeeded**. Fixing only (1) would have turned a loud, safe
#      500 into a silent false success on every box in the fleet — lesson #50.
#
#   An agent-only box needs an agent-only update: fetch one binary, verify it,
#   swap it, restart the unit, and prove the running version changed.
#
# WHY A PID1-OWNED TRANSIENT UNIT:
#   This script restarts dockpanel-agent.service, and that unit is
#   KillMode=control-group (systemd's default). Anything left in the unit's
#   cgroup — including the process doing the restarting — is SIGTERMed. The
#   agent launches this via `systemd-run --collect --unit=`, and the guard below
#   is the belt-and-braces for a manual invocation. It must be a transient
#   SERVICE, owned by PID1. The session-scoped alternative is not a substitute:
#   that kind of unit is created in the caller's context and dies with the
#   invoking session, which was observed killing an update mid-swap (lesson #47).
#   A test pins that, by grepping this file for the flag that would select it —
#   so the flag itself must not appear here, not even in a comment.
#
# Env:
#   DOCKPANEL_VERSION                   required, `vX.Y.Z` release tag
#   DOCKPANEL_AGENT_UPDATE_DETACHED     set by the caller once detached
#   DOCKPANEL_GITHUB_REPO               optional override (default ovexro/dockpanel)
#   DOCKPANEL_AGENT_BIN                 optional override of the binary path
#
# The result is ALWAYS written to /var/lib/dockpanel/last-agent-update.json,
# on every exit path including the abort trap — a run that failed and a run that
# never happened are otherwise indistinguishable (lesson #52).
#
# Exit 0 = the agent is running the target version. Non-zero = it is running the
# version it started on; `stage` in the result file says how far this got.

set -euo pipefail

TARGET="${DOCKPANEL_VERSION:?DOCKPANEL_VERSION is required (vX.Y.Z)}"
GITHUB_REPO="${DOCKPANEL_GITHUB_REPO:-ovexro/dockpanel}"
AGENT_BIN="${DOCKPANEL_AGENT_BIN:-/usr/local/bin/dockpanel-agent}"
SOCKET="${DOCKPANEL_AGENT_SOCKET:-/var/run/dockpanel/agent.sock}"

STATE_DIR=/var/lib/dockpanel
RESULT="$STATE_DIR/last-agent-update.json"
LOG_TAG=dockpanel-agent-update

# Re-exec outside the agent's own cgroup if we are somehow inside it.
if [ -z "${DOCKPANEL_AGENT_UPDATE_DETACHED:-}" ] && command -v systemd-run >/dev/null 2>&1; then
    if grep -q 'dockpanel-agent\.service' /proc/self/cgroup 2>/dev/null; then
        echo "[agent-update] re-executing outside dockpanel-agent.service's cgroup"
        exec systemd-run --quiet --collect \
            --unit="dockpanel-agent-update-manual-$$" \
            --setenv=DOCKPANEL_AGENT_UPDATE_DETACHED=1 \
            --setenv=DOCKPANEL_VERSION="$TARGET" \
            --setenv=DOCKPANEL_GITHUB_REPO="$GITHUB_REPO" \
            bash "$0"
    fi
fi

mkdir -p "$STATE_DIR"
chmod 0700 "$STATE_DIR" 2>/dev/null || true

stage="init"
finished=0
from_version=""
STAGED=""

log() {
    echo "[agent-update] $*"
    command -v systemd-cat >/dev/null 2>&1 && echo "[agent-update] $*" | systemd-cat -t "$LOG_TAG" || true
}

# dp_fetch <url> <out> [max-time] — download, retrying the failures curl's own
# --retry does not. Same shape as setup.sh's dp_fetch, deliberately: four
# programs in this repo perform the identical operation against the identical
# host, and until s354 only the installer retried it at all.
#
# `--retry` covers connection refusals, timeouts and some HTTP codes. It does
# NOT cover a connection dropped MID-TRANSFER, which exits 56 — the error that
# has aborted real installs against the release CDN. curl grew
# `--retry-all-errors` for exactly this, but it landed in 7.71 and the installer
# supports Ubuntu 20.04, which ships 7.68 — so the loop lives in shell, and the
# four copies stay identical rather than drifting apart.
#
# The per-attempt `--max-time` is the caller's, not a new policy: this is the
# one consumer that already bounded its downloads, and those bounds are kept.
#
# ⚠ `rc=$?` on the line after a bare `if` reads 0, because a false `if` with no
# `else` is itself a success. Capture with `|| rc=$?` on the command itself.
# ⚠ Written as explicit `if` blocks, not `[ … ] && …`: under `set -e` a false
# test at statement level aborts the script.
dp_fetch() {
    local url="$1" out="$2" maxtime="${3:-600}" attempt=1 rc=0
    while [ "$attempt" -le 3 ]; do
        rc=0
        curl --retry 3 --retry-delay 2 -fsSL --max-time "$maxtime" "$url" -o "$out" || rc=$?
        if [ "$rc" -eq 0 ]; then return 0; fi
        log "fetch attempt ${attempt}/3 exited ${rc} — $url"
        rm -f "$out"
        attempt=$((attempt + 1))
        if [ "$attempt" -le 3 ]; then sleep 3; fi
    done
    return "$rc"
}

# Written by every exit path. `ok` is the only field a caller should trust:
# an exit status can be the status of the wrong process (which is exactly the
# defect this file was written to fix), a file on disk cannot.
write_result() {
    local ok="$1" reason="$2"
    local esc
    esc=$(printf '%s' "$reason" | tr -d '\000' | sed 's/\\/\\\\/g; s/"/\\"/g' | tr '\n' ' ')
    cat > "$RESULT" <<JSON
{
  "ok": $ok,
  "stage": "$stage",
  "target_version": "$TARGET",
  "from_version": "$from_version",
  "reason": "$esc",
  "at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
JSON
    chmod 0600 "$RESULT" 2>/dev/null || true
}

WORK=""

# Capture the real exit status FIRST — anything that runs before reading `$?`
# (a cleanup `rm`, a log line) overwrites it, and the reason string would then
# describe the janitor instead of the failure.
on_exit() {
    local rc=$?
    [ -n "$WORK" ] && rm -rf "$WORK"
    # A staged binary is 21MB sitting next to the real one; if we died between
    # staging and installing it, nothing else would ever clean it up.
    [ -n "${STAGED:-}" ] && [ -e "${STAGED:-}" ] && rm -f "$STAGED"
    if [ "$finished" != "1" ]; then
        write_result false "aborted at stage '$stage' (exit $rc)"
        log "FAILED at stage '$stage' (exit $rc)"
    fi
}
trap on_exit EXIT

fail() { stage="$1"; write_result false "$2"; finished=1; log "FAILED: $2"; exit "${3:-1}"; }

running_version() {
    curl -sf --max-time 5 --unix-socket "$SOCKET" http://localhost/health 2>/dev/null \
        | sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p'
}

# ── 0. Where we are starting from ────────────────────────────────────────
stage="probe"
from_version="$(running_version || true)"
log "agent reports version '${from_version:-unknown}', target $TARGET"

TARGET_CLEAN="${TARGET#v}"
if [ -n "$from_version" ] && [ "$from_version" = "$TARGET_CLEAN" ]; then
    stage="complete"
    finished=1
    write_result true "already running $TARGET_CLEAN"
    log "already at $TARGET_CLEAN — nothing to do"
    exit 0
fi

# ── 1. Architecture ──────────────────────────────────────────────────────
stage="arch"
case "$(uname -m)" in
    x86_64)          ARCH_LABEL=amd64 ;;
    aarch64|arm64)   ARCH_LABEL=arm64 ;;
    *) fail arch "unsupported architecture $(uname -m)" ;;
esac
ASSET="dockpanel-agent-linux-${ARCH_LABEL}"
BASE="https://github.com/${GITHUB_REPO}/releases/download/${TARGET}"

# ── 2. Download to a file, then verify, then install ─────────────────────
# Never `curl | install`: once the bytes have been streamed into the consumer
# there is no verification window left (lesson #25). And never take the status
# of a pipeline whose last stage is not the fallible one (lesson #51) — each
# curl below writes a file and is status-checked on its own.
stage="download"
WORK="$(mktemp -d "${STATE_DIR}/agent-update-XXXXXX")"

dp_fetch "${BASE}/${ASSET}" "$WORK/agent" 600 \
    || fail download "could not download ${BASE}/${ASSET} (does the release exist?)"

SIZE=$(stat -c %s "$WORK/agent" 2>/dev/null || echo 0)
[ "$SIZE" -gt 1000000 ] || fail download "downloaded agent is only ${SIZE} bytes"

stage="verify"
if dp_fetch "${BASE}/checksums.txt" "$WORK/checksums.txt" 60; then
    EXPECT=$(awk -v a="$ASSET" '$2 == a {print $1}' "$WORK/checksums.txt" | head -1)
    if [ -z "$EXPECT" ]; then
        fail verify "checksums.txt for $TARGET has no entry for $ASSET"
    fi
    ACTUAL=$(sha256sum "$WORK/agent" | awk '{print $1}')
    # A mismatch is NOT a retryable failure — distinct exit code so an operator
    # (or a caller's retry loop) can tell "network hiccup" from "wrong bytes".
    [ "$ACTUAL" = "$EXPECT" ] || fail verify "sha256 mismatch for $ASSET: got $ACTUAL, expected $EXPECT" 99
    log "sha256 verified against the release's checksums.txt"
else
    fail verify "could not download ${BASE}/checksums.txt — refusing to install an unverified binary"
fi

MAGIC=$(head -c 4 "$WORK/agent" | od -An -tx1 | tr -d ' \n')
[ "$MAGIC" = "7f454c46" ] || fail verify "downloaded agent is not an ELF executable (magic $MAGIC)"
chmod 0755 "$WORK/agent"

# ── 3. Swap ──────────────────────────────────────────────────────────────
# `mv` within the same filesystem, never `cp`: writing onto a binary that is
# currently executing fails ETXTBSY, and a restore path that cannot restore is
# not a safety net (lesson #48). Staged into the target's own directory so the
# rename is atomic.
stage="swap"
BIN_DIR="$(dirname "$AGENT_BIN")"
STAGED="${BIN_DIR}/.dockpanel-agent.new.$$"
BACKUP="${AGENT_BIN}.bak"

mv -f "$WORK/agent" "$STAGED" || fail swap "could not stage the new binary into $BIN_DIR"
# Reading a running executable is always allowed; it is WRITING onto its inode
# that fails ETXTBSY. So the backup copy is fine, and the install below is a
# rename of the directory entry — the live process keeps the old inode.
cp -a "$AGENT_BIN" "$BACKUP" 2>/dev/null || true
mv -f "$STAGED" "$AGENT_BIN" || fail swap "could not move the new binary into place"
swapped=1

# SELinux: relabel after the swap, or this update is the last one this agent
# ever performs.
#
# $WORK lives under the state dir, so the downloaded binary carries `var_lib_t`,
# and BOTH moves above are renames within one filesystem — which PRESERVE the
# label rather than taking the destination's. On an Enforcing box systemd then
# cannot exec it (`status=203/EXEC`, "Permission denied"), and because this
# runs on a 6-hourly timer, every RHEL-family install would lose its agent at
# the first automatic update after installation. Measured on Rocky 9.8 at s266.
#
# Deliberately before the restart below, so the verify-running step that follows
# is testing a binary systemd can actually execute. No-op where restorecon does
# not exist.
if command -v restorecon > /dev/null 2>&1; then
    restorecon -F "$AGENT_BIN" 2>/dev/null || true
fi

log "binary swapped ($SIZE bytes)"

# ── 3b. Bring the systemd unit along with the binary ─────────────────────
# The unit is compiled into the agent (`--print-unit`), from the same file
# setup.sh and update.sh copy — so this is not a fourth mirror, it is the one
# source arriving by the one delivery path that reaches a fleet member.
#
# WHY HERE, and not only at agent startup: the agent also reconciles its unit
# when it boots (services/agent_unit.rs), but a sandbox written after the
# process has started does not apply to that process — it would wait for some
# LATER restart, which on a box that is already current may be weeks away. Doing
# it here means the restart below is the one that applies it, and the
# verify-running + rollback steps that follow already cover the outcome.
#
# Every fleet member installed before s271 is running a unit install-agent.sh
# hand-wrote with systemd's protection directives switched off and no writable
# path list at all. This is how they get the real one. (The directives are not
# named here on purpose — the pin suite greps the installers for them.)
stage="unit"
UNIT_PATH=/etc/systemd/system/dockpanel-agent.service
UNIT_BACKUP=""
# Bounded for the same reason install-agent.sh bounds it: a binary that predates
# the flag ignores it and starts the daemon instead, which would hang this
# updater for ever inside its transient unit rather than skipping the step.
if timeout 15 "$AGENT_BIN" --print-unit > "$WORK/unit" 2>/dev/null \
   && grep -q '^ExecStart=/usr/local/bin/dockpanel-agent' "$WORK/unit"; then
    if ! cmp -s "$WORK/unit" "$UNIT_PATH" 2>/dev/null; then
        # The directories first, always. An unprefixed ReadWritePaths entry that
        # does not exist fails the namespace mount and the agent does not start
        # at all (/etc/apt did this to every RPM box, s264; /etc/nginx would do
        # it to every agent-only box). A `-`-prefixed one that is missing is
        # skipped silently and every write beneath it fails read-only (s269), so
        # the prefix is stripped rather than filtered on.
        RWP="$(sed -n 's/^ReadWritePaths=//p' "$WORK/unit" | tr ' ' '\n' | sed 's/^-//' | { grep '^/' || true; })"
        for d in $RWP; do
            [ -d "$d" ] || mkdir -p "$d" 2>/dev/null || true
        done
        UNIT_BACKUP="${WORK}/unit.previous"
        cp -a "$UNIT_PATH" "$UNIT_BACKUP" 2>/dev/null || UNIT_BACKUP=""
        if install -m 644 "$WORK/unit" "$UNIT_PATH"; then
            systemctl daemon-reload || log "daemon-reload returned non-zero after the unit swap"
            log "systemd unit updated from the new binary"
        else
            UNIT_BACKUP=""
            log "could not install the new unit at $UNIT_PATH; keeping the existing one"
        fi
    fi
else
    log "the new binary did not emit a unit (--print-unit); keeping the existing one"
fi

# ── 4. Restart and prove the version actually changed ────────────────────
# The whole point. "The service came back up" is a step, not an outcome
# (lesson #49) — the outcome is what /health reports afterwards.
stage="restart"
systemctl restart dockpanel-agent || log "systemctl restart returned non-zero; still checking the outcome"

stage="verify-running"
NEW_VERSION=""
for _ in $(seq 1 30); do
    sleep 2
    NEW_VERSION="$(running_version || true)"
    [ "$NEW_VERSION" = "$TARGET_CLEAN" ] && break
done

if [ "$NEW_VERSION" != "$TARGET_CLEAN" ]; then
    stage="rollback"
    log "agent reports '${NEW_VERSION:-nothing}' after the swap, expected $TARGET_CLEAN — rolling back"

    # Never suffix a restore with `|| true` and never claim it worked without
    # looking (lesson #48, and #58 for doing it anyway). What goes in the result
    # file is what the agent reports about ITSELF after the attempt, so an
    # operator can tell "we put you back" from "you are on a binary that does
    # not start".
    # Put the UNIT back before the binary, and before any restart. Restoring
    # only the binary would be a restore that cannot restore (lesson #48): if
    # what stopped the agent coming up was the new unit — a ReadWritePaths entry
    # this box cannot satisfy, say — then the old binary starting under it fails
    # exactly the same way, and the rollback would report a box it had not
    # actually rescued.
    if [ -n "$UNIT_BACKUP" ] && [ -f "$UNIT_BACKUP" ]; then
        if install -m 644 "$UNIT_BACKUP" "$UNIT_PATH"; then
            systemctl daemon-reload || log "daemon-reload returned non-zero during unit rollback"
            log "restored the previous systemd unit"
        else
            log "COULD NOT RESTORE THE PREVIOUS UNIT at $UNIT_PATH — this box needs attention"
        fi
    fi

    restored="no backup was taken"
    if [ -f "$BACKUP" ]; then
        # `mv` again: the new agent may well be running from that path right now.
        if mv -f "$BACKUP" "$AGENT_BIN"; then
            systemctl restart dockpanel-agent || log "restart after rollback returned non-zero"
            sleep 3
            back="$(running_version || true)"
            if [ -n "$back" ]; then
                restored="restored, agent reports $back"
            else
                restored="RESTORED THE OLD BINARY BUT THE AGENT IS NOT ANSWERING — this box needs attention"
            fi
        else
            restored="COULD NOT RESTORE $BACKUP — this box is running a binary that did not come up"
        fi
        log "$restored"
    fi
    fail rollback "agent did not come up as $TARGET_CLEAN (saw '${NEW_VERSION:-nothing}'); $restored"
fi

rm -f "$BACKUP"
stage="complete"
finished=1
write_result true "updated ${from_version:-unknown} -> $TARGET_CLEAN"
log "agent is running $TARGET_CLEAN"
exit 0
