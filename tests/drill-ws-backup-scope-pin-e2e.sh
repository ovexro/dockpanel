#!/usr/bin/env bash
# drill-ws-backup-scope-pin-e2e.sh — s465
#
# dockpanel-fanout, batched small-file tail of panel/backend/src/{routes,services}
# (drill_scheduler.rs, deploy_scheduler.rs, alert_runbooks.rs,
# alert_runbook_defaults.rs / prometheus.rs, prometheus_exporter.rs,
# metrics_collector.rs, ws_metrics.rs, system_logs.rs / agent_checkin.rs,
# agent_updates.rs, sboms.rs, chain_report.rs, branding_assets.rs — see
# feedback_dockpanel_audit_scope). Found and fixed 5 real, independently
# skeptic-verified issues (4 on-menu + 1 off-menu completeness-critic find):
#
#   §A drill_scheduler.rs + backup_orchestrator.rs: a scheduled OR on-demand
#      backup drill fired the agent call via plain `post` (60s total-round-trip
#      cap), but the agent-side restore it triggers is budgeted 220s (mariadb)
#      to 360s (volume: 300s restore + 60s probe) — several multiples of the
#      cap. Any drill against a non-trivial backup was killed by the client and
#      recorded `status = 'failed'`, a false DR-failure signal on a healthy
#      backup. Fixed both the scheduled (`enqueue_drill`) and on-demand
#      (`trigger_drill`) paths via `post_long(..., DRILL_TIMEOUT_SECS)`, the
#      same fix pattern `DB_RESTORE_TIMEOUT_SECS` already established for the
#      real (non-drill) restore path in this same file. Confirmed dormant on
#      this box (0 rows in backup_policies/backup_drills/database_backups/
#      volume_backups, drill_enabled defaults off) but reachable the instant a
#      real policy enables drills.
#   §B drill_scheduler.rs: `tick()` stamped `last_drill_at = NOW()`
#      unconditionally BEFORE calling `dispatch_policy_drills`, which has four
#      early-return paths (no backups tied to the policy, local server id
#      unknown, target server unreachable, another drill already running) that
#      make zero agent calls and insert zero `backup_drills` rows. A policy
#      whose server has been down for weeks read as "last drilled Xm ago" on
#      every cron tick — the one case a DR-verification timestamp most needs to
#      flag was the case it hid. Fixed: `dispatch_policy_drills` now returns
#      whether it actually enqueued a drill, and `tick()` only stamps
#      `last_drill_at` when it did.
#   §C ws_metrics.rs: the live-metrics websocket validated the JWT (expiry,
#      blacklist, global session revocation, admin role) ONLY at handshake,
#      before `ws.on_upgrade`. The 5s streaming loop never re-checked any of
#      it — logout, the panic button, and `POST /auth/revoke-all` all closed
#      the door to NEW connections but left an ALREADY-OPEN socket streaming
#      the full process/network/GPU/system feed for as long as the underlying
#      TCP connection stayed alive, unbounded even by the token's own nominal
#      expiry. This directly undermined the panic button's own stated intent
#      (its comment in this same file). Fixed: a shared `claims_now_invalid`
#      check, re-run every loop tick, closes the socket the instant any of the
#      handshake conditions would now reject it.
#   §D backup_orchestrator.rs: `chain_report_json`/`chain_report_pdf` (via
#      `build_chain_report`) and `list_all_backups` accepted a caller-supplied
#      backup id / listed the whole fleet with NO ownership scoping — gated
#      only by `AdminUser`, a pure role check. Any admin could pull another
#      admin's full chain-of-trust report (site domain, db/container:volume
#      names, filenames, hash chain, every verification and drill) as JSON or
#      PDF, and `list_all_backups` was the exact discovery channel for the
#      backup ids needed to do it. Identical bug class already fixed once in
#      this same file for `list_volume_backups`/`restore_volume_backup`
#      (v2.184.0) — just missed for these two. Fixed: every query now joins
#      `servers` and requires `is_local OR user_id = caller`, mirroring the
#      v2.184.0 remedy exactly.
#   §E (off-menu, completeness critic) backup_destinations.rs: `update()`,
#      `remove()`, and `test_connection()` all queried/mutated a destination by
#      bare `id`, gated only by `AdminUser` — asymmetric with `create()` in the
#      same file, which already scopes `server_id` by `is_local OR user_id`.
#      A second admin (DockPanel genuinely supports multiple admin accounts)
#      could overwrite another admin's S3/SFTP destination config with their
#      own credentials — the next scheduled backup for any policy pointing at
#      it would silently upload the target server's data into the SECOND
#      admin's bucket, a backup-exfiltration primitive, with no activity-log
#      entry on this route to notice it by. `list()` had the identical gap
#      (found independently while fixing the other three — a bare
#      unscoped `SELECT *`) and was fixed the same way. Fixed: all four now
#      scope by the same `is_local OR user_id = caller` predicate `create`
#      already established.

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

