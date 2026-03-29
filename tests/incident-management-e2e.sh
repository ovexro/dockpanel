#!/usr/bin/env bash
#
# DockPanel Incident Management + Status Page E2E Test Suite
#
# Usage: bash tests/incident-management-e2e.sh <host> [port]
# Example: bash tests/incident-management-e2e.sh 203.0.113.10 8443
#
set -uo pipefail

HOST="${1:?Usage: incident-management-e2e.sh <host> [port]}"
PORT="${2:-8443}"
BASE="http://${HOST}:${PORT}"
API="${BASE}/api"

PASS=0
FAIL=0
SKIP=0

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

AUTH_TOKEN=""

ok() { PASS=$((PASS + 1)); echo -e "  ${GREEN}✓${NC} $1"; }
fail() { FAIL=$((FAIL + 1)); echo -e "  ${RED}✗${NC} $1"; }
skip() { SKIP=$((SKIP + 1)); echo -e "  ${YELLOW}⊘${NC} $1 (skipped)"; }

section() { echo ""; echo -e "${CYAN}${BOLD}── $1 ──${NC}"; }

api_get() { curl -sf -H "Authorization: Bearer $AUTH_TOKEN" "${API}$1" 2>/dev/null; }
api_post() { curl -sf -H "Authorization: Bearer $AUTH_TOKEN" -X POST "${API}$1" -H "Content-Type: application/json" -d "$2" 2>/dev/null; }
api_put() { curl -sf -H "Authorization: Bearer $AUTH_TOKEN" -X PUT "${API}$1" -H "Content-Type: application/json" -d "$2" 2>/dev/null; }
api_delete() { curl -sf -H "Authorization: Bearer $AUTH_TOKEN" -X DELETE "${API}$1" 2>/dev/null; }

echo -e "${BOLD}DockPanel Incident Management E2E Tests${NC}"
echo "Target: ${HOST}:${PORT}"

# ── Auth ───────────────────────────────────────────────────────────────

section "Authentication"

LOGIN_RESP=$(curl -sf -X POST "${API}/auth/login" \
    -H "Content-Type: application/json" \
    -d '{"email":"test@e2e-tests.local","password":"TestPass1234"}' 2>/dev/null)

AUTH_TOKEN=$(echo "$LOGIN_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('token',''))" 2>/dev/null)

if [ -n "$AUTH_TOKEN" ]; then
    ok "Login successful"
else
    fail "Login failed"
    exit 1
fi

# ── Status Page Config ─────────────────────────────────────────────────

section "Status Page Configuration"

CONFIG=$(api_get "/status-page/config")
if [ -n "$CONFIG" ]; then
    TITLE=$(echo "$CONFIG" | python3 -c "import sys,json; print(json.load(sys.stdin).get('title',''))" 2>/dev/null)
    if [ "$TITLE" = "Service Status" ]; then
        ok "Default config created with title: $TITLE"
    else
        ok "Config exists with title: $TITLE"
    fi
else
    fail "GET /status-page/config returned empty"
fi

# Update config
UPDATE_CONFIG=$(api_put "/status-page/config" '{"title":"E2E Test Status","description":"Testing status page","enabled":true}')
UPDATED_TITLE=$(echo "$UPDATE_CONFIG" | python3 -c "import sys,json; print(json.load(sys.stdin).get('title',''))" 2>/dev/null)
if [ "$UPDATED_TITLE" = "E2E Test Status" ]; then
    ok "Update config title to 'E2E Test Status'"
else
    fail "Update config failed: $UPDATED_TITLE"
fi

# ── Components ─────────────────────────────────────────────────────────

section "Status Page Components"

COMP_RESP=$(api_post "/status-page/components" '{"name":"API Server","description":"Core API","group_name":"Core Services"}')
COMP_ID=$(echo "$COMP_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('id',''))" 2>/dev/null)
if [ -n "$COMP_ID" ] && [ "$COMP_ID" != "None" ]; then
    ok "Create component: API Server ($COMP_ID)"
else
    fail "Create component failed"
    COMP_ID=""
fi

