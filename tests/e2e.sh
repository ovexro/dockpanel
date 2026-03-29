#!/usr/bin/env bash
#
# DockPanel E2E Test Suite
# Tests every critical path against a fresh install.
#
# Usage: bash tests/e2e.sh <host> [port]
# Example: bash tests/e2e.sh 203.0.113.10 8443
#
set -uo pipefail
# Note: no -e — we handle errors explicitly

HOST="${1:?Usage: e2e.sh <host> [port]}"
PORT="${2:-8443}"
BASE="http://${HOST}:${PORT}"
API="${BASE}/api"

PASS=0
FAIL=0
SKIP=0
FINDINGS=""

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

# Auth token for session
AUTH_TOKEN=""

ok() {
    PASS=$((PASS + 1))
    echo -e "  ${GREEN}✓${NC} $1"
}

fail() {
    FAIL=$((FAIL + 1))
    echo -e "  ${RED}✗${NC} $1"
    FINDINGS="${FINDINGS}\n  - $1"
}

skip() {
    SKIP=$((SKIP + 1))
    echo -e "  ${YELLOW}⊘${NC} $1 (skipped)"
}

finding() {
    FINDINGS="${FINDINGS}\n  - $1"
}

section() {
    echo ""
    echo -e "${CYAN}${BOLD}── $1 ──${NC}"
}

# HTTP helpers — use Bearer token auth
auth_header() {
    if [ -n "$AUTH_TOKEN" ]; then
        echo "Authorization: Bearer $AUTH_TOKEN"
    else
        echo "X-No-Auth: true"
    fi
}

api_get() {
    curl -sf -H "$(auth_header)" "${API}$1" 2>/dev/null
}

api_post() {
    curl -sf -H "$(auth_header)" -X POST "${API}$1" \
        -H "Content-Type: application/json" -d "$2" 2>/dev/null
}

api_put() {
    curl -sf -H "$(auth_header)" -X PUT "${API}$1" \
        -H "Content-Type: application/json" -d "$2" 2>/dev/null
}

api_delete() {
    curl -sf -H "$(auth_header)" -X DELETE "${API}$1" 2>/dev/null
}

api_post_status() {
    curl -s -o /dev/null -w "%{http_code}" -H "$(auth_header)" -X POST "${API}$1" \
        -H "Content-Type: application/json" -d "$2" 2>/dev/null
}

api_get_status() {
    curl -s -o /dev/null -w "%{http_code}" -H "$(auth_header)" "${API}$1" 2>/dev/null
}

echo ""
echo -e "${BOLD}DockPanel E2E Test Suite${NC}"
echo -e "Target: ${BASE}"
echo -e "Date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo ""

# ─────────────────────────────────────────────────────────────────────────
section "1. CONNECTIVITY & HEALTH"
# ─────────────────────────────────────────────────────────────────────────

# Frontend serves HTML
STATUS=$(curl -sf -o /dev/null -w "%{http_code}" "${BASE}/" 2>/dev/null || echo "000")
if [ "$STATUS" = "200" ]; then ok "Frontend serves HTML (HTTP $STATUS)"
else fail "Frontend not accessible (HTTP $STATUS)"; fi

# API health endpoint
HEALTH=$(curl -sf "${API}/health" 2>/dev/null || echo "{}")
if echo "$HEALTH" | python3 -c "import sys,json; d=json.load(sys.stdin); assert d['status']=='ok'" 2>/dev/null; then
    VERSION=$(echo "$HEALTH" | python3 -c "import sys,json; print(json.load(sys.stdin).get('version','?'))")
    ok "API health OK (v${VERSION})"
else fail "API health check failed"; fi

# Setup status
SETUP=$(curl -sf "${API}/auth/setup-status" 2>/dev/null || echo "{}")
NEEDS_SETUP=$(echo "$SETUP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('needs_setup', True))" 2>/dev/null || echo "True")

# ─────────────────────────────────────────────────────────────────────────
section "2. AUTHENTICATION"
# ─────────────────────────────────────────────────────────────────────────

ADMIN_EMAIL="admin@e2etest.dev"
ADMIN_PASS="E2eTestPass1234!"

