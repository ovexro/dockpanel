#!/bin/bash
# DockPanel Full E2E Test Suite
# Tests all major features end-to-end against the live API
set -euo pipefail

API="http://127.0.0.1:3080"
PASS=0 FAIL=0 SKIP=0 TOTAL=0

green() { echo -e "\e[32m  ✓ $1\e[0m"; PASS=$((PASS+1)); TOTAL=$((TOTAL+1)); }
red()   { echo -e "\e[31m  ✗ $1\e[0m"; FAIL=$((FAIL+1)); TOTAL=$((TOTAL+1)); }
skip()  { echo -e "\e[33m  ~ $1\e[0m"; SKIP=$((SKIP+1)); TOTAL=$((TOTAL+1)); }

test_api() {
    local method=$1 path=$2 label=$3 expect=${4:-200} body=${5:-}
    local status
    if [ "$method" = "GET" ]; then
        status=$(curl -s -o /tmp/e2e_response -w "%{http_code}" "$API$path" -H "$AUTH" 2>/dev/null)
    else
        status=$(curl -s -o /tmp/e2e_response -w "%{http_code}" -X "$method" "$API$path" -H "$AUTH" -H "Content-Type: application/json" -d "${body:-{}}" 2>/dev/null)
    fi
    if [ "$status" = "$expect" ]; then green "$label ($status)"; else red "$label (got $status, expected $expect)"; fi
}

test_contains() {
    local method=$1 path=$2 label=$3 pattern=$4
    local body
    if [ "$method" = "GET" ]; then
        body=$(curl -s "$API$path" -H "$AUTH" 2>/dev/null)
    else
        body=$(curl -s -X "$method" "$API$path" -H "$AUTH" -H "Content-Type: application/json" -d "${5:-{}}" 2>/dev/null)
    fi
    if echo "$body" | grep -q "$pattern"; then green "$label"; else red "$label (missing: $pattern)"; fi
}

echo "═══════════════════════════════════════════════"
echo "  DockPanel Full E2E Test Suite"
echo "═══════════════════════════════════════════════"

# Auth
TOKEN=$(curl -s -X POST "$API/api/auth/login" -H "Content-Type: application/json" \
    -d "{\"email\":\"admin@dockpanel.dev\",\"password\":\"${DOCKPANEL_TEST_PASSWORD:-testpassword}\"}" -D - 2>/dev/null | grep -oP 'token=\K[^;]+')
[ -z "$TOKEN" ] && echo "FATAL: Login failed" && exit 1
AUTH="Cookie: token=$TOKEN"
echo "  Authenticated as admin@dockpanel.dev"
echo ""

echo "── Authentication ──"
test_api GET /api/auth/me "Get current user"
test_api GET /api/auth/setup-status "Setup status"
test_api GET /api/auth/2fa/status "2FA status"
test_api GET /api/auth/sessions "List sessions"
test_contains GET /api/auth/me "User has admin role" '"role":"admin"'
# Registration: test via direct curl (special case — body needed)
REG_RESP=$(curl -s -X POST "$API/api/auth/register" -H "$AUTH" -H "Content-Type: application/json" -d '{"email":"test@test.com","password":"testtest123"}' 2>/dev/null)
echo "$REG_RESP" | grep -q "disabled\|Disabled\|forbidden\|lockdown" && green "Registration blocked" || red "Registration NOT blocked: $REG_RESP"

echo ""
echo "── Dashboard ──"
test_api GET /api/dashboard/metrics-history "Metrics history"
test_api GET /api/dashboard/docker "Docker status"
test_api GET /api/dashboard/intelligence "Dashboard intelligence"
test_api GET /api/system/disk-io "Disk I/O stats"

echo ""
echo "── Sites ──"
test_api GET /api/sites "List sites"
test_api GET /api/php/versions "PHP versions available"
test_contains GET /api/php/versions "Has PHP version" "php"

echo ""
echo "── Databases ──"
test_api GET /api/databases "List databases"

echo ""
echo "── Docker Apps ──"
test_api GET /api/apps "List Docker apps"
test_api GET /api/apps/templates "App templates"
test_api GET /api/apps/images "Docker images"
test_api GET /api/apps/registries "Docker registries"
test_contains GET /api/apps/templates "Has templates" "name"

echo ""
echo "── Git Deploys ──"
test_api GET /api/git-deploys "List git deploys"

echo ""
echo "── CDN ──"
test_api GET /api/cdn/zones "List CDN zones"

echo ""
echo "── DNS ──"
test_api GET /api/dns/zones "List DNS zones"

echo ""
echo "── Mail ──"
test_api GET /api/mail/domains "Mail domains"
test_api GET /api/mail/queue "Mail queue"

echo ""
echo "── Backup Manager ──"
test_api GET /api/backup-orchestrator/health "Backup health"
test_api GET /api/backup-orchestrator/policies "Backup policies"
test_api GET /api/backup-orchestrator/db-backups "DB backups"
test_api GET /api/backup-orchestrator/volume-backups "Volume backups"
test_api GET /api/backup-orchestrator/verifications "Backup verifications"
test_api GET /api/backup-destinations "Backup destinations"
test_contains GET /api/backup-orchestrator/health "Has backup stats" "total_site_backups"

echo ""
echo "── Monitoring ──"
test_api GET /api/monitors "List monitors"
test_api GET /api/alerts "List alerts"
test_api GET /api/alerts/summary "Alert summary"
test_contains GET /api/alerts/summary "Has alert counts" "firing"

echo ""
echo "── Incidents / Status Page ──"
test_api GET /api/incidents "List incidents"

