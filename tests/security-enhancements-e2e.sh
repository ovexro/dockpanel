#!/bin/bash
# E2E Tests for Security Enhancements (13 features)
# Run after deploying new agent + backend binaries
set -euo pipefail

API="http://127.0.0.1:3080"
PASS=0
FAIL=0
TOTAL=0

green() { echo -e "\e[32m✓ $1\e[0m"; }
red() { echo -e "\e[31m✗ $1\e[0m"; }
test_it() {
    TOTAL=$((TOTAL + 1))
    if eval "$2" > /dev/null 2>&1; then
        green "$1"
        PASS=$((PASS + 1))
    else
        red "$1"
        FAIL=$((FAIL + 1))
    fi
}

echo "═══════════════════════════════════════════════"
echo "  DockPanel Security Enhancements E2E Tests"
echo "═══════════════════════════════════════════════"

# Login to get admin token
echo ""
echo "Setting up..."
TOKEN=$(curl -s -X POST "$API/api/auth/login" \
    -H "Content-Type: application/json" \
    -d '{"email":"admin@dockpanel.dev","password":"REDACTED_DEMO_PASSWORD"}' \
    -D - 2>/dev/null | grep -oP 'token=\K[^;]+' || true)

if [ -z "$TOKEN" ]; then
    echo "Login failed - could not extract token"
    exit 1
fi

echo "Got token: ${TOKEN:0:20}..."
AUTHH="Cookie: token=$TOKEN"

echo ""
echo "── Feature 7: Immutable Audit Log ──"
test_it "GET /api/security/audit-log returns 200" \
    "curl -s -o /dev/null -w '%{http_code}' '$API/api/security/audit-log' -H '$AUTHH' | grep -q 200"

echo ""
echo "── Feature 9/11: Lockdown Management ──"
test_it "GET /api/security/lockdown returns status" \
    "curl -s '$API/api/security/lockdown' -H '$AUTHH' | grep -q 'active'"

test_it "POST lockdown/activate works" \
    "curl -s -X POST '$API/api/security/lockdown/activate' -H '$AUTHH' -H 'Content-Type: application/json' -d '{\"reason\":\"E2E test\"}' | grep -q 'locked'"

test_it "GET lockdown shows active" \
    "curl -s '$API/api/security/lockdown' -H '$AUTHH' | grep -q '\"active\":true'"

test_it "POST lockdown/deactivate works" \
    "curl -s -X POST '$API/api/security/lockdown/deactivate' -H '$AUTHH' | grep -q 'unlocked'"

echo ""
echo "── Feature 8: Registration Approval ──"
test_it "GET /api/security/pending-users returns list" \
    "curl -s -o /dev/null -w '%{http_code}' '$API/api/security/pending-users' -H '$AUTHH' | grep -q 200"

echo ""
echo "── Feature 5: Session Recordings ──"
test_it "GET /api/security/recordings returns list" \
    "curl -s '$API/api/security/recordings' -H '$AUTHH' | grep -q 'recordings'"

echo ""
echo "── Feature 10: Forensic Snapshot ──"
test_it "POST /api/security/forensic-snapshot captures state" \
    "curl -s -X POST '$API/api/security/forensic-snapshot' -H '$AUTHH' | grep -q 'snapshot_dir'"

echo ""
echo "── Feature 6: Tamper-Resistant Logs ──"
test_it "Audit log directory exists" \
    "ls /var/lib/dockpanel/audit/"

echo ""
echo "── Feature 5: Recordings directory ──"
test_it "Recordings directory exists" \
    "ls /var/lib/dockpanel/recordings/"

echo ""
echo "── Feature 2: DB Backup ──"
test_it "DB backup directory exists or can be created" \
    "mkdir -p /var/backups/dockpanel && ls /var/backups/dockpanel/"

echo ""
echo "── Database Migration ──"
test_it "security_audit_log table exists" \
    "docker exec dockpanel-postgres psql -U dockpanel -d dockpanel -c 'SELECT 1 FROM security_audit_log LIMIT 0'"

test_it "lockdown_state table exists" \
    "docker exec dockpanel-postgres psql -U dockpanel -d dockpanel -c 'SELECT active FROM lockdown_state WHERE id = 1'"

test_it "suspicious_events table exists" \
    "docker exec dockpanel-postgres psql -U dockpanel -d dockpanel -c 'SELECT 1 FROM suspicious_events LIMIT 0'"

test_it "terminal_recordings table exists" \
    "docker exec dockpanel-postgres psql -U dockpanel -d dockpanel -c 'SELECT 1 FROM terminal_recordings LIMIT 0'"

test_it "canary_files table exists" \
    "docker exec dockpanel-postgres psql -U dockpanel -d dockpanel -c 'SELECT 1 FROM canary_files LIMIT 0'"

test_it "users.approved column exists" \
    "docker exec dockpanel-postgres psql -U dockpanel -d dockpanel -c 'SELECT approved FROM users LIMIT 0'"

test_it "backups.sha256_hash column exists" \
    "docker exec dockpanel-postgres psql -U dockpanel -d dockpanel -c 'SELECT sha256_hash FROM backups LIMIT 0'"

test_it "Immutable trigger prevents DELETE" \
    "! docker exec dockpanel-postgres psql -U dockpanel -d dockpanel -c \"INSERT INTO security_audit_log (event_type, severity) VALUES ('test', 'info'); DELETE FROM security_audit_log WHERE event_type = 'test';\" 2>&1 | grep -q 'immutable'"

echo ""
echo "── Settings ──"
test_it "Security settings exist" \
    "docker exec dockpanel-postgres psql -U dockpanel -d dockpanel -c \"SELECT value FROM settings WHERE key = 'security_geo_alert_enabled'\" | grep -q true"

echo ""
echo "═══════════════════════════════════════════════"
echo "  Results: $PASS passed, $FAIL failed ($TOTAL total)"
echo "═══════════════════════════════════════════════"

[ "$FAIL" -eq 0 ] && exit 0 || exit 1
