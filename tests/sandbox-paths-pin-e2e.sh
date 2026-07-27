#!/usr/bin/env bash
# Regression pins for s269 — the "-" prefix class in the agent's ReadWritePaths.
#
# systemd's `-` prefix means "bind this path IF IT EXISTS". The two halves of
# the unit's list therefore fail in opposite ways:
#
#   S1  An UNPREFIXED entry that is missing fails the namespace mount and the
#       agent does not start. Loud, immediate, self-correcting.
#   S2  A `-`-PREFIXED entry that is missing is skipped silently. The unit
#       starts, systemd reports success, and every write beneath that path
#       fails with `Read-only file system` for the lifetime of the namespace.
#       Verified directly against systemd at s269: with the directory absent the
#       unit's own ExecStart got `mkdir: cannot create directory: Read-only file
#       system` while `systemctl show -p Result` still said `success`. Creating
#       the directory afterwards does not rescue a RUNNING service — the mount
#       namespace is fixed at start.
#
# That is why the installers pre-create these directories, and why the list they
# create is load-bearing rather than belt-and-braces. It was hand-copied into
# setup.sh AND update.sh, and it drifted: /var/spool/cron reached the unit and
# setup.sh at s268 but never update.sh's loop — so an upgraded box without a
# cron spool got a silently unwritable one, defeating on exactly those boxes the
# "existing installs recover on upgrade" property s268's cron fix was built for.
#
# The repair is to DERIVE the list from the unit instead of mirroring it. These
# pins hold the derivation in place; a future hand-written list fails here.
#
# Pure source analysis: no box, no network, no build.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

UNIT=panel/agent/dockpanel-agent.service
SETUP=scripts/setup.sh
UPDATE=scripts/update.sh

for f in "$UNIT" "$SETUP" "$UPDATE"; do
  [ -f "$f" ] || { echo "missing file: $f"; exit 1; }
done

# The unit is the single source of truth. Read it the same way the installers do.
RWP_LINE=$(grep '^ReadWritePaths=' "$UNIT" | head -1)
ALL=$(printf '%s' "$RWP_LINE" | cut -d= -f2- | tr ' ' '\n' | sed 's/^-//' | grep '^/' || true)
DASH=$(printf '%s' "$RWP_LINE" | cut -d= -f2- | tr ' ' '\n' | grep '^-/' | sed 's/^-//' || true)

echo "── 1. the unit still declares a sandbox worth mirroring ──"

if [ -n "$RWP_LINE" ]; then
  ok "the agent unit declares ReadWritePaths ($(printf '%s\n' "$ALL" | grep -c . ) entries, $(printf '%s\n' "$DASH" | grep -c . ) of them '-'-prefixed)"
else
  bad "the agent unit has no ReadWritePaths — if the sandbox was dropped, delete this suite deliberately rather than letting it pass"
fi

# The `-` prefix is the silent half. If it ever disappears entirely the class is
# gone, but so is tolerance for optional services, so flag it as a real change.
if [ -n "$DASH" ]; then
  ok "'-'-prefixed entries exist, so pre-creation is load-bearing"
else
  bad "no '-'-prefixed entries remain — every path is now mandatory; confirm that is intended"
fi

if grep -qE '^ProtectSystem=strict' "$UNIT"; then
  ok "ProtectSystem=strict — an unbound path really is read-only"
else
  bad "ProtectSystem is no longer strict; the sandbox these paths carve out may not exist"
fi

echo
echo "── 2. both installers DERIVE the list rather than mirroring it ──"

for f in "$SETUP" "$UPDATE"; do
  if grep -q 'agent_rwp_dirs()' "$f"; then
    ok "$f defines agent_rwp_dirs"
  else
    bad "$f no longer derives the directory list from the unit — a hand-written list is how this drifted"
  fi
  # Defining it and never calling it would pass a naive pin while restoring the bug.
  if [ "$(grep -c 'agent_rwp_dirs' "$f")" -ge 2 ]; then
    ok "$f actually calls it"
  else
    bad "$f defines agent_rwp_dirs but never calls it"
  fi
done

# Under `set -euo pipefail` a grep that matches nothing kills the script, and
# these run at install time where the failure is worst.
for f in "$SETUP" "$UPDATE"; do
  if grep -A 6 'agent_rwp_dirs()' "$f" | grep -qF '|| true'; then
    ok "$f's helper tolerates a no-match under pipefail"
  else
    bad "$f's helper can abort the installer when the grep matches nothing (set -euo pipefail)"
  fi
done

echo
echo "── 3. the derivation reproduces the unit exactly ──"

# Run the real helper, as defined in the shipped script, against the real unit.
derive() {
  bash -c '
    set -euo pipefail
    AGENT_SRC="$1"
    '"$(sed -n '/^agent_rwp_dirs() {/,/^}/p' "$SETUP")"'
    agent_rwp_dirs
  ' _ "$(dirname "$UNIT")"
}
DERIVED=$(derive | sort || true)
EXPECTED=$(printf '%s\n' "$ALL" | sort)

if [ "$DERIVED" = "$EXPECTED" ]; then
  ok "the helper yields exactly the unit's $(printf '%s\n' "$EXPECTED" | grep -c .) paths, '-' stripped"
else
  bad "the helper's output does not match the unit: $(diff <(printf '%s\n' "$EXPECTED") <(printf '%s\n' "$DERIVED") | tr '\n' ' ')"
fi

# The '-' prefix must be STRIPPED, not carried through — `mkdir -p -/etc/php`
# would be read as a flag and the directory would never be created.
if printf '%s\n' "$DERIVED" | grep -q '^-'; then
  bad "derived paths still carry the '-' prefix; mkdir would treat it as an option"
else
  ok "the '-' prefix is stripped from every derived path"
fi

echo
echo "── 4. the specific drift that started this, and the ordering that makes it work ──"

# /var/spool/cron is the path that drifted. Pinning it by name keeps the actual
# regression down even if the derivation is rewritten later.
for f in "$SETUP" "$UPDATE"; do
  if printf '%s\n' "$DERIVED" | grep -qx '/var/spool/cron'; then
    ok "$f covers /var/spool/cron (via the derivation)"
  else
    bad "$f does not cover /var/spool/cron — the s268 cron fix stops landing on upgraded boxes"
  fi
done

if printf '%s\n' "$ALL" | grep -qx '/var/spool/cron'; then
  ok "the unit still grants the cron spool, so crontabs remain writable"
else
  bad "/var/spool/cron left the unit — the agent cannot write crontabs at all"
fi

# The directories must exist BEFORE the agent restarts, or the bind is skipped
# for the new namespace and the fix is inert until the next restart.
MK_LINE=$(grep -n 'for d in \$(agent_rwp_dirs)' "$UPDATE" | head -1 | cut -d: -f1)
CP_LINE=$(grep -n 'cp "\$AGENT_SRC/dockpanel-agent.service"' "$UPDATE" | head -1 | cut -d: -f1)
if [ -n "$MK_LINE" ] && [ -n "$CP_LINE" ] && [ "$MK_LINE" -lt "$CP_LINE" ]; then
  ok "update.sh creates the directories (line $MK_LINE) before installing the unit (line $CP_LINE)"
else
  bad "update.sh's directory creation no longer precedes the unit install — the new namespace can be built before the paths exist"
fi

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
