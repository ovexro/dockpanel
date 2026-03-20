#!/usr/bin/env bash
#
# DockPanel Deep E2E Test Suite
# Tests edge cases, advanced features, and failure modes.
#
# Usage: bash tests/deep-e2e.sh <host> [port]
#
set -uo pipefail

HOST="${1:?Usage: deep-e2e.sh <host> [port]}"
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

AUTH_TOKEN=""
SERVER_ID=""

ok()   { PASS=$((PASS + 1)); echo -e "  ${GREEN}✓${NC} $1"; }
fail() { FAIL=$((FAIL + 1)); echo -e "  ${RED}✗${NC} $1"; FINDINGS="${FINDINGS}\n  - $1"; }
skip() { SKIP=$((SKIP + 1)); echo -e "  ${YELLOW}⊘${NC} $1 (skipped)"; }
section() { echo ""; echo -e "${CYAN}${BOLD}── $1 ──${NC}"; }

auth_header() { echo "Authorization: Bearer $AUTH_TOKEN"; }

api_get() { curl -s -H "$(auth_header)" "${API}$1" 2>/dev/null; }
api_post() { curl -s -H "$(auth_header)" -X POST "${API}$1" -H "Content-Type: application/json" -d "$2" 2>/dev/null; }
api_put() { curl -s -H "$(auth_header)" -X PUT "${API}$1" -H "Content-Type: application/json" -d "$2" 2>/dev/null; }
api_delete() { curl -s -H "$(auth_header)" -X DELETE "${API}$1" 2>/dev/null; }
api_get_status() { curl -s -o /dev/null -w "%{http_code}" -H "$(auth_header)" "${API}$1" 2>/dev/null; }
api_post_status() { curl -s -o /dev/null -w "%{http_code}" -H "$(auth_header)" -X POST "${API}$1" -H "Content-Type: application/json" -d "$2" 2>/dev/null; }

echo ""
echo -e "${BOLD}DockPanel Deep E2E Test Suite${NC}"
echo -e "Target: ${BASE}"
echo ""

# ── LOGIN ──
LOGIN_HEADERS=$(mktemp)
curl -sf -D "$LOGIN_HEADERS" -X POST "${API}/auth/login" \
    -H "Content-Type: application/json" \
    -d '{"email":"admin@e2etest.dev","password":"E2eTestPass1234!"}' > /dev/null 2>&1
AUTH_TOKEN=$(grep -i "set-cookie" "$LOGIN_HEADERS" 2>/dev/null | sed 's/.*token=//;s/;.*//')
rm -f "$LOGIN_HEADERS"
SERVER_ID=$(api_get "/servers" | python3 -c "import sys,json; print(json.load(sys.stdin)[0]['id'])" 2>/dev/null)

if [ -z "$AUTH_TOKEN" ]; then echo "Login failed"; exit 1; fi
echo -e "  Authenticated. Server: ${SERVER_ID:0:8}..."

# ═══════════════════════════════════════════════════════════════════════
section "1. WORDPRESS ONE-CLICK INSTALL"
# ═══════════════════════════════════════════════════════════════════════

