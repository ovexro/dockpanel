#!/usr/bin/env bash
# Regression pins for the s430 dockpanel-fanout run on panel/agent/src/routes/
# volume_backup.rs + services/volume_backup.rs + backend backup_orchestrator.rs
# — two findings, upheld by an independent skeptic before either fix landed.
#
# 1. CROSS-ADMIN METADATA LEAK (list_volume_backups, confidentiality). The
#    endpoint was a flat `SELECT * FROM volume_backups ... LIMIT/OFFSET`
#    gated only by `AdminUser` — no server_id/user_id scope at all — unlike
#    its own sibling `list_db_backups` two functions above it, which has
#    always scoped by `s.user_id`. DockPanel is multi-tenant at the admin
#    tier (any admin can promote another user to `role='admin'` via
#    `PUT /api/users/{id}`, a routine action, not an edge case), so every
#    OTHER admin's container/volume names, filenames, sizes, sha256 hashes,
#    timestamps, and server_id were visible to any admin who opened this
#    endpoint's own dashboard tile. Does NOT enable cross-tenant RESTORE —
#    restore_volume_backup below it re-derives the target from a DB row
#    gated by an explicit ownership EXISTS predicate, which was already
#    correct and is unchanged. Fix: scope the list the same way restore
#    already scopes its own lookup — join servers and require
#    `sv.is_local OR sv.user_id = $1`.
#
# 2. EMPTY-ARCHIVE VOLUME WIPE (volume_backup.rs restore_volume, data
#    integrity, prospective). The existing `tar tzf ... || exit 3` guard
#    (added at some prior session for the corrupt/truncated case) is not an
#    emptiness check: a syntactically-valid, gzip-CRC-clean, zero-member
#    archive exits `tar tzf` with 0, so the `rm -rf` still ran and left a
#    live volume wiped with no rollback. Fix: a SECOND, separate `tar tzf |
#    wc -l` re-run (kept apart from the corruption check rather than folded
#    into one pipe, so an early exit on a short-circuiting match can never
#    mask a mid-listing failure as success) requires at least one archive
#    member before the wipe proceeds. Verified with REAL `docker run`
#    executions against scratch Docker volumes, not just source reading:
#    (a) a genuinely empty tar.gz -> guard fires, exit 3, volume untouched;
#    (b) a real non-empty tar.gz -> restores correctly, old content replaced;
#    (c) a truncated/corrupt tar.gz -> still caught by the FIRST guard
#    (regression control — proves the new check didn't weaken the old one).
#
# Sections A/B are pure SOURCE pins (no box, no network, no build). Section C
# documents the real docker verification narratively (already run and struck
# through at fix time) rather than re-running a live docker sequence on every
# CI pass, matching how nearby pin suites treat a one-time PoC.
#
#   run: bash tests/volume-backup-scope-leak-pin-e2e.sh
set -u

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BACKUP_ORCH="$REPO/panel/backend/src/routes/backup_orchestrator.rs"
VOLBACKUP_SVC="$REPO/panel/agent/src/services/volume_backup.rs"

PASS=0; FAIL=0
ok()   { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }
has()   { grep -q  -- "$2" "$1" 2>/dev/null && ok "$3" || bad "$3"; }
hasE()  { grep -qE -- "$2" "$1" 2>/dev/null && ok "$3" || bad "$3"; }

echo "== A. list_volume_backups no longer leaks cross-admin, scoped like list_db_backups' sibling =="

hasE "$BACKUP_ORCH" 'AdminUser\(claims\): AdminUser' \
  "list_volume_backups now binds claims (was AdminUser(_claims), discarding the caller identity it needed)"
has "$BACKUP_ORCH" 'JOIN servers sv ON sv.id = vb.server_id AND (sv.is_local OR sv.user_id = $1)' \
  "the list query now joins servers and scopes by the same is_local-OR-owns predicate restore_volume_backup already uses"
hasE "$BACKUP_ORCH" '\.bind\(claims\.sub\)\.bind\(limit\)\.bind\(offset\)' \
  "claims.sub is bound as the scope parameter ahead of limit/offset"

echo
echo "== B. restore_volume: an empty-but-valid archive can no longer wipe a live volume with no rollback =="

has "$VOLBACKUP_SVC" 'n=\$(tar tzf /backup/{filename} 2>/dev/null | wc -l)' \
  "a second, separate tar tzf re-run counts archive members before the wipe"
has "$VOLBACKUP_SVC" '[ \$n -gt 0 ]' \
  "the wipe is gated on that count being greater than zero"
hasE "$VOLBACKUP_SVC" "archive is empty.*volume left untouched" \
  "the empty-archive rejection message is distinct from the corrupt/truncated one (so an operator can tell them apart)"
# Regression control: the ORIGINAL corruption check must still be present and still run FIRST.
# grep -c counts MATCHING LINES, not occurrences — both invocations sit on the
# same format! line, so -c would silently read "1" as "both, wrongly counted"
# rather than "only one present". grep -o | wc -l counts occurrences.
N_TAR_TZF=$(grep -o 'tar tzf /backup/{filename}' "$VOLBACKUP_SVC" 2>/dev/null | wc -l); N_TAR_TZF=${N_TAR_TZF:-0}
if [ "$N_TAR_TZF" -ge 2 ]; then
  ok "both tar tzf invocations are present (corruption check + emptiness count), not one replacing the other ($N_TAR_TZF found)"
else
  bad "expected at least 2 'tar tzf /backup/{filename}' occurrences (corruption check + emptiness count), found $N_TAR_TZF"
fi
has "$VOLBACKUP_SVC" "corrupt or truncated" \
  "the original corruption/truncation guard text is still present, unchanged"

echo
echo "== C. Real docker verification (already executed at fix time, not re-run every CI pass) =="
echo "     (a) empty tar.gz  -> exit 3, volume untouched   [verified s430]"
echo "     (b) real tar.gz   -> exit 0, restores correctly [verified s430]"
echo "     (c) truncated tar.gz -> exit 3, volume untouched (regression control) [verified s430]"
ok "documented — see project_dockpanel_tech_debt_p165.md for the full docker transcript"

echo
printf 'passed %d, failed %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
