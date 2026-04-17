#!/usr/bin/env bash
# DockPanel Tier 2 Cert Pin E2E Test Suite
#
# Exercises the full Phase 3 #3 Tier 2 flow end-to-end:
#   - Trust-On-First-Use (TOFU) fingerprint capture via /api/agent/checkin
#   - Match no-op on subsequent checkin with same fingerprint
#   - MITM rejection (HTTP 403) on fingerprint mismatch
#   - Malformed-fingerprint rejection (HTTP 400)
#   - Admin rotate-cert-pin (with/without CSRF header when cookie auth
#     is available — the CSRF-header case is skipped if no login password
#     is provided)
#   - Activity-log capture of the rotate action
#   - Re-TOFU after rotate (new fingerprint captured cleanly)
#   - RemoteAgentClient::new_with_pin construction via POST /api/servers/{id}/test
#     (regression guard for the v2.7.18 rustls CryptoProvider panic — if the
#     CryptoProvider is not installed, the API process panics at ClientConfig
#     build time instead of handling the request)
#
# Auth strategy: if DOCKPANEL_TEST_PASSWORD is set, logs in via the panel's
# /api/auth/login endpoint; otherwise mints a short-lived admin JWT locally
# from the JWT_SECRET in /etc/dockpanel/api.env. The CSRF-gate sub-test
# works in either mode because the minted token is equally valid as a
# cookie. Inserts a synthetic "online" server row so the local server is
# never disturbed, and always cleans up via an EXIT trap.
set -uo pipefail

API="${DOCKPANEL_API_URL:-http://127.0.0.1:3080}"
ADMIN_EMAIL="${DOCKPANEL_TEST_EMAIL:-admin@dockpanel.dev}"
ADMIN_PASSWORD="${DOCKPANEL_TEST_PASSWORD:-}"

PASS=0 FAIL=0 SKIP=0 TOTAL=0

green() { echo -e "\e[32m  ✓ $1\e[0m"; PASS=$((PASS+1)); TOTAL=$((TOTAL+1)); }
red()   { echo -e "\e[31m  ✗ $1\e[0m"; FAIL=$((FAIL+1)); TOTAL=$((TOTAL+1)); }
skip()  { echo -e "\e[33m  ~ $1\e[0m"; SKIP=$((SKIP+1)); TOTAL=$((TOTAL+1)); }
sect()  { echo; echo "── $1 ──"; }

psql_exec() {
    # -q suppresses psql's command tags (e.g. "INSERT 0 1") so INSERT ... RETURNING
    # returns ONLY the returned row.
    docker exec dockpanel-postgres psql -U dockpanel -d dockpanel -qtAc "$1" 2>/dev/null
}

echo "═══════════════════════════════════════════════"
echo "  DockPanel Tier 2 Cert Pin E2E Test Suite"
echo "═══════════════════════════════════════════════"

# ── Resolve admin user ────────────────────────────────────────────────────
ADMIN_UID=$(psql_exec "SELECT id FROM users WHERE email = '$ADMIN_EMAIL' AND role = 'admin' LIMIT 1")
if [ -z "$ADMIN_UID" ]; then
    echo "FATAL: Admin user row not found for $ADMIN_EMAIL"
    exit 1
fi

# ── Obtain admin JWT ──────────────────────────────────────────────────────
# Strategy:
#   1. If DOCKPANEL_TEST_PASSWORD is set, log in via /api/auth/login → cookie.
#   2. Otherwise, if /etc/dockpanel/api.env is readable, mint a short-lived
#      admin JWT locally using the panel's JWT_SECRET. This lets the test
#      run as part of dev smoke-checks without hard-coding a password.
# The token is used as a Bearer header (CSRF-exempt). The CSRF-gate test
# below additionally requires a cookie, so it runs only in mode (1).
COOKIE_TOKEN=""
BEARER_TOKEN=""
if [ -n "$ADMIN_PASSWORD" ]; then
    COOKIE_TOKEN=$(curl -s -X POST "$API/api/auth/login" -H "Content-Type: application/json" \
        -d "{\"email\":\"$ADMIN_EMAIL\",\"password\":\"$ADMIN_PASSWORD\"}" -D - 2>/dev/null \
        | grep -oP 'token=\K[^;]+')
    BEARER_TOKEN="$COOKIE_TOKEN"
fi
if [ -z "$BEARER_TOKEN" ] && [ -r /etc/dockpanel/api.env ]; then
    JWT_SECRET=$(grep -E '^JWT_SECRET=' /etc/dockpanel/api.env | cut -d= -f2-)
    if [ -n "$JWT_SECRET" ]; then
        # Pass secret + UID + email via env so no interpolation into the
        # Python source (safe against any odd characters in the secret).
        BEARER_TOKEN=$(JWT_SECRET="$JWT_SECRET" ADMIN_UID="$ADMIN_UID" ADMIN_EMAIL="$ADMIN_EMAIL" \
            python3 - <<'PYEOF'
import jwt, os, time
now = int(time.time())
token = jwt.encode(
    {"sub": os.environ["ADMIN_UID"], "email": os.environ["ADMIN_EMAIL"],
     "role": "admin", "iat": now, "exp": now + 600},
    os.environ["JWT_SECRET"], algorithm="HS256")
print(token)
PYEOF
)
    fi