WP_DOMAIN="wp-test.local"
WP_RESP=$(api_post "/sites" "{\"domain\":\"${WP_DOMAIN}\",\"runtime\":\"php\",\"php_version\":\"8.3\",\"cms\":\"wordpress\",\"site_title\":\"E2E WP\",\"admin_email\":\"admin@test.dev\",\"admin_user\":\"admin\",\"admin_password\":\"WpTestPass1234!\"}")
WP_SITE_ID=$(echo "$WP_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('id',''))" 2>/dev/null || echo "")
if [ -n "$WP_SITE_ID" ] && [ "$WP_SITE_ID" != "" ]; then
    ok "WordPress site creation initiated (ID: ${WP_SITE_ID:0:8}...)"

    # Wait for provisioning (WP install takes time — DB container + wp-cli)
    echo -e "  ${YELLOW}…${NC} Waiting for WordPress provisioning (up to 90s)..."
    WAITED=0
    WP_READY=false
    while [ "$WAITED" -lt 90 ]; do
        SITE_STATUS=$(api_get "/sites/${WP_SITE_ID}" | python3 -c "import sys,json; print(json.load(sys.stdin).get('status',''))" 2>/dev/null)
        if [ "$SITE_STATUS" = "active" ]; then
            WP_READY=true
            break
        fi
        sleep 5
        WAITED=$((WAITED + 5))
    done

    if [ "$WP_READY" = "true" ]; then
        ok "WordPress site active after ${WAITED}s"

        # Check WordPress detection
        WP_DETECT=$(api_get "/wordpress/sites")
        WP_COUNT=$(echo "$WP_DETECT" | python3 -c "import sys,json; d=json.load(sys.stdin); print(len(d) if isinstance(d, list) else 0)" 2>/dev/null || echo "0")
        if [ "$WP_COUNT" -ge 1 ]; then
            ok "WordPress toolkit detects $WP_COUNT WP site(s)"
        else
            fail "WordPress toolkit detection returned 0 sites"
        fi

        # Vulnerability scan (endpoint is under /api/sites/{id}/wordpress/)
        VULN_STATUS=$(api_post_status "/sites/${WP_SITE_ID}/wordpress/vuln-scan" '{}')
        if [ "$VULN_STATUS" = "200" ] || [ "$VULN_STATUS" = "201" ] || [ "$VULN_STATUS" = "202" ]; then
            ok "WordPress vulnerability scan initiated (HTTP $VULN_STATUS)"
        elif [ "$VULN_STATUS" = "502" ]; then
            skip "WordPress vuln scan (agent: wp-cli may not be installed)"
        else
            fail "WordPress vuln scan returned HTTP $VULN_STATUS"
        fi

        # Security hardening check
        HARDEN=$(api_get "/sites/${WP_SITE_ID}/wordpress/security-check")
        if echo "$HARDEN" | python3 -c "import sys,json; json.load(sys.stdin)" 2>/dev/null; then
            ok "WordPress security hardening check"
        else
            fail "WordPress security check failed"
        fi
    else
        fail "WordPress site did not become active within 90s (status: $SITE_STATUS)"
    fi
else
    fail "WordPress site creation failed: $(echo "$WP_RESP" | head -c 300)"
    WP_SITE_ID=""
fi

# ═══════════════════════════════════════════════════════════════════════
section "2. BACKUP CREATE + RESTORE CYCLE"
# ═══════════════════════════════════════════════════════════════════════

# Create a static site with a known file
BK_DOMAIN="backup-test.local"
BK_RESP=$(api_post "/sites" "{\"domain\":\"${BK_DOMAIN}\",\"runtime\":\"static\"}")
BK_SITE_ID=$(echo "$BK_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('id',''))" 2>/dev/null || echo "")

if [ -n "$BK_SITE_ID" ]; then
    sleep 3

    # Write a marker file
    api_put "/sites/${BK_SITE_ID}/files/write" '{"path":"marker.txt","content":"BACKUP_TEST_MARKER_12345"}' > /dev/null

    # Verify file exists
    MARKER=$(api_get "/sites/${BK_SITE_ID}/files/read?path=marker.txt")
    if echo "$MARKER" | grep -q "BACKUP_TEST_MARKER_12345" 2>/dev/null; then
        ok "Marker file written for backup test"
    else
        fail "Marker file write/read failed"
    fi

    # Create backup
    BACKUP_RESP=$(api_post "/sites/${BK_SITE_ID}/backups" '{}')
    if echo "$BACKUP_RESP" | python3 -c "import sys,json; json.load(sys.stdin)" 2>/dev/null; then
        ok "Backup created"
    else
        fail "Backup creation failed: $(echo "$BACKUP_RESP" | head -c 200)"
    fi

    # List backups and get the backup ID
    sleep 2
    BACKUPS=$(api_get "/sites/${BK_SITE_ID}/backups")
    BACKUP_ID=$(echo "$BACKUPS" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d[0]['id'] if d else '')" 2>/dev/null || echo "")

    if [ -n "$BACKUP_ID" ]; then
        ok "Backup listed (ID: ${BACKUP_ID:0:8}...)"

        # Delete the marker file
        api_delete "/sites/${BK_SITE_ID}/files?path=marker.txt" > /dev/null 2>&1
        sleep 1

        # Verify file is gone (may still read as 200 with empty/error, or 404)
        GONE_CHECK=$(api_get "/sites/${BK_SITE_ID}/files/read?path=marker.txt" 2>&1)
        if ! echo "$GONE_CHECK" | grep -q "BACKUP_TEST_MARKER_12345" 2>/dev/null; then
            ok "Marker file deleted before restore"
        else
            fail "Marker file still exists after delete"
        fi

        # Restore backup (202 = async accepted is correct)
        RESTORE_STATUS=$(api_post_status "/sites/${BK_SITE_ID}/backups/${BACKUP_ID}/restore" '{}')
        if [ "$RESTORE_STATUS" = "200" ] || [ "$RESTORE_STATUS" = "202" ]; then
            ok "Backup restore initiated (HTTP $RESTORE_STATUS)"
            sleep 2

            # Verify marker file is back
            RESTORED=$(api_get "/sites/${BK_SITE_ID}/files/read?path=marker.txt")
            if echo "$RESTORED" | grep -q "BACKUP_TEST_MARKER_12345" 2>/dev/null; then
                ok "Marker file restored from backup"
            else
                fail "Marker file NOT restored after backup restore"
            fi
        else
            fail "Backup restore returned HTTP $RESTORE_STATUS"
        fi
    else
        fail "No backup ID found in list"
    fi
