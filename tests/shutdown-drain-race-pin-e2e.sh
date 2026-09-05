#!/usr/bin/env bash
# shutdown-drain-race-pin-e2e.sh — s466
#
# Standing carry from s465 (project_dockpanel_tech_debt_p204): the demo
# deploy's own `systemctl stop dockpanel-api` triggered the graceful-shutdown
# watchdog (main.rs::shutdown_signal, GRACEFUL_SHUTDOWN_TIMEOUT = 20s) for the
# FIRST time in 12 sessions of monitoring — a real forced exit
# (`std::process::exit(1)`, systemd `status=1/FAILURE`), not another clean
# non-trigger.
#
# Root cause: `axum::serve(...).with_graceful_shutdown(...)` waits for every
# IN-FLIGHT connection to close on its OWN; a WebSocket or SSE handler that
# only exits when the CLIENT disconnects (or, for the 5 provision-log SSE
# streams, when the underlying job finishes) can hold that drain open past
# the 20s window, because axum spawns each connection onto its own task that
# the shutdown signal resolving does not cancel.
#
# Fixed by threading a shutdown broadcast into every long-lived connection
# handler in the crate:
#
#   §A AppState gained `shutdown_tx: tokio::sync::broadcast::Sender<()>`,
#      populated from the SAME channel `spawn_supervised` already hands every
#      background service (main.rs) — the channel's creation moved earlier so
#      the AppState struct literal can hold a clone of it.
#   §B New shared helper `helpers::shutdown_signal_fut(&AppState)` — a future
#      that resolves on the first shutdown broadcast. Feeds either
#      `StreamExt::take_until` (SSE) or a `tokio::select!` race (WebSocket).
#   §C ws_metrics.rs::handle_socket — the ONLY axum `WebSocketUpgrade`/
#      `on_upgrade` site in the whole backend crate. `routes/terminal.rs` and
#      `routes/logs.rs` DO exist in this crate, but only mint short-lived
#      signed JWT tickets — the actual WebSocket is dialed by the BROWSER
#      directly against the agent (nginx pins that path to the agent socket;
#      the panel deliberately does not proxy it), so neither file ever holds
#      a connection open through this process. Now races its whole
#      per-tick fetch/send/recv cycle against
#      the shutdown signal via `tokio::select!`, not just a check at the top
#      of the loop: a single `agent.get()` call already budgets up to 60s
#      (`AgentClient::request`), which alone exceeds the 20s drain window.
#   §D All 6 SSE handlers in the crate (the complete set — confirmed by
#      grepping for every `Sse::new` site) now end with
#      `.take_until(shutdown_signal_fut(&state))`:
#      `notifications.rs::stream` (keyed on the PROCESS-LIFETIME `notif_tx`
#      broadcast — this one would otherwise never end on its own; every admin
#      tab with the panel open holds one), plus the 5 provision-log-shaped
#      streams (`sites.rs::provision_log`, `git_deploys.rs::deploy_log`,
#      `docker_apps.rs::deploy_log`, `system.rs::install_log`,
#      `migration.rs::progress`) whose channel closes on its own once the job
#      finishes, but which can still run well past the drain window while a
#      job is in progress at shutdown time.
#
# Adversarially reviewed via a 3-lens skeptic workflow (correctness,
# completeness-vs-independent-grep, blast-radius) before shipping — see
# project_dockpanel_tech_debt_p205 for the verdicts.

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

echo
echo "=================================================="
echo "  Graceful-shutdown drain race — source pins (s466)"
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

MAIN=panel/backend/src/main.rs
MAIN_C=$(code "$MAIN")
HELPERS=panel/backend/src/helpers.rs
HELPERS_C=$(code "$HELPERS")
WS_METRICS=panel/backend/src/routes/ws_metrics.rs
WS_METRICS_C=$(code "$WS_METRICS")
NOTIFICATIONS=panel/backend/src/routes/notifications.rs
NOTIFICATIONS_C=$(code "$NOTIFICATIONS")
SITES=panel/backend/src/routes/sites.rs
SITES_C=$(code "$SITES")
GIT_DEPLOYS=panel/backend/src/routes/git_deploys.rs
GIT_DEPLOYS_C=$(code "$GIT_DEPLOYS")
DOCKER_APPS=panel/backend/src/routes/docker_apps.rs
DOCKER_APPS_C=$(code "$DOCKER_APPS")
SYSTEM=panel/backend/src/routes/system.rs
SYSTEM_C=$(code "$SYSTEM")
MIGRATION=panel/backend/src/routes/migration.rs
MIGRATION_C=$(code "$MIGRATION")

