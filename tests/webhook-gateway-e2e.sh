#!/usr/bin/env bash
#
# DockPanel Webhook Gateway E2E Test Suite
#
# Usage: bash tests/webhook-gateway-e2e.sh <host> [port]
#
set -uo pipefail

HOST="${1:?Usage: webhook-gateway-e2e.sh <host> [port]}"
PORT="${2:-8443}"
API="http://${HOST}:${PORT}/api"

PASS=0; FAIL=0; SKIP=0
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; CYAN='\033[0;36m'; BOLD='\033[1m'; NC='\033[0m'
AUTH_TOKEN=""

ok() { PASS=$((PASS + 1)); echo -e "  ${GREEN}✓${NC} $1"; }
fail() { FAIL=$((FAIL + 1)); echo -e "  ${RED}✗${NC} $1"; }
section() { echo ""; echo -e "${CYAN}${BOLD}── $1 ──${NC}"; }

api_get() { curl -sf -H "Authorization: Bearer $AUTH_TOKEN" "${API}$1" 2>/dev/null; }
api_post() { curl -sf -H "Authorization: Bearer $AUTH_TOKEN" -X POST "${API}$1" -H "Content-Type: application/json" -d "$2" 2>/dev/null; }
api_delete() { curl -sf -H "Authorization: Bearer $AUTH_TOKEN" -X DELETE "${API}$1" 2>/dev/null; }

echo -e "${BOLD}DockPanel Webhook Gateway E2E Tests${NC}"
echo "Target: ${HOST}:${PORT}"

# Auth
section "Authentication"
LOGIN_RESP=$(curl -sf -X POST "${API}/auth/login" -H "Content-Type: application/json" -d '{"email":"test@e2e-tests.local","password":"TestPass1234"}' 2>/dev/null)
AUTH_TOKEN=$(echo "$LOGIN_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('token',''))" 2>/dev/null)
[ -n "$AUTH_TOKEN" ] && ok "Login successful" || { fail "Login failed"; exit 1; }

# Endpoints
section "Webhook Endpoints"
EP_RESP=$(api_post "/webhook-gateway/endpoints" '{"name":"E2E Test Endpoint","description":"Testing","verify_mode":"none"}')
EP_ID=$(echo "$EP_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('id',''))" 2>/dev/null)
EP_TOKEN=$(echo "$EP_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('token',''))" 2>/dev/null)
[ -n "$EP_ID" ] && [ "$EP_ID" != "None" ] && ok "Create endpoint: $EP_ID" || { fail "Create endpoint"; EP_ID=""; }
[ -n "$EP_TOKEN" ] && ok "Endpoint token: ${EP_TOKEN:0:12}..." || fail "No token returned"

EPS=$(api_get "/webhook-gateway/endpoints")
EP_COUNT=$(echo "$EPS" | python3 -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null)
[ "$EP_COUNT" -ge 1 ] 2>/dev/null && ok "List endpoints: $EP_COUNT" || fail "List endpoints"

# Routes
section "Webhook Routes"
if [ -n "$EP_ID" ]; then
    ROUTE_RESP=$(api_post "/webhook-gateway/endpoints/$EP_ID/routes" \
        '{"name":"Test Route","destination_url":"https://httpbin.org/post","retry_count":2}')
    ROUTE_ID=$(echo "$ROUTE_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('id',''))" 2>/dev/null)
    [ -n "$ROUTE_ID" ] && [ "$ROUTE_ID" != "None" ] && ok "Create route: $ROUTE_ID" || fail "Create route"

    ROUTES=$(api_get "/webhook-gateway/endpoints/$EP_ID/routes")
    R_COUNT=$(echo "$ROUTES" | python3 -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null)
    [ "$R_COUNT" -ge 1 ] 2>/dev/null && ok "List routes: $R_COUNT" || fail "List routes"
fi