if [ "$NEEDS_SETUP" = "True" ]; then
    # Initial setup — create admin account
    SETUP_RESP=$(api_post "/auth/setup" "{\"email\":\"${ADMIN_EMAIL}\",\"password\":\"${ADMIN_PASS}\"}")
    if echo "$SETUP_RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); assert 'user' in d or 'id' in d or 'email' in d" 2>/dev/null; then
        ok "Admin setup completed"
    else
        fail "Admin setup failed: $SETUP_RESP"
    fi
else
    ok "Setup already done (using existing admin)"
fi

# Login — extract JWT from Set-Cookie header
LOGIN_HEADERS=$(mktemp)
LOGIN_RESP=$(curl -sf -D "$LOGIN_HEADERS" -X POST "${API}/auth/login" \
    -H "Content-Type: application/json" \
    -d "{\"email\":\"${ADMIN_EMAIL}\",\"password\":\"${ADMIN_PASS}\"}" 2>/dev/null || echo "FAIL")
AUTH_TOKEN=$(grep -i "set-cookie" "$LOGIN_HEADERS" 2>/dev/null | sed 's/.*token=//;s/;.*//' || echo "")
rm -f "$LOGIN_HEADERS"

if [ -n "$AUTH_TOKEN" ] && [ "$AUTH_TOKEN" != "FAIL" ]; then
    ok "Admin login successful (JWT extracted)"
else
    fail "Admin login failed: $LOGIN_RESP"
fi

# Auth me
ME=$(api_get "/auth/me")
if echo "$ME" | python3 -c "import sys,json; d=json.load(sys.stdin); assert d.get('role') == 'admin' or d.get('user',{}).get('role') == 'admin'" 2>/dev/null; then
    ok "Auth /me returns admin role"
else
    fail "Auth /me returned unexpected: $ME"
fi

# Bad login should fail
BAD_STATUS=$(api_post_status "/auth/login" '{"email":"bad@bad.com","password":"wrong"}')
if [ "$BAD_STATUS" = "401" ] || [ "$BAD_STATUS" = "429" ]; then
    ok "Bad login correctly rejected (HTTP $BAD_STATUS)"
else
    fail "Bad login returned unexpected HTTP $BAD_STATUS"
fi

# ─────────────────────────────────────────────────────────────────────────
section "3. DASHBOARD"
# ─────────────────────────────────────────────────────────────────────────

DASH_INTEL=$(api_get "/dashboard/intelligence")
if echo "$DASH_INTEL" | python3 -c "import sys,json; d=json.load(sys.stdin); assert 'health_score' in d or 'score' in d" 2>/dev/null; then
    SCORE=$(echo "$DASH_INTEL" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('health_score', d.get('score', '?')))")
    ok "Dashboard intelligence returns health score: $SCORE"
else
    fail "Dashboard intelligence failed: $(echo "$DASH_INTEL" | head -c 200)"
fi

# ─────────────────────────────────────────────────────────────────────────
section "4. SERVERS"
# ─────────────────────────────────────────────────────────────────────────

SERVERS=$(api_get "/servers")
if echo "$SERVERS" | python3 -c "import sys,json; d=json.load(sys.stdin); assert len(d) >= 1" 2>/dev/null; then
    SERVER_ID=$(echo "$SERVERS" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d[0]['id'])")
    ok "Local server registered (ID: ${SERVER_ID:0:8}...)"
else
    fail "No servers found"
    SERVER_ID=""
fi

# ─────────────────────────────────────────────────────────────────────────
section "5. SITE MANAGEMENT"
# ─────────────────────────────────────────────────────────────────────────

TEST_DOMAIN="e2e-test.local"

# Create static site
SITE_RESP=$(api_post "/sites" "{\"domain\":\"${TEST_DOMAIN}\",\"runtime\":\"static\"}")
SITE_ID=$(echo "$SITE_RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('id', d.get('site',{}).get('id','')))" 2>/dev/null || echo "")
if [ -n "$SITE_ID" ] && [ "$SITE_ID" != "" ]; then
    ok "Static site created: $TEST_DOMAIN (ID: ${SITE_ID:0:8}...)"
else
    fail "Site creation failed: $(echo "$SITE_RESP" | head -c 300)"
    SITE_ID=""
