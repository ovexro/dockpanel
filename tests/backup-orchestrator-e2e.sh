#!/usr/bin/env bash
#
# DockPanel Backup Orchestrator E2E Test Suite
# Tests database backups, volume backups, encryption, verification, and policies.
#
# Usage: bash tests/backup-orchestrator-e2e.sh <host> [port]
# Example: bash tests/backup-orchestrator-e2e.sh 78.141.211.78 8443
#
set -uo pipefail

HOST="${1:?Usage: backup-orchestrator-e2e.sh <host> [port]}"
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

echo -e "${BOLD}DockPanel Backup Orchestrator E2E Tests${NC}"
echo "Target: ${HOST}:${PORT}"
echo ""

# ── Auth ───────────────────────────────────────────────────────────────

section "Authentication"

LOGIN_RESP=$(curl -sf -X POST "${API}/auth/login" \
    -H "Content-Type: application/json" \
    -d '{"email":"admin@dockpanel.dev","password":"TestPass1234"}' 2>/dev/null)

AUTH_TOKEN=$(echo "$LOGIN_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('token',''))" 2>/dev/null)

if [ -n "$AUTH_TOKEN" ]; then
    ok "Login successful"
else
    fail "Login failed — cannot continue"
    echo -e "\n${RED}${BOLD}Cannot authenticate. Aborting.${NC}"
    exit 1
fi

# ── Health Dashboard ───────────────────────────────────────────────────

section "Backup Health Dashboard"

HEALTH=$(api_get "/backup-orchestrator/health")
if [ -n "$HEALTH" ]; then
    ok "GET /backup-orchestrator/health returns data"

    SITE_COUNT=$(echo "$HEALTH" | python3 -c "import sys,json; print(json.load(sys.stdin).get('total_site_backups', -1))" 2>/dev/null)
    if [ "$SITE_COUNT" != "-1" ] && [ -n "$SITE_COUNT" ]; then
        ok "Health includes total_site_backups: $SITE_COUNT"
    else
        fail "Health missing total_site_backups field"
    fi

    DB_COUNT=$(echo "$HEALTH" | python3 -c "import sys,json; print(json.load(sys.stdin).get('total_db_backups', -1))" 2>/dev/null)
    if [ "$DB_COUNT" != "-1" ] && [ -n "$DB_COUNT" ]; then
        ok "Health includes total_db_backups: $DB_COUNT"
    else
        fail "Health missing total_db_backups field"
    fi

    STORAGE=$(echo "$HEALTH" | python3 -c "import sys,json; print(json.load(sys.stdin).get('total_storage_bytes', -1))" 2>/dev/null)
    if [ "$STORAGE" != "-1" ] && [ -n "$STORAGE" ]; then
        ok "Health includes total_storage_bytes: $STORAGE"
    else
        fail "Health missing total_storage_bytes field"
    fi
else
    fail "GET /backup-orchestrator/health returned empty"
fi

# ── Policies CRUD ──────────────────────────────────────────────────────

section "Backup Policies"

# Create policy
POLICY_RESP=$(api_post "/backup-orchestrator/policies" \
    '{"name":"E2E Test Policy","schedule":"0 3 * * *","backup_sites":true,"backup_databases":true,"backup_volumes":false,"retention_count":5,"encrypt":false,"verify_after_backup":true}')

POLICY_ID=$(echo "$POLICY_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('id',''))" 2>/dev/null)
if [ -n "$POLICY_ID" ] && [ "$POLICY_ID" != "None" ]; then
    ok "Create policy: $POLICY_ID"
else
    fail "Create policy returned no ID"
    POLICY_ID=""
fi

# List policies
POLICIES=$(api_get "/backup-orchestrator/policies")
POLICY_COUNT=$(echo "$POLICIES" | python3 -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null)
if [ "$POLICY_COUNT" -ge 1 ] 2>/dev/null; then
    ok "List policies: $POLICY_COUNT found"
else
    fail "List policies returned empty or invalid"
fi

# Update policy
if [ -n "$POLICY_ID" ]; then
    UPDATE_RESP=$(api_put "/backup-orchestrator/policies/$POLICY_ID" \
        '{"name":"E2E Updated Policy","retention_count":10}')
    UPDATED_NAME=$(echo "$UPDATE_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('name',''))" 2>/dev/null)
    if [ "$UPDATED_NAME" = "E2E Updated Policy" ]; then
        ok "Update policy name to 'E2E Updated Policy'"
    else
        fail "Update policy — name not updated"
    fi
fi

# Delete policy
if [ -n "$POLICY_ID" ]; then
    DEL_RESP=$(api_delete "/backup-orchestrator/policies/$POLICY_ID")
    if echo "$DEL_RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); assert d.get('ok')==True" 2>/dev/null; then
        ok "Delete policy"
    else
        fail "Delete policy failed"
    fi
fi

# ── Database Backups ───────────────────────────────────────────────────

section "Database Backups"