echo ""
echo "── Security ──"
test_api GET /api/security/overview "Security overview"
test_api GET /api/security/firewall "Firewall status"
test_api GET /api/security/fail2ban "Fail2ban status"
test_api GET /api/security/posture "Security posture"
test_api GET /api/security/scans "Scan history"
test_api GET /api/security/lockdown "Lockdown status"
test_api GET /api/security/audit-log "Immutable audit log"
test_api GET /api/security/recordings "Terminal recordings"
test_api GET /api/security/pending-users "Pending user approvals"
test_api GET /api/security/login-audit "Login audit"
test_api GET /api/security/panel-jail/status "Panel jail status"
test_api GET /api/security/report "Compliance report" 200
test_contains GET /api/security/lockdown "Has lockdown status" '"active"'
test_contains GET /api/security/firewall "Has firewall rules" "active"

echo ""
echo "── Security: Lockdown Cycle ──"
# Direct lockdown test (needs explicit Content-Type)
LOCK_RESP=$(curl -s -X POST "$API/api/security/lockdown/activate" -H "$AUTH" -H "Content-Type: application/json" -d '{"reason":"E2E test"}' -w "\n%{http_code}" 2>/dev/null)
LOCK_STATUS=$(echo "$LOCK_RESP" | tail -1)
[ "$LOCK_STATUS" = "200" ] && green "Activate lockdown (200)" || red "Activate lockdown ($LOCK_STATUS)"

LOCK_CHECK=$(curl -s "$API/api/security/lockdown" -H "$AUTH" 2>/dev/null)
echo "$LOCK_CHECK" | grep -q '"active":true' && green "Lockdown is active" || red "Lockdown not active: $LOCK_CHECK"

UNLOCK_RESP=$(curl -s -X POST "$API/api/security/lockdown/deactivate" -H "$AUTH" -H "Content-Type: application/json" -d '{}' -w "\n%{http_code}" 2>/dev/null)
UNLOCK_STATUS=$(echo "$UNLOCK_RESP" | tail -1)
[ "$UNLOCK_STATUS" = "200" ] && green "Deactivate lockdown (200)" || red "Deactivate lockdown ($UNLOCK_STATUS)"

UNLOCK_CHECK=$(curl -s "$API/api/security/lockdown" -H "$AUTH" 2>/dev/null)
echo "$UNLOCK_CHECK" | grep -q '"active":false' && green "Lockdown is inactive" || red "Lockdown still active"

echo ""
echo "── Secrets Manager ──"
test_api GET /api/secrets/vaults "List secret vaults"

echo ""
echo "── Integrations ──"
test_api GET /api/webhook-gateway/endpoints "Webhook endpoints"
test_api GET /api/extensions "Extensions"

echo ""
echo "── Notifications ──"
test_api GET /api/notifications/unread-count "Unread notification count"

echo ""
echo "── Users ──"
test_api GET /api/users "List users"
test_contains GET /api/users "Has admin user" "admin@dockpanel.dev"

echo ""
echo "── Settings ──"
test_api GET /api/settings "Get all settings"
test_api GET /api/settings/health "System health"
test_api GET /api/branding "Panel branding"
test_contains GET /api/settings/health "DB is healthy" '"db":"ok"'
test_contains GET /api/settings/health "Agent is healthy" '"agent":"ok"'

echo ""
echo "── System ──"
test_api GET /api/health "Health check"
test_api GET /api/system/info "System info"
test_api GET /api/system/processes "Running processes"
test_api GET /api/system/network "Network interfaces"
test_api GET /api/system/updates/count "Available updates"

echo ""
echo "── Logs ──"
test_api GET /api/logs "System logs"
test_api GET /api/logs/stats "Log statistics"
test_api GET /api/logs/sizes "Log file sizes"
test_api GET /api/logs/docker "Docker log containers"
test_api GET /api/activity "Activity log"

echo ""
echo "── API Keys ──"
test_api GET /api/api-keys "List API keys"

echo ""
echo "── Servers ──"
test_api GET /api/servers "List servers"
test_contains GET /api/servers "Has local server" "id"

echo ""
echo "── Database Migration ──"
test_it() { if docker exec dockpanel-postgres psql -U dockpanel -d dockpanel -c "$1" > /dev/null 2>&1; then green "$2"; else red "$2"; fi; TOTAL=$((TOTAL+1)); }
test_it "SELECT 1 FROM security_audit_log LIMIT 0" "security_audit_log table exists"
test_it "SELECT active FROM lockdown_state WHERE id = 1" "lockdown_state table exists"
test_it "SELECT 1 FROM suspicious_events LIMIT 0" "suspicious_events table exists"
test_it "SELECT 1 FROM terminal_recordings LIMIT 0" "terminal_recordings table exists"
test_it "SELECT 1 FROM canary_files LIMIT 0" "canary_files table exists"
test_it "SELECT approved FROM users LIMIT 0" "users.approved column exists"
test_it "SELECT sha256_hash FROM backups LIMIT 0" "backups.sha256_hash column exists"

echo ""
echo "── Filesystem ──"
[ -d /var/lib/dockpanel/audit ] && green "Audit log directory exists" || red "Audit log directory missing"
[ -d /var/lib/dockpanel/recordings ] && green "Recordings directory exists" || red "Recordings directory missing"
[ -d /var/backups/dockpanel ] && green "DB backup directory exists" || red "DB backup directory missing"
lsattr -d /var/lib/dockpanel/audit/ 2>/dev/null | grep -q "a" && green "Audit dir has append-only flag" || red "Audit dir missing append-only flag"

echo ""
echo "═══════════════════════════════════════════════"
echo "  Results: $PASS passed, $FAIL failed, $SKIP skipped ($TOTAL total)"
echo "═══════════════════════════════════════════════"

[ "$FAIL" -eq 0 ] && exit 0 || exit 1