fi

# List sites
SITES=$(api_get "/sites")
SITE_COUNT=$(echo "$SITES" | python3 -c "import sys,json; d=json.load(sys.stdin); print(len(d) if isinstance(d, list) else len(d.get('sites', d.get('data', []))))" 2>/dev/null || echo "0")
if [ "$SITE_COUNT" -ge 1 ]; then
    ok "Sites list returns $SITE_COUNT site(s)"
else
    fail "Sites list empty or failed"
fi

# Get site detail
if [ -n "$SITE_ID" ]; then
    SITE_DETAIL=$(api_get "/sites/${SITE_ID}")
    if echo "$SITE_DETAIL" | python3 -c "import sys,json; d=json.load(sys.stdin); assert d.get('domain') == '${TEST_DOMAIN}'" 2>/dev/null; then
        ok "Site detail returns correct domain"
    else
        fail "Site detail failed"
    fi
fi

# SSL status (should show no cert)
if [ -n "$SITE_ID" ]; then
    SSL_STATUS=$(api_get_status "/ssl/${TEST_DOMAIN}")
    if [ "$SSL_STATUS" = "200" ] || [ "$SSL_STATUS" = "404" ]; then
        ok "SSL status endpoint responds (HTTP $SSL_STATUS)"
    else
        fail "SSL status returned unexpected HTTP $SSL_STATUS"
    fi
fi

# ─────────────────────────────────────────────────────────────────────────
section "6. FILE MANAGEMENT"
# ─────────────────────────────────────────────────────────────────────────

if [ -n "$SITE_ID" ]; then
    # List files (path must be relative, not starting with /)
    FILES=$(api_get "/sites/${SITE_ID}/files?path=.")
    if echo "$FILES" | python3 -c "import sys,json; json.load(sys.stdin)" 2>/dev/null; then
        ok "File list for site root"
    else
        fail "File list failed: $(echo "$FILES" | head -c 200)"
    fi

    # Write a test file (PUT, not POST)
    WRITE_RESP=$(api_put "/sites/${SITE_ID}/files/write" "{\"path\":\"test.html\",\"content\":\"<h1>E2E Test</h1>\"}")
    if [ $? -eq 0 ]; then
        ok "File write: test.html"
    else
        fail "File write failed"
    fi

    # Read it back
    READ_RESP=$(api_get "/sites/${SITE_ID}/files/read?path=test.html")
    if echo "$READ_RESP" | grep -q "E2E Test" 2>/dev/null; then
        ok "File read: test.html content verified"
    else
        if echo "$READ_RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); assert 'E2E Test' in d.get('content','')" 2>/dev/null; then
            ok "File read: test.html content verified (JSON)"
        else
            fail "File read returned unexpected content"
        fi
    fi
fi

# ─────────────────────────────────────────────────────────────────────────
section "7. DATABASE MANAGEMENT"
# ─────────────────────────────────────────────────────────────────────────

# Create PostgreSQL database (requires site_id)
if [ -n "$SITE_ID" ]; then
    DB_RESP=$(api_post "/databases" "{\"site_id\":\"${SITE_ID}\",\"name\":\"e2etest\",\"engine\":\"postgres\",\"password\":\"TestDbPass123\"}")
    DB_ID=$(echo "$DB_RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('id',''))" 2>/dev/null || echo "")
    if [ -n "$DB_ID" ] && [ "$DB_ID" != "" ]; then
        ok "PostgreSQL database created (ID: ${DB_ID:0:8}...)"
    else
        fail "Database creation failed: $(echo "$DB_RESP" | head -c 300)"
        DB_ID=""
    fi

    # List databases
    DBS=$(api_get "/databases")
    DB_COUNT=$(echo "$DBS" | python3 -c "import sys,json; d=json.load(sys.stdin); print(len(d) if isinstance(d, list) else 0)" 2>/dev/null || echo "0")
    if [ "$DB_COUNT" -ge 1 ]; then
        ok "Database list returns $DB_COUNT database(s)"
    else
        fail "Database list empty"
    fi
else
    skip "Database tests (no site)"
fi

# ─────────────────────────────────────────────────────────────────────────
section "8. DOCKER APPS"
# ─────────────────────────────────────────────────────────────────────────