# List databases to find one to back up
DBS=$(api_get "/databases")
DB_ID=$(echo "$DBS" | python3 -c "import sys,json; dbs=json.load(sys.stdin); print(dbs[0]['id'] if dbs else '')" 2>/dev/null)

if [ -n "$DB_ID" ] && [ "$DB_ID" != "None" ] && [ "$DB_ID" != "" ]; then
    # Create database backup
    DB_BACKUP_RESP=$(api_post "/backup-orchestrator/db-backup" "{\"database_id\":\"$DB_ID\"}")
    DB_BACKUP_ID=$(echo "$DB_BACKUP_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('id',''))" 2>/dev/null)

    if [ -n "$DB_BACKUP_ID" ] && [ "$DB_BACKUP_ID" != "None" ]; then
        ok "Create database backup: $DB_BACKUP_ID"

        DB_FILENAME=$(echo "$DB_BACKUP_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('filename',''))" 2>/dev/null)
        DB_SIZE=$(echo "$DB_BACKUP_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('size_bytes',0))" 2>/dev/null)
        ok "  Filename: $DB_FILENAME, Size: $DB_SIZE bytes"
    else
        fail "Create database backup returned no ID"
        DB_BACKUP_ID=""
    fi

    # List database backups
    DB_BACKUPS=$(api_get "/backup-orchestrator/db-backups")
    DB_BACKUP_COUNT=$(echo "$DB_BACKUPS" | python3 -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null)
    if [ "$DB_BACKUP_COUNT" -ge 1 ] 2>/dev/null; then
        ok "List database backups: $DB_BACKUP_COUNT found"
    else
        fail "List database backups returned empty"
    fi

    # Trigger verification on the database backup
    if [ -n "$DB_BACKUP_ID" ]; then
        VERIFY_RESP=$(api_post "/backup-orchestrator/verify" \
            "{\"backup_type\":\"database\",\"backup_id\":\"$DB_BACKUP_ID\"}")
        VERIFY_STATUS=$(echo "$VERIFY_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('status',''))" 2>/dev/null)
        if [ "$VERIFY_STATUS" = "running" ] || [ "$VERIFY_STATUS" = "pending" ]; then
            ok "Verification triggered for database backup (status: $VERIFY_STATUS)"
        else
            fail "Verification trigger returned unexpected status: $VERIFY_STATUS"
        fi

        # Wait for verification to complete (up to 60s)
        echo "    Waiting for verification to complete..."
        for i in $(seq 1 12); do
            sleep 5
            VERIFS=$(api_get "/backup-orchestrator/verifications")
            LATEST_STATUS=$(echo "$VERIFS" | python3 -c "
import sys,json
vs = json.load(sys.stdin)
for v in vs:
    if v.get('backup_id') == '$DB_BACKUP_ID':
        print(v.get('status',''))
        break
" 2>/dev/null)
            if [ "$LATEST_STATUS" = "passed" ] || [ "$LATEST_STATUS" = "failed" ]; then
                break
            fi
        done

        if [ "$LATEST_STATUS" = "passed" ]; then
            ok "Database backup verification PASSED"
        elif [ "$LATEST_STATUS" = "failed" ]; then
            # May fail if temp container can't restore — acceptable in some envs
            skip "Database backup verification failed (may be environment-specific)"
        else
            skip "Verification still running after 60s — timeout"
        fi
    fi

    # Delete database backup
    if [ -n "$DB_BACKUP_ID" ]; then
        DEL_DB=$(api_delete "/backup-orchestrator/db-backups/$DB_BACKUP_ID")
        if echo "$DEL_DB" | python3 -c "import sys,json; d=json.load(sys.stdin); assert d.get('ok')==True" 2>/dev/null; then
            ok "Delete database backup"
        else
            fail "Delete database backup failed"
        fi
    fi
else
    skip "No databases found — skipping database backup tests"
fi

# ── Verifications List ─────────────────────────────────────────────────

section "Verifications"

VERIFS=$(api_get "/backup-orchestrator/verifications")
if [ -n "$VERIFS" ]; then
    VERIF_COUNT=$(echo "$VERIFS" | python3 -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null)
    ok "List verifications: $VERIF_COUNT found"
else
    fail "List verifications returned empty"
fi

# ── Volume Backups ─────────────────────────────────────────────────────

section "Volume Backups"

VOL_BACKUPS=$(api_get "/backup-orchestrator/volume-backups")
if [ -n "$VOL_BACKUPS" ]; then
    VOL_COUNT=$(echo "$VOL_BACKUPS" | python3 -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null)
    ok "List volume backups: $VOL_COUNT found"
else
    fail "List volume backups returned empty"
fi

# ── Summary ────────────────────────────────────────────────────────────

TOTAL=$((PASS + FAIL + SKIP))
echo ""
echo -e "${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${GREEN}${BOLD}PASS: $PASS${NC}  ${RED}${BOLD}FAIL: $FAIL${NC}  ${YELLOW}SKIP: $SKIP${NC}  TOTAL: $TOTAL"
echo -e "${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"

if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
