#!/usr/bin/env bash
# update-connection-resilience-pin-e2e.sh — s447
#
# Pins the docker_apps.rs finding named in [[project_dockpanel_tech_debt_p185]]
# §A: Update / change-image / edit-env recreate a container between
# `remove_container` and `create_container`, and until this fix every one of
# those calls ran INLINE on the request's own connection task — agent-side
# (`update`, `update_env`, `change_image` in panel/agent/src/routes/docker_apps.rs)
# AND, for two of the three doors, backend-side too
# (`update_env`/`update_image` in panel/backend/src/routes/docker_apps.rs;
# `update_app` already spawned). A dropped connection (demo.dockpanel.dev is
# Cloudflare-proxied with a ~100s origin budget, well inside the 900s these
# calls are allowed) cancelled the in-flight future mid-recreate: the app
# container vanishes, only its bind-mounted data survives.
#
# Fix: wrap each call in `tokio::spawn(...).await` — the spawned task
# outlives a dropped connection because it is scheduled onto the runtime
# independently of the future that's awaiting its JoinHandle.
#
# Control: 0 `tokio::spawn` sites in the agent routes file before this fix.
#
# Pure source analysis: no box, no network, no build.

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

echo
echo "=================================================="
echo "  docker_apps.rs recreate doors — connection resilience (s447)"
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
has()  { grep -qE -- "$2" <<< "$1"; }
flat() { tr '\n' ' ' <<< "$1" | tr -s ' '; }

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

AGENT_ROUTES=panel/agent/src/routes/docker_apps.rs
BACKEND_ROUTES=panel/backend/src/routes/docker_apps.rs
[ -f "$AGENT_ROUTES" ] || { bad "SETUP subject missing: $AGENT_ROUTES"; exit 1; }
[ -f "$BACKEND_ROUTES" ] || { bad "SETUP subject missing: $BACKEND_ROUTES"; exit 1; }
AGENT_SRC=$(code "$AGENT_ROUTES")
BACKEND_SRC=$(code "$BACKEND_ROUTES")

# ── §A whole-file control: agent routes gained spawns (was 0) ──────────────

AGENT_SPAWN_COUNT=$(grep -c 'tokio::spawn' "$AGENT_ROUTES")
if [ "$AGENT_SPAWN_COUNT" -ge 3 ]; then
  ok "A1 agent routes/docker_apps.rs has $AGENT_SPAWN_COUNT tokio::spawn sites (>= 3, control was 0)"
else
  bad "A1 agent routes/docker_apps.rs has only $AGENT_SPAWN_COUNT tokio::spawn sites, expected >= 3"
fi

# ── §B each of the 3 agent handlers spawns its recreate call ───────────────

for FN in update update_env change_image; do
  BODY=$(flat "$(fnbody "$AGENT_SRC" "$FN")")
  if [ -z "$BODY" ]; then
    bad "B:$FN could not extract agent handler"
  elif has "$BODY" 'tokio::spawn'; then
    ok "B:$FN spawns its recreate call onto its own task"
  else
    bad "B:$FN still awaits the recreate call inline — a dropped connection cancels it mid-recreate"
  fi
done

# ── §C both backend doors spawn too (update_app already did) ───────────────

for FN in update_app update_env update_image; do
  BODY=$(flat "$(fnbody "$BACKEND_SRC" "$FN")")
  if [ -z "$BODY" ]; then
    bad "C:$FN could not extract backend handler"
  elif has "$BODY" 'tokio::spawn'; then
    ok "C:$FN spawns its long agent call onto its own task"
  else
    bad "C:$FN still awaits the agent call inline — a browser disconnect cancels the backend leg mid-recreate"
  fi
done

echo
echo "=================================================="
echo "  PASS=$PASS FAIL=$FAIL"
echo "=================================================="

[ "$FAIL" -eq 0 ]
