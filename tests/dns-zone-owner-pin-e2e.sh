#!/usr/bin/env bash
# dns-zone-owner-pin-e2e.sh — a DNS zone belongs to the admin who added it.
#
#   A zone carries its owner's stored Cloudflare credential. v2.16.0 removed the
#   `user_id` scope from get_zone/list_zones ("any admin may manage any zone"),
#   so a second admin could read, change and delete the first admin's zones and
#   act through the first admin's Cloudflare token, while SECURITY.md claimed
#   every resource endpoint verifies ownership. Reported by a third party;
#   fixed by scoping every zone lookup by the caller.
#
# Pure source analysis: no box, no network, no build.
# NO PIPES INTO `grep -q` (pipefail): every arm uses a here-string.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

DNS=panel/backend/src/routes/dns.rs
[ -f "$DNS" ] || bad "MISSING SUBJECT FILE: $DNS"

echo "== §A  every zone lookup in dns.rs is scoped to the caller =="

GETZ=$(awk '/^async fn get_zone\(/{i=1} i{print} i && /^}$/{exit}' "$DNS")
if [ "$(grep -c . <<< "$GETZ")" -ge 8 ]; then ok "A1-control get_zone body extracted"; else bad "A1-control get_zone body not extracted (the extractor broke)"; fi

if grep -qE 'owner_id: Uuid' <<< "$GETZ"; then ok "A2 get_zone takes the owner"; else bad "A2 get_zone must take an owner_id"; fi
if grep -qE 'WHERE id = \$1 AND user_id = \$2' <<< "$GETZ"; then ok "A3 get_zone SQL filters on user_id"; else bad "A3 get_zone SQL must filter on user_id"; fi
if grep -qE '\.bind\(owner_id\)' <<< "$GETZ"; then ok "A4 get_zone binds the owner"; else bad "A4 get_zone must bind owner_id"; fi

TOTAL=$(grep -c 'get_zone(&state' "$DNS")
SCOPED=$(grep -c 'get_zone(&state, id, claims.sub)' "$DNS")
if [ "$TOTAL" -ge 11 ]; then ok "A5-control $TOTAL get_zone call sites found"; else bad "A5-control only $TOTAL call sites (the counter broke)"; fi
if [ "$TOTAL" = "$SCOPED" ]; then ok "A6 all $TOTAL call sites pass claims.sub"; else bad "A6 $((TOTAL-SCOPED)) call site(s) do not pass claims.sub"; fi

LISTZ=$(awk '/^pub async fn list_zones\(/{i=1} i{print} i && /^}$/{exit}' "$DNS")
if grep -qE 'FROM dns_zones WHERE user_id = \$1' <<< "$LISTZ"; then ok "A7 list_zones lists only the caller's zones"; else bad "A7 list_zones must filter on user_id"; fi
if grep -qE '\.bind\(claims\.sub\)' <<< "$LISTZ"; then ok "A8 list_zones binds the caller"; else bad "A8 list_zones must bind claims.sub"; fi

if grep -qE 'any admin may manage any zone' "$DNS"; then bad "A9 the 'any admin may manage any zone' rationale is back"; else ok "A9 no 'any admin may manage any zone' comment"; fi

echo "== §B  the docs say what the code does =="
if grep -qE 'owner-scoped' docs/guides/roles-and-ownership.md; then ok "B1 roles-and-ownership states zone ownership"; else bad "B1 roles-and-ownership must state that DNS zones are per-admin"; fi

echo; echo "passed $PASS, failed $FAIL"
[ "$FAIL" = 0 ]
