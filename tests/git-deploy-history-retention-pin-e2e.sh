#!/usr/bin/env bash
# Regression pin for #132 (part 1): Git Deploy history retention.
# git_deploy_history grew for ever (nothing deleted it). The column
# `history_retention` (0 = keep all, else 5..10000) now bounds it, and a sweep
# trims each deploy to its newest N rows without touching in-flight rows.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1
PASS=0; FAIL=0
ok()  { printf '\033[32m✓\033[0m %s\n' "$1"; PASS=$((PASS+1)); }
bad() { printf '\033[31m✗\033[0m %s — %s\n' "$1" "$2"; FAIL=$((FAIL+1)); }
has() { /usr/bin/grep -qE -- "$3" "$2" && ok "$1" || bad "$1" "pattern '$3' not in $2"; }

GD=panel/backend/src/routes/git_deploys.rs
CL=panel/backend/src/services/preview_cleanup.rs
UI=panel/frontend/src/pages/GitDeploys.tsx
MIG=$(ls panel/backend/migrations/*git_deploy_history_retention.sql 2>/dev/null | head -1)

[ -n "$MIG" ] && ok "migration exists" || bad "migration exists" "none"
has "column defaults to 0 (keep all)" "${MIG:-/dev/null}" 'history_retention INT NOT NULL DEFAULT 0'
has "validator accepts only 0 or 5..10000" "$GD" 'n == 0 \|\| \(5\.\.=10_000\)'
has "create validates" "$GD" 'validate_history_retention\(body\.history_retention'
has "update validates" "$GD" 'body\.history_retention\.map\(validate_history_retention\)'
has "update writes via COALESCE" "$GD" 'history_retention = COALESCE\(\$22, history_retention\)'
has "SELECT list returns the column" "$GD" 'd\.history_retention'
has "sweep is wired into the cleanup loop" "$CL" 'prune_deploy_history\(&db\)'
has "sweep skips in-flight rows" "$CL" "status NOT IN \('building', 'deploying'\)"
has "sweep only touches deploys that opted in" "$CL" 'd\.history_retention > 0'
has "UI sends the field" "$UI" 'history_retention: formHistoryRetention'
has "UI loads the stored value" "$UI" 'setFormHistoryRetention\(selected\.history_retention'

echo
[ "$FAIL" -eq 0 ] && printf '\033[32mPASS %d  FAIL %d\033[0m\n' "$PASS" "$FAIL" || printf '\033[31mPASS %d  FAIL %d\033[0m\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