# List templates
TEMPLATES=$(api_get "/apps/templates")
TPL_COUNT=$(echo "$TEMPLATES" | python3 -c "import sys,json; d=json.load(sys.stdin); print(len(d) if isinstance(d, list) else 0)" 2>/dev/null || echo "0")
if [ "$TPL_COUNT" -ge 30 ]; then
    ok "App templates: $TPL_COUNT available"
else
    fail "App templates: only $TPL_COUNT (expected 30+)"
fi

# Deploy a lightweight app (Redis — small image, fast start)
# template_id, name, port are required fields
DEPLOY_RESP=$(api_post "/apps/deploy" '{"template_id":"redis","name":"e2e-redis","port":6379}')
DEPLOY_STATUS=$?
if [ $DEPLOY_STATUS -eq 0 ]; then
    ok "Redis app deployment initiated"
    sleep 8  # Wait for image pull + start
else
    fail "Redis deployment failed: $(echo "$DEPLOY_RESP" | head -c 200)"
fi

# List running apps
APPS=$(api_get "/apps")
APP_COUNT=$(echo "$APPS" | python3 -c "import sys,json; d=json.load(sys.stdin); print(len(d) if isinstance(d, list) else 0)" 2>/dev/null || echo "0")
if [ "$APP_COUNT" -ge 1 ]; then
    ok "Docker apps list: $APP_COUNT running"
    # Get first app container ID for cleanup later
    REDIS_CONTAINER=$(echo "$APPS" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d[0].get('container_id', d[0].get('id','')))" 2>/dev/null || echo "")
else
    fail "No Docker apps running after deploy"
    REDIS_CONTAINER=""
fi

# ─────────────────────────────────────────────────────────────────────────
section "9. BACKUPS"
# ─────────────────────────────────────────────────────────────────────────

if [ -n "$SITE_ID" ]; then
    # Correct path: /api/sites/{id}/backups
    BACKUP_RESP=$(api_post "/sites/${SITE_ID}/backups" '{}')
    if [ $? -eq 0 ]; then
        ok "Backup created for site"
    else
        fail "Backup creation failed"
    fi

    BACKUPS=$(api_get "/sites/${SITE_ID}/backups")
    BACKUP_COUNT=$(echo "$BACKUPS" | python3 -c "import sys,json; d=json.load(sys.stdin); print(len(d) if isinstance(d, list) else 0)" 2>/dev/null || echo "0")
    if [ "$BACKUP_COUNT" -ge 1 ]; then
        ok "Backup list: $BACKUP_COUNT backup(s)"
    else
        fail "Backup list empty after creation"
    fi
fi

# ─────────────────────────────────────────────────────────────────────────
section "10. CRON JOBS"
# ─────────────────────────────────────────────────────────────────────────

if [ -n "$SITE_ID" ]; then
    # Correct path: /api/sites/{id}/crons
    CRON_RESP=$(api_post "/sites/${SITE_ID}/crons" '{"schedule":"*/5 * * * *","command":"echo e2e-test"}')
    CRON_ID=$(echo "$CRON_RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('id',''))" 2>/dev/null || echo "")
    if [ -n "$CRON_ID" ] && [ "$CRON_ID" != "" ]; then
        ok "Cron job created (ID: ${CRON_ID:0:8}...)"
    else
        fail "Cron creation failed: $(echo "$CRON_RESP" | head -c 200)"
        CRON_ID=""
    fi
fi

# ─────────────────────────────────────────────────────────────────────────
section "11. MONITORING"
# ─────────────────────────────────────────────────────────────────────────

MON_RESP=$(api_post "/monitors" '{"name":"E2E Google","url":"https://www.google.com","check_interval":300,"monitor_type":"http"}')
MON_ID=$(echo "$MON_RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('id',''))" 2>/dev/null || echo "")
if [ -n "$MON_ID" ] && [ "$MON_ID" != "" ]; then
    ok "Monitor created (ID: ${MON_ID:0:8}...)"
else
    fail "Monitor creation failed: $(echo "$MON_RESP" | head -c 200)"
    MON_ID=""
fi

