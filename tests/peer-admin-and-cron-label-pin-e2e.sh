#!/usr/bin/env bash
# peer-admin-and-cron-label-pin-e2e.sh — two boundaries the panel promises.
#
#   §A  An administrator never reaches another administrator's account. v2.196.0
#       guarded /api/users with `is_other_admin`; PUT/DELETE /api/reseller/users/{id}
#       looked an admin's target up with `WHERE id = $1` and no role check, so the
#       guard was one door of two. Deleting through that door also skipped the
#       server re-point, and `servers.user_id` cascades to `sites`.
#   §B  A cron label stays on its one crontab line. It is written after the
#       `# dockpanel:<id>` marker; a line break started a second, unmarked line in
#       root's crontab that no sync ever removed and the command filter never saw.
#       Validated by the panel AND by the agent (the sink).
#
# Pure source analysis: no box, no network, no build.
# NO PIPES INTO `grep -q` (pipefail): every arm uses a here-string.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

RES=panel/backend/src/routes/reseller_dashboard.rs
BCR=panel/backend/src/routes/crons.rs
ACR=panel/agent/src/routes/crons.rs
for f in "$RES" "$BCR" "$ACR"; do [ -f "$f" ] || bad "MISSING SUBJECT FILE: $f"; done

echo "== §A  the reseller routes never hand an administrator another administrator =="

UPD=$(awk '/^pub async fn update_user\(/{i=1} i{print} i && /^}$/{exit}' "$RES")
DEL=$(awk '/^pub async fn delete_user\(/{i=1} i{print} i && /^}$/{exit}' "$RES")
if [ "$(grep -c . <<< "$UPD")" -ge 20 ]; then ok "A1-control update_user body extracted"; else bad "A1-control update_user not extracted (the extractor broke)"; fi
if [ "$(grep -c . <<< "$DEL")" -ge 20 ]; then ok "A2-control delete_user body extracted"; else bad "A2-control delete_user not extracted (the extractor broke)"; fi

GUARD="WHERE id = \\\$1 AND \\(role <> 'admin' OR id = \\\$2\\)"
if grep -qE "$GUARD" <<< "$UPD"; then ok "A3 update_user: an admin target is only ever the caller"; else bad "A3 update_user admin lookup must exclude other administrators"; fi
if grep -qE "$GUARD" <<< "$DEL"; then ok "A4 delete_user: an admin target is only ever the caller"; else bad "A4 delete_user admin lookup must exclude other administrators"; fi
if grep -qE 'SELECT id, email FROM users WHERE id = \$1"' <<< "$UPD$DEL"; then bad "A5 an unscoped 'FROM users WHERE id = \$1' lookup is back"; else ok "A5 no unscoped users lookup remains in either handler"; fi
if grep -qE 'UPDATE servers SET user_id = \$1 WHERE user_id = \$2' <<< "$DEL"; then ok "A6 delete_user re-points the account's servers first"; else bad "A6 delete_user must re-point servers before the delete (cascade to sites)"; fi
if grep -qE 'claims\.role == "admin"' <<< "$DEL"; then ok "A7 the re-point is for an administrator caller only"; else bad "A7 a reseller must not be handed a machine"; fi
if grep -qE 'tx\.commit' <<< "$DEL"; then ok "A8 re-point and delete share one transaction"; else bad "A8 re-point + delete must be one transaction"; fi

echo "== §B  a cron label cannot start a second crontab line =="

if grep -qE 'fn validate_label' "$BCR"; then ok "B1 the panel has a label validator"; else bad "B1 the panel must validate label"; fi
if grep -qE 'is_control\(\)' "$BCR"; then ok "B2 the panel rejects control characters"; else bad "B2 the panel validator must reject control characters"; fi
CRE=$(awk '/^pub async fn create\(/{i=1} i{print} i && /^}$/{exit}' "$BCR")
UPC=$(awk '/^pub async fn update\(/{i=1} i{print} i && /^}$/{exit}' "$BCR")
if grep -qE 'validate_label\(' <<< "$CRE"; then ok "B3 create validates the label"; else bad "B3 create must call validate_label"; fi
if grep -qE 'validate_label\(' <<< "$UPC"; then ok "B4 update validates the label"; else bad "B4 update must call validate_label"; fi
if grep -qE 'fn is_safe_label' "$ACR" && grep -qE 'fn is_safe_marker_field' "$ACR"; then ok "B5 the agent validates label and id itself"; else bad "B5 the agent is the sink and must not trust the panel"; fi
SYNC=$(awk '/^async fn sync_crons\(/{i=1} i{print} i && /^}$/{exit}' "$ACR")
if grep -qE 'is_safe_label\(' <<< "$SYNC" && grep -qE 'is_safe_marker_field\(' <<< "$SYNC"; then ok "B6 sync_crons skips a row that fails either check"; else bad "B6 sync_crons must skip rows with an unsafe label or id"; fi

echo; echo "passed $PASS, failed $FAIL"
[ "$FAIL" = 0 ]
