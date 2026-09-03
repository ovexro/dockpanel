#!/usr/bin/env bash
# Regression pins for the s455 dockpanel-fanout run over
# panel/agent/src/services/database.rs + panel/agent/src/routes/database.rs
# (workflow wf_42ca36ed-5e2; project_dockpanel_tech_debt ledger p194).
#
#   D1  `reconcile_network_icc` (the one-time pre-s242 ICC-hardening migration
#       for the shared `dockpanel-db` bridge) aborted on the FIRST reconnect
#       failure — the network was already torn down and recreated by that
#       point, so every container LATER in the batch than the failure was
#       left with ZERO network attachment (its published port stops
#       routing) and no server-side record of which containers were
#       orphaned. Live-proven against a real, fully disposable Docker
#       network (never the real `dockpanel-db`): two real containers
#       straddling a deliberately-unconnectable id both ended up reconnected
#       after the fix, and the aggregated error named exactly the bad id.
#   D2  `routes/files.rs::download_file` read an entire file into a `Vec<u8>`
#       (`tokio::fs::read`) before sending a single response byte, with no
#       size cap — unlike its sibling `read_file` (2MB cap, same file). The
#       agent is the ONE process per box mediating deploys/backups/terminal/
#       Docker for every tenant, and runs under a 512MB `MemoryMax` cgroup;
#       an ordinary non-admin site owner with any large file in their own
#       webroot (a DB dump, an unrotated log, media) could OOM-kill the
#       shared agent with one GET request. Fixed by streaming instead of
#       buffering — memory use becomes constant regardless of file size, so
#       no arbitrary cap is needed.
#
# Pure source analysis except where noted; no box, no network, no build.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

# Strip comments and the test module, so a pin can never be satisfied (or
# tripped) by prose describing the very thing it forbids.
code()  { sed '/#\[cfg(test)\]/q' "$1" | grep -vE '^[[:space:]]*(///|//!|//|\*|/\*)'; }
has()   { [ -n "$(code "$1" | grep -F -- "$2")" ]; }
hasre() { [ -n "$(code "$1" | grep -E -- "$2")" ]; }

# One named function's body, comment-stripped. Never pipe this into `grep
# -q` — see security-firewalld-ssh-include-pin-e2e.sh's own header for why
# (the exact SIGPIPE-under-pipefail class this project tracks).
fnbody()    { code "$1" | awk "/$2/,/^}/"; }
bodyhas()   { [ -n "$(fnbody "$1" "$2" | grep -F -- "$3")" ]; }
bodyhasre() { [ -n "$(fnbody "$1" "$2" | grep -E -- "$3")" ]; }

DB_SVC=panel/agent/src/services/database.rs
FILES_ROUTE=panel/agent/src/routes/files.rs
FILES_SVC=panel/agent/src/services/files.rs

for f in "$DB_SVC" "$FILES_ROUTE" "$FILES_SVC"; do
  [ -f "$f" ] || { echo "missing source file: $f"; exit 1; }
done

echo "── 1. D1: the ICC-network reconcile is best-effort, not abort-on-first-failure ──"

if has "$DB_SVC" "fn reconcile_network_icc_at"; then
  ok "reconcile_network_icc is path/network-parameterized for testability"
else
  bad "reconcile_network_icc_at is gone"
fi

if bodyhasre "$DB_SVC" "fn reconcile_network_icc_at" 'if let Err\(e\) = docker'; then
  ok "the reconnect loop catches each failure instead of propagating with ?"
else
  bad "the reconnect loop no longer catches individual failures — abort-on-first-failure may be back"
fi

if bodyhasre "$DB_SVC" "fn reconcile_network_icc_at" '\.connect_network\('; then
  ok "connect_network is still called from the reconcile function"
else
  bad "connect_network call is gone from reconcile_network_icc_at"
fi

if bodyhas "$DB_SVC" "fn reconcile_network_icc_at" "failed.push"; then
  ok "every reconnect failure is collected, not just the first"
else
  bad "failures are no longer accumulated — the aggregated error report is gone"
fi

if bodyhas "$DB_SVC" "fn reconcile_network_icc_at" "failed.join"; then
  ok "the returned error names every failed container, not just one"
else
  bad "the error no longer lists every failed container"
fi

# Deliberately NOT using has() here — has()/code() strip the #[cfg(test)]
# module (so a pin can't be tautologically satisfied by test-only code), but
# this assertion's whole point is confirming a specific TEST exists.
if grep -qF "reconcile_network_icc_reconnects_every_container_despite_one_bad_id" "$DB_SVC"; then
  ok "a live Docker-backed regression test exists for this exact shape"
else
  bad "the live reconnect-loop regression test is gone"
fi

echo
echo "── 2. D2: file download streams instead of buffering the whole file in memory ──"

if bodyhas "$FILES_ROUTE" "async fn download_file" "tokio::fs::read(&safe)"; then
  bad "download_file reads the whole file into memory again — the OOM path is back"
else
  ok "download_file no longer reads the whole file into a Vec<u8>"
fi

if bodyhasre "$FILES_ROUTE" "async fn download_file" 'ReaderStream::new|Body::from_stream'; then
  ok "download_file streams the response instead of buffering it"
else
  bad "download_file no longer streams — check what replaced tokio::fs::read"
fi

if has "$FILES_ROUTE" "tokio::fs::File::open(&safe)"; then
  ok "download_file opens the file for streaming rather than reading it eagerly"
else
  bad "download_file no longer opens the file via tokio::fs::File — streaming path may be gone"
fi

if hasre "panel/agent/Cargo.toml" '^tokio-util'; then
  ok "tokio-util is a direct dependency (ReaderStream needs it, not just a transitive one)"
else
  bad "tokio-util is no longer a direct dependency — the streaming fix cannot compile without it"
fi

# The sibling function's own cap must be untouched — this pin suite is about
# the DOWNLOAD path specifically, not about relaxing the text-editor guard.
if bodyhasre "$FILES_SVC" "pub async fn read_file" '2 \* 1024 \* 1024'; then
  ok "read_file's own 2MB text-editor cap is still in place, untouched"
else
  bad "read_file's 2MB cap is gone — unrelated regression"
fi

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