# ── §A AppState carries a shutdown broadcast ────────────────────────────────
echo "── §A main.rs: AppState carries the same shutdown broadcast every background service gets ──"

if has "$MAIN_C" 'pub shutdown_tx: tokio::sync::broadcast::Sender<\(\)>,'; then
  ok "A1 AppState has a shutdown_tx: broadcast::Sender<()> field"
else
  bad "A1 AppState is missing the shutdown_tx field"
fi

CHANNEL_LINE=$(grep -nE 'let \(shutdown_tx, _\) = tokio::sync::broadcast::channel::<\(\)>\(1\);' "$MAIN" | head -1 | cut -d: -f1)
STATE_LITERAL_LINE=$(grep -nE '^\s*let state = AppState \{' "$MAIN" | head -1 | cut -d: -f1)
CHANNEL_COUNT=$(grep -cE 'let \(shutdown_tx, _\) = tokio::sync::broadcast::channel::<\(\)>\(1\);' "$MAIN")

if [ "$CHANNEL_COUNT" -eq 1 ]; then
  ok "A2 the shutdown_tx channel is created exactly once (no leftover duplicate declaration)"
else
  bad "A2 expected exactly 1 shutdown_tx channel::<()>(1) creation, found $CHANNEL_COUNT"
fi

if [ -n "$CHANNEL_LINE" ] && [ -n "$STATE_LITERAL_LINE" ] && [ "$CHANNEL_LINE" -lt "$STATE_LITERAL_LINE" ]; then
  ok "A3 shutdown_tx is created BEFORE the AppState struct literal (so the literal can hold a clone)"
else
  bad "A3 shutdown_tx creation is not positioned before 'let state = AppState {' — the literal cannot reference it"
fi

if has "$MAIN_C" 'shutdown_tx: shutdown_tx\.clone\(\),'; then
  ok "A4 the AppState literal carries shutdown_tx: shutdown_tx.clone()"
else
  bad "A4 the AppState literal no longer clones shutdown_tx into itself"
fi

# Positive control: the pre-existing background-service wiring is untouched.
if has "$MAIN_C" 'spawn_supervised\("backup_scheduler", &shutdown_tx,' && has "$MAIN_C" 'spawn_supervised\("cleanup", &shutdown_tx,'; then
  ok "A5 (control) spawn_supervised still wires background services to shutdown_tx by reference"
else
  bad "A5 (control) spawn_supervised's shutdown_tx wiring was altered"
fi

# ── A6/A7: the CRITICAL fix-of-the-fix — where the broadcast actually fires ─
#
# The first cut of this fix (caught by an adversarial skeptic workflow before
# shipping) sent `shutdown_tx.send(())` AFTER
# `axum::serve(...).with_graceful_shutdown(...).await` returned. That await
# does not return until every in-flight connection's task has ended — so a
# route handler racing itself against `shutdown_tx` for exactly that reason
# was waiting on a signal that could only be sent once it, and every sibling
# connection, had already closed some OTHER way. Circular: the fix was a
# no-op for its own stated purpose. The real fix moves the send INSIDE
# `shutdown_signal()`, which is the `signal` future axum polls FIRST — before
# it stops accepting connections or starts draining existing ones — so
# subscribers get the broadcast the instant the OS signal arrives, not after
# the drain that's waiting on them has already finished.
if has "$MAIN_C" 'async fn shutdown_signal\(shutdown_tx: tokio::sync::broadcast::Sender<\(\)>\)' \
  && has "$MAIN_C" '\.with_graceful_shutdown\(shutdown_signal\(shutdown_tx\.clone\(\)\)\)'; then
  ok "A6 shutdown_signal takes shutdown_tx and is invoked with a clone of it"
else
  bad "A6 shutdown_signal no longer takes/receives shutdown_tx — the send-timing fix regressed"
fi

