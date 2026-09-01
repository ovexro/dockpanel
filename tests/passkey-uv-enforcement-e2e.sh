#!/bin/bash
# Passkey UV/2FA-skip enforcement — EXECUTE-class, needs a live dockpanel-api.
#
# This is the "virtual-authenticator harness" `tests/passkey-ceremony-pin-e2e.sh`
# §G4 named as the missing piece: a real WebAuthn client+authenticator,
# independent of the server's own code, proving end to end that a passkey
# which demonstrated user verification (PIN/biometric) at registration is
# REQUIRED to demonstrate it again at login, while a possession-only
# credential (registered before this ship, or from an authenticator that
# never verifies) is unaffected — the grandfathering guarantee.
#
# Third EXECUTE-class member of the pin family alongside nginx-headers and
# update-rollback (reference_dockpanel_ops) — needs a running box, not pure
# source analysis. Builds and runs `passkey_virtual_authenticator`, a Cargo
# binary in panel/backend/src/bin/, which does the actual CBOR/ECDSA work and
# prints its own ✓/✗ marks in the same family format; this wrapper just
# builds it and forwards its exit code.
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

echo
echo "=============================================="
echo "  Passkey UV enforcement (s443)"
echo "=============================================="
echo

if ! command -v curl >/dev/null || ! curl -sf "http://127.0.0.1:3080/api/health" >/dev/null 2>&1; then
  printf '  \033[33m~\033[0m SKIPPED — no dockpanel-api reachable at 127.0.0.1:3080\n'
  echo "0 passed, 0 failed, 1 skipped"
  exit 0
fi

source /root/.cargo/env 2>/dev/null || true
if ! (cd panel/backend && cargo build --bin passkey_virtual_authenticator 2>&1 | tail -20); then
  printf '  \033[31m✗\033[0m BUILD FAILED\n'
  exit 1
fi

export DOCKPANEL_TEST_BASE_URL="${DOCKPANEL_TEST_BASE_URL:-http://127.0.0.1:3080}"
export DOCKPANEL_TEST_EMAIL="${DOCKPANEL_TEST_EMAIL:-admin@dockpanel.dev}"
export DOCKPANEL_TEST_PASSWORD="${DOCKPANEL_TEST_PASSWORD:-testpassword}"

panel/backend/target/debug/passkey_virtual_authenticator
exit $?
