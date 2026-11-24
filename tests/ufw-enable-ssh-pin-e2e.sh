#!/usr/bin/env bash
# Regression pin: an installed-but-INACTIVE ufw must be enabled, and never at the cost of SSH.
#
# The defect: Ubuntu ships ufw installed and disabled. setup.sh read "the ufw command
# exists" as "a firewall is enforcing", logged "UFW already installed" and "ports 80,
# 443, 8443 allowed", listed UFW among the installed services, and left the box with no
# firewall (found on a fresh Ubuntu 24.04 VPS: `ufw status` = inactive after a clean
# install). Debian, which has no ufw, installed AND enabled it, so the two distros
# ended up differently protected.
#
# The fix enables it, but the old "none" path only ever allowed port 22, which would
# lock out the installer's own session on any box whose sshd listens elsewhere, so both
# paths now share ufw_enable_with_ssh, which opens every sshd port first.
#
# No root and no ufw needed: the REAL functions from scripts/setup.sh run against fake
# `ufw`, `ss` and `sshd` on PATH.
#
#   run: bash tests/ufw-enable-ssh-pin-e2e.sh
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

FNS=$(awk '/^ssh_listen_ports\(\) \{/,/^\}/; /^ufw_enable_with_ssh\(\) \{/,/^\}/' "$SETUP")
if [ -z "$FNS" ] || [ "$(printf '%s\n' "$FNS" | grep -c 'ufw_enable_with_ssh' || true)" = 0 ]; then
  bad "ssh_listen_ports and ufw_enable_with_ssh are defined in scripts/setup.sh"
  echo "══ $PASS passed, $FAIL failed ══"
  exit 1
fi
ok "ssh_listen_ports and ufw_enable_with_ssh are defined in scripts/setup.sh"

# Fake ufw: logs every call; `status` answers inactive until `--force enable` ran.
# FAKE_UFW_FAIL=enable makes the enable step fail.
cat > "$TMP/bin/ufw" <<'UFW'
#!/bin/bash
echo "$*" >> "$FAKE_DIR/ufw.log"
case "$*" in
  "status") if [ -f "$FAKE_DIR/enabled" ]; then echo "Status: active"; else echo "Status: inactive"; fi ;;
  "--force enable") [ "${FAKE_UFW_FAIL:-}" = enable ] && exit 1; touch "$FAKE_DIR/enabled" ;;
esac
exit 0
UFW
# Fake ss: FAKE_SS holds the lines to print (-H -ltnp format).
cat > "$TMP/bin/ss" <<'SS'
#!/bin/bash
printf '%s' "${FAKE_SS:-}"
SS
# Fake sshd -T (fallback source): prints FAKE_SSHD_PORT as "port N" when set.
cat > "$TMP/bin/sshd" <<'SSHD'
#!/bin/bash
[ -n "${FAKE_SSHD_PORT:-}" ] && echo "port $FAKE_SSHD_PORT"
exit 0
SSHD
chmod +x "$TMP/bin/ufw" "$TMP/bin/ss" "$TMP/bin/sshd"

SS_2222=$'LISTEN 0 128 0.0.0.0:2222 0.0.0.0:* users:(("sshd",pid=1,fd=3))\nLISTEN 0 128 [::]:2222 [::]:* users:(("sshd",pid=1,fd=4))\nLISTEN 0 128 0.0.0.0:8443 0.0.0.0:* users:(("nginx",pid=2,fd=5))\n'

run_fn() { # run_fn <expression> -> echoes its output; env FAKE_* passes through
  rm -f "$TMP/ufw.log" "$TMP/enabled"
  FAKE_DIR="$TMP" PATH="$TMP/bin:$PATH" bash -c "$FNS"$'\n'"$1" 2>/dev/null || true
}
ulog() { if [ -f "$TMP/ufw.log" ]; then tr '\n' '|' < "$TMP/ufw.log"; else printf ''; fi; }

echo "§1 sshd ports are discovered, deduplicated, and nothing else counts"
check "sshd on 2222 (v4+v6) and nginx on 8443 -> only 2222" \
  "$(FAKE_SS="$SS_2222" run_fn 'ssh_listen_ports' | tr '\n' ' ')" "2222 "
check "no listener visible, sshd -T says 2200 -> 2200" \
  "$(FAKE_SS='' FAKE_SSHD_PORT=2200 run_fn 'ssh_listen_ports' | tr '\n' ' ')" "2200 "
check "nothing detectable at all -> 22" "$(FAKE_SS='' run_fn 'ssh_listen_ports' | tr '\n' ' ')" "22 "

echo "§2 inactive ufw gets enabled with default deny and SSH opened FIRST"
OUT=$(FAKE_SS="$SS_2222" run_fn 'ufw_enable_with_ssh; echo "rc=$?"')
check "rc 0" "$(printf '%s\n' "$OUT" | grep -c 'rc=0' || true)" "1"
check "calls in order: deny incoming, allow outgoing, allow 2222, enable, status" \
  "$(FAKE_SS="$SS_2222" run_fn 'ufw_enable_with_ssh >/dev/null; cat "$FAKE_DIR/ufw.log" | tr "\n" "|"')" \
  "default deny incoming|default allow outgoing|allow 2222/tcp|--force enable|status|"

echo "§3 a failing enable is reported, never claimed"
check "enable fails -> rc 1" "$(FAKE_SS="$SS_2222" FAKE_UFW_FAIL=enable run_fn 'ufw_enable_with_ssh >/dev/null; echo $?')" "1"

echo "§4 wiring in setup.sh"
check "the inactive-ufw branch calls ufw_enable_with_ssh" \
  "$(awk '/^        ufw\)$/,/^            ;;/' "$SETUP" | grep -c 'ufw_enable_with_ssh' || true)" "1"
check "the no-firewall path calls ufw_enable_with_ssh" \
  "$(awk '/^        none\)$/,/^            ;;/' "$SETUP" | grep -c 'ufw_enable_with_ssh' || true)" "1"
check "no bare 'ufw allow 22/tcp' is left (it locks out sshd on any other port)" \
  "$(grep -c 'ufw allow 22/tcp' "$SETUP" || true)" "0"
check "an inactive ufw is never reported as 'already installed' success" \
  "$(awk '/^        ufw\)$/,/^            ;;/' "$SETUP" | grep -c 'inactive' || true)" "1"

echo
echo "══ $PASS passed, $FAIL failed ══"
[ "$FAIL" -eq 0 ]
