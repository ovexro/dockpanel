#!/usr/bin/env bash
# Regression pin: the installer must WAIT for the dpkg lock, visibly, not die on it.
#
# The defect: a fresh Ubuntu 24.04 starts unattended-upgrade at first boot. On a
# 2 vCPU/4 GB cloud VPS it held /var/lib/dpkg/lock-frontend for 13+ minutes.
# setup.sh's only defence was `DPkg::Lock::Timeout "300"` (per apt-get call, five
# minutes), so the Node.js step waited its five minutes and then died with
# "E: Unable to acquire the dpkg frontend lock" (exit 100), a third of the way
# through the install. Reproduced on a real VPS; fixed by wait_for_apt_lock.
#
# These cases need no root and no apt: they run the REAL function out of
# scripts/setup.sh against a fake `apt-get` on PATH and a fake `sleep`.
#
#   run: bash tests/apt-lock-wait-pin-e2e.sh
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SETUP="$REPO/scripts/setup.sh"
PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }
check() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (expected [$3], got [$2])"; fi; }

TMP=$(mktemp -d)
[ -n "$TMP" ] && [ -d "$TMP" ] || { echo "mktemp failed"; exit 1; }
trap 'rm -rf -- "${TMP:?}"' EXIT
mkdir -p "$TMP/bin"

FN=$(awk '/^wait_for_apt_lock\(\) \{/,/^\}/' "$SETUP")
if [ -z "$FN" ]; then
  bad "wait_for_apt_lock is defined in scripts/setup.sh"
  echo "══ $PASS passed, $FAIL failed ══"
  exit 1
fi
ok "wait_for_apt_lock is defined in scripts/setup.sh"

# Fake apt-get: the first $FAKE_LOCK_CALLS calls fail with the lock message, then
# it succeeds; FAKE_APT_ERR makes every call fail with a different error.
cat > "$TMP/bin/apt-get" <<'APT'
#!/bin/bash
n=$(cat "$FAKE_DIR/calls" 2>/dev/null || echo 0); n=$((n+1)); echo "$n" > "$FAKE_DIR/calls"
if [ -n "${FAKE_APT_ERR:-}" ]; then echo "$FAKE_APT_ERR"; exit 100; fi
if [ "$n" -le "${FAKE_LOCK_CALLS:-0}" ]; then
  echo "E: Could not get lock /var/lib/dpkg/lock-frontend. It is held by process 1 (unattended-upgr)"
  echo "E: Unable to acquire the dpkg frontend lock (/var/lib/dpkg/lock-frontend), is another process using it?"
  exit 100
fi
exit 0
APT
cat > "$TMP/bin/sleep" <<'SLP'
#!/bin/bash
exit 0
SLP
chmod +x "$TMP/bin/apt-get" "$TMP/bin/sleep"

run_case() { # run_case <lock-calls> [err] -> "rc probes notices warnings"
  rm -f "$TMP/calls" "$TMP/log" "$TMP/rc"
  FAKE_DIR="$TMP" FAKE_LOCK_CALLS="$1" FAKE_APT_ERR="${2:-}" PATH="$TMP/bin:$PATH" PKG_MGR=apt \
    bash -c '
      info() { echo "INFO $*" >> "$FAKE_DIR/log"; }
      warn() { echo "WARN $*" >> "$FAKE_DIR/log"; }
      '"$FN"'
      wait_for_apt_lock; echo "$?" > "$FAKE_DIR/rc"
    ' >/dev/null 2>&1 || true
  local infos=0 warns=0
  if [ -f "$TMP/log" ]; then
    infos=$(grep -c '^INFO' "$TMP/log" || true); warns=$(grep -c '^WARN' "$TMP/log" || true)
  fi
  printf '%s %s %s %s' "$(cat "$TMP/rc" 2>/dev/null || echo ?)" "$(cat "$TMP/calls" 2>/dev/null || echo ?)" "$infos" "$warns"
}

echo "§1 lock held, then released: wait, tell the operator once, return success"
check "5 locked probes then free -> rc 0 after 6 probes, one notice, no warning" "$(run_case 5)" "0 6 1 0"

echo "§2 lock free at once: no waiting, no noise"
check "free lock -> rc 0, 1 probe, no notice" "$(run_case 0)" "0 1 0 0"

echo "§3 any other apt failure ends the wait (the real step reports it)"
check "'Failed to fetch' -> rc 0 after exactly 1 probe, no notice" \
  "$(run_case 0 'E: Failed to fetch http://x  Temporary failure resolving')" "0 1 0 0"

echo "§4 a lock that never clears gives up after the limit, with a warning"
# 1800s / 10s = 180 sleeps, so 181 probes. The "still waiting" notice repeats every
# 120s: 1 first notice + 14 repeats (at 120..1680s) = 15 INFO lines.
check "forever locked -> rc 0, 181 probes, 15 notices, one warning" "$(run_case 100000)" "0 181 15 1"

echo "§5 wiring: the wait runs after the package manager is chosen and before anything installs"
L_DET=$(grep -n '^    detect_pkg_manager$' "$SETUP" | head -1 | cut -d: -f1 || true)
L_WAIT=$(grep -n '^    wait_for_apt_lock$' "$SETUP" | cut -d: -f1 | awk -v d="${L_DET:-0}" '$1 > d { print; exit }' || true)
L_FW=$(grep -n '^    detect_firewall$' "$SETUP" | head -1 | cut -d: -f1 || true)
if [ -n "$L_DET" ] && [ -n "$L_WAIT" ] && [ -n "$L_FW" ] && [ "$L_DET" -lt "$L_WAIT" ] && [ "$L_WAIT" -lt "$L_FW" ]; then
  ok "main() calls wait_for_apt_lock between detect_pkg_manager and detect_firewall"
else
  bad "main() must call wait_for_apt_lock right after detect_pkg_manager (lines: det=$L_DET wait=$L_WAIT fw=$L_FW)"
fi

echo "§6 the probe is the lock itself, never a process name"
check "function does not use pgrep (the shutdown helper shares the worker's name)" \
  "$(printf '%s\n' "$FN" | grep -c 'pgrep' || true)" "0"
check "probe uses DPkg::Lock::Timeout=0" "$(printf '%s\n' "$FN" | grep -c 'DPkg::Lock::Timeout=0' || true)" "1"

echo "§7 the whole install is covered, not only its start (Debian 13 lost the race the first fix left)"
RUN_BODY=$(awk '/^run\(\) \{/,/^\}/' "$SETUP")
check "run() waits for the lock before every step" "$(printf '%s\n' "$RUN_BODY" | grep -c 'wait_for_apt_lock' || true)" "1"
check "installer writes a 1800s apt lock timeout" "$(grep -c 'DPkg::Lock::Timeout \"1800\"' "$SETUP" || true)" "1"
SUMMARY_BODY=$(awk '/^print_summary\(\) \{/,/^\}/' "$SETUP")
check "print_summary restores the 300s runtime timeout" "$(printf '%s\n' "$SUMMARY_BODY" | grep -c 'Timeout \"300\"' || true)" "1"

echo
echo "══ $PASS passed, $FAIL failed ══"
[ "$FAIL" -eq 0 ]
