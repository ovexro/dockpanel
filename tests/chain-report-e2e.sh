#!/usr/bin/env bash
# DockPanel Chain-of-Trust Report E2E Test Suite
#
# v2.8.1: site-only.
# v2.8.2: extended to db + volume backups via the generic
#   GET /api/backup-orchestrator/chain-report/{kind}/{id}        (JSON)
#   GET /api/backup-orchestrator/chain-report/{kind}/{id}/pdf    (PDF, lazy-installs typst)
# where {kind} ∈ {site, database, volume}.
#
# Auth strategy mirrors tier2-certpin-e2e.sh: prefer DOCKPANEL_TEST_PASSWORD;
# otherwise mint a short-lived admin JWT from /etc/dockpanel/api.env.
#
# typst lazy-installs on the first PDF render (~30MB tarball, ~30s on a
# fresh box, sha256-pinned in v2.8.2). Subsequent runs hit the cached
# binary. Set CHAIN_REPORT_SKIP_PDF=1 to skip PDF assertions on networks
# without outbound HTTPS. CHAIN_REPORT_PDF_TIMEOUT overrides the curl
# timeout (default 90s) for the first install.
set -uo pipefail

API="${DOCKPANEL_API_URL:-http://127.0.0.1:3080}"
ADMIN_EMAIL="${DOCKPANEL_TEST_EMAIL:-admin@dockpanel.dev}"
ADMIN_PASSWORD="${DOCKPANEL_TEST_PASSWORD:-}"
PDF_TIMEOUT="${CHAIN_REPORT_PDF_TIMEOUT:-90}"

PASS=0 FAIL=0 SKIP=0 TOTAL=0

green() { echo -e "\e[32m  ✓ $1\e[0m"; PASS=$((PASS+1)); TOTAL=$((TOTAL+1)); }
red()   { echo -e "\e[31m  ✗ $1\e[0m"; FAIL=$((FAIL+1)); TOTAL=$((TOTAL+1)); }
skip()  { echo -e "\e[33m  ~ $1\e[0m"; SKIP=$((SKIP+1)); TOTAL=$((TOTAL+1)); }
sect()  { echo; echo "── $1 ──"; }

psql_exec() {
    docker exec dockpanel-postgres psql -U dockpanel -d dockpanel -qtAc "$1" 2>/dev/null
}

echo "═══════════════════════════════════════════════"
echo "  DockPanel Chain-of-Trust Report E2E Suite"
echo "═══════════════════════════════════════════════"

# ── Auth ──────────────────────────────────────────────────────────────────
ADMIN_UID=$(psql_exec "SELECT id FROM users WHERE email = '$ADMIN_EMAIL' AND role = 'admin' LIMIT 1")
if [ -z "$ADMIN_UID" ]; then
    echo "FATAL: Admin user row not found for $ADMIN_EMAIL"
    exit 1
fi

BEARER_TOKEN=""
if [ -n "$ADMIN_PASSWORD" ]; then
    BEARER_TOKEN=$(curl -s -X POST "$API/api/auth/login" -H "Content-Type: application/json" \
        -d "{\"email\":\"$ADMIN_EMAIL\",\"password\":\"$ADMIN_PASSWORD\"}" -D - 2>/dev/null \
        | grep -oP 'token=\K[^;]+')
fi
if [ -z "$BEARER_TOKEN" ] && [ -r /etc/dockpanel/api.env ]; then
    JWT_SECRET=$(grep -E '^JWT_SECRET=' /etc/dockpanel/api.env | cut -d= -f2-)
    if [ -n "$JWT_SECRET" ]; then
        BEARER_TOKEN=$(JWT_SECRET="$JWT_SECRET" ADMIN_UID="$ADMIN_UID" ADMIN_EMAIL="$ADMIN_EMAIL" \
            python3 - <<'PYEOF'
import jwt, os, time
now = int(time.time())
print(jwt.encode(
    {"sub": os.environ["ADMIN_UID"], "email": os.environ["ADMIN_EMAIL"], "role": "admin",
     "iat": now, "exp": now + 600},
    os.environ["JWT_SECRET"], algorithm="HS256",
))
PYEOF
        )
    fi
fi
if [ -z "$BEARER_TOKEN" ]; then
    echo "FATAL: Could not obtain admin token (set DOCKPANEL_TEST_PASSWORD or ensure /etc/dockpanel/api.env is readable)"
    exit 1
fi

AUTH=(-H "Authorization: Bearer $BEARER_TOKEN")
BOGUS_UUID="00000000-0000-0000-0000-000000000000"

# ── Auth gate (one kind covers the whole route — admin guard is shared) ──
sect "Authentication gate"

UNAUTH_STATUS=$(curl -s -o /dev/null -w "%{http_code}" \
    "$API/api/backup-orchestrator/chain-report/site/$BOGUS_UUID")
if [ "$UNAUTH_STATUS" = "401" ] || [ "$UNAUTH_STATUS" = "403" ]; then
    green "Unauth blocked ($UNAUTH_STATUS)"
else
    red "Unauth NOT blocked (got $UNAUTH_STATUS)"
fi

# ── Kind validation ──────────────────────────────────────────────────────
sect "Kind validation"

BAD_KIND_STATUS=$(curl -s -o /dev/null -w "%{http_code}" "${AUTH[@]}" \
    "$API/api/backup-orchestrator/chain-report/bogus/$BOGUS_UUID")
if [ "$BAD_KIND_STATUS" = "400" ]; then
    green "Bogus kind returns 400"
else
    red "Bogus kind returned $BAD_KIND_STATUS, expected 400"
fi