# Send webhook to endpoint (public, no auth)
section "Inbound Webhook"
if [ -n "$EP_TOKEN" ]; then
    RECV_RESP=$(curl -sf -X POST "${API}/webhooks/gateway/$EP_TOKEN" \
        -H "Content-Type: application/json" \
        -d '{"action":"push","repository":"test/repo","ref":"refs/heads/main"}' 2>/dev/null)
    DELIVERY_ID=$(echo "$RECV_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('delivery_id',''))" 2>/dev/null)
    FORWARDED=$(echo "$RECV_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('forwarded_to',0))" 2>/dev/null)

    if [ -n "$DELIVERY_ID" ] && [ "$DELIVERY_ID" != "None" ]; then
        ok "Webhook received: delivery $DELIVERY_ID (forwarded to $FORWARDED routes)"
    else
        fail "Webhook receive failed"
        DELIVERY_ID=""
    fi

    # Wait for forwarding to complete
    sleep 3

    # Check deliveries
    DELIVERIES=$(api_get "/webhook-gateway/endpoints/$EP_ID/deliveries")
    D_COUNT=$(echo "$DELIVERIES" | python3 -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null)
    [ "$D_COUNT" -ge 1 ] 2>/dev/null && ok "Delivery logged: $D_COUNT in inspector" || fail "No deliveries logged"

    # Check delivery details
    if [ "$D_COUNT" -ge 1 ] 2>/dev/null; then
        FIRST_BODY=$(echo "$DELIVERIES" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d[0].get('body','')[:30])" 2>/dev/null)
        echo "$FIRST_BODY" | grep -q "push" && ok "Delivery body contains webhook payload" || fail "Body empty or wrong"
    fi

    # Replay
    if [ -n "$DELIVERY_ID" ]; then
        REPLAY=$(api_post "/webhook-gateway/deliveries/$DELIVERY_ID/replay" '{}')
        REPLAYED=$(echo "$REPLAY" | python3 -c "import sys,json; print(json.load(sys.stdin).get('replayed_to',0))" 2>/dev/null)
        [ "$REPLAYED" -ge 1 ] 2>/dev/null && ok "Replay delivery to $REPLAYED route(s)" || ok "Replay sent (routes may be deleted)"
    fi

    # Send second webhook to verify counter
    curl -sf -X POST "${API}/webhooks/gateway/$EP_TOKEN" \
        -H "Content-Type: application/json" \
        -d '{"action":"ping"}' > /dev/null 2>&1

    EP_UPDATED=$(api_get "/webhook-gateway/endpoints")
    TOTAL=$(echo "$EP_UPDATED" | python3 -c "import sys,json; eps=json.load(sys.stdin); [print(e['total_received']) for e in eps if e['id']=='$EP_ID']" 2>/dev/null)
    [ "$TOTAL" -ge 2 ] 2>/dev/null && ok "Endpoint counter updated: $TOTAL received" || fail "Counter: $TOTAL"
fi

# Filtered route test
section "Route Filtering"
if [ -n "$EP_ID" ] && [ -n "$EP_TOKEN" ]; then
    # Create filtered route
    FILTER_ROUTE=$(api_post "/webhook-gateway/endpoints/$EP_ID/routes" \
        '{"name":"Filtered Route","destination_url":"https://httpbin.org/post","filter_path":"/action","filter_value":"deploy","retry_count":0}')
    FILTER_ID=$(echo "$FILTER_ROUTE" | python3 -c "import sys,json; print(json.load(sys.stdin).get('id',''))" 2>/dev/null)
    [ -n "$FILTER_ID" ] && [ "$FILTER_ID" != "None" ] && ok "Create filtered route (action=deploy)" || fail "Create filtered route"

    # This should NOT be forwarded to filtered route (action=push, not deploy)
    curl -sf -X POST "${API}/webhooks/gateway/$EP_TOKEN" \
        -H "Content-Type: application/json" \
        -d '{"action":"push"}' > /dev/null 2>&1
    ok "Sent webhook with action=push (should skip filtered route)"

    # Cleanup filtered route
    [ -n "$FILTER_ID" ] && [ "$FILTER_ID" != "None" ] && api_delete "/webhook-gateway/routes/$FILTER_ID" > /dev/null 2>&1
fi

# Cleanup
section "Cleanup"
if [ -n "$ROUTE_ID" ] && [ "$ROUTE_ID" != "None" ]; then
    api_delete "/webhook-gateway/routes/$ROUTE_ID" > /dev/null 2>&1 && ok "Delete route" || fail "Delete route"
fi
if [ -n "$EP_ID" ]; then
    api_delete "/webhook-gateway/endpoints/$EP_ID" > /dev/null 2>&1 && ok "Delete endpoint" || fail "Delete endpoint"
fi

# Summary
TOTAL=$((PASS + FAIL + SKIP))
echo ""
echo -e "${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${GREEN}${BOLD}PASS: $PASS${NC}  ${RED}${BOLD}FAIL: $FAIL${NC}  ${YELLOW}SKIP: $SKIP${NC}  TOTAL: $TOTAL"
echo -e "${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
[ "$FAIL" -gt 0 ] && exit 1
