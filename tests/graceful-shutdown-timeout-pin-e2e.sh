#!/usr/bin/env bash
# graceful-shutdown-timeout-pin-e2e.sh
#
# THE SENTENCE THIS SUITE EXISTS TO PROVE:
#
#   "SIGTERM forces the process to exit within a bound no matter what happens
#    downstream of it, WITHOUT ever bounding the server's normal lifetime."
#
# THE DEFECT (measured on a real deploy). `axum::serve(...)
# .with_graceful_shutdown(shutdown_signal())` waits for EVERY in-flight
# connection to finish before returning. An open SSE stream or a WebSocket
# terminal session (this panel serves both, indefinitely) never closes on its
# own, so a plain SIGTERM idled the full ~90s systemd default TimeoutStopSec
# before systemd force-killed the process.
#
# A FIRST FIX ATTEMPT WAS WRONG, AND THIS SUITE EXISTS TO STOP IT RECURRING.
# Wrapping the WHOLE `axum::serve(...).with_graceful_shutdown(...)` future in
# `tokio::time::timeout(N, ...)` looks plausible but is a critical regression:
# that future runs for the server's entire uptime (it does not resolve until
# BOTH the shutdown signal fires AND every connection drains), so timing out
# the whole thing kills the process ~N seconds after every single boot,
# whether or not a shutdown was ever requested. Caught by adversarial review
# before shipping — never by this file's own author reading the diff twice.
#
# THE ACTUAL FIX. Leave `axum::serve(...).with_graceful_shutdown(shutdown_signal())`
# bare and unbounded (that is correct — it MUST run for the server's whole
# life). Instead, arm a watchdog INSIDE `shutdown_signal()`, AFTER it has
# already resolved (ctrl_c or SIGTERM received): `tokio::spawn` a task that
# sleeps `GRACEFUL_SHUTDOWN_TIMEOUT` then calls `std::process::exit(1)`. The
# watchdog's clock only starts once the signal has fired, and a genuine hard
# process exit (not merely dropping a future) is required because axum spawns
# each connection onto its own independent task that dropping the orchestrator
# future does not cancel, and the unconditional `shutdown_db.close().await`
# that runs afterward is itself unbounded.
#
# So the mutations this suite must survive fall into two families: (1)
# RE-UNBOUNDING mutations — remove the watchdog, let its branch fall silent,
# swap the hard exit for a soft return — and (2) THE EXACT REGRESSION ABOVE —
# re-wrapping axum::serve(...).with_graceful_shutdown(...) in a timeout that
# bounds the whole server lifetime instead of just the post-signal drain.
# Mutation-tested against both the pre-fix revision and a synthetic
# reintroduction of the wrong first attempt: every arm below is required to
# go RED against at least one of them.
#
#   §A  the watchdog is armed AFTER the signal resolves, inside
#       shutdown_signal() — never wrapping axum::serve(...) itself
#   §B  the bound is a small, named, finite constant, shared by the sleep and
#       the log message — not unbounded, not duplicated
#   §C  the watchdog actually calls std::process::exit(...) — a real hard
#       exit, not a log-only no-op
#   §D  NEGATIVE CONTROL: axum::serve(...).with_graceful_shutdown(...) is NOT
#       wrapped in any tokio::time::timeout anywhere in this file — this is
#       the exact whole-lifetime-wrap regression from the first fix attempt
#   §E  the original bare `if let Err(e) = axum::serve(...)
#       .with_graceful_shutdown(shutdown_signal()).await` shape is still
#       there, still logging the same message — the fix must not have
#       touched main()'s own error handling to add the watchdog
#   §F  the watchdog is tokio::spawn'd (an independent task), not awaited
#       inline — inline would delay shutdown_signal() itself from resolving,
#       which would delay axum's drain from even starting
#
# Pure source analysis over panel/backend/src/main.rs: no box, no network, no
# build required.
#
# NO PIPES INTO `grep -q`. Under `set -o pipefail` grep -q closes the pipe on
# its first match, the upstream dies of SIGPIPE (141), and pipefail reports
# the whole pipeline failed — so an arm would go red on correct code,
# non-deterministically. Every arm here feeds grep a here-string.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

MAIN_RS=panel/backend/src/main.rs

[ -f "$MAIN_RS" ] || bad "MISSING SUBJECT FILE: $MAIN_RS"

# Comments out, CODE INTACT — a bare comment-stripping regex can eat real code
# whenever a string literal contains `/*`.
code() {
  perl -0777 -pe '
    s{\{/\*.*?\*/\}}{}gs;
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
  ' "$1"
}
subj()  { local t; t=$(code "$1"); [ -n "$t" ] || return 1; printf '%s' "$t"; }
has()   { grep -qE -- "$2" <<< "$1"; }
count() { grep -cE -- "$2" <<< "$1" || true; }
flat()  { tr '\n' ' ' <<< "$1" | tr -s ' '; }
# One function's body, bounded by its OWN closing brace in column 0 — not the
# whole file.
fn_body() { awk -v pat="$1" '$0 ~ pat {f=1} f {print} f && /^\}/ {exit}' <<< "$2"; }