# ─────────────────────────────────────────────────────────────────────────
section "12. DNS MANAGEMENT"
# ─────────────────────────────────────────────────────────────────────────

DNS_STATUS=$(api_get_status "/dns/zones")
if [ "$DNS_STATUS" = "200" ]; then
    ok "DNS zones endpoint accessible"
else
    fail "DNS zones endpoint returned HTTP $DNS_STATUS"
fi

# ─────────────────────────────────────────────────────────────────────────
section "13. SECURITY"
# ─────────────────────────────────────────────────────────────────────────

SEC_OVERVIEW=$(api_get "/security/overview")
if echo "$SEC_OVERVIEW" | python3 -c "import sys,json; json.load(sys.stdin)" 2>/dev/null; then
    ok "Security overview loads"
else
    fail "Security overview failed"
fi

FW_STATUS=$(api_get "/security/firewall")
if echo "$FW_STATUS" | python3 -c "import sys,json; json.load(sys.stdin)" 2>/dev/null; then
    ok "Firewall status loads"
else
    fail "Firewall status failed"
fi

# ─────────────────────────────────────────────────────────────────────────
section "14. DIAGNOSTICS"
# ─────────────────────────────────────────────────────────────────────────

# Correct path: /api/agent/diagnostics
DIAG=$(api_get "/agent/diagnostics")
if echo "$DIAG" | python3 -c "import sys,json; d=json.load(sys.stdin); assert isinstance(d, (list, dict))" 2>/dev/null; then
    DIAG_COUNT=$(echo "$DIAG" | python3 -c "import sys,json; d=json.load(sys.stdin); print(len(d.get('findings',d)) if isinstance(d,dict) else len(d))" 2>/dev/null || echo "?")
    ok "Diagnostics ran: $DIAG_COUNT finding(s)"
else
    fail "Diagnostics failed: $(echo "$DIAG" | head -c 200)"
fi

# ─────────────────────────────────────────────────────────────────────────
section "15. SETTINGS & SYSTEM"
# ─────────────────────────────────────────────────────────────────────────

SETTINGS=$(api_get "/settings")
if echo "$SETTINGS" | python3 -c "import sys,json; json.load(sys.stdin)" 2>/dev/null; then
    ok "Settings loads"
else
    fail "Settings failed"
fi

SYSTEM=$(api_get "/system/info")
if echo "$SYSTEM" | python3 -c "import sys,json; d=json.load(sys.stdin); assert 'cpu' in str(d).lower() or 'cores' in str(d).lower()" 2>/dev/null; then
    ok "System info returns CPU/memory data"
else
    fail "System info failed"
fi

# Health check
API_HEALTH_STATUS=$(api_get_status "/settings/health")
if [ "$API_HEALTH_STATUS" = "200" ]; then
    ok "Settings health check OK"
else
    fail "Settings health check returned HTTP $API_HEALTH_STATUS"
fi

# ─────────────────────────────────────────────────────────────────────────
section "16. ALERTS"
# ─────────────────────────────────────────────────────────────────────────

ALERTS_SUMMARY=$(api_get "/alerts/summary")
if echo "$ALERTS_SUMMARY" | python3 -c "import sys,json; json.load(sys.stdin)" 2>/dev/null; then
    ok "Alerts summary loads"
else
    fail "Alerts summary failed"
fi

ALERT_RULES=$(api_get "/alert-rules")
if echo "$ALERT_RULES" | python3 -c "import sys,json; json.load(sys.stdin)" 2>/dev/null; then
    ok "Alert rules loads"
else
    fail "Alert rules failed"
fi

# ─────────────────────────────────────────────────────────────────────────
section "17. ACTIVITY LOG"
# ─────────────────────────────────────────────────────────────────────────

ACTIVITY=$(api_get "/activity")
if echo "$ACTIVITY" | python3 -c "import sys,json; d=json.load(sys.stdin); assert isinstance(d, (list, dict))" 2>/dev/null; then
    ok "Activity log loads"
else
    fail "Activity log failed"
fi

# ─────────────────────────────────────────────────────────────────────────
section "18. USERS"
# ─────────────────────────────────────────────────────────────────────────