fi
if [ -z "$BEARER_TOKEN" ]; then
    echo "FATAL: Could not obtain an admin token."
    echo "       Set DOCKPANEL_TEST_PASSWORD=<password> or run with read access to /etc/dockpanel/api.env."
    exit 1
fi
BEARER="Authorization: Bearer $BEARER_TOKEN"

# ── Create synthetic test server ──────────────────────────────────────────
# status='online' so AgentRegistry::for_server() resolves the row (it filters
# status != 'pending'). agent_url points at a loopback port with no listener;
# the TLS handshake in the regression-guard section will fail, but that is
# the intent — we want to prove the API stays alive.
TEST_TOKEN="tier2-pin-test-$(date +%s)-$RANDOM"
TEST_TOKEN_HASH=$(printf '%s' "$TEST_TOKEN" | sha256sum | cut -d' ' -f1)
TEST_URL="https://127.0.0.1:9994"
TEST_NAME="tier2-pin-test-$$"
TEST_SERVER_ID=$(psql_exec "INSERT INTO servers (user_id, name, agent_token, agent_token_hash, agent_url, status, is_local) VALUES ('$ADMIN_UID', '$TEST_NAME', '$TEST_TOKEN', '$TEST_TOKEN_HASH', '$TEST_URL', 'online', false) RETURNING id")

if [ -z "$TEST_SERVER_ID" ]; then
    echo "FATAL: Could not insert synthetic test server row"
    exit 1
fi

cleanup() {
    psql_exec "DELETE FROM servers WHERE id = '$TEST_SERVER_ID'" > /dev/null
    psql_exec "DELETE FROM activity_logs WHERE target_name = '$TEST_NAME'" > /dev/null
    rm -f /tmp/tier2_resp
}
trap cleanup EXIT

echo "  Authenticated as $ADMIN_EMAIL ($( [ -n "$COOKIE_TOKEN" ] && echo 'login cookie' || echo 'local-minted JWT' ))"
echo "  Synthetic server ${TEST_SERVER_ID:0:8}… created (url=$TEST_URL)"

# ── Checkin fixtures ──────────────────────────────────────────────────────
FP_A=$(printf 'a%.0s' $(seq 1 64))
FP_B=$(printf 'b%.0s' $(seq 1 64))
FP_WRONG=$(printf 'c%.0s' $(seq 1 64))
FP_BAD_FORMAT="not-a-hex-fingerprint"

do_checkin() {
    local fp="$1"
    local payload
    if [ -n "$fp" ]; then
        payload="{\"server_id\":\"$TEST_SERVER_ID\",\"cert_fingerprint\":\"$fp\"}"
    else
        payload="{\"server_id\":\"$TEST_SERVER_ID\"}"
    fi
    curl -s -o /tmp/tier2_resp -w "%{http_code}" -X POST "$API/api/agent/checkin" \
        -H "Authorization: Bearer $TEST_TOKEN" \
        -H "Content-Type: application/json" \
        -d "$payload" 2>/dev/null
}

# ─────────────────────────────────────────────────────────────────────────
sect "TOFU + Checkin Flow"
# ─────────────────────────────────────────────────────────────────────────

STATUS=$(do_checkin "$FP_A")
[ "$STATUS" = "200" ] && green "TOFU checkin returns 200" \
    || red "TOFU checkin status $STATUS (expected 200)"

STORED=$(psql_exec "SELECT COALESCE(cert_fingerprint, '') FROM servers WHERE id = '$TEST_SERVER_ID'")
[ "$STORED" = "$FP_A" ] && green "TOFU captured fingerprint in DB" \
    || red "TOFU capture failed — DB value: ${STORED:-<empty>}"

STATUS=$(do_checkin "$FP_A")
[ "$STATUS" = "200" ] && green "Match checkin (same pin) returns 200" \
    || red "Match checkin status $STATUS (expected 200)"

STATUS=$(do_checkin "$FP_WRONG")
[ "$STATUS" = "403" ] && green "MITM checkin rejected (403)" \
    || red "MITM checkin status $STATUS (expected 403)"

STATUS=$(do_checkin "$FP_BAD_FORMAT")
[ "$STATUS" = "400" ] && green "Malformed fingerprint rejected (400)" \
    || red "Malformed-format status $STATUS (expected 400)"

STORED=$(psql_exec "SELECT COALESCE(cert_fingerprint, '') FROM servers WHERE id = '$TEST_SERVER_ID'")
[ "$STORED" = "$FP_A" ] && green "Pin unchanged after rejected attempts" \
    || red "Pin unexpectedly mutated to: ${STORED:-<null>}"

# ─────────────────────────────────────────────────────────────────────────
sect "Admin Rotate Cert Pin"
# ─────────────────────────────────────────────────────────────────────────