FULL_S=$(subj "$MAIN_RS" || true)
FULL_FLAT=$(flat "$FULL_S")

# Anchored on the function name alone (no '() {' in the pattern) — awk's
# `-v pat=` assignment runs its OWN escape pass before the regex engine sees
# the string, so a bash-escaped '\(' arrives as a bare '(' and is read as an
# ERE grouping metacharacter rather than a literal paren.
MAIN_FN=$(fn_body '^async fn main' "$FULL_S")
MAIN_FLAT=$(flat "$MAIN_FN")
SHUTDOWN_FN=$(fn_body '^async fn shutdown_signal' "$FULL_S")
SHUTDOWN_FLAT=$(flat "$SHUTDOWN_FN")

if [ -z "$MAIN_FN" ]; then
  bad "could not bound async fn main() in $MAIN_RS — every §D/§E arm below is unreachable"
fi
if [ -z "$SHUTDOWN_FN" ]; then
  bad "could not bound async fn shutdown_signal() in $MAIN_RS — every §A/§B/§C/§F arm below is unreachable"
fi

echo "§A  the watchdog is armed AFTER the signal resolves, inside shutdown_signal()"

# A1 — a tokio::spawn(...) block exists AFTER the tokio::select! block within
# shutdown_signal(), containing a sleep on GRACEFUL_SHUTDOWN_TIMEOUT. Ordering
# matters: this proves the watchdog's clock starts once the signal has ALREADY
# fired, not from process start.
if [ -z "$SHUTDOWN_FN" ]; then
  bad "A1 SKIPPED: could not bound shutdown_signal()"
else
  SELECT_LN=$(grep -n 'tokio::select! {' <<< "$SHUTDOWN_FN" | head -1 | cut -d: -f1)
  SPAWN_LN=$(grep -n 'tokio::spawn(async' <<< "$SHUTDOWN_FN" | head -1 | cut -d: -f1)
  SLEEP_LN=$(grep -n 'tokio::time::sleep(GRACEFUL_SHUTDOWN_TIMEOUT)' <<< "$SHUTDOWN_FN" | head -1 | cut -d: -f1)
  if [ -z "${SELECT_LN:-}" ] || [ -z "${SPAWN_LN:-}" ] || [ -z "${SLEEP_LN:-}" ]; then
    bad "A1 could not locate one of: tokio::select!, tokio::spawn(async, or the GRACEFUL_SHUTDOWN_TIMEOUT sleep inside shutdown_signal()"
  elif [ "$SELECT_LN" -lt "$SPAWN_LN" ] && [ "$SPAWN_LN" -lt "$SLEEP_LN" ]; then
    ok "A1 a tokio::spawn'd watchdog sleeping on GRACEFUL_SHUTDOWN_TIMEOUT sits AFTER tokio::select! inside shutdown_signal() — its clock starts once the signal has already fired"
  else
    bad "A1 the watchdog is missing or is not strictly after tokio::select! inside shutdown_signal() — the bound may no longer start from the signal"
  fi
fi

echo "§B  the bound is a small, named, finite constant, shared by the sleep and the log"

# B1 — a plain, finite Duration::from_secs(N) constant exists, N is small.
if has "$FULL_FLAT" \
  'const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs\([0-9]+\);'; then
  DUR_N=$(grep -oE 'const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs\([0-9]+\)' <<< "$FULL_FLAT" \
            | grep -oE '[0-9]+' | tail -1)
  if [ -n "${DUR_N:-}" ] && [ "$DUR_N" -gt 0 ] 2>/dev/null && [ "$DUR_N" -le 30 ] 2>/dev/null; then
    ok "B1 GRACEFUL_SHUTDOWN_TIMEOUT is a finite constant of ${DUR_N}s (0 < N <= 30) — short enough to exit well inside systemd's kill window"
  else
    bad "B1 GRACEFUL_SHUTDOWN_TIMEOUT is ${DUR_N:-unknown}s — not a short bound any more"
  fi
else
  bad "B1 no finite 'const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(N)' found — the bound may be missing, dynamic, or Duration::MAX"
fi

# B2 — the SAME constant drives both the watchdog's sleep and the warning it
# logs on firing, via .as_secs(). Independently-spelled durations could drift.
if [ -z "$SHUTDOWN_FN" ]; then
  bad "B2 SKIPPED: could not bound shutdown_signal()"
elif has "$SHUTDOWN_FLAT" 'GRACEFUL_SHUTDOWN_TIMEOUT\.as_secs\(\)' \
  && [ "$(count "$SHUTDOWN_FN" 'GRACEFUL_SHUTDOWN_TIMEOUT')" -ge 2 ]; then
  ok "B2 the logged duration reads .as_secs() off the same constant the watchdog sleeps on — no duplicated literal to drift"
else
  bad "B2 the watchdog's log message no longer derives from the same named constant it sleeps on"
fi

echo "§C  the watchdog actually calls std::process::exit — a real hard exit"