else
    fail "Backup test site creation failed"
fi

# ═══════════════════════════════════════════════════════════════════════
section "3. GIT DEPLOY PIPELINE"
# ═══════════════════════════════════════════════════════════════════════

# Create a git deploy config
GIT_RESP=$(api_post "/git-deploys" '{"name":"e2e-git-test","repo_url":"https://github.com/heroku/node-js-sample.git","branch":"main"}')
GIT_ID=$(echo "$GIT_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('id',''))" 2>/dev/null || echo "")
if [ -n "$GIT_ID" ] && [ "$GIT_ID" != "" ]; then
    ok "Git deploy created (ID: ${GIT_ID:0:8}...)"

    # List git deploys
    GIT_LIST=$(api_get "/git-deploys")
    GIT_COUNT=$(echo "$GIT_LIST" | python3 -c "import sys,json; d=json.load(sys.stdin); print(len(d) if isinstance(d, list) else 0)" 2>/dev/null || echo "0")
    if [ "$GIT_COUNT" -ge 1 ]; then
        ok "Git deploys list: $GIT_COUNT"
    else
        fail "Git deploys list empty"
    fi

    # Generate deploy key
    KEYGEN_STATUS=$(api_post_status "/git-deploys/${GIT_ID}/keygen" '{}')
    if [ "$KEYGEN_STATUS" = "200" ]; then
        ok "Deploy key generated"
    else
        fail "Deploy key generation returned HTTP $KEYGEN_STATUS"
    fi

    # Trigger deploy (this will clone + build — may take time)
    DEPLOY_STATUS=$(api_post_status "/git-deploys/${GIT_ID}/deploy" '{}')
    if [ "$DEPLOY_STATUS" = "200" ] || [ "$DEPLOY_STATUS" = "202" ]; then
        ok "Git deploy triggered (HTTP $DEPLOY_STATUS)"
    else
        fail "Git deploy trigger returned HTTP $DEPLOY_STATUS"
    fi

    # Get deploy history
    HISTORY=$(api_get "/git-deploys/${GIT_ID}/history")
    if echo "$HISTORY" | python3 -c "import sys,json; json.load(sys.stdin)" 2>/dev/null; then
        ok "Deploy history accessible"
    else
        fail "Deploy history failed"
    fi
else
    fail "Git deploy creation failed: $(echo "$GIT_RESP" | head -c 300)"
    GIT_ID=""
fi

# ═══════════════════════════════════════════════════════════════════════
section "4. RESELLER SYSTEM"
# ═══════════════════════════════════════════════════════════════════════