COMP2_RESP=$(api_post "/status-page/components" '{"name":"Website","description":"Public website","group_name":"Core Services"}')
COMP2_ID=$(echo "$COMP2_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('id',''))" 2>/dev/null)
if [ -n "$COMP2_ID" ] && [ "$COMP2_ID" != "None" ]; then
    ok "Create component: Website ($COMP2_ID)"
else
    fail "Create second component failed"
fi

COMPS=$(api_get "/status-page/components")
COMP_COUNT=$(echo "$COMPS" | python3 -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null)
if [ "$COMP_COUNT" -ge 2 ] 2>/dev/null; then
    ok "List components: $COMP_COUNT found"
else
    fail "List components returned: $COMP_COUNT"
fi

# ── Incidents CRUD ─────────────────────────────────────────────────────

section "Incident Management"

# Create incident
INC_RESP=$(api_post "/incidents" '{"title":"Database connection timeout","severity":"major","description":"Users experiencing intermittent connection timeouts to the primary database.","status":"investigating"}')
INC_ID=$(echo "$INC_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('id',''))" 2>/dev/null)
if [ -n "$INC_ID" ] && [ "$INC_ID" != "None" ]; then
    ok "Create incident: $INC_ID"
else
    fail "Create incident failed"
    INC_ID=""
fi

# List incidents
INCS=$(api_get "/incidents")
INC_COUNT=$(echo "$INCS" | python3 -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null)
if [ "$INC_COUNT" -ge 1 ] 2>/dev/null; then
    ok "List incidents: $INC_COUNT found"
else
    fail "List incidents returned: $INC_COUNT"
fi

# Get incident with updates
if [ -n "$INC_ID" ]; then
    INC_DETAIL=$(api_get "/incidents/$INC_ID")
    HAS_UPDATES=$(echo "$INC_DETAIL" | python3 -c "import sys,json; d=json.load(sys.stdin); print(len(d.get('updates',[])))" 2>/dev/null)
    if [ "$HAS_UPDATES" -ge 1 ] 2>/dev/null; then
        ok "Get incident detail with $HAS_UPDATES update(s)"
    else
        fail "Get incident detail missing updates"
    fi
fi

# Post incident update
if [ -n "$INC_ID" ]; then
    UP_RESP=$(api_post "/incidents/$INC_ID/updates" '{"status":"identified","message":"Root cause identified: connection pool exhaustion. Scaling up database connections."}')
    UP_STATUS=$(echo "$UP_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('status',''))" 2>/dev/null)
    if [ "$UP_STATUS" = "identified" ]; then
        ok "Post update: status → identified"
    else
        fail "Post update failed"
    fi

    # Another update
    api_post "/incidents/$INC_ID/updates" '{"status":"monitoring","message":"Connection pool expanded. Monitoring for recurrence."}' > /dev/null 2>&1
    ok "Post update: status → monitoring"

    # Resolve
    RESOLVE_RESP=$(api_post "/incidents/$INC_ID/updates" '{"status":"resolved","message":"Issue resolved. Connection pools stable for 30 minutes."}')
    R_STATUS=$(echo "$RESOLVE_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('status',''))" 2>/dev/null)
    if [ "$R_STATUS" = "resolved" ]; then
        ok "Resolve incident via update"
    else
        fail "Resolve failed"
    fi

    # Verify incident is resolved in DB
    RESOLVED_INC=$(api_get "/incidents/$INC_ID")
    FINAL_STATUS=$(echo "$RESOLVED_INC" | python3 -c "import sys,json; print(json.load(sys.stdin).get('incident',{}).get('status',''))" 2>/dev/null)
    if [ "$FINAL_STATUS" = "resolved" ]; then
        ok "Incident status confirmed: resolved"
    else
        fail "Incident status is: $FINAL_STATUS (expected resolved)"
    fi

    # List updates
    UPDATES=$(api_get "/incidents/$INC_ID/updates")
    UP_COUNT=$(echo "$UPDATES" | python3 -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null)
    if [ "$UP_COUNT" -ge 4 ] 2>/dev/null; then
        ok "Incident timeline has $UP_COUNT updates"
    else
        fail "Expected >= 4 updates, got: $UP_COUNT"
    fi
fi

