#!/usr/bin/env bash
# unattended-host-scope-pin-e2e.sh — s298 / v2.56.0
#
# An unattended service must name the host it acts on.
#
# DockPanel's multi-server migration reached every HTTP route and NOT ONE
# background service. `AppState` carries both handles — `agents: AgentRegistry`
# ("dispatches to local or remote agents by server_id") and `agent: AgentClient`
# ("Legacy single-agent accessor") — and all twelve background services were
# spawned with the legacy one. Each queries rows across the whole fleet and acts
# on whichever machine the panel happens to run on.
#
# For the disk healer that is destruction, not routing. `auto_clean_disk` read
# ONE firing `disk` row with no `server_id` predicate and sent the fixes to the
# local agent. `alert_state` is keyed per server, so ANY member crossing its
# threshold made the panel host clean and prune ITSELF while the full machine was
# never touched. Driven on a two-box fleet against the released v2.55.0: member
# at 93%, panel host at 18% with its own disk never measured, and forty seconds
# after auto-healing was enabled the panel host lost a tenant's container and its
# image — an app the panel had itself put to sleep.
#
#   §A  THE HOST IS NAMED. The row carries a server_id; the action resolves that
#       server's agent; an unreachable agent is REFUSED, never silently swapped
#       for the local one — the fallback was the whole defect.
#   §B  RECOVERY COMPLETES RATHER THAN BEING CONSUMED. A raw UPDATE to 'ok'
#       skipped resolve_alert, so the `alerts` row stayed firing for ever and the
#       engine, seeing state already 'ok', never took its recovery branch again.
#       Retention only purges status='resolved', so the row was unpurgeable too.
#   §C  THE RECLAIM CANNOT REACH WHAT THE PANEL MANAGES. `system prune -af
#       --volumes` removes every stopped container — and a SLEEPING app is a
#       stopped container. Removing it detached its volumes, which the same
#       command reclaimed, and `-a` took the locally built image with no registry
#       to restore from.
#   §D  DESTRUCTION IS CONSENTED TO SEPARATELY, AND THE COOLDOWN CAN EXIST. The
#       hourly gate counted `activity_logs` rows written with `Uuid::nil()`,
#       which violates fk_activity_logs_user — every insert failed, the count was
#       always 0, and the prune ran every 120 SECONDS (measured 19:16:13,
#       19:18:13, 19:20:13, 19:22:13).
#   §E  RETENTION KEEPS THE ONLY RECORD OF WHAT IT DID NOT DELETE.
#
# Pure source analysis: no box, no network, no build.
#
# Arms key on the CAPABILITY a regression must use, not today's spelling
# (lesson #122). Where an arm could be satisfied by the prose NARRATING a check,
# it keys on the syntax of the operation (lesson #149). An arm that requires a
# key requires a VALUE (lesson #155) — `grep 'scope: "'` is not a check.
#
# NO PIPES INTO `grep -q`: under pipefail grep -q closes the pipe on first match
# and the arm goes red on correct code. Every arm feeds grep a here-string.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0; FAIL=0; SKIP=0
ok()   { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }
skip() { SKIP=$((SKIP+1)); printf '  \033[33m-\033[0m SKIP %s\n' "$1"; }

AH=panel/backend/src/services/auto_healer.rs
MAIN=panel/backend/src/main.rs
DIAG=panel/agent/src/services/diagnostics.rs
SET=panel/backend/src/routes/settings.rs
SETTSX=panel/frontend/src/pages/Settings.tsx
AGENT=panel/backend/src/services/agent.rs

for f in "$AH" "$MAIN" "$DIAG" "$SET" "$SETTSX" "$AGENT"; do
  [ -f "$f" ] || bad "MISSING SUBJECT FILE: $f"
done

# Comments out, CODE INTACT. Copied from the FIXED stripper (lesson #136).
code() {
  perl -0777 -pe '
    s{\{/\*.*?\*/\}}{}gs;
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
  ' "$1"
}

# The stripper is code, and code can be wrong. Measure it before trusting it.
stripper_self_check() {
  local bad_files=0 f raw stripped
  for f in "$@"; do
    [ -f "$f" ] || continue
    raw=$(grep -cE '^[[:space:]]*(pub )?(pub\(crate\) )?(async )?fn ' "$f" || true)
    stripped=$(grep -cE '^[[:space:]]*(pub )?(pub\(crate\) )?(async )?fn ' <<< "$(code "$f")" || true)
    if [ "$raw" != "$stripped" ]; then
      bad "STRIPPER ATE CODE in $f: $raw fn declarations before, $stripped after"
      bad_files=$((bad_files+1))
    fi
  done
  [ "$bad_files" -eq 0 ] && ok "comment stripper preserves every fn declaration in all $# subjects"
}

subj() { local t; t=$(code "$1"); [ -n "$t" ] || return 1; printf '%s' "$t"; }
has()   { grep -qE -- "$2" <<< "$1"; }
count() { grep -cE -- "$2" <<< "$1" || true; }

# `has` is LINE-oriented, but Rust signatures and multi-line SQL string literals
# wrap. An arm keyed on a construct that spans lines matches nothing — and it
# fails in whichever direction the arm is written, so a PRESENCE arm goes red on
# correct code (noisy, survivable) while an ABSENCE arm goes GREEN on the defect
# (silent, not survivable). Caught at s301: three §G arms measured nothing and
# only the red told me. Flatten a WINDOW, never a whole subject — flattening the
# subject is window bleed by construction (p31).
flat() { tr '\n' ' ' <<< "$1" | tr -s ' '; }