echo
echo "=================================================="
echo "  Drill timeout / ws revocation / backup ownership scope — source pins (s465)"
echo "=================================================="
echo

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

code() {
  [ -f "$1" ] || return 0
  perl -0777 -pe '
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
    s{^\s*///.*$}{}gm;
  ' "$1"
}

has()    { grep -qE -- "$2" <<< "$1"; }
lacks()  { ! grep -qE -- "$2" <<< "$1"; }
count()  { grep -cE -- "$2" <<< "$1"; }

fnbody() {
  awk -v fn="$2" '
    index($0, "fn " fn "(") && !started { started=1 }
    started {
      n=gsub(/\{/,"{"); m=gsub(/\}/,"}"); depth += n - m; print
      if (opened || n>0) opened=1
      if (opened && depth<=0) exit
    }
  ' <<< "$1"
}

AGENT_SVC=panel/backend/src/services/agent.rs
AGENT_SVC_C=$(code "$AGENT_SVC")
DRILL_SCHED=panel/backend/src/services/drill_scheduler.rs
DRILL_SCHED_C=$(code "$DRILL_SCHED")
BACKUP_ORCH=panel/backend/src/routes/backup_orchestrator.rs
BACKUP_ORCH_C=$(code "$BACKUP_ORCH")
WS_METRICS=panel/backend/src/routes/ws_metrics.rs
WS_METRICS_C=$(code "$WS_METRICS")
BACKUP_DEST=panel/backend/src/routes/backup_destinations.rs
BACKUP_DEST_C=$(code "$BACKUP_DEST")

# ── §A drill timeout: post -> post_long(DRILL_TIMEOUT_SECS) ────────────────
echo "── §A drill_scheduler.rs + backup_orchestrator.rs: drills use a timeout sized for the restore, not the round trip ──"

if has "$AGENT_SVC_C" 'pub const DRILL_TIMEOUT_SECS: u64 = 420;'; then
  ok "A1 DRILL_TIMEOUT_SECS constant defined (420s, > the 360s volume-drill worst case)"
else
  bad "A1 DRILL_TIMEOUT_SECS constant missing or changed"
fi

ENQUEUE_BODY=$(fnbody "$DRILL_SCHED_C" "enqueue_drill")
if has "$ENQUEUE_BODY" 'post_long\(&agent_path, Some\(body\), crate::services::agent::DRILL_TIMEOUT_SECS\)'; then
  ok "A2 scheduled path (enqueue_drill) calls post_long with DRILL_TIMEOUT_SECS"
else
  bad "A2 scheduled path no longer uses post_long/DRILL_TIMEOUT_SECS — regressed to the 60s cap"
fi

TRIGGER_DRILL_LONG_CALLS=$(count "$BACKUP_ORCH_C" 'post_long\("/backups/drill/(site|db|volume)", Some\(body\), crate::services::agent::DRILL_TIMEOUT_SECS\)')
if [ "$TRIGGER_DRILL_LONG_CALLS" -eq 3 ]; then
  ok "A3 on-demand path (trigger_drill) calls post_long with DRILL_TIMEOUT_SECS for all 3 drill types"
else
  bad "A3 expected 3 post_long(DRILL_TIMEOUT_SECS) call sites in trigger_drill, found $TRIGGER_DRILL_LONG_CALLS"
fi

# Positive control: the real (non-drill) DB restore timeout constant is untouched.
if has "$AGENT_SVC_C" 'pub const DB_RESTORE_TIMEOUT_SECS: u64 = 270;'; then
  ok "A4 (control) DB_RESTORE_TIMEOUT_SECS unchanged at 270s"
else
  bad "A4 (control) DB_RESTORE_TIMEOUT_SECS was touched — verify this wasn't accidentally altered"
fi

# ── §B last_drill_at: stamped only after a drill actually enqueues ─────────
echo "── §B drill_scheduler.rs: last_drill_at only advances when a drill actually dispatched ──"

DISPATCH_BODY=$(fnbody "$DRILL_SCHED_C" "dispatch_policy_drills")
if has "$DISPATCH_BODY" '\-> bool'; then
  ok "B1 dispatch_policy_drills now returns bool"
else
  bad "B1 dispatch_policy_drills no longer returns bool — regressed"
fi

if has "$DISPATCH_BODY" 'let mut dispatched = false;' && has "$DISPATCH_BODY" '^\s*dispatched$'; then
  ok "B2 dispatch_policy_drills tracks and returns whether it actually enqueued a drill"
else
  bad "B2 the dispatched-tracking logic is missing"
fi

TICK_BODY=$(fnbody "$DRILL_SCHED_C" "tick")
if has "$TICK_BODY" 'if dispatch_policy_drills\(db, agents, policy\)\.await \{'; then
  ok "B3 tick() gates the last_drill_at UPDATE on dispatch_policy_drills actually returning true"
else
  bad "B3 tick() no longer gates on dispatch_policy_drills' return value"
fi

if lacks "$TICK_BODY" 'dispatch_policy_drills\(db, agents, policy\)\.await;'; then
  ok "B4 the old unconditional (unguarded) dispatch call is gone"
else
  bad "B4 an unconditional dispatch_policy_drills call still exists — the stamp could run unconditionally again"
fi

# Positive control: the UPDATE statement itself is unchanged.
if has "$TICK_BODY" 'UPDATE backup_policies SET last_drill_at = NOW\(\) WHERE id = \$1'; then
  ok "B5 (control) the last_drill_at UPDATE statement text is unchanged"
else
  bad "B5 (control) the UPDATE statement was altered — verify the fix didn't change its shape"
fi

# ── §C ws_metrics.rs: per-tick revocation/expiry recheck ────────────────────
echo "── §C ws_metrics.rs: the live-metrics socket re-validates every tick, not just at handshake ──"

if has "$WS_METRICS_C" 'fn claims_now_invalid'; then
  ok "C1 claims_now_invalid helper exists"
else
  bad "C1 claims_now_invalid helper missing"
fi

CLAIMS_INVALID_BODY=$(fnbody "$WS_METRICS_C" "claims_now_invalid")
if has "$CLAIMS_INVALID_BODY" 'claims\.exp as i64\) < chrono::Utc::now\(\)\.timestamp\(\)'; then
  ok "C2 claims_now_invalid checks the token's own expiry, not just revocation"
else
  bad "C2 the expiry check is missing — exposure would still be unbounded past the token's own exp"
fi

if has "$CLAIMS_INVALID_BODY" 'token_blacklist\.read\(\)\.await\.contains\(jti\)' && has "$CLAIMS_INVALID_BODY" 'sessions_revoked_at\.read\(\)\.await'; then
  ok "C3 claims_now_invalid checks both the token blacklist and global session revocation"
else
  bad "C3 blacklist/global-revocation checks missing from claims_now_invalid"
fi

HANDLE_SOCKET_BODY=$(fnbody "$WS_METRICS_C" "handle_socket")
# s466 CORR: handle_socket's signature went multi-line (a 5th param, shutdown_rx,
# was added — see the graceful-shutdown-race pin suite), so state/claims no
# longer share a line; check each param and the per-tick call separately.
if has "$HANDLE_SOCKET_BODY" 'state: AppState,' && has "$HANDLE_SOCKET_BODY" 'claims: Claims,' && has "$HANDLE_SOCKET_BODY" 'claims_now_invalid\(&state, &claims\)\.await'; then
  ok "C4 handle_socket takes state+claims and calls claims_now_invalid every loop iteration"
else
  bad "C4 handle_socket no longer threads state/claims through to a per-tick check"
fi

if has "$WS_METRICS_C" 'handle_socket\(socket, agent, state, claims, shutdown_rx\)'; then
  ok "C5 the handshake now hands state+claims(+shutdown_rx, s466) to handle_socket"
else
  bad "C5 on_upgrade no longer passes state/claims to handle_socket"
fi

# Positive control: the handshake's own original checks are all still present.
if has "$WS_METRICS_C" 'Token has been revoked' && has "$WS_METRICS_C" 'Session revoked\. Please log in again\.' && has "$WS_METRICS_C" 'Admin access required'; then
  ok "C6 (control) the handshake's own blacklist/revocation/admin-role checks are unchanged"
else
  bad "C6 (control) a handshake-time check was removed — verify this wasn't accidentally dropped"
fi

# ── §D backup_orchestrator.rs: chain-report + list-all-backups ownership scope ─
echo "── §D backup_orchestrator.rs: chain-report and list-all-backups scoped to owned/local servers ──"

BUILD_CHAIN_BODY=$(fnbody "$BACKUP_ORCH_C" "build_chain_report")
if has "$BUILD_CHAIN_BODY" 'caller_id: Uuid'; then
  ok "D1 build_chain_report now takes a caller_id parameter"
else
  bad "D1 build_chain_report has no caller_id parameter — still unscoped"
fi

SCOPE_JOINS=$(count "$BUILD_CHAIN_BODY" 'JOIN servers sv ON sv\.id = .*server_id.* AND \(sv\.is_local OR sv\.user_id = \$2\)')
if [ "$SCOPE_JOINS" -eq 3 ]; then
  ok "D2 all 3 per-kind queries (site/database/volume) join servers and scope by is_local OR user_id"
else
  bad "D2 expected 3 ownership-scoped server joins in build_chain_report, found $SCOPE_JOINS"
fi

CHAIN_CALLERS=$(count "$BACKUP_ORCH_C" 'build_chain_report\(&state, kind, id, claims\.sub\)')
if [ "$CHAIN_CALLERS" -eq 2 ]; then
  ok "D3 both chain_report_json and chain_report_pdf pass claims.sub as caller_id"
else
  bad "D3 expected 2 call sites passing claims.sub, found $CHAIN_CALLERS"
fi

if has "$BACKUP_ORCH_C" 'pub async fn chain_report_json\(\s*State\(state\): State<AppState>,\s*AdminUser\(claims\): AdminUser,' \
  || (has "$BACKUP_ORCH_C" 'AdminUser\(claims\): AdminUser' && lacks "$(fnbody "$BACKUP_ORCH_C" "chain_report_json")" 'AdminUser\(_claims\)'); then
  ok "D4 chain_report_json binds claims (not the discarded _claims)"
else
  bad "D4 chain_report_json still discards claims"
fi

if lacks "$(fnbody "$BACKUP_ORCH_C" "chain_report_pdf")" 'AdminUser\(_claims\)'; then
  ok "D5 chain_report_pdf binds claims (not the discarded _claims)"
else
  bad "D5 chain_report_pdf still discards claims"
fi

LIST_ALL_BODY=$(fnbody "$BACKUP_ORCH_C" "list_all_backups")
if has "$LIST_ALL_BODY" 'server_id IS NULL OR srv\.is_local OR srv\.user_id = \$5' \
  && has "$LIST_ALL_BODY" 'server_id IS NULL OR srv\.is_local OR srv\.user_id = \$3'; then
  ok "D6 list_all_backups scopes both the SELECT and COUNT queries by is_local OR user_id"
else
  bad "D6 list_all_backups ownership scoping missing from one or both queries"
fi

if lacks "$LIST_ALL_BODY" 'AdminUser\(_claims\)'; then
  ok "D7 list_all_backups binds claims (not the discarded _claims)"
else
  bad "D7 list_all_backups still discards claims"
fi

# Positive control: the v2.184.0 sibling fix this mirrors is still intact.
LVB_BODY=$(fnbody "$BACKUP_ORCH_C" "list_volume_backups")
if has "$LVB_BODY" 'sv\.is_local OR sv\.user_id = \$1'; then
  ok "D8 (control) list_volume_backups' own v2.184.0 ownership scope is unchanged"
else
  bad "D8 (control) the sibling fix this session's remedy mirrors was itself altered"
fi

# ── §E backup_destinations.rs: list/update/remove/test_connection scoped ────
echo "── §E backup_destinations.rs: destination management scoped to owned/local servers ──"

LIST_DEST_BODY=$(fnbody "$BACKUP_DEST_C" "list")
if has "$LIST_DEST_BODY" 'server_id IS NULL OR s\.is_local OR s\.user_id = \$1' && lacks "$LIST_DEST_BODY" 'AdminUser\(_claims\)'; then
  ok "E1 list() is ownership-scoped and binds claims"
else
  bad "E1 list() is missing the ownership scope or still discards claims"
fi

UPDATE_DEST_BODY=$(fnbody "$BACKUP_DEST_C" "update")
if has "$UPDATE_DEST_BODY" 'bd\.server_id IS NULL OR s\.is_local OR s\.user_id = \$2' && lacks "$UPDATE_DEST_BODY" 'AdminUser\(_claims\)'; then
  ok "E2 update() has an ownership pre-check and binds claims"
else
  bad "E2 update() is missing the ownership pre-check or still discards claims"
fi

REMOVE_DEST_BODY=$(fnbody "$BACKUP_DEST_C" "remove")
if has "$REMOVE_DEST_BODY" 'bd\.server_id IS NULL OR s\.is_local OR s\.user_id = \$2' && lacks "$REMOVE_DEST_BODY" 'AdminUser\(_claims\)'; then
  ok "E3 remove() has an ownership pre-check and binds claims"
else
  bad "E3 remove() is missing the ownership pre-check or still discards claims"
fi

TEST_CONN_BODY=$(fnbody "$BACKUP_DEST_C" "test_connection")
if has "$TEST_CONN_BODY" 'bd\.server_id IS NULL OR s\.is_local OR s\.user_id = \$2' && lacks "$TEST_CONN_BODY" 'AdminUser\(_claims\)'; then
  ok "E4 test_connection() scopes its destination lookup by ownership and binds claims"
else
  bad "E4 test_connection() is missing the ownership scope or still discards claims"
fi

# Positive control: create()'s own pre-existing ownership check (the pattern
# the other four now mirror) is unchanged.
CREATE_DEST_BODY=$(fnbody "$BACKUP_DEST_C" "create")
if has "$CREATE_DEST_BODY" 'is_local OR user_id = \$2'; then
  ok "E5 (control) create()'s own server-ownership check is unchanged"
else
  bad "E5 (control) create()'s ownership check was altered"
fi

# Positive control: DESTINATION_CALLER_PREDICATE (the narrower READ-scope
# constant used by selectable()/backup_schedules.rs) was deliberately left
# untouched — this fix mirrors create()'s broader is_local-inclusive
# MANAGEMENT predicate instead, not this one.
if has "$BACKUP_DEST_C" 'pub const DESTINATION_CALLER_PREDICATE: &str = "\(bd\.server_id IS NULL OR s\.user_id = \$1\)";'; then
  ok "E6 (control) DESTINATION_CALLER_PREDICATE is unchanged"
else
  bad "E6 (control) DESTINATION_CALLER_PREDICATE was altered — this session deliberately left it alone"
fi

echo
echo "=================================================="
echo "  PASS=$PASS FAIL=$FAIL"
echo "=================================================="

[ "$FAIL" -eq 0 ]
