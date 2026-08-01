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
echo "unattended-host-scope-pin-e2e — s298 / v2.56.0"
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
if has "$CLEAN" "log_activity\("; then
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
echo "----------------------------------------------"
printf '  PASS %d   FAIL %d   SKIP %d\n' "$PASS" "$FAIL" "$SKIP"
echo
[ "$FAIL" -eq 0 ] || exit 1