SHUTDOWN_SIGNAL_BODY=$(fnbody "$MAIN_C" "shutdown_signal")
# Relative line numbers WITHIN the extracted function body only (never
# whole-file line numbers — shutdown_signal's own select! is not the first
# tokio::select! in main.rs, spawn_supervised and the cleanup task both have
# one earlier), so ordering here can't be fooled by an unrelated select!
# elsewhere in the file.
SELECT_REL=$(grep -nE 'tokio::select! \{' <<< "$SHUTDOWN_SIGNAL_BODY" | head -1 | cut -d: -f1)
SEND_REL=$(grep -nE 'let _ = shutdown_tx\.send\(\(\)\);' <<< "$SHUTDOWN_SIGNAL_BODY" | head -1 | cut -d: -f1)
WATCHDOG_REL=$(grep -nE 'tokio::spawn\(async \{' <<< "$SHUTDOWN_SIGNAL_BODY" | head -1 | cut -d: -f1)
if [ -n "$SELECT_REL" ] && [ -n "$SEND_REL" ] && [ -n "$WATCHDOG_REL" ] \
  && [ "$SEND_REL" -gt "$SELECT_REL" ] && [ "$SEND_REL" -lt "$WATCHDOG_REL" ]; then
  ok "A7 inside shutdown_signal(), the send fires AFTER the OS-signal select! and BEFORE the watchdog spawn — not after the drain in main()"
else
  bad "A7 shutdown_tx.send(()) is not correctly ordered inside shutdown_signal() — the circular-wait bug may have returned"
fi

# Positive control: main() no longer sends a second, too-late broadcast after
# the serve-await returns (that was the actual defect this A6/A7 pair fixes).
POST_SERVE_MAIN_TAIL=$(fnbody "$MAIN_C" "main")
if lacks "$POST_SERVE_MAIN_TAIL" 'Sending shutdown signal to background services'; then
  ok "A8 (control) main() no longer claims to send the shutdown signal itself post-drain"
else
  bad "A8 (control) the misleading post-drain 'sending shutdown signal' log/send reappeared in main()"
fi

# ── §B helpers::shutdown_signal_fut ─────────────────────────────────────────
echo "── §B helpers.rs: the shared shutdown-future helper ──"

if has "$HELPERS_C" 'pub fn shutdown_signal_fut\(state: &crate::AppState\) -> impl std::future::Future<Output = \(\)> \+ use<>'; then
  ok "B1 shutdown_signal_fut has the expected signature (+ use<>, opting out of Rust 2024's default RPITIT capture)"
else
  bad "B1 shutdown_signal_fut signature missing or changed"
fi

FUT_BODY=$(fnbody "$HELPERS_C" "shutdown_signal_fut")
if has "$FUT_BODY" 'state\.shutdown_tx\.subscribe\(\)' && has "$FUT_BODY" 'rx\.recv\(\)\.await'; then
  ok "B2 shutdown_signal_fut subscribes and awaits the broadcast"
else
  bad "B2 shutdown_signal_fut no longer subscribes/awaits correctly"
fi

# ── §C ws_metrics.rs: the metrics websocket races shutdown mid-cycle ───────
echo "── §C ws_metrics.rs: handle_socket races its whole per-tick cycle against shutdown, not just at loop-top ──"

if has "$WS_METRICS_C" 'let shutdown_rx = state\.shutdown_tx\.subscribe\(\);' && has "$WS_METRICS_C" 'handle_socket\(socket, agent, state, claims, shutdown_rx\)'; then
  ok "C1 handler() subscribes and hands shutdown_rx to handle_socket"
else
  bad "C1 handler() no longer wires shutdown_rx into handle_socket"
fi

HANDLE_SOCKET_BODY=$(fnbody "$WS_METRICS_C" "handle_socket")
if has "$HANDLE_SOCKET_BODY" 'mut shutdown_rx: tokio::sync::broadcast::Receiver<\(\)>,'; then
  ok "C2 handle_socket takes shutdown_rx as a broadcast::Receiver<()> parameter"
else
  bad "C2 handle_socket no longer takes a shutdown_rx parameter"
fi

if has "$HANDLE_SOCKET_BODY" 'let tick = async \{' && has "$HANDLE_SOCKET_BODY" 'keep_going = tick =>' && has "$HANDLE_SOCKET_BODY" '_ = shutdown_rx\.recv\(\) =>'; then
  ok "C3 the per-tick work is raced against shutdown_rx.recv() via tokio::select!"
else
  bad "C3 the tokio::select! race between the per-tick work and shutdown is missing"
fi