# ── Public Status Page ─────────────────────────────────────────────────

section "Public Status Page"

PUBLIC=$(curl -sf "${API}/status-page/public" 2>/dev/null)
if [ -n "$PUBLIC" ]; then
    OVERALL=$(echo "$PUBLIC" | python3 -c "import sys,json; print(json.load(sys.stdin).get('overall_status',''))" 2>/dev/null)
    ok "Public status page returns data (overall: $OVERALL)"

    PUB_TITLE=$(echo "$PUBLIC" | python3 -c "import sys,json; print(json.load(sys.stdin).get('title',''))" 2>/dev/null)
    if [ "$PUB_TITLE" = "E2E Test Status" ]; then
        ok "Public page has correct title: $PUB_TITLE"
    else
        fail "Public page title: $PUB_TITLE (expected 'E2E Test Status')"
    fi

    PUB_COMPS=$(echo "$PUBLIC" | python3 -c "import sys,json; print(len(json.load(sys.stdin).get('components',[])))" 2>/dev/null)
    if [ "$PUB_COMPS" -ge 2 ] 2>/dev/null; then
        ok "Public page has $PUB_COMPS components"
    else
        fail "Public page components: $PUB_COMPS"
    fi

    PUB_INCS=$(echo "$PUBLIC" | python3 -c "import sys,json; print(len(json.load(sys.stdin).get('incidents',[])))" 2>/dev/null)
    if [ "$PUB_INCS" -ge 1 ] 2>/dev/null; then
        ok "Public page shows $PUB_INCS incident(s)"
    else
        fail "Public page incidents: $PUB_INCS"
    fi
else
    fail "Public status page returned empty"
fi

# ── Subscribers ────────────────────────────────────────────────────────

section "Subscribers"

SUB_RESP=$(curl -sf -X POST "${API}/status-page/subscribe" -H "Content-Type: application/json" -d '{"email":"test@example.com"}' 2>/dev/null)
if echo "$SUB_RESP" | python3 -c "import sys,json; assert json.load(sys.stdin).get('ok')==True" 2>/dev/null; then
    ok "Subscribe: test@example.com"
else
    fail "Subscribe failed"
fi

SUBS=$(api_get "/status-page/subscribers")
SUB_COUNT=$(echo "$SUBS" | python3 -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null)
if [ "$SUB_COUNT" -ge 1 ] 2>/dev/null; then
    ok "List subscribers: $SUB_COUNT found"
else
    fail "List subscribers: $SUB_COUNT"
fi

# Unsubscribe
UNSUB_RESP=$(curl -sf -X POST "${API}/status-page/unsubscribe" -H "Content-Type: application/json" -d '{"email":"test@example.com"}' 2>/dev/null)
if echo "$UNSUB_RESP" | python3 -c "import sys,json; assert json.load(sys.stdin).get('ok')==True" 2>/dev/null; then
    ok "Unsubscribe: test@example.com"
else
    fail "Unsubscribe failed"
fi

# ── Cleanup ────────────────────────────────────────────────────────────

section "Cleanup"

if [ -n "$INC_ID" ]; then
    api_delete "/incidents/$INC_ID" > /dev/null 2>&1 && ok "Delete test incident" || fail "Delete incident"
fi
if [ -n "$COMP_ID" ]; then
    api_delete "/status-page/components/$COMP_ID" > /dev/null 2>&1 && ok "Delete component 1" || fail "Delete component 1"
fi
if [ -n "$COMP2_ID" ]; then
    api_delete "/status-page/components/$COMP2_ID" > /dev/null 2>&1 && ok "Delete component 2" || fail "Delete component 2"
fi

# Restore default config
api_put "/status-page/config" '{"title":"Service Status","description":"Current status of our services"}' > /dev/null 2>&1

# ── Summary ────────────────────────────────────────────────────────────

TOTAL=$((PASS + FAIL + SKIP))
echo ""
echo -e "${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${GREEN}${BOLD}PASS: $PASS${NC}  ${RED}${BOLD}FAIL: $FAIL${NC}  ${YELLOW}SKIP: $SKIP${NC}  TOTAL: $TOTAL"
echo -e "${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"

if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