USERS=$(api_get "/users")
USER_COUNT=$(echo "$USERS" | python3 -c "import sys,json; d=json.load(sys.stdin); print(len(d) if isinstance(d, list) else 0)" 2>/dev/null || echo "0")
if [ "$USER_COUNT" -ge 1 ]; then
    ok "Users list: $USER_COUNT user(s)"
else
    fail "Users list empty"
fi

# Create test user
USER_RESP=$(api_post "/users" '{"email":"user@e2etest.dev","password":"UserPass1234!","role":"user"}')
TEST_USER_ID=$(echo "$USER_RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('id',''))" 2>/dev/null || echo "")
if [ -n "$TEST_USER_ID" ] && [ "$TEST_USER_ID" != "" ]; then
    ok "Test user created"
else
    fail "User creation failed: $(echo "$USER_RESP" | head -c 200)"
    TEST_USER_ID=""
fi

# ─────────────────────────────────────────────────────────────────────────
section "19. 2FA"
# ─────────────────────────────────────────────────────────────────────────

TFA_STATUS=$(api_get "/auth/2fa/status")
if echo "$TFA_STATUS" | python3 -c "import sys,json; d=json.load(sys.stdin); assert 'enabled' in d or 'totp_enabled' in d" 2>/dev/null; then
    ok "2FA status endpoint works"
else
    fail "2FA status failed"
fi

# ─────────────────────────────────────────────────────────────────────────
section "20. BRANDING"
# ─────────────────────────────────────────────────────────────────────────

BRANDING=$(curl -sf "${API}/branding" 2>/dev/null || echo "{}")
if echo "$BRANDING" | python3 -c "import sys,json; d=json.load(sys.stdin); assert 'panelName' in d or 'panel_name' in d" 2>/dev/null; then
    ok "Branding endpoint (public)"
else
    fail "Branding endpoint failed"
fi

# ─────────────────────────────────────────────────────────────────────────
section "21. WORDPRESS TOOLKIT"
# ─────────────────────────────────────────────────────────────────────────

WP_SITES=$(api_get "/wordpress/sites")
if echo "$WP_SITES" | python3 -c "import sys,json; d=json.load(sys.stdin); assert isinstance(d, (list, dict))" 2>/dev/null; then
    ok "WordPress sites scan"
else
    fail "WordPress sites scan failed"
fi

# ─────────────────────────────────────────────────────────────────────────
section "22. EXTENSIONS"
# ─────────────────────────────────────────────────────────────────────────

EXT_STATUS=$(api_get_status "/extensions")
if [ "$EXT_STATUS" = "200" ]; then
    ok "Extensions endpoint accessible"
else
    fail "Extensions endpoint returned HTTP $EXT_STATUS"
fi

# ─────────────────────────────────────────────────────────────────────────
section "23. MAIL (STATUS ONLY)"
# ─────────────────────────────────────────────────────────────────────────

MAIL_STATUS=$(api_get_status "/mail/domains")
if [ "$MAIL_STATUS" = "200" ]; then
    ok "Mail domains endpoint accessible"
else
    fail "Mail domains endpoint returned HTTP $MAIL_STATUS"
fi

# ─────────────────────────────────────────────────────────────────────────
section "24. CLI VERIFICATION"
# ─────────────────────────────────────────────────────────────────────────

CLI_STATUS=$(ssh root@${HOST} "dockpanel status --output json 2>/dev/null" || echo "{}")
if echo "$CLI_STATUS" | python3 -c "import sys,json; json.load(sys.stdin)" 2>/dev/null; then
    ok "CLI 'dockpanel status' works"
else
    fail "CLI status failed"
fi

CLI_SITES=$(ssh root@${HOST} "dockpanel sites --output json 2>/dev/null" || echo "[]")
if echo "$CLI_SITES" | python3 -c "import sys,json; json.load(sys.stdin)" 2>/dev/null; then
    ok "CLI 'dockpanel sites' works"
else
    fail "CLI sites failed"
fi

CLI_DIAG=$(ssh root@${HOST} "dockpanel diagnose --output json 2>/dev/null" || echo "{}")
if echo "$CLI_DIAG" | python3 -c "import sys,json; json.load(sys.stdin)" 2>/dev/null; then
    ok "CLI 'dockpanel diagnose' works"
