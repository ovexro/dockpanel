#!/usr/bin/env bash
# cron-package-declared-pin-e2e.sh — the installer declares the cron package it uses.
#
#   setup.sh installs the daily database backup with `crontab`, and the agent writes
#   every site cron through it, but no installer declared the package. Debian 13 cloud
#   images ship without it: the backup job was never installed and POST /api/sites/{id}/crons
#   answered 502 ("`crontab -l` failed to run: No such file or directory"). Measured on a
#   throwaway VPS, 2026-11-28.
#
# Pure source analysis. NO PIPES INTO `grep -q` (pipefail): every arm uses a here-string.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

SETUP=scripts/setup.sh; UPDATE=scripts/update.sh
for f in "$SETUP" "$UPDATE"; do [ -f "$f" ] || bad "MISSING SUBJECT FILE: $f"; done

APT=$(grep -E 'pkg_install curl openssl ca-certificates openssh-client gnupg lsb-release' "$SETUP")
RPM=$(grep -E 'pkg_install curl openssl ca-certificates openssh-clients' "$SETUP")
if [ -n "$APT" ] && [ -n "$RPM" ]; then ok "1-control both base-package lines found"; else bad "1-control base-package lines not found (the grep broke)"; fi
if grep -qE '\bcron\b' <<< "$APT"; then ok "2 apt base packages include cron"; else bad "2 apt base packages must include cron"; fi
if grep -qE '\bcronie\b' <<< "$RPM"; then ok "3 RPM base packages include cronie"; else bad "3 RPM base packages must include cronie"; fi
if grep -qE 'systemctl enable --now cron ' "$SETUP" && grep -qE 'systemctl enable --now crond ' "$SETUP"; then ok "4 setup.sh starts the cron service"; else bad "4 setup.sh must enable and start cron/crond"; fi
if grep -qE '! command -v crontab' "$UPDATE"; then ok "5 update.sh repairs a box that has no crontab"; else bad "5 update.sh must install cron on a box without it"; fi
if grep -qE 'apt-get install -y cron ' "$UPDATE" && grep -qE 'install -y cronie' "$UPDATE"; then ok "6 update.sh covers apt and RPM"; else bad "6 update.sh must cover apt and RPM"; fi

echo; echo "passed $PASS, failed $FAIL"
[ "$FAIL" = 0 ]