# C1 — after the sleep, the watchdog calls std::process::exit(...), not just
# a log. A soft log-only "watchdog" would not save us from a hung per-
# connection task or a stalled shutdown_db.close().await — see the file
# header. It must also log first (an operator needs to see WHY the process
# just vanished), so both must be present, log before exit.
if [ -z "$SHUTDOWN_FN" ]; then
  bad "C1 SKIPPED: could not bound shutdown_signal()"
else
  WARN_LN=$(grep -n 'tracing::warn!(' <<< "$SHUTDOWN_FN" | head -1 | cut -d: -f1)
  EXIT_LN=$(grep -n 'std::process::exit(' <<< "$SHUTDOWN_FN" | head -1 | cut -d: -f1)
  if [ -z "${WARN_LN:-}" ] || [ -z "${EXIT_LN:-}" ]; then
    bad "C1 could not find both a tracing::warn! and a std::process::exit inside shutdown_signal() — the watchdog may be a no-op"
  elif [ "$WARN_LN" -lt "$EXIT_LN" ]; then
    ok "C1 the watchdog logs via tracing::warn! THEN calls std::process::exit — an operator sees why, and the process actually terminates"
  else
    bad "C1 std::process::exit appears before the warning is logged, or the ordering could not be established"
  fi
fi

echo "§D  NEGATIVE CONTROL: axum::serve(...).with_graceful_shutdown(...) is never wrapped in a timeout"

# D1 — THE EXACT REGRESSION THIS SUITE MUST CATCH. If axum::serve(...)
# .with_graceful_shutdown(...) is ever nested inside a tokio::time::timeout(...)
# again, that bounds the SERVER'S ENTIRE LIFETIME (the future does not resolve
# until the shutdown signal ALSO fires), not just the post-signal drain — the
# process would exit ~N seconds after every boot, shutdown or not. A regex
# checking only for the watchdog's PRESENCE (§A-§C) would stay green even if
# this catastrophic shape were reintroduced ALONGSIDE it.
if has "$FULL_FLAT" \
  'tokio::time::timeout\([^)]*axum::serve\(listener, app\)\.with_graceful_shutdown\(shutdown_signal\(\)\)'; then
  bad "D1 axum::serve(...).with_graceful_shutdown(...) is wrapped in a tokio::time::timeout — this bounds the WHOLE SERVER LIFETIME, not just shutdown, and would force-exit the process ~N seconds after every single boot"
else
  ok "D1 axum::serve(...).with_graceful_shutdown(...) is never wrapped in a tokio::time::timeout — the server's normal lifetime is unbounded, as it must be"
fi

echo "§E  the original bare error-handling shape around axum::serve is still there"

# E1 — the pre-existing `if let Err(e) = axum::serve(...)
# .with_graceful_shutdown(shutdown_signal()).await { tracing::error!(...) }`
# shape, UNCHANGED, still logs the same message. Adding the watchdog inside
# shutdown_signal() must not have touched main()'s own error handling.
# s466 CORR: shutdown_signal() now takes shutdown_tx (a separate fix — it
# needs the sender to broadcast the instant the OS signal arrives, not after
# this same await already returned — see shutdown-drain-race-pin-e2e.sh §A6/A7)
# so the call site gained an argument; the shape this arm actually cares about
# (the bare if-let-Err wrapping THIS SAME await, logging the same message) is
# unchanged regardless of what's inside the parens.
if [ -z "$MAIN_FN" ]; then
  bad "E1 SKIPPED: could not bound main()"
elif has "$MAIN_FLAT" \
  'if let Err\(e\) = axum::serve\(listener, app\)[[:space:]]*\.with_graceful_shutdown\(shutdown_signal\(shutdown_tx\.clone\(\)\)\)[[:space:]]*\.await' \
  && has "$MAIN_FLAT" 'tracing::error!\("API server error: \{e\}"\)'; then
  ok "E1 the bare 'if let Err(e) = axum::serve(...).with_graceful_shutdown(...).await { tracing::error!(...) }' shape is intact"
else
  bad "E1 the original server-error handling around axum::serve is gone or no longer logs the same message"
fi

echo "§F  the watchdog is spawned as an independent task, not awaited inline"

# F1 — 'tokio::spawn(async {' precedes the sleep/warn/exit body, so
# shutdown_signal() itself returns immediately once spawned, letting axum's
# drain begin in parallel with the watchdog's countdown. Awaiting the sleep
# inline (no tokio::spawn) would delay the signal future from resolving at
# all until the FULL bound elapsed, defeating graceful shutdown entirely.
if [ -z "$SHUTDOWN_FN" ]; then
  bad "F1 SKIPPED: could not bound shutdown_signal()"
elif has "$SHUTDOWN_FLAT" 'tokio::spawn\(async \{[^}]*tokio::time::sleep\(GRACEFUL_SHUTDOWN_TIMEOUT\)\.await'; then
  ok "F1 the watchdog's sleep+warn+exit body is tokio::spawn'd — shutdown_signal() returns immediately so axum's drain starts in parallel"
else
  bad "F1 the watchdog's sleep is not inside a tokio::spawn — shutdown_signal() may block on it directly, delaying the drain from ever starting"
fi

echo
echo "PASS $PASS / FAIL $FAIL"
[ "$FAIL" -eq 0 ]