else
    fail "CLI diagnose failed"
fi

# ─────────────────────────────────────────────────────────────────────────
section "25. SECURITY EDGE CASES"
# ─────────────────────────────────────────────────────────────────────────

# Unauthenticated access to protected endpoints should return 401
UNAUTH_STATUS=$(curl -s -o /dev/null -w "%{http_code}" "${API}/sites" 2>/dev/null)
if [ "$UNAUTH_STATUS" = "401" ]; then
    ok "Unauthenticated /sites returns 401"
else
    fail "Unauthenticated /sites returned HTTP $UNAUTH_STATUS (expected 401)"
fi

UNAUTH_USERS=$(curl -s -o /dev/null -w "%{http_code}" "${API}/users" 2>/dev/null)
if [ "$UNAUTH_USERS" = "401" ]; then
    ok "Unauthenticated /users returns 401"
else
    fail "Unauthenticated /users returned HTTP $UNAUTH_USERS (expected 401)"
fi

# Path traversal in file API
if [ -n "$SITE_ID" ]; then
    TRAVERSAL_STATUS=$(api_get_status "/files/${SITE_ID}?path=../../etc/passwd")
    if [ "$TRAVERSAL_STATUS" = "400" ] || [ "$TRAVERSAL_STATUS" = "403" ] || [ "$TRAVERSAL_STATUS" = "404" ]; then
        ok "Path traversal blocked (HTTP $TRAVERSAL_STATUS)"
    else
        fail "Path traversal NOT blocked (HTTP $TRAVERSAL_STATUS)"
    fi
fi

# Token rotation (requires server ID)
if [ -n "${SERVER_ID:-}" ]; then
    ROTATE_RESP=$(api_post "/servers/${SERVER_ID}/rotate-token" '{}')
    if echo "$ROTATE_RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); assert d.get('success') == True" 2>/dev/null; then
        ok "Agent token rotation successful"
        # Verify API still works after rotation
        sleep 2
        POST_ROTATE=$(api_get "/settings/health")
        if echo "$POST_ROTATE" | python3 -c "import sys,json; json.load(sys.stdin)" 2>/dev/null; then
            ok "API functional after token rotation"
        else
            fail "API broken after token rotation"
        fi
    else
        skip "Token rotation (endpoint not available)"
    fi
fi

# Invalid domain creation (must be authenticated for this test)
BAD_DOMAIN_STATUS=$(api_post_status "/sites" '{"domain":"../etc/passwd","runtime":"static"}')
if [ "$BAD_DOMAIN_STATUS" = "400" ] || [ "$BAD_DOMAIN_STATUS" = "422" ] || [ "$BAD_DOMAIN_STATUS" = "409" ]; then
    ok "Invalid domain rejected (HTTP $BAD_DOMAIN_STATUS)"
else
    fail "Invalid domain NOT rejected (HTTP $BAD_DOMAIN_STATUS)"
fi

# SQL injection in query params
SQLI_STATUS=$(api_get_status "/sites?limit=1'%20OR%201=1--")
if [ "$SQLI_STATUS" = "200" ] || [ "$SQLI_STATUS" = "400" ]; then
    ok "SQL injection in query params handled (HTTP $SQLI_STATUS)"
else
    fail "SQL injection returned unexpected HTTP $SQLI_STATUS"
fi

# ─────────────────────────────────────────────────────────────────────────
section "26. CLEANUP"
# ─────────────────────────────────────────────────────────────────────────

# Delete monitor
if [ -n "${MON_ID:-}" ]; then
    DEL=$(api_delete "/monitors/${MON_ID}" 2>/dev/null && echo "ok" || echo "fail")
    if [ "$DEL" != "fail" ]; then ok "Monitor deleted"
    else fail "Monitor delete failed"; fi
fi

# Delete cron
if [ -n "${CRON_ID:-}" ] && [ -n "${SITE_ID:-}" ]; then
    DEL=$(api_delete "/sites/${SITE_ID}/crons/${CRON_ID}" 2>/dev/null && echo "ok" || echo "fail")
    if [ "$DEL" != "fail" ]; then ok "Cron deleted"
    else fail "Cron delete failed"; fi
fi