# CSRF gate fires only on cookie-auth mutating requests (Bearer is exempt).
# The JWT we minted is valid whether sent as Bearer or in the token cookie,
# so we can exercise the gate by passing it as a cookie without the
# X-Requested-With header.
#
# Note: for admin-only routes like this one, AdminUser::from_request_parts
# wraps the underlying AuthUser error — including the CSRF 403 — as 401
# ("Authentication required"). So the observable status for the blocked
# case is 401, not 403. The delta vs. the "with header" request below
# (200) is what proves the gate fired.
STATUS=$(curl -s -o /dev/null -w "%{http_code}" -X POST \
    "$API/api/servers/$TEST_SERVER_ID/rotate-cert-pin" \
    -H "Cookie: token=$BEARER_TOKEN" 2>/dev/null)
case "$STATUS" in
    401|403) green "Rotate blocked without X-Requested-With ($STATUS)" ;;
    *)       red "Rotate without CSRF header got $STATUS (expected 401/403)" ;;
esac

# Same cookie + X-Requested-With → should succeed (covers the happy path too)
STATUS=$(curl -s -o /dev/null -w "%{http_code}" -X POST \
    "$API/api/servers/$TEST_SERVER_ID/rotate-cert-pin" \
    -H "Cookie: token=$BEARER_TOKEN" -H "X-Requested-With: XMLHttpRequest" \
    -H "Content-Type: application/json" -d '{}' 2>/dev/null)
[ "$STATUS" = "200" ] && green "Rotate accepted with X-Requested-With (200)" \
    || red "Rotate with CSRF header got $STATUS (expected 200)"

STORED=$(psql_exec "SELECT COALESCE(cert_fingerprint, '') FROM servers WHERE id = '$TEST_SERVER_ID'")
[ -z "$STORED" ] && green "Pin cleared after rotate" \
    || red "Pin not cleared after rotate (value: $STORED)"

LOG_COUNT=$(psql_exec "SELECT COUNT(*) FROM activity_logs WHERE action = 'server.rotate_cert_pin' AND target_name = '$TEST_NAME'")
[ "${LOG_COUNT:-0}" -ge 1 ] && green "Activity log captured server.rotate_cert_pin" \
    || red "No rotate activity log found (count: ${LOG_COUNT:-0})"

STATUS=$(do_checkin "$FP_B")
[ "$STATUS" = "200" ] && green "Re-TOFU checkin returns 200" \
    || red "Re-TOFU checkin status $STATUS (expected 200)"

STORED=$(psql_exec "SELECT COALESCE(cert_fingerprint, '') FROM servers WHERE id = '$TEST_SERVER_ID'")
[ "$STORED" = "$FP_B" ] && green "Re-TOFU captured new fingerprint" \
    || red "Re-TOFU capture failed — DB value: ${STORED:-<empty>}"

# ─────────────────────────────────────────────────────────────────────────
sect "PinnedFingerprintVerifier Construction (v2.7.18 regression guard)"
# ─────────────────────────────────────────────────────────────────────────

# With cert_fingerprint set on the server row, AgentRegistry::for_server
# calls RemoteAgentClient::new_with_pin, which constructs a
# rustls::ClientConfig via ::builder(). Under rustls 0.23+ this reads
# CryptoProvider::get_default(); without a process-level install_default()
# the call panics — which surfaces as HTTP 500 because axum's panic
# handler catches it. The agent_url points at an empty port so reqwest
# will fail to connect, and the handler should return 502 "Agent
# unreachable". Asserting exactly 502 distinguishes the happy path
# (graceful connect failure) from the v2.7.18 regression (panic → 500).
STATUS=$(curl -s -o /tmp/tier2_resp -w "%{http_code}" -X POST \
    "$API/api/servers/$TEST_SERVER_ID/test" \
    -H "$BEARER" -H "Content-Type: application/json" -d '{}' --max-time 30 2>/dev/null)
case "$STATUS" in
    ""|000)
        red "API did not respond to /test — suspected crash (status '$STATUS')"
        ;;
    500)
        BODY=$(cat /tmp/tier2_resp 2>/dev/null | head -c 200)
        red "Test endpoint returned 500 — possible rustls CryptoProvider panic: $BODY"
        ;;
    502)
        green "Test endpoint returned 502 (agent unreachable, CryptoProvider OK)"
        ;;
    *)
        red "Test endpoint returned unexpected status $STATUS (expected 502)"
        ;;
esac

HEALTH=$(curl -s -o /dev/null -w "%{http_code}" "$API/api/health" 2>/dev/null)
[ "$HEALTH" = "200" ] && green "API still responsive after pinned-TLS call" \
    || red "API health check after /test returned $HEALTH"

# ─────────────────────────────────────────────────────────────────────────
echo ""
echo "═══════════════════════════════════════════════"
echo "  Results: $PASS passed, $FAIL failed, $SKIP skipped ($TOTAL total)"
echo "═══════════════════════════════════════════════"

[ "$FAIL" -eq 0 ] && exit 0 || exit 1