# Positive control: the 4 concurrent agent fetches this loop exists to make
# are unchanged — this fix restructures control flow, not the payload.
AGENT_GET_CALLS=$(count "$HANDLE_SOCKET_BODY" 'agent\.get\("/(system/info|system/processes|system/network|apps/gpu-info)"\)')
if [ "$AGENT_GET_CALLS" -eq 4 ]; then
  ok "C4 (control) all 4 agent.get() endpoints are still fetched per tick"
else
  bad "C4 (control) expected 4 agent.get() calls in handle_socket, found $AGENT_GET_CALLS"
fi

# C5 — adversarial-review follow-up: the trailing close-frame send is timed,
# so a black-holed peer can't stall this connection's own task (and with it,
# axum's drain wait for it) past the shutdown signal that just fired.
if has "$HANDLE_SOCKET_BODY" 'tokio::time::timeout\(Duration::from_secs\(2\), socket\.send\(Message::Close\(None\)\)\)\.await'; then
  ok "C5 the trailing close-frame send is wrapped in a 2s timeout, not a bare await"
else
  bad "C5 the trailing close-frame send is unbounded again — a black-holed peer could stall this connection's own drain"
fi

# ── §D every SSE stream in the crate ends on shutdown too ──────────────────
echo "── §D all 6 SSE handlers in the crate race their stream against shutdown ──"

if has "$NOTIFICATIONS_C" '\.take_until\(crate::helpers::shutdown_signal_fut\(&state\)\)'; then
  ok "D1 notifications.rs::stream (the process-lifetime notif_tx feed) takes_until shutdown"
else
  bad "D1 notifications.rs::stream is missing the shutdown race"
fi

if has "$SITES_C" '\.take_until\(crate::helpers::shutdown_signal_fut\(&state\)\)'; then
  ok "D2 sites.rs::provision_log takes_until shutdown"
else
  bad "D2 sites.rs::provision_log is missing the shutdown race"
fi

if has "$GIT_DEPLOYS_C" '\.take_until\(crate::helpers::shutdown_signal_fut\(&state\)\)'; then
  ok "D3 git_deploys.rs::deploy_log takes_until shutdown"
else
  bad "D3 git_deploys.rs::deploy_log is missing the shutdown race"
fi

if has "$DOCKER_APPS_C" '\.take_until\(crate::helpers::shutdown_signal_fut\(&state\)\)'; then
  ok "D4 docker_apps.rs::deploy_log takes_until shutdown"
else
  bad "D4 docker_apps.rs::deploy_log is missing the shutdown race"
fi

if has "$SYSTEM_C" '\.take_until\(crate::helpers::shutdown_signal_fut\(&state\)\)'; then
  ok "D5 system.rs::install_log takes_until shutdown"
else
  bad "D5 system.rs::install_log is missing the shutdown race"
fi

if has "$MIGRATION_C" '\.take_until\(crate::helpers::shutdown_signal_fut\(&state\)\)'; then
  ok "D6 migration.rs::progress takes_until shutdown"
else
  bad "D6 migration.rs::progress is missing the shutdown race"
fi

# Whole-crate completeness control: exactly 6 Sse::new sites exist and exactly
# 6 take_until(shutdown_signal_fut(...)) sites exist — if a 7th SSE handler is
# ever added without this race, this count catches it; if take_until count
# ever drops below Sse::new count, a regression removed the race somewhere.
SSE_SITES=$(grep -rlE 'Sse::new' panel/backend/src/routes/*.rs | wc -l | tr -d ' ')
SHUTDOWN_RACED_FILES=$(grep -rlE '\.take_until\(crate::helpers::shutdown_signal_fut\(&state\)\)' panel/backend/src/routes/*.rs | wc -l | tr -d ' ')
if [ "$SSE_SITES" -eq 6 ] && [ "$SHUTDOWN_RACED_FILES" -eq 6 ]; then
  ok "D7 (crate-wide control) all 6 Sse::new sites are exactly the 6 files racing shutdown — no gap, no drift"
else
  bad "D7 (crate-wide control) Sse::new sites=$SSE_SITES, shutdown-raced files=$SHUTDOWN_RACED_FILES — mismatch, investigate"
fi

echo
echo "=================================================="
echo "  PASS=$PASS FAIL=$FAIL"
echo "=================================================="

[ "$FAIL" -eq 0 ]