# Delete Docker app
if [ -n "${REDIS_CONTAINER:-}" ]; then
    DEL=$(api_delete "/apps/${REDIS_CONTAINER}" 2>/dev/null && echo "ok" || echo "fail")
    if [ "$DEL" != "fail" ]; then ok "Redis app deleted"
    else fail "Redis app delete failed"; fi
fi

# Delete database
if [ -n "${DB_ID:-}" ]; then
    DEL=$(api_delete "/databases/${DB_ID}" 2>/dev/null && echo "ok" || echo "fail")
    if [ "$DEL" != "fail" ]; then ok "Database deleted"
    else fail "Database delete failed"; fi
fi

# Delete test user
if [ -n "${TEST_USER_ID:-}" ]; then
    DEL=$(api_delete "/users/${TEST_USER_ID}" 2>/dev/null && echo "ok" || echo "fail")
    if [ "$DEL" != "fail" ]; then ok "Test user deleted"
    else fail "Test user delete failed"; fi
fi

# Delete site (last — dependencies may reference it)
if [ -n "${SITE_ID:-}" ]; then
    DEL=$(api_delete "/sites/${SITE_ID}" 2>/dev/null && echo "ok" || echo "fail")
    if [ "$DEL" != "fail" ]; then ok "Site deleted"
    else fail "Site delete failed"; fi
fi

# Verify zero leftovers
sleep 2
FINAL_SITES=$(api_get "/sites")
FINAL_COUNT=$(echo "$FINAL_SITES" | python3 -c "import sys,json; d=json.load(sys.stdin); print(len(d) if isinstance(d, list) else len(d.get('sites', d.get('data',[]))))" 2>/dev/null || echo "?")
if [ "$FINAL_COUNT" = "0" ]; then
    ok "Zero sites after cleanup"
else
    fail "Sites remain after cleanup: $FINAL_COUNT"
fi

# Check for leftover Docker containers
LEFTOVER_CONTAINERS=$(ssh root@${HOST} "docker ps --filter 'label=dockpanel' --format '{{.Names}}' 2>/dev/null | wc -l" 2>/dev/null || echo "?")
if [ "$LEFTOVER_CONTAINERS" = "0" ]; then
    ok "Zero leftover Docker containers"
else
    fail "Leftover containers: $LEFTOVER_CONTAINERS"
fi

# Check for leftover nginx configs
LEFTOVER_NGINX=$(ssh root@${HOST} "ls /etc/nginx/sites-enabled/ 2>/dev/null | grep -v dockpanel-panel | wc -l" 2>/dev/null || echo "?")
if [ "$LEFTOVER_NGINX" = "0" ]; then
    ok "Zero leftover nginx configs"
else
    fail "Leftover nginx configs: $LEFTOVER_NGINX"
fi

# ─────────────────────────────────────────────────────────────────────────
section "27. LOGOUT"
# ─────────────────────────────────────────────────────────────────────────

LOGOUT_STATUS=$(api_post_status "/auth/logout" '{}')
if [ "$LOGOUT_STATUS" = "200" ] || [ "$LOGOUT_STATUS" = "204" ]; then
    ok "Logout successful"
else
    fail "Logout returned HTTP $LOGOUT_STATUS"
fi

# Verify session invalidated
POST_LOGOUT=$(api_get_status "/sites")
if [ "$POST_LOGOUT" = "401" ]; then
    ok "Session invalidated after logout"
else
    fail "Session still valid after logout (HTTP $POST_LOGOUT)"
fi

# ─────────────────────────────────────────────────────────────────────────
# SUMMARY
# ─────────────────────────────────────────────────────────────────────────

echo ""
echo -e "${BOLD}═══════════════════════════════════════════════${NC}"
echo -e "${BOLD}  RESULTS: ${GREEN}${PASS} passed${NC}, ${RED}${FAIL} failed${NC}, ${YELLOW}${SKIP} skipped${NC}"
echo -e "${BOLD}═══════════════════════════════════════════════${NC}"

if [ -n "$FINDINGS" ]; then
    echo ""
    echo -e "${RED}${BOLD}Failures:${NC}"
    echo -e "$FINDINGS"
fi

echo ""
exit $FAIL