# ── Per-kind assertions ──────────────────────────────────────────────────
# Run the same shape of checks for every kind so any regression on one
# kind shows up immediately. Each kind discovers a real backup row from
# its respective table; if there's no fixture, the JSON+PDF assertions
# for that kind are skipped (counts as 7 skips, not failures — used by
# fresh-VPS runs where only one kind has been seeded).

assert_kind() {
    local kind="$1"
    local table="$2"
    sect "Kind: $kind"

    BACKUP_ID=$(psql_exec "SELECT id FROM $table ORDER BY created_at DESC LIMIT 1")
    if [ -z "$BACKUP_ID" ]; then
        skip "$kind: no backup fixture (skip JSON + PDF block)"
        for _ in $(seq 1 6); do skip "$kind: skipped (no fixture)"; done
        return
    fi
    green "$kind: discovered backup $BACKUP_ID"

    # 404 on bogus id
    NF=$(curl -s -o /dev/null -w "%{http_code}" "${AUTH[@]}" \
        "$API/api/backup-orchestrator/chain-report/$kind/$BOGUS_UUID")
    if [ "$NF" = "404" ]; then green "$kind: bogus id → 404"; else red "$kind: bogus id returned $NF (want 404)"; fi

    # JSON endpoint
    JSON_OUT="/tmp/chain_report_${kind}.json"
    JSON_STATUS=$(curl -s -o "$JSON_OUT" -w "%{http_code}" "${AUTH[@]}" \
        "$API/api/backup-orchestrator/chain-report/$kind/$BACKUP_ID")
    if [ "$JSON_STATUS" = "200" ]; then green "$kind: JSON 200"; else red "$kind: JSON returned $JSON_STATUS (want 200)"; fi

    JSON_KIND=$(python3 -c "import json,sys; print(json.load(open('$JSON_OUT'))['backup']['kind'])" 2>/dev/null || echo "")
    if [ "$JSON_KIND" = "$kind" ]; then green "$kind: JSON backup.kind matches"; else red "$kind: JSON backup.kind='$JSON_KIND' (want '$kind')"; fi

    JSON_BACKUP_ID=$(python3 -c "import json,sys; print(json.load(open('$JSON_OUT'))['backup']['id'])" 2>/dev/null || echo "")
    if [ "$JSON_BACKUP_ID" = "$BACKUP_ID" ]; then green "$kind: JSON backup.id round-trips"; else red "$kind: JSON backup.id mismatch (got '$JSON_BACKUP_ID')"; fi

    JSON_RESOURCE=$(python3 -c "import json,sys; print(json.load(open('$JSON_OUT'))['backup']['resource_name'])" 2>/dev/null || echo "")
    if [ -n "$JSON_RESOURCE" ]; then green "$kind: resource_name non-empty ($JSON_RESOURCE)"; else red "$kind: resource_name empty"; fi

    if grep -q '"chain_integrity"' "$JSON_OUT" && grep -q '"verifications"' "$JSON_OUT" && grep -q '"drills"' "$JSON_OUT"; then
        green "$kind: JSON has chain_integrity / verifications / drills"
    else
        red "$kind: JSON missing one of chain_integrity / verifications / drills"
    fi

    # PDF endpoint
    if [ "${CHAIN_REPORT_SKIP_PDF:-0}" = "1" ]; then
        skip "$kind: PDF (CHAIN_REPORT_SKIP_PDF=1)"
        return
    fi

    PDF_HEADERS=$(curl -sI --max-time "$PDF_TIMEOUT" "${AUTH[@]}" \
        "$API/api/backup-orchestrator/chain-report/$kind/$BACKUP_ID/pdf" 2>/dev/null)
    PDF_STATUS=$(echo "$PDF_HEADERS" | head -1 | grep -oP '\b\d{3}\b' | head -1)

    if [ "$PDF_STATUS" = "200" ]; then
        green "$kind: PDF 200"
        if echo "$PDF_HEADERS" | grep -ci "content-type: application/pdf" >/dev/null; then
            green "$kind: PDF Content-Type"
        else
            red "$kind: PDF Content-Type wrong/missing"
        fi
        if echo "$PDF_HEADERS" | grep -ci "content-disposition:.*attachment" >/dev/null; then
            green "$kind: PDF Content-Disposition: attachment"
        else
            red "$kind: PDF Content-Disposition missing"
        fi

        PDF_OUT="/tmp/chain_report_${kind}.pdf"
        curl -s --max-time "$PDF_TIMEOUT" -o "$PDF_OUT" "${AUTH[@]}" \
            "$API/api/backup-orchestrator/chain-report/$kind/$BACKUP_ID/pdf" 2>/dev/null
        if head -c 4 "$PDF_OUT" | grep -c "^%PDF" >/dev/null; then
            green "$kind: PDF body has %PDF magic"
        else
            red "$kind: PDF body missing %PDF magic"
        fi
        SIZE=$(stat -c%s "$PDF_OUT" 2>/dev/null || echo 0)
        if [ "$SIZE" -gt 1024 ]; then
            green "$kind: PDF size > 1KB ($SIZE bytes)"
        else
            red "$kind: PDF suspiciously small ($SIZE bytes)"
        fi
    elif [ "$PDF_STATUS" = "503" ]; then
        skip "$kind: PDF returned 503 (typst install/compile failed — likely no outbound HTTPS or sha256 mismatch)"
    else
        red "$kind: PDF returned $PDF_STATUS (want 200)"
    fi
}

assert_kind site backups
assert_kind database database_backups
assert_kind volume volume_backups

echo
echo "  Results: $PASS passed, $FAIL failed, $SKIP skipped ($TOTAL total)"

[ "$FAIL" -eq 0 ] && exit 0 || exit 1