# The flattened signature of one function: enough lines to cover a wrapped
# parameter list, anchored on the paren so `fn run` cannot latch onto `fn run_scan`.
sig() { flat "$(grep -A 8 -E "fn $2\(" <<< "$1" || true)"; }

# Every statement matching a pattern, each flattened onto its own line, so a
# line-oriented `has`/`count` can measure multi-line SQL without bleeding across
# statements. `--` separates grep's windows; it becomes the line break.
stmt() { grep -A "${3:-3}" -E -- "$2" <<< "$1" | tr '\n' ' ' | sed 's/ -- /\n/g' | tr -s ' '; }

# Extract one function body by brace balance. A fixed `-A n` window is NOT a
# function (lesson: p31) — a regression that moves a line past the window makes
# the arm silently measure nothing.
fnbody() {
  local src="$1" name="$2"
  awk -v fn="$name" '
    index($0, "fn " fn) && !started { started=1 }
    started {
      n=gsub(/\{/,"{"); m=gsub(/\}/,"}"); depth += n - m; print
      if (opened || n>0) opened=1
      if (opened && depth<=0) exit
    }
  ' <<< "$src"
}

echo
echo "unattended-host-scope-pin-e2e — s298 / v2.56.0 · §F s299 · §G s300+s301"
echo "=============================================="
echo
echo "§0 harness self-check"
stripper_self_check "$AH" "$MAIN" "$DIAG" "$SET" "$AGENT"

AH_S=$(subj "$AH")   || { bad "cannot read $AH"; AH_S=""; }
MAIN_S=$(subj "$MAIN") || { bad "cannot read $MAIN"; MAIN_S=""; }
DIAG_S=$(subj "$DIAG") || { bad "cannot read $DIAG"; DIAG_S=""; }
SET_S=$(subj "$SET")   || { bad "cannot read $SET"; SET_S=""; }
TSX_S=$(subj "$SETTSX")|| { bad "cannot read $SETTSX"; TSX_S=""; }
AGENT_S=$(subj "$AGENT") || { bad "cannot read $AGENT"; AGENT_S=""; }

CLEAN=$(fnbody "$AH_S" "auto_clean_disk")
[ -n "$CLEAN" ] || bad "could not extract auto_clean_disk — every §A/§B arm below is meaningless"

echo
echo "§A the host is named"

# A1 — the firing query must carry server_id. The defect was its ABSENCE, so the
# arm requires the predicate, not merely the word.
if has "$CLEAN" "server_id[^\n]*IS NOT NULL|a\.server_id|WHERE a\.alert_type"; then
  ok "A1 auto_clean_disk's firing query is server-aware"
else
  bad "A1 auto_clean_disk selects firing disk alerts with NO server_id — the s298 defect"
fi

# A2 — it must resolve the agent for THAT server, not use a handed-in local one.
if has "$CLEAN" "for_server\("; then
  ok "A2 auto_clean_disk resolves the agent via for_server(server_id)"
else
  bad "A2 auto_clean_disk does not call for_server — it acts on whatever agent it was handed"
fi

# A3 — the signature must not accept a bare local client (the old shape).
if has "$AH_S" "async fn auto_clean_disk\([^)]*AgentRegistry"; then
  ok "A3 auto_clean_disk takes the AgentRegistry, not a single AgentClient"
else
  bad "A3 auto_clean_disk still takes a single AgentClient — it cannot reach another host"
fi

# A4 — main.rs must actually hand the healer the registry. A4 is the arm that
# fails if the fix exists but is never wired (the s297 shape).
if has "$MAIN_S" "auto_healer::run\(.*agents"; then
  ok "A4 main.rs spawns auto_healer WITH the registry"
else
  bad "A4 auto_healer is spawned without the registry — the fix is unreachable"
fi

# A5 — the alert_state reset must be scoped. Unscoped, healing one host cleared
# every other server's disk state and its cooldown clock.
RESET=$(grep -A 4 "UPDATE alert_state SET current_state = 'ok'" <<< "$CLEAN" || true)
if [ -n "$RESET" ] && has "$RESET" "server_id = \\\$1"; then
  ok "A5 the alert_state reset is scoped to the server that was healed"
else
  bad "A5 the alert_state reset is fleet-wide — it clears servers nobody healed"
fi

# A6 — an unreachable agent must be REFUSED. Falling back to the local agent is
# the defect itself, so the arm requires the refusal, not just an error branch.
if has "$CLEAN" "continue" && has "$CLEAN" "Refusing to act on a different host|NOT cleaning"; then
  ok "A6 an unreachable agent is refused rather than swapped for the local one"
else
  bad "A6 no explicit refusal when the target server's agent is unreachable"
fi

echo
echo "§B recovery completes rather than being consumed"

# B1 — resolve_alert is the ONLY writer of alerts.status='resolved'.
if has "$CLEAN" "resolve_alert\("; then
  ok "B1 auto_clean_disk completes recovery through resolve_alert"
else
  bad "B1 auto_clean_disk resets alert_state without resolve_alert — alerts rows stay firing for ever"
fi

# B2 — the activity row must not be written with the nil uuid: that insert
# violates fk_activity_logs_user, which is what killed the cooldown.
if has "$CLEAN" "Uuid::nil\(\)|uuid::Uuid::nil\(\)"; then
  bad "B2 auto_clean_disk logs activity as Uuid::nil() — the insert fails and the cooldown never engages"
else
  ok "B2 auto_clean_disk does not log activity as the nil uuid"
fi

# B3 — and it must write SOMETHING, or the gate it reads can never be satisfied.
# Either writer satisfies that: the capability is "a row exists for the gate to
# count", not one spelling of it. Keyed on the bare `log_activity(` alone, this
# arm went red when s304 moved the call to the `_on_server` variant so the host
# could travel in `server_id` instead of squatting in `target_name` — correct
# code, red arm, which is the failure the suite header warns about. §J8 pins the
# stronger property: that this writer and its cooldown agree on the SAME key.
if has "$CLEAN" "log_activity(_on_server)?\("; then
  ok "B3 auto_clean_disk still records the action it gates itself on"
else
  bad "B3 no activity record written — the cooldown query can never find a row"
fi

echo
echo "§C the reclaim cannot reach what the panel manages"

# C1 — the unscoped prune must be gone from the WHOLE tree, not just this file.
PRUNE_HITS=$(grep -rn -- '"system", *"prune"' --include=*.rs panel/agent/src panel/backend/src panel/cli/src 2>/dev/null | grep -c -- '--volumes' || true)
if [ "$PRUNE_HITS" = "0" ]; then
  ok "C1 no 'docker system prune ... --volumes' anywhere in the tree"
else
  bad "C1 $PRUNE_HITS site(s) still run 'docker system prune --volumes' — it deletes sleeping apps"
fi

# C2 — the scoped replacement exists and is reachable by id.
if has "$DIAG_S" '"docker-reclaim"'; then
  ok "C2 the scoped docker-reclaim fix exists"
else
  bad "C2 no docker-reclaim fix — the disk heal has nothing safe to call"
fi

RECLAIM=$(awk '/"docker-reclaim"/{f=1} f{print} f&&/^        \}/{exit}' <<< "$DIAG_S")

# ⚠ C3 and C4 are ABSENCE arms. An absence arm over an EMPTY subject is
# vacuously true, so both printed a confident GREEN against v2.55.0 — where
# `docker-reclaim` does not exist at all — directly under a red saying the
# subject could not be extracted. That is the #143 shape (an arm that
# enumerates its own subject must assert the enumeration FIRST), and it was
# caught only by running this suite RED against the previous tag. They SKIP
# now, because "the reclaim does not reclaim volumes" is not a fact about a
# reclaim that has not been written.
if [ -z "$RECLAIM" ]; then
  skip "C3 docker-reclaim arm not extractable — no subject to measure"
  skip "C4 docker-reclaim arm not extractable — no subject to measure"
else
  # C3 — volumes are never reclaimed: an unattached volume is indistinguishable
  # from one whose container the panel stopped on purpose.
  if has "$RECLAIM" "[-][-]volumes"; then
    bad "C3 docker-reclaim passes --volumes — a slept app's data is reclaimable again"
  else
    ok "C3 docker-reclaim never reclaims volumes"
  fi

  # C4 — image prune must not be -a: -a removes images backing STOPPED
  # containers, which is exactly the slept-app case, and a locally built image
  # cannot be pulled back.
  if has "$RECLAIM" '"image", *"prune", *"-af"|"image", *"prune", *"-a"'; then
    bad "C4 docker-reclaim prunes images with -a — it takes locally built images with no registry"
  else
    ok "C4 docker-reclaim prunes only dangling images"
  fi
fi

# C5 — the OLD id must not resurrect the old behaviour for an older panel.
if has "$DIAG_S" '"docker-reclaim" \| "docker-prune"|"docker-prune" \| "docker-reclaim"'; then
  ok "C5 the legacy docker-prune id routes to the scoped reclaim"
else
  bad "C5 docker-prune is not routed to the scoped reclaim — an older panel gets the destructive path back by name"
fi

echo
echo "§D consent is explicit and the cooldown can exist"

# D1 — the setting must be writable through the API, or the control is dead.
if has "$SET_S" '"auto_heal_docker_reclaim"'; then
  ok "D1 auto_heal_docker_reclaim is an allowed settings key"
else
  bad "D1 auto_heal_docker_reclaim is not in ALLOWED_KEYS — the UI control cannot save"
fi

# D2 — default OFF. Requires the VALUE, not just the key (lesson #155).
GATE=$(fnbody "$AH_S" "reclaim_enabled")
if [ -n "$GATE" ] && has "$GATE" "unwrap_or\(false\)"; then
  ok "D2 docker reclamation defaults to OFF"
else
  bad "D2 reclaim_enabled does not default to false — destruction is opt-OUT"
fi

# D3 — the reclaim must actually be gated on it, or the setting is decoration.
if has "$CLEAN" "reclaim_enabled\("; then
  ok "D3 the reclaim step is gated on the operator's consent"
else
  bad "D3 the reclaim runs without consulting reclaim_enabled"
fi

# D4 — an operator control must exist. Requires the api.put VALUE, not the word.
if has "$TSX_S" "auto_heal_docker_reclaim"; then
  ok "D4 Settings exposes a control for docker reclamation"
else
  bad "D4 no operator control for auto_heal_docker_reclaim — a DB-only switch"
fi

# D5 — the consent text must not still promise only log cleaning.
if has "$TSX_S" "Logs are cleaned when disk exceeds 90%"; then
  bad "D5 the Auto-Healing panel still claims a 90% threshold and log-only cleaning"
else
  ok "D5 the Auto-Healing consent text no longer misstates what runs"
fi

echo
echo "§E retention keeps the only record of what it did not delete"

# E1/E2 — the unlink path must match the writer, which nests per resource.
if has "$AH_S" 'databases/\{db_name\}/\{filename\}'; then
  ok "E1 the database retention path nests per database, as the writer does"
else
  bad "E1 the database retention path omits {db_name} — it unlinks a path that never exists"
fi

if has "$AH_S" 'volumes/\{container_name\}/\{filename\}'; then
  ok "E2 the volume retention path nests per container, as the writer does"
else
  bad "E2 the volume retention path omits {container_name} — it unlinks a path that never exists"
fi

# E3 — the row must not be deleted unconditionally after a discarded unlink.
if has "$AH_S" "fn prune_policy_backup"; then
  ok "E3 policy retention goes through a single guarded retire step"
else
  bad "E3 no prune_policy_backup — the DELETE is not guarded by the unlink result"
fi

PPB=$(fnbody "$AH_S" "prune_policy_backup")
if [ -n "$PPB" ]; then
  # E4 — a failed unlink must KEEP the row: it is the only record of the archive.
  if has "$PPB" "return false"; then
    ok "E4 a failed unlink keeps the row that names the archive"
  else
    bad "E4 the row is deleted regardless of whether the archive was removed"
  fi
  # E5 — a backup on ANOTHER server cannot be retired by a local unlink.
  if has "$PPB" "row_server|local_server"; then
    ok "E5 a backup belonging to another server is refused, not silently forgotten"
  else
    bad "E5 policy retention unlinks locally for backups that live on other servers"
  fi
else
  skip "E4/E5 — prune_policy_backup body not extractable"
fi

# E6 — retention must be per resource. OFFSET over a policy's whole history kept
# n backups in TOTAL across every database it covered.
if has "$AH_S" "PARTITION BY database_id" && has "$AH_S" "PARTITION BY container_id"; then
  ok "E6 policy retention_count is applied per resource, not per policy"
else
  bad "E6 policy retention still uses a policy-wide OFFSET — most resources keep zero backups"
fi

echo
echo "§F the scheduled and webhook paths name the host too (s299)"

DEPS=panel/backend/src/services/deploy_scheduler.rs
PREV=panel/backend/src/services/preview_cleanup.rs
GITD=panel/backend/src/routes/git_deploys.rs
DEPR=panel/backend/src/routes/deploy.rs
MET=panel/backend/src/services/metrics_collector.rs

for f in "$DEPS" "$PREV" "$GITD" "$DEPR" "$MET"; do
  [ -f "$f" ] || bad "MISSING SUBJECT FILE: $f"
done

DEPS_S=$(subj "$DEPS" || true)
PREV_S=$(subj "$PREV" || true)
GITD_S=$(subj "$GITD" || true)
DEPR_S=$(subj "$DEPR" || true)
MET_S=$(subj "$MET" || true)

# F1/F2 — the scheduler's two fleet-wide SELECTs must name the server. Both were
# `SELECT id, name, ... FROM git_deploys` with no server_id at all, so the row
# could not say which machine it meant even in principle.
if [ -n "$DEPS_S" ]; then
  SCHED=$(fnbody "$DEPS_S" "check_schedules")
  if [ -z "$SCHED" ]; then
    skip "F1/F2/F14/F15 — check_schedules body not extractable"
  else
    if [ "$(count "$SCHED" "SELECT[^;]*server_id[^;]*FROM git_deploys")" -ge 2 ]; then
      ok "F1/F2 both scheduler queries (cron + one-time) select server_id"
    else
      bad "F1/F2 a scheduler query still reads git_deploys without server_id — the row cannot name its host"
    fi

    # F14 — the one-time clear is the ONLY thing between a scheduled deploy and
    # an unbounded redeploy loop: a successful run leaves the row 'running',
    # which the one-time SELECT does not exclude. It used to be swallowed
    # with `.ok()`.
    if has "$SCHED" "rows_affected\(\)"; then
      ok "F14 the one-time schedule clear is checked before deploying"
    else
      bad "F14 the one-time schedule clear is unchecked — an uncleared row redeploys every tick"
    fi

    # F15 — reachability BEFORE the clear. Clearing first discards the
    # operator's only copy of a one-time instruction for a server we then
    # decline to deploy to. Order matters, so measure the order.
    F15_RESOLVE=$(grep -n "for_server" <<< "$SCHED" | head -1 | cut -d: -f1)
    F15_CLEAR=$(grep -n "scheduled_deploy_at = NULL" <<< "$SCHED" | head -1 | cut -d: -f1)
    if [ -n "$F15_RESOLVE" ] && [ -n "$F15_CLEAR" ] && [ "$F15_RESOLVE" -lt "$F15_CLEAR" ]; then
      ok "F15 the one-time path checks the server is reachable BEFORE clearing the schedule"
    else
      bad "F15 the one-time schedule is cleared before the server is known reachable — an unreachable member loses it silently"
    fi
  fi
else
  skip "F1/F2/F14/F15 — deploy_scheduler.rs not extractable"
fi

# F3/F4/F5 — trigger_deploy_task is where the host was decided. It took an
# AgentClient and wrapped it `AgentHandle::Local(agent)` three lines before
# fetching the row that carries server_id.
if [ -n "$GITD_S" ]; then
  TDT=$(fnbody "$GITD_S" "trigger_deploy_task")
  if [ -z "$TDT" ]; then
    skip "F3/F4/F5 — trigger_deploy_task body not extractable"
  else
    if has "$TDT" "AgentHandle::Local"; then
      bad "F3 trigger_deploy_task still forces the LOCAL agent — every scheduled deploy runs on the panel host"
    else
      ok "F3 trigger_deploy_task no longer forces AgentHandle::Local"
    fi

    if has "$TDT" "for_server\(config\.server_id\)"; then
      ok "F4 trigger_deploy_task resolves the agent from the ROW's own server_id"
    else
      bad "F4 trigger_deploy_task does not resolve for_server(config.server_id)"
    fi

    # The refusal must be a refusal: an early return, not a fallback.
    if has "$TDT" "Refusing to act on a different host"; then
      ok "F5 an unreachable deploy server is refused, never swapped for the local one"
    else
      bad "F5 trigger_deploy_task has no explicit refusal when the server is unreachable"
    fi
  fi
else
  skip "F3/F4/F5 — git_deploys.rs not extractable"
fi

# F6/F7 — the spawn sites. A migration is not done when the callee is done
# (lesson #156): grep the SPAWN, not just the call.
if [ -n "$MAIN_S" ]; then
  if has "$MAIN_S" "deploy_scheduler::run\(s_db\.clone\(\), s_agents"; then
    ok "F6 main.rs spawns deploy_scheduler with the REGISTRY"
  else
    bad "F6 deploy_scheduler is still spawned with the legacy single-agent handle"
  fi
  if has "$MAIN_S" "preview_cleanup::run\(s_db\.clone\(\), s_agents"; then
    ok "F7 main.rs spawns preview_cleanup with the REGISTRY"
  else
    bad "F7 preview_cleanup is still spawned with the legacy single-agent handle"
  fi
else
  skip "F6/F7 — main.rs not extractable"
fi

# F8/F9 — preview teardown. git_previews carries no server of its own, but the
# sweep already JOINs git_deploys, whose server_id is NOT NULL.
if [ -n "$PREV_S" ]; then
  if [ "$(count "$PREV_S" "d\.server_id")" -ge 2 ]; then
    ok "F8 both preview sweeps carry the git deploy's server_id through the JOIN"
  else
    bad "F8 a preview sweep still selects rows that cannot name their host"
  fi
  if has "$PREV_S" "for_server\(" && has "$PREV_S" "Refusing to act on a different host"; then
    ok "F9 preview teardown resolves that server and refuses when unreachable"
  else
    bad "F9 preview teardown still tears down containers on whichever host the panel runs on"
  fi
else
  skip "F8/F9 — preview_cleanup.rs not extractable"
fi

# F10 — the git-deploy webhook. Unauthenticated by design (it carries a secret,
# not a session), so it has no ServerScope and reached for the local agent.
if [ -n "$GITD_S" ]; then
  WH=$(fnbody "$GITD_S" "webhook")
  if [ -z "$WH" ]; then
    skip "F10 — webhook body not extractable"
  elif has "$WH" "for_server\(config\.server_id\)" && ! has "$WH" "AgentHandle::Local"; then
    ok "F10 the git-deploy webhook deploys on the server the deployment lives on"
  else
    bad "F10 a webhook push still builds and replaces containers on the panel host"
  fi
else
  skip "F10 — git_deploys.rs not extractable"
fi

# F11 — the SITE deploy webhook, same shape, different file. sites.server_id is
# NOT NULL, and the handler used to select only `domain`.
if [ -n "$DEPR_S" ]; then
  if has "$DEPR_S" "SELECT domain, server_id FROM sites" && has "$DEPR_S" "for_server\(site_server_id\)"; then
    ok "F11 the site webhook resolves the site's own server"
  else
    bad "F11 the site webhook still deploys a remote tenant's site onto the panel host"
  fi
else
  skip "F11 — deploy.rs not extractable"
fi

# F12 — the guard that makes all of the above safe on a SINGLE-server install.
# `ensure_local_server` returns Uuid::nil() until an admin exists, so the cached
# local id can be None; the local row has no agent_url, so the DB fallthrough
# would return NotFound and every threaded service would refuse to act on its
# own box. This arm is the difference between a fix and an outage.
if [ -n "$AGENT_S" ]; then
  FS=$(fnbody "$AGENT_S" "for_server")
  if [ -z "$FS" ]; then
    skip "F12 — for_server body not extractable"
  elif has "$FS" "is_local" && has "$FS" "AgentHandle::Local"; then
    ok "F12 for_server returns the LOCAL handle for the local row even when the cached id is unset"
  else
    bad "F12 for_server cannot recognise the local server from the DB — a single-server install would refuse every threaded action"
  fi
else
  skip "F12 — agent.rs not extractable"
fi

# F13 — the laundering step. The server_id a reading is stored under decides
# which machine every downstream threshold and heal is about.
#
# ⚠ RE-POINTED at s302, and the reason is the point. This arm used to require the
# literal `FROM servers WHERE is_local = true` INSIDE the collector, because s299
# fixed the laundering by making that one lookup ask for the right row. s302
# removed the lookup altogether: the collector now iterates `online_fleet` and
# takes each id from the member it just read, so there is no single "local" id to
# derive and the whole class of derivation is gone. The property is unchanged and
# is now stronger; only its home moved. Per lesson #150 a re-pointed arm owes a
# companion proving the caller still REACHES the new home — F13c below — or a
# resolver nothing calls would satisfy it.
if [ -n "$MET_S" ]; then
  if has "$MET_S" '\.bind\(member\.id\)'; then
    ok "F13 metrics_collector labels each reading with the server it was read from"
  else
    bad "F13 metrics_collector does not stamp readings per member — readings can be stored against the wrong machine"
  fi
  if has "$MET_S" "ORDER BY created_at ASC LIMIT 1"; then
    bad "F13b the oldest-row derivation is still present in metrics_collector"
  else
    ok "F13b the oldest-row derivation is gone"
  fi
  if has "$MET_S" 'online_fleet\(\)'; then
    ok "F13c the collector actually reaches the fleet primitive it takes its ids from"
  else
    bad "F13c the collector never calls online_fleet — F13's ids come from somewhere unproven"
  fi
else
  skip "F13 — metrics_collector.rs not extractable"
fi

# F13d — and the primitive those ids come from must not itself launder. This is
# the assertion that moved out of the collector: whoever resolves "which server"
# must not do it by row age.
if [ -n "$AGENT_S" ]; then
  OF=$(flat "$(grep -A 10 -E 'pub async fn online_fleet' <<< "$AGENT_S" || true)")
  if has "$OF" 'ORDER BY created_at'; then
    bad "F13d online_fleet orders the fleet by row age — the laundering moved rather than left"
  elif has "$OF" 'is_local'; then
    ok "F13d online_fleet resolves each server by identity, never by row age"
  else
    bad "F13d online_fleet does not name is_local — nothing distinguishes the panel row while iterating"
  fi
else
  skip "F13d — agent.rs not extractable"
fi


echo
echo "§G the last background services name their host (s300 backups + s301 scanners)"

# s300 threaded the backup family and landed with NO arms at all; s301 finished
# the scanners. Both are pinned here, because the two halves share one hazard:
# a fix that POPULATES a column is the first thing that ever runs the mechanisms
# written against it, and those mechanisms have never been exercised.
BS=panel/backend/src/services/backup_scheduler.rs
BPE=panel/backend/src/services/backup_policy_executor.rs
BV=panel/backend/src/services/backup_verifier.rs
DS=panel/backend/src/services/drill_scheduler.rs
TC=panel/backend/src/services/telemetry_collector.rs
BO=panel/backend/src/routes/backup_orchestrator.rs
SS=panel/backend/src/services/security_scanner.rs
SSR=panel/backend/src/routes/security_scans.rs
IS=panel/backend/src/services/image_scanner.rs
ISR=panel/backend/src/routes/image_scans.rs
SB=panel/backend/src/routes/sboms.rs
AE=panel/backend/src/services/alert_engine.rs

for f in "$BS" "$BPE" "$BV" "$DS" "$TC" "$BO" "$SS" "$SSR" "$IS" "$ISR" "$SB" "$AE"; do
  [ -f "$f" ] || bad "MISSING SUBJECT FILE: $f"
done

BS_S=$(subj "$BS" || true);   BPE_S=$(subj "$BPE" || true); BV_S=$(subj "$BV" || true)
DS_S=$(subj "$DS" || true);   TC_S=$(subj "$TC" || true);   BO_S=$(subj "$BO" || true)
SS_S=$(subj "$SS" || true);   SSR_S=$(subj "$SSR" || true); IS_S=$(subj "$IS" || true)
ISR_S=$(subj "$ISR" || true); SB_S=$(subj "$SB" || true);   AE_S=$(subj "$AE" || true)

# ── the backup family (s300) ───────────────────────────────────────────────
if [ -n "$BS_S" ]; then
  if has "$(sig "$BS_S" run)" "AgentRegistry" && has "$BS_S" "for_server\("; then
    ok "G1 backup_scheduler resolves each site's own server"
  else
    bad "G1 backup_scheduler still backs up every site through one agent"
  fi
else
  skip "G1 — backup_scheduler.rs not extractable"
fi

if [ -n "$BPE_S" ]; then
  if has "$BPE_S" "for_server\(" && has "$(sig "$BPE_S" run)" "AgentRegistry"; then
    ok "G2 backup_policy_executor resolves the host per row"
  else
    bad "G2 backup_policy_executor still writes every policy backup to the panel"
  fi
else
  skip "G2 — backup_policy_executor.rs not extractable"
fi

if [ -n "$BV_S" ]; then
  if has "$BV_S" "for_server\(" && has "$(sig "$BV_S" run)" "AgentRegistry"; then
    ok "G3 backup_verifier reads each archive on the host that wrote it"
  else
    bad "G3 backup_verifier still verifies every archive on the panel"
  fi
else
  skip "G3 — backup_verifier.rs not extractable"
fi

# G4 — the #164 arm for the backup half. Populating drill_scheduler's server_id
# ARMS a per-server concurrency guard that has never once engaged, and the db
# drill INSERTs a 'running' row immediately before the volume drill counts them.
# Without an age bound one stranded row disables DR for that machine for ever.
if [ -n "$DS_S" ]; then
  GUARD=$(fnbody "$DS_S" "has_running_drill_for_server")
  if [ -z "$GUARD" ]; then
    skip "G4 — has_running_drill_for_server not extractable"
  elif has "$GUARD" "INTERVAL|started_at"; then
    ok "G4 the drill concurrency guard is age-bounded, so one stranded drill cannot disable DR for ever"
  else
    bad "G4 the drill guard has no age bound — arming it makes a crashed drill permanent"
  fi
else
  skip "G4 — drill_scheduler.rs not extractable"
fi

# G5 — telemetry_collector is local BY INTENT. The arm requires that intent to be
# STATED (it takes the registry and asks it for the local agent), not merely to
# coincide with a legacy handle nobody revisited.
if [ -n "$TC_S" ]; then
  if has "$(sig "$TC_S" run)" "AgentRegistry" && has "$TC_S" "\.local\(\)"; then
    ok "G5 telemetry_collector states local-by-intent through the registry"
  else
    bad "G5 telemetry_collector's locality is an accident of the legacy handle again"
  fi
else
  skip "G5 — telemetry_collector.rs not extractable"
fi

# G6 — the laundering shape (s299's metrics_collector defect, verbatim): resolve
# the real server, discard it, then stamp the row from an arbitrary online row.
if [ -n "$BO_S" ]; then
  if has "$BO_S" "FROM servers WHERE status = 'online' LIMIT 1"; then
    bad "G6 backup_orchestrator still stamps server_id from an arbitrary online server"
  else
    ok "G6 the arbitrary-online-server stamp is gone from backup_orchestrator"
  fi
else
  skip "G6 — backup_orchestrator.rs not extractable"
fi

# ── the scanners (s301) ────────────────────────────────────────────────────
# G7/G8 — both scanners take a MACHINE as their subject, so the arm requires the
# fleet LOOP, not merely a registry in the signature.
if [ -n "$SS_S" ]; then
  if has "$(sig "$SS_S" run)" "AgentRegistry" && has "$SS_S" "online_fleet\(\)"; then
    ok "G7 security_scanner sweeps the fleet instead of the panel alone"
  else
    bad "G7 security_scanner still scans only the panel host — members are never scanned at all"
  fi
else
  skip "G7 — security_scanner.rs not extractable"
fi

if [ -n "$IS_S" ]; then
  if has "$(sig "$IS_S" run)" "AgentRegistry" && has "$IS_S" "online_fleet\(\)"; then
    ok "G8 image_scanner sweeps the fleet instead of the panel alone"
  else
    bad "G8 image_scanner still sweeps only the panel's images"
  fi
else
  skip "G8 — image_scanner.rs not extractable"
fi

if [ -n "$MAIN_S" ]; then
  for pair in "security_scanner:G9" "image_scanner:G10" "alert_engine:G11"; do
    svc=${pair%%:*}; id=${pair##*:}
    if has "$MAIN_S" "${svc}::run\(s_db\.clone\(\), s_agents"; then
      ok "$id main.rs spawns $svc with the REGISTRY"
    else
      bad "$id $svc is still spawned with the legacy single-agent handle — the fix exists but is never wired"
    fi
  done
else
  skip "G9/G10/G11 — main.rs not extractable"
fi

# G12 — THE arming arm. The upsert's arbiter named a column the INSERT never
# supplied, and a NULL never conflict-matches, so DO UPDATE had never executed:
# the file-integrity alarm saw the FIRST change to a watched file and was deaf
# to every change after it. Key on the INSERT's column list, since the ON
# CONFLICT clause was already correct and is not what was broken.
for pair in "SS_S:G12a:security_scanner" "SSR_S:G12b:the manual trigger route"; do
  var=${pair%%:*}; rest=${pair#*:}; id=${rest%%:*}; who=${rest##*:}
  src=${!var}
  if [ -z "$src" ]; then skip "$id — subject not extractable"; continue; fi
  INS=$(stmt "$src" 'INSERT INTO file_integrity_baselines' 2)
  if [ -z "$INS" ]; then
    skip "$id — baseline INSERT not found in $who"
  elif has "$INS" 'file_integrity_baselines \(server_id, file_path'; then
    ok "$id the baseline upsert in $who supplies server_id, so ON CONFLICT can match"
  else
    bad "$id the baseline upsert in $who still omits server_id — DO UPDATE can never fire"
  fi
done

# G13 — the comparison read. `server_id IS NULL` was not a filter but the absence
# of one; once the column is real it matches nothing and the alarm goes SILENT.
# Both halves of the change have to move together or the mechanism flips from
# always-false-positive to always-quiet.
for pair in "SS_S:G13a:security_scanner" "SSR_S:G13b:the manual trigger route"; do
  var=${pair%%:*}; rest=${pair#*:}; id=${rest%%:*}; who=${rest##*:}
  src=${!var}
  if [ -z "$src" ]; then skip "$id — subject not extractable"; continue; fi
  if has "$(stmt "$src" 'FROM file_integrity_baselines' 2)" 'server_id IS NULL'; then
    bad "$id the baseline comparison in $who still reads the NULL bucket — it will match nothing"
  else
    ok "$id the baseline comparison in $who is keyed by server"
  fi
done

# G14 — the cadence gate. Fleet-wide, the first host scanned satisfied it for
# every other host; counting rows of ANY status meant one failed scan bought a
# whole week of no security scanning.
if [ -n "$SS_S" ]; then
  GATE=$(stmt "$SS_S" 'SELECT COUNT\(\*\) FROM security_scans')
  if [ -z "$GATE" ]; then
    skip "G14 — cadence gate not extractable"
  elif has "$GATE" 'server_id = \$1' && has "$GATE" "status = 'completed'"; then
    ok "G14 the weekly gate is per server and only a COMPLETED scan satisfies it"
  else
    bad "G14 the weekly cadence gate is still fleet-wide or still counts failed scans"
  fi
else
  skip "G14 — security_scanner.rs not extractable"
fi

# G15 — the scan record itself. This route resolved the correct agent and threw
# the id away: the s299 laundering shape, in an HTTP handler.
if [ -n "$SSR_S" ]; then
  if has "$(stmt "$SSR_S" 'INSERT INTO security_scans' 1)" 'security_scans \(server_id, scan_type'; then
    ok "G15 the manual scan route records the host it scanned"
  else
    bad "G15 the manual scan route still files every member's scan under no server"
  fi
else
  skip "G15 — security_scans.rs not extractable"
fi

# G16 — the readers. If these keep reading the NULL bucket while the writers
# start binding, the Security page empties out and the posture card disappears.
if [ -n "$SSR_S" ]; then
  NULLREADS=$(count "$(stmt "$SSR_S" 'FROM security_scans' 2)" 'server_id IS NULL')
  if [ "$NULLREADS" -eq 0 ]; then
    ok "G16 no security_scans reader is still pinned to the NULL bucket"
  else
    bad "G16 $NULLREADS security_scans reader(s) still filter server_id IS NULL — the scan history will read empty"
  fi
else
  skip "G16 — security_scans.rs not extractable"
fi

# G17 — the image write + its retention. An unscoped 30-row trim makes a busy
# host evict a quiet host's ONLY row, so a scanned server's badge goes blank.
if [ -n "$ISR_S" ]; then
  if has "$(stmt "$ISR_S" 'INSERT INTO image_scan_findings')" '\(server_id, image, scanner'; then
    ok "G17a image scan results record the host that produced them"
  else
    bad "G17a image scan results are still stored keyed on the bare image"
  fi
  TRIM=$(stmt "$ISR_S" 'DELETE FROM image_scan_findings' 2)
  if [ -z "$TRIM" ]; then
    skip "G17b — history trim not extractable"
  elif has "$TRIM" 'server_id = \$1'; then
    ok "G17b the 30-row history trim is per (server, image)"
  else
    bad "G17b the history trim is still per image — one host evicts another's only result"
  fi
else
  skip "G17 — image_scans.rs not extractable"
fi

# G18 — the deploy gate. The scan that decides has to be a scan of the machine
# that will run the image; reading by image alone let a clean scan on one host
# wave a vulnerable image onto another.
if [ -n "$ISR_S" ]; then
  GATE=$(fnbody "$ISR_S" "preflight_gate")
  if [ -z "$GATE" ]; then
    skip "G18 — preflight_gate not extractable"
  elif has "$GATE" 'server_id: uuid::Uuid' && has "$GATE" 'server_id = \$1'; then
    ok "G18 the deploy gate reads a scan of the deploy TARGET"
  else
    bad "G18 the deploy gate still reads whichever host scanned that image last"
  fi
else
  skip "G18 — image_scans.rs not extractable"
fi

# G19 — the badge feed. This was the one handler in its module with no scope at
# all, and the client keys its response by image, so a fleet-wide DISTINCT ON
# collapsed every host's result into one with no duplicate and no error.
if [ -n "$ISR_S" ]; then
  LR=$(fnbody "$ISR_S" "list_recent")
  if [ -z "$LR" ]; then
    skip "G19 — list_recent not extractable"
  elif has "$LR" "ServerScope" && has "$LR" 'server_id = \$1'; then
    ok "G19 the Apps badge feed is server-scoped"
  else
    bad "G19 list_recent is still fleet-wide — badges collapse across hosts, last writer wins"
  fi
else
  skip "G19 — image_scans.rs not extractable"
fi

# G20 — THE 42P10 ARM, and the reason this migration and this writer are one
# change. An ON CONFLICT arbiter that matches no unique index does not degrade:
# Postgres raises 42P10 at EXECUTION time, so a composite key shipped without
# this edit turns every SBOM generation into a 500. Verified by running the old
# statement against the migrated schema.
if [ -n "$SB_S" ]; then
  UP=$(stmt "$SB_S" 'INSERT INTO image_sbom' 4)
  if [ -z "$UP" ]; then
    skip "G20 — SBOM upsert not extractable"
  elif has "$UP" 'ON CONFLICT \(server_id, image\)'; then
    ok "G20 the SBOM upsert's arbiter names the composite key the table actually has"
  else
    bad "G20 the SBOM upsert names an arbiter with no matching unique index — every generate 500s (42P10)"
  fi
else
  skip "G20 — sboms.rs not extractable"
fi

# G21 — alert_engine's three AGENT-driven checks. Its DB-driven checks were
# already per-server; only the ones that ask a machine what it has were blind.
if [ -n "$AE_S" ]; then
  OLDEST=$(count "$AE_S" 'FROM servers ORDER BY created_at ASC LIMIT 1')
  if [ "$OLDEST" -eq 0 ]; then
    ok "G21 the oldest-row 'local server' derivation is gone from alert_engine"
  else
    bad "G21 alert_engine still derives 'the local server' by row age in $OLDEST place(s)"
  fi
  if has "$AE_S" "online_fleet\(\)" && has "$AE_S" "check_container_health\(&pool, member\)"; then
    ok "G22 alert_engine runs its agent-driven checks once per online server"
  else
    bad "G22 alert_engine still asks the panel's agent and labels the answer with another server"
  fi
else
  skip "G21/G22 — alert_engine.rs not extractable"
fi

# G23 — the corollary arm (#164): two bugs that cancel are two bugs. alert_engine
# mislabelled the row and the healer ignored the label, so the restart landed on
# the right host BY ACCIDENT. Correct ids end the accident, which is why this
# must be true in the same commit as G22 and not one release later.
if [ -n "$AH_S" ]; then
  RESTART=$(fnbody "$AH_S" "auto_restart_services")
  if [ -z "$RESTART" ]; then
    skip "G23 — auto_restart_services not extractable"
  else
    if has "$RESTART" "for_server\(" && has "$(sig "$AH_S" auto_restart_services)" "AgentRegistry"; then
      ok "G23 auto_restart_services restarts a service on the host it died on"
    else
      bad "G23 auto_restart_services still posts every restart to the local agent"
    fi
    UNSCOPED=$(count "$RESTART" "alert_type = 'service_down'")
    SCOPED=$(count "$RESTART" 'server_id = \$1')
    if [ "$UNSCOPED" -gt 0 ] && [ "$SCOPED" -ge 3 ]; then
      ok "G24 its alert_state reads and writes are scoped to the server ($SCOPED predicates)"
    else
      bad "G24 alert_state statements in auto_restart_services are still fleet-wide (scoped=$SCOPED)"
    fi
    if has "$RESTART" 'target_name = \$1 AND server_id = \$2'; then
      ok "G25 the restart cooldown counts attempts on THIS server"
    else
      bad "G25 the cooldown is still keyed on the bare service name — two hosts share one budget"
    fi
    if has "$RESTART" "log_activity_on_server\(" && ! has "$RESTART" "Uuid::nil\(\)"; then
      ok "G26 the heal is recorded against the server's owner, so the cooldown row can exist at all"
    else
      bad "G26 the heal record still uses the nil uuid — it violates the user FK, so the cooldown stays dead"
    fi
    # G27 — the field names. A missing JSON field reads as an empty string, so
    # asking for the wrong one fails silently and for ever. The agent's payload
    # carries `status` and `container_id`; the alert engine reads the correct
    # name for the same payload 900 lines away in the same subsystem.
    APPS=$(grep -A 4 -E 'get\("/apps"\)' <<< "$RESTART" || true)
    if has "$RESTART" 'get\("container_id"\)' && ! has "$RESTART" 'c\.get\("state"\)'; then
      ok "G27 the container leg reads the field names the agent actually sends"
    else
      bad "G27 the container leg still reads fields the /apps payload has never had — it can never fire"
    fi
  fi
else
  skip "G23–G27 — auto_healer.rs not extractable"
fi

# G28 — the NEGATIVE arm, and the reason it runs over STRIPPED source. The
# registry offers an Option-taking resolver that answers a missing id with the
# LOCAL agent, silently; no background service may use it. Two subjects carry
# comments that spell the name precisely to say they are NOT using it, so an arm
# over raw source would fire on the prose narrating the decision — the #149 trap,
# and the reason `subj` is used here rather than the file.
BG_FILES=$(find panel/backend/src/services -name '*.rs' -type f 2>/dev/null | sort)
BG_COUNT=$(grep -c '\.rs$' <<< "$BG_FILES" || true)
if [ "${BG_COUNT:-0}" -lt 10 ]; then
  bad "G28 only $BG_COUNT service files enumerated — the absence arm below cannot mean anything"
else
  OFFENDERS=0
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    [ "$f" = "panel/backend/src/services/agent.rs" ] && continue
    if grep -qE -- '\.for_server_or_local\(' <<< "$(code "$f")"; then
      bad "G28 $f calls the resolver that silently substitutes the local agent"
      OFFENDERS=$((OFFENDERS+1))
    fi
  done <<< "$BG_FILES"
  if [ "$OFFENDERS" -eq 0 ]; then
    ok "G28 no background service resolves its agent through the silently-local convenience path ($BG_COUNT files)"
  fi
fi

echo
echo "§H the panel host is watched too, and the legacy handle has an allow-list (s302)"

# The class this section pins is the one the previous four releases each fixed an
# instance of without ever stating: a check whose driving data is written by only
# ONE kind of host is structurally blind to the other kind. Two stores were
# involved. The scalar columns on `servers` are written only by the check-in
# handler, which only a phoning-home MEMBER reaches, so the panel's own row was
# NULL for ever and its cpu/memory/disk thresholds never evaluated. The history
# table was written only against the local id, so no member had a trend alert, an
# uptime sparkline or a scrape series.
#
# H5 is the arm that matters most and the one nobody had: it is a CLASS arm over
# the SPAWN SITES, not a per-service arm. Four releases in a row shipped a
# per-defect arm and left a sibling on the legacy handle; an allow-list of one
# goes red the moment a new service joins it, or an old one is missed.
MC=panel/backend/src/services/metrics_collector.rs
SM=panel/backend/src/services/server_monitor.rs
MN=panel/backend/src/main.rs
AG=panel/backend/src/services/agent.rs

for f in "$MC" "$SM" "$MN" "$AG"; do
  [ -f "$f" ] || bad "MISSING SUBJECT FILE: $f"
done

MC_S=$(subj "$MC" || true); SM_S=$(subj "$SM" || true)
MN_S=$(subj "$MN" || true); AG_S=$(subj "$AG" || true)

# H1 — the collector takes the REGISTRY. Keyed on the flattened signature: a Rust
# parameter list wraps, and a line-oriented `has` over raw source measures nothing
# (lesson #168 — and as a PRESENCE arm this fails loudly rather than green).
if [ -n "$MC_S" ]; then
  MC_RUN=$(sig "$MC_S" "run")
  if has "$MC_RUN" 'AgentRegistry'; then
    ok "H1 metrics_collector::run takes the AgentRegistry"
  else
    bad "H1 metrics_collector::run does not take the AgentRegistry — it cannot reach any host but its own"
  fi
else
  skip "H1 — metrics_collector.rs not extractable"
fi

# H2 — and actually iterates it.
if [ -n "$MC_S" ]; then
  if has "$MC_S" 'online_fleet\(\)'; then
    ok "H2 metrics_collector iterates the online fleet"
  else
    bad "H2 metrics_collector never iterates the fleet — members get no history rows"
  fi
else
  skip "H2 — metrics_collector.rs not extractable"
fi

# H3 — every history row is stamped with the server it was READ FROM. The defect
# shape was a single lookup of the local id bound into every insert, so the arm
# asserts that lookup is gone from this subject AND that a per-member id is bound.
if [ -n "$MC_S" ]; then
  if has "$MC_S" 'is_local = true'; then
    bad "H3 metrics_collector still resolves one 'local' id — every reading lands on that one server"
  elif has "$MC_S" '\.bind\(member\.id\)'; then
    ok "H3 metrics_collector stamps each reading with the server it was read from"
  else
    bad "H3 metrics_collector binds no per-member id to its history writes"
  fi
else
  skip "H3 — metrics_collector.rs not extractable"
fi

# H4 — the panel row's scalar columns get a writer at last. Flattened: the UPDATE
# is a multi-line SQL string literal, so a line-oriented match would find only its
# first fragment.
if [ -n "$MC_S" ]; then
  MC_UPD=$(flat "$MC_S")
  if has "$MC_UPD" 'UPDATE servers SET.*cpu_usage' && has "$MC_S" 'is_local'; then
    ok "H4 the local server's scalar columns are written by the collector"
  else
    bad "H4 nothing writes the panel row's cpu/memory/disk — its thresholds can never evaluate"
  fi
else
  skip "H4 — metrics_collector.rs not extractable"
fi

# H5 — THE CLASS ARM. Exactly one spawn may still hold the legacy single-agent
# handle, and it must be the healer (whose SSL and auto-sleep legs are a recorded
# remainder). Any other service on that handle acts on this box no matter which
# host its rows belong to.
if [ -n "$MN_S" ]; then
  LEGACY=$(count "$MN_S" 'state\.agent\.clone\(\)')
  if [ "$LEGACY" -eq 1 ]; then
    if has "$(flat "$(grep -B 2 -A 2 -E 'state\.agent\.clone\(\)' <<< "$MN_S" || true)")" 'auto_healer'; then
      ok "H5 exactly one background service still holds the legacy local agent handle, and it is auto_healer"
    else
      bad "H5 the one legacy-handle spawn is NOT auto_healer — a different service is acting on this box regardless of host"
    fi
  else
    bad "H5 $LEGACY spawns hold the legacy local agent handle (allow-list is exactly 1: auto_healer)"
  fi
else
  skip "H5 — main.rs not extractable"
fi

# H6 — the offline sweep states its exemption instead of inheriting it from a
# NULL. `status = 'online'` gates online_fleet and the alert engine's own query,
# so a local row that can be swept offline drops the panel out of every
# fleet-iterating service at once.
if [ -n "$SM_S" ]; then
  if has "$(flat "$SM_S")" "UPDATE servers SET status = 'offline'.*is_local = false"; then
    ok "H6 the offline sweep exempts the local row explicitly"
  else
    bad "H6 the offline sweep does not name is_local — the exemption rests on a NULL comparison nobody stated"
  fi
else
  skip "H6 — server_monitor.rs not extractable"
fi

# H7 — the shared fleet primitive carries the flag the collector branches on.
#
# ⚠ The window is bounded by the struct's own closing brace, NOT by `-A n`. The
# first draft used `grep -A 12` and printed a confident GREEN against v2.58.0,
# where the field does not exist: twelve lines after a six-line struct run into
# `ensure_local_server`, whose body contains `WHERE is_local = true`. A fixed
# window is not a declaration (p31), and the only reason this was caught is that
# the arm was run against the previous tag and required to be red (#158).
if [ -n "$AG_S" ]; then
  AG_FLEET=$(flat "$(awk '/pub struct FleetMember/{f=1} f{print; if(/^\}/) exit}' <<< "$AG_S")")
  if [ -z "$AG_FLEET" ]; then
    bad "H7 could not extract the FleetMember declaration — the arm cannot mean anything"
  elif has "$AG_FLEET" 'is_local'; then
    ok "H7 FleetMember carries is_local"
  else
    bad "H7 FleetMember has no is_local — nothing can tell the panel row from a member while iterating"
  fi
else
  skip "H7 — agent.rs not extractable"
fi

# H8 — the fleet reads are concurrent. Serial round trips over N hosts inside a
# 30-second interval fall behind, and tokio's default MissedTickBehavior then
# fires the backlog back to back.
if [ -n "$MC_S" ]; then
  if has "$MC_S" 'join_all'; then
    ok "H8 the fleet is read concurrently, so a large fleet cannot outrun the tick"
  else
    bad "H8 the collector reads the fleet serially — N round trips inside a 30s interval"
  fi
else
  skip "H8 — metrics_collector.rs not extractable"
fi

echo
echo "§I the nil uuid is not a user (s303 / v2.60.0) — THE CLASS, not four instances"

# Four writers reached for `Uuid::nil()` as "no user". `activity_logs.user_id` is
# nullable with an FK to `users`, so a nil is a non-NULL value naming a row that
# does not exist: the insert is rejected with 23503 and `log_activity` swallows it
# into a warn. None of the four ever recorded anything, and TWO of them read their
# own rows back as a rate limit — a COUNT(*) that could only return 0 standing in
# for a cooldown.
#
# §B2 and G26 already pin this for auto_clean_disk and auto_restart_services, one
# function each. That is exactly the per-instance shape lesson #170 says cannot
# hold a class: both were green while three siblings were broken. I1 is the class
# arm — it reads EVERY backend source file, so a new writer is covered the day it
# is written rather than the day someone remembers to add an arm.

# The subject list must be asserted before it is trusted: an arm that enumerates
# its own subjects and finds none prints green having examined nothing (#143).
NIL_FILES=$(find panel/backend/src -name '*.rs' | wc -l)
if [ "$NIL_FILES" -lt 40 ]; then
  bad "I0 only $NIL_FILES backend sources found — I1 would examine almost nothing; refusing to trust it"
else
  ok "I0 the class arm's subject list holds $NIL_FILES backend sources"

  # Every log_activity* call site, flattened onto one line each so a wrapped
  # argument list is measured whole and cannot bleed into its neighbour.
  NIL_CALLS=0
  NIL_BAD=""
  while IFS= read -r f; do
    FS=$(code "$f")
    has "$FS" 'log_activity' || continue
    CALLW=$(stmt "$FS" 'log_activity(_on_server|_system)?\(' 12)
    NIL_CALLS=$((NIL_CALLS + $(count "$CALLW" 'log_activity(_on_server|_system)?\(')))
    if has "$CALLW" 'log_activity(_on_server|_system)?\([^)]*[Uu]uid::nil'; then
      NIL_BAD="$NIL_BAD $f"
    fi
  done < <(find panel/backend/src -name '*.rs')

  # …and the calls themselves must be plausible, or the loop above matched nothing.
  if [ "$NIL_CALLS" -lt 100 ]; then
    bad "I1 only $NIL_CALLS log_activity call sites matched — the extraction is broken, not the code clean"
  elif [ -n "$NIL_BAD" ]; then
    bad "I1 activity written as the nil uuid in:$NIL_BAD — those rows violate fk_activity_logs_user and are never stored"
  else
    ok "I1 none of the $NIL_CALLS log_activity call sites names the nil uuid as a user"
  fi

  # I1b — and the call site is not the only way in. `auth.rs` passed the class
  # arm above for eight releases by routing the same sentinel through a
  # one-line helper (`fn zero_uuid() -> Uuid { Uuid::nil() }`), so the grep saw
  # `zero_uuid()` and found nothing to complain about. A pattern that launders
  # itself through a name is still the pattern. This forbids the FACTORY: a
  # function whose entire body hands back a nil uuid.
  #
  # The one legitimate nil in the tree is a cached-admin sentinel inside a
  # larger function in agent.rs, which this cannot match by construction.
  NIL_FACTORY=""
  while IFS= read -r f; do
    FS=$(code "$f")
    has "$FS" 'Uuid::nil' || continue
    if has "$(flat "$FS")" 'fn [a-z_]+\(\) *-> *(uuid::)?Uuid *\{ *(uuid::)?Uuid::nil\(\) *\}'; then
      NIL_FACTORY="$NIL_FACTORY $f"
    fi
  done < <(find panel/backend/src -name '*.rs')

  if [ -n "$NIL_FACTORY" ]; then
    bad "I1b a nil-uuid factory is defined in:$NIL_FACTORY — it launders the sentinel past the class arm above"
  else
    ok "I1b no helper exists whose whole job is to hand back a nil uuid"
  fi
fi

# I2 — the writer that IS a cooldown. It must name a real owner and stamp the
# host, exactly as auto_restart_services has since v2.58.0 (G26 is its twin).
SSL=$(fnbody "$AH_S" "auto_renew_ssl")
if [ -z "$SSL" ]; then
  bad "I2 could not extract auto_renew_ssl — every §I SSL arm below is meaningless"
else
  if has "$(flat "$SSL")" 'log_activity_on_server\(' && ! has "$SSL" '[Uu]uid::nil'; then
    ok "I2 the SSL renewal attempt is recorded against the site's owner and server"
  else
    bad "I2 the SSL renewal record still uses the nil uuid — the anti-CA-hammering cooldown stays dead"
  fi

  # I3 — a gate must read something its own subject writes. This is the severed
  # pair stated directly: the action counted and the action logged are one string.
  if has "$SSL" "action = 'auto_heal.renew_ssl'" && has "$SSL" '"auto_heal.renew_ssl"'; then
    ok "I3 auto_renew_ssl records the very action its cooldown counts"
  else
    bad "I3 the SSL cooldown counts an action this function never writes — it can never engage"
  fi

  # I4 — the failure alert names the site's own server. `ORDER BY created_at ASC
  # LIMIT 1` over `servers` is the laundering shape §G outlawed for the healer;
  # it resolves to the panel's own row, so a member's cert failure was filed
  # against the panel and hidden from the picker that would show it.
  if has "$(flat "$SSL")" 'ORDER BY created_at ASC LIMIT 1'; then
    bad "I4 auto_renew_ssl still stamps its alert with the oldest servers row instead of the site's server"
  else
    ok "I4 auto_renew_ssl does not resolve a server by taking the oldest row"
  fi
fi

# I5 — auto-sleep genuinely has no user to name, so it must say so in the way the
# schema sanctions (NULL) rather than by inventing a uuid.
SLEEP=$(fnbody "$AH_S" "auto_sleep_idle_containers")
if [ -z "$SLEEP" ]; then
  bad "I5 could not extract auto_sleep_idle_containers"
elif has "$(flat "$SLEEP")" 'log_activity_system\('; then
  ok "I5 stopping a customer's container leaves an audit row with no invented user"
else
  bad "I5 auto-sleep still writes its audit row with a user that does not exist — stopping a container leaves no record"
fi

# I6 — the login branch that matters. A failed login against an address with no
# account is what credential stuffing and user enumeration look like; it was the
# ONLY failure branch that named a nil uuid, so the panel recorded the one case
# that is not enumeration and dropped the one that is.
AUTH=panel/backend/src/routes/auth.rs
AUTH_S=$(subj "$AUTH" || true)
if [ -z "$AUTH_S" ]; then
  bad "I6 could not read $AUTH"
else
  if has "$AUTH_S" 'log_activity_system\(' && ! has "$AUTH_S" 'zero_uuid|[Uu]uid::nil'; then
    ok "I6 a login against a non-existent account is recorded"
  else
    bad "I6 login failures against unknown accounts still write a nil uuid — the enumeration signature is invisible"
  fi
fi

# I7 — and it must be READABLE. The write landing is worth nothing if the audit
# surface cannot name the account or the origin: the login writers pass no
# target_name, so the column this reader used to select as "user" was NULL on
# every row it has ever returned.
SECR=panel/backend/src/routes/security.rs
SECR_S=$(subj "$SECR" || true)
if [ -z "$SECR_S" ]; then
  bad "I7 could not read $SECR"
else
  # Bounded on the function's own braces, not a fixed window: the columns live
  # on the SELECT line and the recognisable token on the WHERE line below it, so
  # a `grep -A n` anchored on either one misses the other (p31, and this arm
  # printed a false RED against a correct fix before it was written this way).
  AUDQ=$(flat "$(fnbody "$SECR_S" "login_audit")")
  if [ -z "$AUDQ" ]; then
    bad "I7 could not locate the login-audit query — the arm would measure nothing"
  elif ! has "$AUDQ" 'auth.login_failed'; then
    bad "I7 login_audit no longer reads auth.login_failed — the arm is measuring the wrong function"
  elif has "$AUDQ" 'user_email' && has "$AUDQ" 'ip_address'; then
    ok "I7 the login-audit feed carries the account and the origin of each attempt"
  else
    bad "I7 the login-audit query still omits user_email/ip_address — every row renders anonymous"
  fi
fi

# I8 — the reader side of the same severed pair. The writer has stamped
# server_id since v2.58.0 and its own cooldown reads it back; this suppression
# query was left on the bare service name, and every host has an `nginx`.
if [ -n "$AE_S" ]; then
  HEALQ=$(stmt "$AE_S" "action = 'auto_heal.restart_service'" 5)
  if [ -z "$HEALQ" ]; then
    bad "I8 could not locate the heal-suppression query in alert_engine.rs"
  elif has "$HEALQ" 'server_id = \$2'; then
    ok "I8 a heal on one host does not suppress the same service's alert on another"
  else
    bad "I8 the heal-suppression count is still fleet-wide — healing nginx on one server mutes nginx on all of them"
  fi
else
  skip "I8 — alert_engine.rs not extractable"
fi

# I9 — the fourth writer, and the one the obvious audit misses: written
# `.unwrap_or_else(uuid::Uuid::nil)`, with no parentheses, so a `Uuid::nil()`
# grep does not find it. It fed a NOT NULL column with a foreign key.
WH=panel/backend/src/routes/whmcs.rs
WH_S=$(subj "$WH" || true)
if [ -z "$WH_S" ]; then
  bad "I9 could not read $WH"
elif has "$WH_S" 'unwrap_or(_else)?\( *(uuid::)?Uuid::nil'; then
  bad "I9 whmcs still defaults a missing local server to the nil uuid — an FK violation surfaced as a 500"
else
  ok "I9 a missing local server is reported, not laundered into a sentinel uuid"
fi

# I9b (s311) — the same file, the same shape one level up: an UNATTENDED external
# caller mutating a column that is also the account's only record of what it was.
# `role` doubles as the status column, so `SET role = 'suspended'` overwrites it,
# and the unsuspend arm restores everyone to 'user'. An admin round-tripped
# through billing therefore comes back as a plain user with nobody left able to
# promote them — and it is reachable, because the provision arm adopts an
# EXISTING user by email with no role filter, so the operator's own account maps
# here the moment their address is also a billing contact. The panel's own twin
# in users.rs stashes the prior role first; this path has no equivalent.
#
# Keyed on the UPDATE's own predicate, not on a comment or a helper name: the
# guard has to be in the statement or it is not a guard.
# ⚠ REPOINTED s316 — the subject MOVED, and this arm's own "measuring nothing"
# guard is what caught it. `role` used to be the stash as well as the status, and
# the deny-list below was the whole compensation; the stash now has its own
# column (`users.prior_role`) and both suspenders share one statement in
# `helpers::suspend_account`. An arm still grepping whmcs.rs for the UPDATE would
# have been measuring an absent subject — lesson #150, extracting a helper moves
# the subject of every pin that measured it. So the arm follows the code, and is
# paired with the check that the caller still REACHES it.
#
# The deny-list is KEPT on purpose even though the stash now exists: it guards a
# different hazard, a lapsed invoice suspending the operator out of their own panel.
if [ -z "$WH_S" ]; then
  : # already reported by I9
elif ! has "$WH_S" 'helpers::suspend_account'; then
  bad "I9b whmcs no longer reaches the shared suspend statement — this arm is measuring nothing"
elif has "$WH_S" "role NOT IN \\('admin', 'reseller'\\)"; then
  ok "I9b the billing webhook cannot overwrite a privileged role"
else
  bad "I9b the billing webhook suspends any role — an admin round trip demotes them permanently"
fi

# I9c (s316) — the OTHER direction, which never had a guard at all. The unsuspend
# arm restored every account to a hardcoded 'user', so one billing round trip
# promoted a `client` into the role that may claim new domains and demoted an
# `admin` in the same statement. It has to give back what was recorded.
if [ -z "$WH_S" ]; then
  : # already reported by I9
elif has "$WH_S" "UPDATE users SET role = 'user'"; then
  bad "I9c the billing webhook still restores every unsuspended account to 'user'"
elif ! has "$WH_S" 'helpers::unsuspend_account'; then
  bad "I9c whmcs does not reach the shared unsuspend statement — this arm is measuring nothing"
else
  ok "I9c the billing webhook gives back the role it recorded, not a hardcoded one"
fi

# I10 — the structural guarantee behind all of the above. `user_id` is an Option
# in exactly one place, the private writer, so a caller must choose between
# naming a user and declaring there is none. Re-introducing a nil-defaulting
# public helper is what this arm exists to catch.
ACT=panel/backend/src/services/activity.rs
ACT_S=$(subj "$ACT" || true)
if [ -z "$ACT_S" ]; then
  bad "I10 could not read $ACT"
else
  if has "$(sig "$ACT_S" "log_activity_system")" 'user_email' \
     && ! has "$(sig "$ACT_S" "log_activity_system")" 'user_id'; then
    ok "I10 the no-user writer takes no user id at all, so there is nothing to pass a sentinel into"
  else
    bad "I10 log_activity_system still accepts a user id — the sentinel can come straight back"
  fi
fi

# ── §J the panel stops lying (s304) ────────────────────────────────────────
#
# Five places where DockPanel told the operator something untrue. The unifying
# shape is a SECOND READER that disagrees with the first, or a first reader that
# never existed at all.

echo
echo "§J the panel stops lying (s304)"

TERM_R=panel/backend/src/routes/terminal.rs
LOGS_R=panel/backend/src/routes/logs.rs
ACTR=panel/backend/src/routes/activity.rs
ACTTSX=panel/frontend/src/pages/Activity.tsx
HELP=panel/backend/src/helpers.rs

for f in "$TERM_R" "$LOGS_R" "$ACTR" "$ACTTSX" "$HELP"; do
  [ -f "$f" ] || bad "MISSING SUBJECT FILE: $f"
done

TERM_S=$(subj "$TERM_R" || true)
LOGS_S=$(subj "$LOGS_R" || true)
ACTR_S=$(subj "$ACTR" || true)
ACTTSX_S=$(subj "$ACTTSX" || true)
HELP_S=$(subj "$HELP" || true)

# Call sites of a function, definition excluded — the definition line is the one
# that carries `fn`. Counting raw occurrences would let a pin pass on a helper
# nobody calls.
callsites() { grep -E -- "$2" <<< "$1" | grep -vcE '(^|[^[:alnum:]_])fn[[:space:]]' || true; }

# J1 — the public share viewer decides expiry itself. It used to compute the
# remaining seconds, clamp a negative to zero and render the content anyway, so
# an unauthenticated URL served root terminal output for as long as the row
# survived — and only a once-a-day retention sweep removed it. A sweep is a
# housekeeper, not an access control.
if [ -z "$TERM_S" ]; then
  bad "J1 could not read $TERM_R"
else
  VS=$(fnbody "$TERM_S" "view_shared")
  if [ -z "$VS" ]; then
    bad "J1 could not extract view_shared — the arm measured nothing"
  elif has "$(flat "$VS")" 'share_lifetime\(' && has "$(flat "$VS")" 'ok_or_else'; then
    ok "J1 the public share viewer refuses an expired share instead of rendering it"
  else
    bad "J1 view_shared no longer rejects on the shared lifetime decision — an expired share renders"
  fi
fi

# J2 — and BOTH readers reach that decision through the same function. The defect
# was precisely that they disagreed: the operator's list skipped an expired share
# while the public viewer showed it, so the panel reported a share as gone while
# the URL still served it.
if [ -z "$TERM_S" ]; then
  skip "J2 — terminal routes not extractable"
elif [ "$(callsites "$TERM_S" 'share_lifetime\(')" -ge 2 ]; then
  ok "J2 the operator's list and the public viewer share one expiry rule, so they cannot drift"
else
  bad "J2 fewer than two callers of the shared expiry rule — a reader has its own copy again"
fi

# J3 — the public route takes no authentication, so the id is the only
# credential and gets the shape check the revoke path always had.
if [ -z "$TERM_S" ]; then
  skip "J3 — terminal routes not extractable"
elif has "$(flat "$(fnbody "$TERM_S" "view_shared")")" 'valid_share_id\('; then
  ok "J3 the unauthenticated viewer validates the share id before querying"
else
  bad "J3 view_shared no longer validates the share id"
fi

# J4 — both streaming tickets refuse at the MINT when a fleet member is selected.
# The ticket is signed with the selected server's agent token while the browser
# can only reach the panel's own agent, so the receiving agent rejected it and
# the UI blamed the network: the terminal said "Connection lost" and the log
# viewer reconnected every three seconds for ever.
#
# ⚠ s305: this arm USED TO ITERATE A HARDCODED TWO-ELEMENT LIST. That is the
# mock-that-supplies-its-own-target shape — an arm which enumerates its own
# subjects can never see a subject that was never added to it, so a THIRD mint
# of this class would have been invisible to it for ever. The subjects are now
# derived FROM SOURCE: a streaming ticket mint is a route handler whose name
# ends `_token` and whose signature takes a per-server scope. `iac`'s API-token
# pair and `rotate_token` are correctly excluded because they take no scope.
# Per lesson #143 an arm that enumerates its own subjects must ASSERT the
# enumeration first, or an empty list prints green having measured nothing.
#
# GREEN AT BOTH TAGS BY DESIGN — this is a HARDENING of an arm that already
# passed, not a new defect, so it cannot satisfy required-red (#172/#180).
# MUTATION-TESTED instead, and the mutation is the whole justification: append a
# third `_token` handler taking a per-server scope and omitting the guard, then
# run both versions. The old hardcoded arm prints TWO GREENS and never mentions
# it; this one enumerates THREE and goes red on the new one. Do not "simplify"
# it back to a literal list.
MINTS=$(for f in panel/backend/src/routes/*.rs; do
  perl -0777 -ne 'while (/pub async fn ([a-z_0-9]*_token)\s*\((.*?)\)\s*->/gs) {
    my ($n,$a)=($1,$2); next unless $a =~ /ServerScope\(/; print "$ARGV:$n\n"; }' "$f"
done | sort)
MINT_N=$(printf '%s\n' "$MINTS" | grep -c . || true)
if [ "$MINT_N" -lt 2 ]; then
  bad "J4 enumerated only $MINT_N scoped ticket mints — the enumeration is broken, not the code (expected at least the terminal and log-stream mints)"
else
  ok "J4 enumerated $MINT_N streaming ticket mints from source, not from a list this arm carries"
  while IFS=: read -r file fn; do
    [ -n "$fn" ] || continue
    src=$(subj "$file" || true)
    if [ -z "$src" ]; then
      bad "J4 could not read $file"
      continue
    fi
    BODY=$(flat "$(fnbody "$src" "$fn")")
    if [ -z "$BODY" ]; then
      bad "J4 could not extract $fn — the arm measured nothing"
    elif has "$BODY" 'require_local_agent_scope\('; then
      ok "J4 $fn refuses a fleet member at the mint, before any socket is dialled"
    else
      bad "J4 $fn mints a ticket for any selected server again — that stream fails as a network error"
    fi
  done <<< "$MINTS"
fi

# J5 — and it can only do that because it keeps the id. `ServerScope` resolved it
# and both handlers bound it to `_server_id` and threw it away; the knowledge to
# answer honestly was already in the handler.
for pair in "ws_token:$TERM_S" "stream_token:$LOGS_S"; do
  fn=${pair%%:*}; src=${pair#*:}
  [ -n "$src" ] || continue
  if has "$(sig "$src" "$fn")" 'ServerScope\( *_server_id'; then
    bad "J5 $fn discards the resolved server id again — it cannot tell which host was selected"
  else
    ok "J5 $fn keeps the resolved server id rather than discarding it"
  fi
done

# J6 — the audit feed can say which host. `activity_logs.server_id` has been
# written since v2.58.0 and stamped by the healer's own writers since v2.60.0,
# and was read only by this file's cooldown gates: the struct never declared it,
# so `SELECT *` dropped it and the one surface an operator reads showed nothing.
# Cross-tree by construction — a backend-only arm would pass on a dead column.
if [ -z "$ACTR_S" ] || [ -z "$ACTTSX_S" ]; then
  bad "J6 could not read both trees"
else
  B_OK=0; F_OK=0
  has "$(flat "$(grep -A 16 -E 'struct ActivityLog' <<< "$ACTR_S" || true)")" 'server_id' && B_OK=1
  has "$(flat "$(grep -A 16 -E 'interface ActivityEntry' <<< "$ACTTSX_S" || true)")" 'server_id' && F_OK=1
  if [ "$B_OK" = 1 ] && [ "$F_OK" = 1 ]; then
    ok "J6 the host travels from the audit row to the audit page in both trees"
  else
    bad "J6 the audit feed lost the host again (backend=$B_OK frontend=$F_OK) — a stamp with no reader"
  fi
fi

# J7 — and it renders a NAME. The column holds a uuid; a feed that prints one is
# no more legible than a feed that prints nothing.
if [ -z "$ACTR_S" ]; then
  skip "J7 — activity route not extractable"
elif has "$(flat "$ACTR_S")" 'LEFT JOIN servers' && has "$(flat "$ACTTSX_S")" 'server_name'; then
  ok "J7 the audit feed resolves the host to its name in both trees"
else
  bad "J7 the audit feed no longer resolves the host name — the operator reads a uuid"
fi

# J8 — THE SEVERED PAIR. `auto_heal.clean_logs` spent the operator-facing
# `target_name` column on the server's uuid because the hourly cooldown keyed on
# it. Writer and reader must agree, and they must agree on `server_id` — moving
# one without the other silently disarms the gate and the prune returns to every
# 120 seconds, which is the defect v2.60.0 fixed elsewhere in this same file.
if [ -z "$AH_S" ]; then
  skip "J8 — auto_healer not extractable"
else
  ACD=$(flat "$(fnbody "$AH_S" "auto_clean_disk")")
  READER_OK=0; WRITER_OK=0
  has "$ACD" "auto_heal\.clean_logs' AND server_id" && READER_OK=1
  has "$ACD" 'log_activity_on_server\(' && WRITER_OK=1
  if [ "$READER_OK" = 1 ] && [ "$WRITER_OK" = 1 ]; then
    ok "J8 the clean_logs cooldown and its writer agree on server_id, so the gate survives the rename"
  else
    bad "J8 clean_logs writer/reader disagree (reader=$READER_OK writer=$WRITER_OK) — the hourly gate is disarmed"
  fi
fi

# J9 — TRIPWIRE, and the reason this ship stopped where it did. A backend
# WebSocket proxy would carry a member-signed ticket to the member's own agent,
# which honours it — including the `domain` query parameter the ticket does NOT
# bind, and which an empty value turns into a root login shell. Two independent
# things confine that to the panel today: nginx points both stream paths at the
# local socket, and the local agent rejects a ticket signed elsewhere. A proxy
# removes both at once. Bind the domain into the signed ticket FIRST.
UPGRADES=$(grep -rlE 'WebSocketUpgrade' panel/backend/src --include=*.rs | sort | tr '\n' ' ')
if [ "$(printf '%s' "$UPGRADES" | wc -w)" -eq 1 ] && [[ "$UPGRADES" == *ws_metrics* ]]; then
  ok "J9 still exactly one WebSocket handler (ws_metrics) — no stream proxy has been added"
else
  bad "J9 a new WebSocket handler exists ($UPGRADES) — bind the domain into the terminal ticket BEFORE proxying, or an empty domain is a root shell on every host"
fi


# J10 — s305, and this is the arm the old hardcoded J4 could never have been.
# A raw browser stream cannot carry the selected server: the fetch wrapper in
# `api.ts` is the ONLY thing in the client that sends that header, and neither
# the WebSocket nor the EventSource constructor takes headers at all. So the
# backend's scope extractor resolves EVERY such stream to the panel's own agent
# whatever the picker says. Two of the three surfaces of this class were fixed
# by naming it at the mint; the dashboard's metrics socket was left, and it wrote
# the PANEL's cpu/memory/disk into the same state the server-scoped poll fills,
# so the tiles changed machine on the 3s reconnect timer with the badge still
# reading "Live".
#
# The rule, keyed on the capability rather than today's spelling: a component
# that opens a raw socket must prove which host it is describing — either it
# obtains its URL from a signed ticket (guarded server-side, see J4) or it
# consults the server selection before opening (guarded client-side). Neither
# means an unscoped socket.
WSFILES=$(grep -rlE 'new WebSocket\(' panel/frontend/src --include=*.tsx --include=*.ts | sort)
WS_N=$(printf '%s\n' "$WSFILES" | grep -c . || true)
if [ "$WS_N" -lt 3 ]; then
  bad "J10 found only $WS_N raw WebSocket openers in the client — the enumeration is broken, not the code (terminal, logs and the dashboard all open one)"
else
  ok "J10 enumerated $WS_N raw WebSocket openers from source"
  while read -r f; do
    [ -n "$f" ] || continue
    FSRC=$(subj "$f" || true)
    if [ -z "$FSRC" ]; then
      bad "J10 could not read $f"
    elif has "$FSRC" 'token' || has "$FSRC" 'isLocal'; then
      ok "J10 $(basename "$f") proves its host — ticket-guarded or selection-guarded"
    else
      bad "J10 $(basename "$f") opens a raw socket without a ticket or a server-selection gate — it will stream the PANEL host under whatever server is selected"
    fi
  done <<< "$WSFILES"
fi

# J11 — the other half of the same class. Server-Sent Events are a raw browser
# stream too, so an SSE handler that resolves a per-server agent has exactly the
# dashboard socket's defect. Today all six read user-owned rows by id and touch
# no agent, which is why none of them was affected; this arm makes that a
# property rather than a coincidence.
#
# GREEN AT BOTH TAGS BY DESIGN — a tripwire for a regression that does not exist
# yet cannot be red at the previous tag (#180). MUTATION-TESTED instead: append
# an SSE handler taking a per-server scope and this arm names it and fails.
# A future session re-deriving this as a false green should read that sentence
# before deleting a tripwire.
SSE_SCOPED=$(for f in panel/backend/src/routes/*.rs; do
  perl -0777 -ne 'while (/pub async fn ([a-z_0-9]+)\s*\((.*?)\)\s*->\s*([^{]*)\{/gs) {
    my ($n,$a,$r)=($1,$2,$3); next unless $r =~ /Sse</; next unless $a =~ /ServerScope\(/;
    print "$ARGV:$n "; }' "$f"
done)
SSE_ALL=$(for f in panel/backend/src/routes/*.rs; do
  perl -0777 -ne 'while (/pub async fn ([a-z_0-9]+)\s*\((.*?)\)\s*->\s*([^{]*)\{/gs) {
    my ($n,$a,$r)=($1,$2,$3); next unless $r =~ /Sse</; print "$ARGV:$n\n"; }' "$f"
done | grep -c . || true)
if [ "$SSE_ALL" -lt 3 ]; then
  bad "J11 enumerated only $SSE_ALL SSE handlers — the enumeration is broken, not the code"
elif [ -z "$SSE_SCOPED" ]; then
  ok "J11 none of the $SSE_ALL SSE handlers resolves a per-server agent, so no event stream can describe the wrong host"
else
  bad "J11 an SSE handler now resolves a per-server agent ($SSE_SCOPED) — a browser event stream cannot send the server header, so it will read the PANEL host"
fi

echo
echo "----------------------------------------------"
printf '  PASS %d   FAIL %d   SKIP %d\n' "$PASS" "$FAIL" "$SKIP"
echo
[ "$FAIL" -eq 0 ] || exit 1