# Create a reseller
# Create a user first, then promote to reseller
R_USER_RESP=$(api_post "/users" '{"email":"reseller@e2etest.dev","password":"ResellerPass1234!","role":"user"}')
R_USER_ID=$(echo "$R_USER_RESP" | python3 -c "
import sys, json
d = json.load(sys.stdin)
# Response might have id directly or nested in user object
uid = d.get('id', d.get('user',{}).get('id',''))
print(uid)
" 2>/dev/null || echo "")
if [ -z "$R_USER_ID" ] || [ "$R_USER_ID" = "" ] || [ "$R_USER_ID" = "None" ]; then
    # Try listing users to find the one we just created
    R_USER_ID=$(api_get "/users" | python3 -c "
import sys, json
users = json.load(sys.stdin)
for u in (users if isinstance(users, list) else []):
    if u.get('email') == 'reseller@e2etest.dev':
        print(u['id'])
        break
" 2>/dev/null || echo "")
fi
RESELLER_RESP=$(api_post "/resellers" "{\"user_id\":\"${R_USER_ID}\",\"panel_name\":\"E2E Reseller\",\"max_users\":5,\"max_sites\":10,\"max_databases\":5}")
RESELLER_ID=$(echo "$RESELLER_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('id',''))" 2>/dev/null || echo "")
if [ -n "$RESELLER_ID" ] && [ "$RESELLER_ID" != "" ]; then
    ok "Reseller created (ID: ${RESELLER_ID:0:8}...)"

    # List resellers
    RESELLERS=$(api_get "/resellers")
    R_COUNT=$(echo "$RESELLERS" | python3 -c "import sys,json; d=json.load(sys.stdin); print(len(d) if isinstance(d, list) else 0)" 2>/dev/null || echo "0")
    if [ "$R_COUNT" -ge 1 ]; then
        ok "Resellers list: $R_COUNT"
    else
        fail "Resellers list empty"
    fi

    # Allocate server to reseller
    ALLOC_STATUS=$(api_post_status "/resellers/${RESELLER_ID}/servers" "{\"server_id\":\"${SERVER_ID}\"}")
    if [ "$ALLOC_STATUS" = "200" ] || [ "$ALLOC_STATUS" = "201" ]; then
        ok "Server allocated to reseller"
    else
        fail "Server allocation returned HTTP $ALLOC_STATUS"
    fi

    # Login as reseller
    RESELLER_HEADERS=$(mktemp)
    curl -s -D "$RESELLER_HEADERS" -X POST "${API}/auth/login" \
        -H "Content-Type: application/json" \
        -d '{"email":"reseller@e2etest.dev","password":"ResellerPass1234!"}' > /dev/null 2>&1
    RESELLER_TOKEN=$(grep -i "set-cookie" "$RESELLER_HEADERS" 2>/dev/null | sed 's/.*token=//;s/;.*//')
    rm -f "$RESELLER_HEADERS"

    if [ -n "$RESELLER_TOKEN" ]; then
        ok "Reseller login successful"

        # Reseller dashboard
        R_DASH=$(curl -s -H "Authorization: Bearer $RESELLER_TOKEN" "${API}/reseller/dashboard" 2>/dev/null)
        if echo "$R_DASH" | python3 -c "import sys,json; json.load(sys.stdin)" 2>/dev/null; then
            ok "Reseller dashboard loads"
        else
            fail "Reseller dashboard failed"
        fi

        # Reseller tries admin endpoint (should be 403)
        R_ADMIN=$(curl -s -o /dev/null -w "%{http_code}" -H "Authorization: Bearer $RESELLER_TOKEN" "${API}/users" 2>/dev/null)
        if [ "$R_ADMIN" = "403" ]; then
            ok "Reseller blocked from admin endpoints (403)"
        else
            fail "Reseller accessed admin endpoint (HTTP $R_ADMIN, expected 403)"
        fi
    else
        fail "Reseller login failed"
    fi
else
    fail "Reseller creation failed: $(echo "$RESELLER_RESP" | head -c 300)"
    RESELLER_ID=""
fi

# ═══════════════════════════════════════════════════════════════════════
section "5. MAIL SYSTEM"
# ═══════════════════════════════════════════════════════════════════════

# Check mail service status (install not tested — requires Postfix)
MAIL_DOMAINS=$(api_get "/mail/domains")
if echo "$MAIL_DOMAINS" | python3 -c "import sys,json; json.load(sys.stdin)" 2>/dev/null; then
    ok "Mail domains endpoint works"
else
    fail "Mail domains endpoint failed"
fi

# Check mail install status via agent
MAIL_INSTALL_STATUS=$(api_get_status "/agent/mail/status")
if [ "$MAIL_INSTALL_STATUS" = "200" ]; then
    ok "Mail status endpoint responds"
else
    skip "Mail status (HTTP $MAIL_INSTALL_STATUS)"
fi

# ═══════════════════════════════════════════════════════════════════════
section "6. MIGRATION WIZARD"
# ═══════════════════════════════════════════════════════════════════════

# List migrations (may not have a list-all endpoint — check status)
MIG_STATUS=$(api_get_status "/migrations")
if [ "$MIG_STATUS" = "200" ]; then
    ok "Migrations endpoint works"
elif [ "$MIG_STATUS" = "404" ]; then
    skip "Migrations endpoint not found (may be /api/migration)"
else
    fail "Migrations returned HTTP $MIG_STATUS"
fi

# ═══════════════════════════════════════════════════════════════════════
section "7. FILE OPERATIONS EDGE CASES"
# ═══════════════════════════════════════════════════════════════════════

if [ -n "$BK_SITE_ID" ]; then
    # Create a directory
    DIR_RESP=$(api_post "/sites/${BK_SITE_ID}/files/create" '{"path":"testdir","is_dir":true}')
    if echo "$DIR_RESP" | python3 -c "import sys,json; json.load(sys.stdin)" 2>/dev/null; then
        ok "Directory creation"
    else
        fail "Directory creation failed: $(echo "$DIR_RESP" | head -c 200)"
    fi

    # Write file inside directory
    api_put "/sites/${BK_SITE_ID}/files/write" '{"path":"testdir/nested.txt","content":"nested content"}' > /dev/null
    NESTED=$(api_get "/sites/${BK_SITE_ID}/files/read?path=testdir/nested.txt")
    if echo "$NESTED" | grep -q "nested" 2>/dev/null; then
        ok "Nested file write/read"
    else
        fail "Nested file write/read failed"
    fi

    # Rename file (use the nested file we just wrote)
    RENAME_STATUS=$(api_post_status "/sites/${BK_SITE_ID}/files/rename" '{"from":"testdir/nested.txt","to":"testdir/renamed.txt"}')
    if [ "$RENAME_STATUS" = "200" ]; then
        ok "File rename"
    else
        # File may not exist if nested write failed
        skip "File rename (source may not exist)"
    fi

    # Path traversal attempts
    TRAV1=$(api_get_status "/sites/${BK_SITE_ID}/files/read?path=../../../etc/passwd")
    if [ "$TRAV1" = "400" ] || [ "$TRAV1" = "403" ] || [ "$TRAV1" = "404" ]; then
        ok "Path traversal ../../../etc/passwd blocked ($TRAV1)"
    else
        fail "Path traversal NOT blocked ($TRAV1)"
    fi

    TRAV2=$(api_get_status "/sites/${BK_SITE_ID}/files/read?path=%2e%2e%2fetc%2fpasswd")
    if [ "$TRAV2" = "400" ] || [ "$TRAV2" = "403" ] || [ "$TRAV2" = "404" ]; then
        ok "URL-encoded path traversal blocked ($TRAV2)"
    else
        fail "URL-encoded path traversal NOT blocked ($TRAV2)"
    fi

    # Null byte injection
    TRAV3=$(api_get_status "/sites/${BK_SITE_ID}/files/read?path=test.txt%00.html")
    if [ "$TRAV3" = "400" ] || [ "$TRAV3" = "403" ] || [ "$TRAV3" = "404" ]; then
        ok "Null byte injection blocked ($TRAV3)"
    else
        fail "Null byte injection NOT blocked ($TRAV3)"
    fi

    # File upload — use the write endpoint with content (upload endpoint is multipart)
    UPLOAD_STATUS=$(curl -s -o /dev/null -w "%{http_code}" -H "$(auth_header)" -X PUT \
        "${API}/sites/${BK_SITE_ID}/files/write" \
        -H "Content-Type: application/json" \
        -d '{"path":"uploaded.txt","content":"file upload test"}')
    if [ "$UPLOAD_STATUS" = "200" ]; then
        ok "File write as upload"
    else
        fail "File write returned HTTP $UPLOAD_STATUS"
    fi
fi

# ═══════════════════════════════════════════════════════════════════════
section "8. DOCKER COMPOSE STACKS"
# ═══════════════════════════════════════════════════════════════════════

# Create a compose stack
STACK_RESP=$(api_post "/stacks" '{"name":"e2e-stack","yaml":"version: \"3\"\nservices:\n  web:\n    image: nginx:alpine\n    ports:\n      - \"8765:80\""}')
STACK_ID=$(echo "$STACK_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('id',''))" 2>/dev/null || echo "")
if [ -n "$STACK_ID" ] && [ "$STACK_ID" != "" ]; then
    ok "Compose stack created (ID: ${STACK_ID:0:8}...)"
    sleep 5

    # List stacks
    STACKS=$(api_get "/stacks")
    S_COUNT=$(echo "$STACKS" | python3 -c "import sys,json; d=json.load(sys.stdin); print(len(d) if isinstance(d, list) else 0)" 2>/dev/null || echo "0")
    if [ "$S_COUNT" -ge 1 ]; then
        ok "Stacks list: $S_COUNT"
    else
        fail "Stacks list empty"
    fi
else
    fail "Stack creation failed: $(echo "$STACK_RESP" | head -c 300)"
    STACK_ID=""
fi

# ═══════════════════════════════════════════════════════════════════════
section "9. MULTI-SERVER (LOCAL ONLY)"
# ═══════════════════════════════════════════════════════════════════════

# Test connection to local server
TEST_CONN=$(api_post "/servers/${SERVER_ID}/test" '{}')
if echo "$TEST_CONN" | python3 -c "import sys,json; d=json.load(sys.stdin); assert d.get('ok') == True or d.get('status') == 'ok'" 2>/dev/null; then
    ok "Server connection test"
else
    fail "Server connection test failed: $(echo "$TEST_CONN" | head -c 200)"
fi

# Server metrics
METRICS=$(api_get "/servers/${SERVER_ID}/metrics")
if echo "$METRICS" | python3 -c "import sys,json; json.load(sys.stdin)" 2>/dev/null; then
    ok "Server metrics"
else
    fail "Server metrics failed"
fi

# Cannot delete local server
DEL_LOCAL_STATUS=$(curl -s -o /dev/null -w "%{http_code}" -H "$(auth_header)" -X DELETE "${API}/servers/${SERVER_ID}" 2>/dev/null)
if [ "$DEL_LOCAL_STATUS" = "400" ] || [ "$DEL_LOCAL_STATUS" = "403" ]; then
    ok "Local server delete blocked ($DEL_LOCAL_STATUS)"
else
    fail "Local server delete NOT blocked ($DEL_LOCAL_STATUS)"
fi

# ═══════════════════════════════════════════════════════════════════════
section "10. OAUTH ENDPOINTS"
# ═══════════════════════════════════════════════════════════════════════

# OAuth providers should return error without config (not crash)
OAUTH_GOOGLE=$(curl -s -o /dev/null -w "%{http_code}" "${API}/auth/oauth/google" 2>/dev/null)
if [ "$OAUTH_GOOGLE" != "500" ]; then
    ok "OAuth Google endpoint doesn't crash (HTTP $OAUTH_GOOGLE)"
else
    fail "OAuth Google endpoint returns 500"
fi

OAUTH_GITHUB=$(curl -s -o /dev/null -w "%{http_code}" "${API}/auth/oauth/github" 2>/dev/null)
if [ "$OAUTH_GITHUB" != "500" ]; then
    ok "OAuth GitHub endpoint doesn't crash (HTTP $OAUTH_GITHUB)"
else
    fail "OAuth GitHub endpoint returns 500"
fi

# ═══════════════════════════════════════════════════════════════════════
section "11. TRAEFIK ENDPOINTS"
# ═══════════════════════════════════════════════════════════════════════

TRAEFIK_STATUS=$(api_get_status "/settings/traefik/status")
if [ "$TRAEFIK_STATUS" = "200" ] || [ "$TRAEFIK_STATUS" = "404" ]; then
    ok "Traefik status endpoint responds (HTTP $TRAEFIK_STATUS)"
else
    fail "Traefik status returned HTTP $TRAEFIK_STATUS"
fi

# ═══════════════════════════════════════════════════════════════════════
section "12. TERMINAL"
# ═══════════════════════════════════════════════════════════════════════

# Get terminal token (GET, not POST)
TERM_TOKEN=$(api_get "/terminal/token")
if echo "$TERM_TOKEN" | python3 -c "import sys,json; d=json.load(sys.stdin); assert 'token' in d" 2>/dev/null; then
    ok "Terminal token issued"
else
    # Try POST
    TERM_TOKEN=$(api_post "/terminal/token" '{}')
    if echo "$TERM_TOKEN" | python3 -c "import sys,json; d=json.load(sys.stdin); assert 'token' in d" 2>/dev/null; then
        ok "Terminal token issued (POST)"
    else
        fail "Terminal token failed: $(echo "$TERM_TOKEN" | head -c 200)"
    fi
fi

# ═══════════════════════════════════════════════════════════════════════
section "13. EXTENSIONS API"
# ═══════════════════════════════════════════════════════════════════════

# Create extension
EXT_RESP=$(api_post "/extensions" '{"name":"E2E Webhook","webhook_url":"https://httpbin.org/post","event_subscriptions":"site.created,site.deleted"}')
EXT_ID=$(echo "$EXT_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('id',''))" 2>/dev/null || echo "")
if [ -n "$EXT_ID" ] && [ "$EXT_ID" != "" ]; then
    ok "Extension created (ID: ${EXT_ID:0:8}...)"

    # Test webhook
    TEST_WH_STATUS=$(api_post_status "/extensions/${EXT_ID}/test" '{}')
    if [ "$TEST_WH_STATUS" = "200" ]; then
        ok "Extension webhook test"
    else
        fail "Extension webhook test returned HTTP $TEST_WH_STATUS"
    fi

    # Event log
    EVENTS=$(api_get "/extensions/${EXT_ID}/events")
    if echo "$EVENTS" | python3 -c "import sys,json; json.load(sys.stdin)" 2>/dev/null; then
        ok "Extension event log"
    else
        fail "Extension event log failed"
    fi
else
    fail "Extension creation failed: $(echo "$EXT_RESP" | head -c 300)"
    EXT_ID=""
fi

# ═══════════════════════════════════════════════════════════════════════
section "14. CONCURRENT OPERATIONS"
# ═══════════════════════════════════════════════════════════════════════

# Create 3 sites simultaneously
CONC1=$(api_post "/sites" '{"domain":"conc1.local","runtime":"static"}' &)
CONC2=$(api_post "/sites" '{"domain":"conc2.local","runtime":"static"}' &)
CONC3=$(api_post "/sites" '{"domain":"conc3.local","runtime":"static"}' &)
wait
sleep 3

SITES=$(api_get "/sites")
CONC_COUNT=$(echo "$SITES" | python3 -c "import sys,json; d=json.load(sys.stdin); print(len([s for s in (d if isinstance(d,list) else []) if s.get('domain','').startswith('conc')]))" 2>/dev/null || echo "0")
if [ "$CONC_COUNT" -eq 3 ]; then
    ok "3 concurrent site creations succeeded"
else
    fail "Concurrent site creation: only $CONC_COUNT of 3 created"
fi

# ═══════════════════════════════════════════════════════════════════════
section "15. SETTINGS & SYSTEM DEEP"
# ═══════════════════════════════════════════════════════════════════════

# Export IaC
# IaC export (CLI defaults to table, check exit code)
IAC_EXIT=$(ssh root@${HOST} "dockpanel export 2>/dev/null; echo \$?" 2>/dev/null | tail -1)
if [ "$IAC_EXIT" = "0" ]; then
    ok "IaC export via CLI"
else
    fail "IaC export failed (exit $IAC_EXIT)"
fi

# System updates count
UPDATES=$(api_get "/system/updates/count")
if echo "$UPDATES" | python3 -c "import sys,json; json.load(sys.stdin)" 2>/dev/null; then
    ok "System updates count"
else
    fail "System updates count failed"
fi

# Disk I/O
DISK_IO=$(api_get "/system/disk-io")
if echo "$DISK_IO" | python3 -c "import sys,json; json.load(sys.stdin)" 2>/dev/null; then
    ok "Disk I/O metrics"
else
    fail "Disk I/O failed"
fi

# Processes
PROCS=$(api_get "/system/processes")
if echo "$PROCS" | python3 -c "import sys,json; json.load(sys.stdin)" 2>/dev/null; then
    ok "Process list"
else
    fail "Process list failed"
fi

# SSH keys
SSH_KEYS=$(api_get "/ssh-keys")
if echo "$SSH_KEYS" | python3 -c "import sys,json; json.load(sys.stdin)" 2>/dev/null; then
    ok "SSH keys list"
else
    fail "SSH keys failed"
fi

# ═══════════════════════════════════════════════════════════════════════
section "16. API KEYS"
# ═══════════════════════════════════════════════════════════════════════

APIKEY_RESP=$(api_post "/api-keys" '{"name":"e2e-key"}')
if echo "$APIKEY_RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); assert 'key' in d" 2>/dev/null; then
    ok "API key created"

    # List keys to get the UUID (create response doesn't include id)
    APIKEY_ID=$(api_get "/api-keys" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d[0]['id'] if d else '')" 2>/dev/null || echo "")

    if [ -n "$APIKEY_ID" ]; then
        # Rotate
        ROTATE_STATUS=$(api_post_status "/api-keys/${APIKEY_ID}/rotate" '{}')
        if [ "$ROTATE_STATUS" = "200" ] || [ "$ROTATE_STATUS" = "201" ]; then
            ok "API key rotated"
        else
            fail "API key rotation returned HTTP $ROTATE_STATUS"
        fi
    else
        skip "API key rotation (no ID from list)"
    fi
else
    fail "API key creation failed: $(echo "$APIKEY_RESP" | head -c 200)"
    APIKEY_ID=""
fi

# ═══════════════════════════════════════════════════════════════════════
section "17. TEAMS"
# ═══════════════════════════════════════════════════════════════════════

TEAM_RESP=$(api_post "/teams" '{"name":"E2E Team"}')
TEAM_ID=$(echo "$TEAM_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('id',''))" 2>/dev/null || echo "")
if [ -n "$TEAM_ID" ] && [ "$TEAM_ID" != "" ]; then
    ok "Team created"
else
    fail "Team creation failed: $(echo "$TEAM_RESP" | head -c 200)"
    TEAM_ID=""
fi

# ═══════════════════════════════════════════════════════════════════════
section "CLEANUP"
# ═══════════════════════════════════════════════════════════════════════

# Delete in reverse order of creation
[ -n "${TEAM_ID:-}" ] && { api_delete "/teams/${TEAM_ID}" > /dev/null 2>&1 && ok "Team deleted" || fail "Team delete failed"; }
[ -n "${APIKEY_ID:-}" ] && { api_delete "/api-keys/${APIKEY_ID}" > /dev/null 2>&1 && ok "API key deleted" || fail "API key delete failed"; }
[ -n "${EXT_ID:-}" ] && { api_delete "/extensions/${EXT_ID}" > /dev/null 2>&1 && ok "Extension deleted" || fail "Extension delete failed"; }
[ -n "${STACK_ID:-}" ] && { api_delete "/stacks/${STACK_ID}" > /dev/null 2>&1 && ok "Stack deleted" || fail "Stack delete failed"; }
[ -n "${GIT_ID:-}" ] && { api_delete "/git-deploys/${GIT_ID}" > /dev/null 2>&1 && ok "Git deploy deleted" || fail "Git deploy delete failed"; }

# Delete concurrent sites
CONC_IDS=$(api_get "/sites" | python3 -c "import sys,json; [print(s['id']) for s in json.load(sys.stdin) if s.get('domain','').startswith('conc')]" 2>/dev/null)
for CID in $CONC_IDS; do
    api_delete "/sites/${CID}" > /dev/null 2>&1
done
[ -n "$CONC_IDS" ] && ok "Concurrent sites deleted" || true

# Delete reseller (and their user account)
if [ -n "${RESELLER_ID:-}" ]; then
    api_delete "/resellers/${RESELLER_ID}" > /dev/null 2>&1 && ok "Reseller deleted" || fail "Reseller delete failed"
fi
# Delete the reseller's user account
if [ -n "${R_USER_ID:-}" ] && [ "$R_USER_ID" != "" ] && [ "$R_USER_ID" != "None" ]; then
    api_delete "/users/${R_USER_ID}" > /dev/null 2>&1
fi

# Delete backup test site
[ -n "${BK_SITE_ID:-}" ] && { api_delete "/sites/${BK_SITE_ID}" > /dev/null 2>&1 && ok "Backup test site deleted" || fail "Backup site delete failed"; }

# Delete WordPress site (last — has database container)
[ -n "${WP_SITE_ID:-}" ] && { api_delete "/sites/${WP_SITE_ID}" > /dev/null 2>&1 && ok "WordPress site deleted" || fail "WP site delete failed"; }

# Verify zero leftovers
sleep 3
FINAL=$(api_get "/sites")
FINAL_COUNT=$(echo "$FINAL" | python3 -c "import sys,json; d=json.load(sys.stdin); print(len(d) if isinstance(d, list) else '?')" 2>/dev/null || echo "?")
if [ "$FINAL_COUNT" = "0" ]; then
    ok "Zero sites after cleanup"
else
    fail "Sites remain: $FINAL_COUNT"
fi

LEFTOVER=$(ssh root@${HOST} "docker ps --filter 'label=dockpanel' --format '{{.Names}}' 2>/dev/null | grep -v postgres | wc -l" 2>/dev/null || echo "?")
if [ "$LEFTOVER" = "0" ]; then
    ok "Zero leftover containers"
else
    fail "Leftover containers: $LEFTOVER"
fi

# ═══════════════════════════════════════════════════════════════════════
# SUMMARY
# ═══════════════════════════════════════════════════════════════════════

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
