#!/usr/bin/env bash
# compose-stack-pin-e2e.sh — s296 / v2.54.0
#
# DockPanel's "Compose" support does not run `docker compose`. It creates each
# service directly through the Docker API, which is what lets the panel apply
# its own sandboxing — and which means every convenience `docker compose` gives
# for free has to be reproduced deliberately. Until v2.54.0 almost none of it
# was, and the result was a feature that could not work: no user-defined
# network, so `postgresql://app:pw@database:5432` never resolved, so every
# multi-service package the panel ships died on boot while reporting success.
#
#   §A  A STACK IS A NETWORK. Every service is attached to one, and answers to
#       its compose service key on it. This is the arm that fails if the whole
#       defect is reintroduced.
#   §B  A STACK IS A NAMESPACE. Container, volume and network names are scoped
#       by stack, so the `db` service of two packages can coexist.
#   §C  THE PARSER DOES NOT SILENTLY DISCARD WHAT THE AUTHOR WROTE. `command:`,
#       `depends_on:`, `labels:` and long-form `ports:` were all parsed away by
#       a struct that did not declare them — which turned a queue worker into a
#       second copy of the web server. `dockpanel.*` labels are still ours.
#   §D  "RUNNING" MEANS RUNNING. `create` + `start` returning Ok is not a
#       status, and a 200 carrying N failures is not a successful deploy.
#   §E  A STACK'S DOMAIN GOES THROUGH THE ONE DOOR (v2.52.0) AND ITS TEARDOWN
#       THROUGH THE OWNERSHIP GUARD (v2.53.0). A new domain-claiming path that
#       neither registers with nor honours those is how a domain gets two owners.
#   §F  A FAILED EDIT DOES NOT REPLACE THE ONLY COPY. `docker_stacks` has no
#       history table; the stored YAML is it.
#   §G  A CRASHED CONTAINER IS VISIBLE. It is the only one anybody opens the
#       logs page for.
#   §H  THE UPDATE BANNER IS A COMPARISON, NOT A PRESENCE TEST (GitHub #98).
#
# Pure source analysis: no box, no network, no build.
#
# Arms are written against the CAPABILITY a regression must use, not today's
# spelling (lesson #122), and were attacked after they went green (lesson #132).
#
# NO PIPES INTO `grep -q`. Under `set -o pipefail` grep -q closes the pipe on its
# first match, the upstream dies of SIGPIPE (141), and pipefail reports the whole
# pipeline failed — so an arm goes red on correct code, non-deterministically.
# Every arm here feeds grep a here-string.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0; FAIL=0; SKIP=0
ok()   { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }
skip() { SKIP=$((SKIP+1)); printf '  \033[33m-\033[0m SKIP %s\n' "$1"; }

COMPOSE=panel/agent/src/services/compose.rs
APPS_ROUTE=panel/agent/src/routes/docker_apps.rs
LOGS_ROUTE=panel/agent/src/routes/logs.rs
STACKS_BE=panel/backend/src/routes/stacks.rs
CLAIM=panel/backend/src/services/domain_claim.rs
TELE_COLL=panel/backend/src/services/telemetry_collector.rs
TELE_ROUTE=panel/backend/src/routes/telemetry.rs
UPDATE_ROUTE=panel/backend/src/routes/update.rs
APPS_TSX=panel/frontend/src/pages/Apps.tsx
GITDEP=panel/backend/src/routes/git_deploys.rs
BE_ROUTER=panel/backend/src/routes/mod.rs

for f in "$COMPOSE" "$APPS_ROUTE" "$LOGS_ROUTE" "$STACKS_BE" "$CLAIM" \
         "$TELE_COLL" "$TELE_ROUTE" "$UPDATE_ROUTE" "$APPS_TSX"; do
  [ -f "$f" ] || bad "MISSING SUBJECT FILE: $f"
done

# Comments out, CODE INTACT.
#
# The obvious `s{/\*.*?\*/}{}gs` shipped in four suites before s294 and was
# DELETING CODE: `/*` occurs inside string literals, so it opened a "block
# comment" that ran to the next `*/`. A truncated subject makes an ABSENCE arm
# pass on code the stripper merely removed, which is the worst way for a pin to
# be wrong. So a block comment is recognised only where one is actually written.
code() {
  perl -0777 -pe '
    s{\{/\*.*?\*/\}}{}gs;
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
  ' "$1"
}

# The stripper is code, and code can be wrong. Measure it before trusting
# anything downstream: every `fn` declaration must survive the strip.
stripper_self_check() {
  local bad_files=0 f raw_decls stripped_decls
  for f in "$@"; do
    [ -f "$f" ] || continue
    raw_decls=$(grep -cE '^[[:space:]]*(pub )?(pub\(crate\) )?(async )?fn ' "$f" || true)
    stripped_decls=$(grep -cE '^[[:space:]]*(pub )?(pub\(crate\) )?(async )?fn ' <<< "$(code "$f")" || true)
    if [ "$raw_decls" != "$stripped_decls" ]; then
      bad "STRIPPER ATE CODE in $f: $raw_decls fn declarations before, $stripped_decls after"
      bad_files=$((bad_files+1))
    fi
  done
  [ "$bad_files" -eq 0 ] && ok "comment stripper preserves every fn declaration in all $# subjects"
}

# An arm whose subject could not be extracted must SKIP, not print a confident
# green next to a red about the same subject.
subj() { local t; t=$(code "$1"); [ -n "$t" ] || return 1; printf '%s' "$t"; }

has()   { grep -qE -- "$2" <<< "$1"; }
count() { grep -cE -- "$2" <<< "$1" || true; }

# Only DEAD-CODE markers belong here (lesson #133).
live() {
  ! grep -qE -- '(if false|&& false|\|\| true|let _unused)' <<< "$1"
}

# The body of one top-level fn, bounded by the NEXT top-level fn.
# A fixed `-A <n>` window is NOT a function (lesson #131).
fnbody() {
  awk -v name="$2" '
    /^[[:space:]]*(pub )?(pub\(crate\) )?(async )?fn / {
      if ($0 ~ "(pub |pub\\(crate\\) )?(async )?fn " name "\\(") { inside=1; next }
      inside=0
    }
    inside { print }
  ' <<< "$1"
}

# Assert a window both matches and is live, in one place so no arm forgets.
derives() {
  local w="$1" pat="$2"
  [ -n "$w" ] && grep -qE -- "$pat" <<< "$w" && live "$w"
}

# First 1-indexed line of $2 within $1, or 0. Used where the DEFECT IS THE
# ORDER — a pin that only checks both statements exist passes on the bug.
lineof() {
  local n
  n=$(grep -nE -- "$2" <<< "$1" | head -1 | cut -d: -f1)
  printf '%s' "${n:-0}"
}

C=$(subj "$COMPOSE")      || C=""
AR=$(subj "$APPS_ROUTE")  || AR=""
LR=$(subj "$LOGS_ROUTE")  || LR=""
SB=$(subj "$STACKS_BE")   || SB=""
CL=$(subj "$CLAIM")       || CL=""
TC=$(subj "$TELE_COLL")   || TC=""
TR=$(subj "$TELE_ROUTE")  || TR=""
UR=$(subj "$UPDATE_ROUTE")|| UR=""

echo
echo "compose-stack-pin-e2e — a stack is a network, a namespace, and an honest status"
echo

# ── stripper self-check ────────────────────────────────────────────────
echo "§0 the harness measures its own preprocessing"
stripper_self_check "$COMPOSE" "$APPS_ROUTE" "$LOGS_ROUTE" "$STACKS_BE" "$CLAIM" \
                    "$TELE_COLL" "$TELE_ROUTE" "$UPDATE_ROUTE"

# ── §A a stack is a network ────────────────────────────────────────────
echo
echo "§A a stack is a network, and a service answers to its compose key on it"
if [ -z "$C" ]; then
  skip "§A — $COMPOSE could not be read"
else
  DS=$(fnbody "$C" "deploy_service")
  if derives "$DS" 'networking_config:[[:space:]]*Some'; then
    ok "A1 the container is created WITH a network attachment"
  else
    bad "A1 no networking_config — every service lands on the default bridge, where sibling names do not resolve"
  fi
  if derives "$DS" 'aliases:[[:space:]]*Some\(svc\.aliases'; then
    ok "A2 the attachment registers aliases (the names siblings were configured with)"
  else
    bad "A2 attached with no alias — the network exists but 'database' still does not resolve"
  fi
  DC=$(fnbody "$C" "deploy_compose")
  if derives "$DC" 'ensure_stack_network'; then
    ok "A3 the network is created before any service is deployed"
  else
    bad "A3 nothing creates the network — the attachment above fails on every service"
  fi
  PC=$(fnbody "$C" "parse_compose")
  # Keyed on the property: the alias list must be seeded from the services-map
  # KEY, which is what a compose file's own DSNs name.
  if derives "$PC" 'aliases[[:space:]]*=[[:space:]]*vec!\[name'; then
    ok "A4 the alias is the compose service key, not the container name"
  else
    bad "A4 the service key is not registered as an alias"
  fi
  if derives "$C" 'fn remove_stack_network'; then
    ok "A5 the network is torn down with the stack"
  else
    bad "A5 nothing removes the network — one leaks per stack, forever"
  fi
  RSN=$(fnbody "$C" "remove_stack_network")
  if derives "$RSN" '\.get\("dockpanel\.managed"\)'; then
    ok "A6 the teardown refuses a network it cannot prove is ours"
  else
    bad "A6 the teardown removes a network by NAME alone"
  fi
fi

# ── §B a stack is a namespace ──────────────────────────────────────────
echo
echo "§B container, volume and network names are scoped by stack"
if [ -z "$C" ]; then
  skip "§B — $COMPOSE could not be read"
else
  PC=$(fnbody "$C" "parse_compose")
  if derives "$PC" 'dockpanel-compose-\{scope\}'; then
    ok "B1 the container name carries the stack scope"
  else
    bad "B1 unscoped container name — two packages both want dockpanel-compose-db"
  fi
  SNN=$(fnbody "$C" "stack_network_name")
  if derives "$SNN" 'stack_scope'; then
    ok "B2 the network name is derived from the stack id"
  else
    bad "B2 one shared network across every stack"
  fi
  DS=$(fnbody "$C" "deploy_service")
  if derives "$DS" 'scoped_volume'; then
    ok "B3 named volumes are resolved through the stack scope"
  else
    bad "B3 bare named volumes — a second stack from the same package mounts the first one's database"
  fi
  # The safety half of B3: scoping a volume must never orphan data that is
  # already in one. Renaming a volume does not move what is inside it.
  SV=$(fnbody "$C" "scoped_volume")
  if derives "$SV" 'inspect_volume'; then
    ok "B4 an existing volume is reused rather than renamed out from under its data"
  else
    bad "B4 the scoping renames unconditionally — an upgraded install loses sight of its data"
  fi
  # An author-chosen container_name must not become the host-level Docker name,
  # or the namespace is only as good as the compose files people paste.
  if derives "$PC" 'aliases\.push\(explicit'; then
    ok "B5 an author's container_name becomes an alias, not the Docker name"
  else
    bad "B5 container_name is taken as the Docker name — the collision returns through the YAML"
  fi
fi

# ── §C the parser keeps what the author wrote ──────────────────────────
echo
echo "§C nothing the author wrote is discarded in silence"
if [ -z "$C" ]; then
  skip "§C — $COMPOSE could not be read"
else
  # serde drops unknown keys silently, so a field that is not DECLARED is a
  # field that is thrown away without a word. Each of these was.
  for pair in "command:command:" "depends_on:depends_on:" "labels:labels:"; do
    field=${pair%%:*}
    if derives "$C" "^[[:space:]]*${field}:[[:space:]]*Option<"; then
      ok "C-${field} '${field}:' is declared, so serde keeps it"
    else
      bad "C-${field} '${field}:' is not declared — serde discards it without a word"
    fi
  done
  DS=$(fnbody "$C" "deploy_service")
  if derives "$DS" 'cmd:[[:space:]]*svc\.command'; then
    ok "C4 a parsed command actually reaches the container"
  else
    bad "C4 command is parsed and then not applied — the worker runs the image's default CMD"
  fi
  PP=$(fnbody "$C" "parse_ports")
  if derives "$PP" 'Value::Mapping'; then
    ok "C5 long-form ports (target/published) are published — that is what compose tooling emits"
  else
    bad "C5 long-form ports fall through to the discard arm: no published port, no warning"
  fi
  PC=$(fnbody "$C" "parse_compose")
  if derives "$PC" 'starts_with\("dockpanel\.'; then
    ok "C6 author labels are kept but the dockpanel namespace is stripped"
  else
    bad "C6 a compose file can set dockpanel.managed or dockpanel.stack_id on its own containers"
  fi
  # Ordering is not cosmetic: HashMap iteration is randomised per process, so
  # the same YAML used to start its services in a different order every time.
  if derives "$C" 'services:[[:space:]]*Option<BTreeMap'; then
    ok "C7 services are iterated in a deterministic order"
  else
    bad "C7 randomised iteration order over the services map"
  fi
  if derives "$PC" 'order_by_dependencies'; then
    ok "C8 depends_on orders the deploy"
  else
    bad "C8 depends_on is parsed and ignored — the app can be created before its database"
  fi
fi

# ── §D "running" means running ─────────────────────────────────────────
echo
echo "§D a service is 'running' because it is, not because start() returned Ok"
if [ -z "$C" ]; then
  skip "§D — $COMPOSE could not be read"
else
  DS=$(fnbody "$C" "deploy_service")
  if derives "$DS" 'confirm_running\(.*\.await\?'; then
    ok "D1 the deploy confirms the container stayed up"
  else
    bad "D1 'running' is asserted from create+start alone — a crash-on-boot reports success"
  fi
  CR=$(fnbody "$C" "confirm_running")
  if derives "$CR" 'inspect_container'; then
    ok "D2 the confirmation reads the container's real state"
  else
    bad "D2 the confirmation does not inspect anything"
  fi
  if derives "$CR" 'tail_logs'; then
    ok "D3 a failure carries the container's own output, so it can be diagnosed"
  else
    bad "D3 the failure says only that it failed"
  fi
fi
if [ -z "$SB" ]; then
  skip "§D backend — $STACKS_BE could not be read"
else
  # The agent reports per-service failure INSIDE a 200, so an HTTP-status check
  # is not a deploy check.
  if derives "$SB" 'fn deployed_service_states'; then
    ok "D4 the backend has a way to ask whether anything actually came up"
  else
    bad "D4 the backend reads only the HTTP status, which is 200 for a total failure"
  fi
  CRE=$(fnbody "$SB" "create")
  if derives "$CRE" 'deployed_service_states'; then
    ok "D5 create refuses to keep a stack where nothing came up"
  else
    bad "D5 a stack whose every service failed is still saved as a stack"
  fi
fi

# ── §E one door owns a domain; one guard takes it down ─────────────────
echo
echo "§E a stack's domain is claimed through the one door and released through the guard"
if [ -z "$SB" ] || [ -z "$CL" ]; then
  skip "§E — subject unreadable"
else
  CRE=$(fnbody "$SB" "create")
  UPD=$(fnbody "$SB" "update")
  if derives "$CRE" 'ensure_claimable|claim_stack_domain'; then
    ok "E1 create claims the domain before writing anything"
  else
    bad "E1 a stack can take a domain a site or app already holds"
  fi
  if derives "$UPD" 'ensure_claimable|claim_stack_domain'; then
    ok "E2 update claims too — an edit is a second chance to collide"
  else
    bad "E2 the edit path claims nothing"
  fi
  # A holder the claim system cannot SEE is a holder that does not exist: the
  # next site to want this domain would be told it is free.
  FO=$(fnbody "$CL" "find_occupant")
  if derives "$FO" 'FROM docker_stacks'; then
    ok "E3 the claim check can see a stack as a holder"
  else
    bad "E3 stacks write vhosts but are invisible to the claim check — two owners, one domain"
  fi
  if derives "$CL" 'Stack\(Uuid\)'; then
    ok "E4 a stack may re-claim its own domain across an edit"
  else
    bad "E4 no self-exclusion — a stack collides with itself on the second edit"
  fi
fi
if [ -z "$AR" ]; then
  skip "§E agent — $APPS_ROUTE could not be read"
else
  SA=$(fnbody "$AR" "stack_action")
  if derives "$SA" 'unexpose_domain'; then
    ok "E5 removing a stack takes its vhost and certificates down with it"
  else
    bad "E5 the vhost outlives the stack, pointing at a port nothing answers on"
  fi
  UD=$(fnbody "$AR" "unexpose_domain")
  if derives "$UD" 'ownership::app_vhost'; then
    ok "E6 the vhost delete proves the vhost is still this stack's"
  else
    bad "E6 the vhost is deleted on the name alone — a site that took the domain over loses its config"
  fi
  if derives "$UD" 'cert_dir_in_use_elsewhere'; then
    ok "E7 the certificate delete honours a shared wildcard lineage"
  else
    bad "E7 removing one stack can break every other vhost sharing that certificate"
  fi
  # Writing a vhost to an upstream that is already dead hands the operator a
  # 502 and a certificate for it.
  CD=$(fnbody "$AR" "compose_deploy")
  if derives "$CD" 'status == "running"'; then
    ok "E8 a domain is only fronted onto a stack that came up"
  else
    bad "E8 a vhost is written even when every service failed"
  fi
fi

# ── §F a failed edit does not replace the only copy ────────────────────
echo
echo "§F the stored YAML is the only copy, so it outlives a failed redeploy"
if [ -z "$SB" ]; then
  skip "§F — $STACKS_BE could not be read"
else
  UPD=$(fnbody "$SB" "update")
  if derives "$UPD" 'restore_previous'; then
    ok "F1 a failed redeploy puts the previous definition back"
  else
    bad "F1 a failed redeploy leaves the stack down with no way back"
  fi
  # THE DEFECT IS THE ORDER. Both statements existed before; the write simply
  # ran first and unconditionally.
  GUARD=$(lineof "$UPD" 'running == 0')
  WRITE=$(lineof "$UPD" 'UPDATE docker_stacks SET yaml')
  if [ "$GUARD" -gt 0 ] && [ "$WRITE" -gt 0 ] && [ "$GUARD" -lt "$WRITE" ] && live "$UPD"; then
    ok "F2 the YAML is persisted only AFTER the new definition is known to run"
  else
    bad "F2 the YAML write is not gated behind the did-it-run check (guard@$GUARD write@$WRITE)"
  fi
  # The escape-vector validator guarded the endpoint nothing calls.
  if derives "$CRE" 'validate_compose_yaml'; then
    ok "F3 create runs the container-escape validator"
  else
    bad "F3 the endpoint the UI posts to skips the validator entirely"
  fi
fi

# ── §G a crashed container is visible ──────────────────────────────────
echo
echo "§G the container you need the logs of is the one that stopped"
if [ -z "$LR" ]; then
  skip "§G — $LOGS_ROUTE could not be read"
else
  DCN=$(fnbody "$LR" "docker_containers")
  if derives "$DCN" '"-a"'; then
    ok "G1 the log picker lists stopped containers too"
  else
    bad "G1 'docker ps' without -a — a crashed service is missing from the one page that explains it"
  fi
fi
if [ -f "$APPS_TSX" ]; then
  # The stack table had no logs affordance at all, which is why reports about
  # stacks arrive with no detail in them.
  if grep -qE 'handleLogs\(svc\.container_id\)' "$APPS_TSX"; then
    ok "G2 a stack service has a Logs control"
  else
    bad "G2 a stack service cannot be inspected from the panel"
  fi
else
  skip "§G2 — $APPS_TSX could not be read"
fi

# ── §H the update banner is a comparison (GitHub #98) ──────────────────
echo
echo "§H 'an update is available' is a comparison, not a non-empty string"
if [ -z "$TR" ] || [ -z "$TC" ] || [ -z "$UR" ]; then
  skip "§H — subject unreadable"
else
  US=$(fnbody "$TR" "update_status")
  if derives "$US" 'is_newer_release'; then
    ok "H1 the read path compares the advertised version against this binary"
  else
    bad "H1 the banner renders from presence — any stored value, however old, shows an update"
  fi
  if derives "$TR" 'fn get_config'; then
    GC=$(fnbody "$TR" "get_config")
    if derives "$GC" 'is_newer_release'; then
      ok "H2 the config payload carries the same derived answer"
    else
      bad "H2 the Telemetry page's own consumers still test presence"
    fi
  else
    skip "H2 — get_config not found"
  fi
  # update.sh never touches the settings table, so after an upgrade the row
  # still advertises the version now running. Nothing else closes that window.
  RUN=$(fnbody "$TC" "run")
  RECON=$(lineof "$RUN" 'reconcile_stored_update')
  SLEEP=$(lineof "$RUN" 'sleep')
  if [ "$RECON" -gt 0 ] && [ "$SLEEP" -gt 0 ] && [ "$RECON" -lt "$SLEEP" ]; then
    ok "H3 the stored advertisement is reconciled on boot, before anything sleeps"
  else
    bad "H3 no boot reconcile — every upgrade advertises itself for the whole poll interval (recon@$RECON sleep@$SLEEP)"
  fi
  RAT=$(fnbody "$UR" "reject_apply_target")
  if derives "$RAT" 'is_newer_release'; then
    ok "H4 apply refuses a target no newer than the running panel"
  else
    bad "H4 a stale lower advertisement is applicable as a silent downgrade"
  fi
  # ABSENCE ARM. The old comparator dropped a non-numeric segment and shifted
  # the rest, so 2.53.0-rc.1 parsed as [2,53,1] and outranked its own GA. An
  # absence arm is worthless unless its subject was really read (lesson #143),
  # which the subj()/SKIP guard above establishes for both files.
  if ! has "$TC" 'fn compare_versions' && ! has "$TR" 'fn compare_versions'; then
    ok "H5 the segment-dropping comparator is gone, not merely bypassed"
  else
    bad "H5 compare_versions is still present — a second, wrong answer to the same question"
  fi
fi

# ── I. a compose git-deploy may not report a deploy that did not happen (s395) ──
#
# Two defects, one shape: the panel wrote `status = 'running'` for work the agent
# had not done.
#
#  * The compose engine SKIPS any service it cannot resolve to a registry image,
#    so a repository with a Dockerfile — the one whose compose file builds from
#    source — deployed without its application while the row said running.
#  * `deploy_compose` reports per-service outcomes INSIDE a 200 and never errors
#    for a service that failed to start, so both callers took the HTTP status as
#    the answer. With no teardown in the engine, the SECOND compose deploy of the
#    same deployment collides with its own container names and every service
#    fails — still reported as running.
I_GD=$(subj "$GITDEP")  || { bad "I0 could not extract $GITDEP"; I_GD=""; }
I_RT=$(subj "$BE_ROUTER") || { bad "I0 could not extract $BE_ROUTER"; I_RT=""; }
if [ -n "$I_GD" ] && [ -n "$I_RT" ]; then
  ok "I0 subjects extracted (git_deploys ${#I_GD}, routes/mod ${#I_RT} chars)"

  # ⚠ The predicate must be the AGENT'S question. The agent deserialises `image`
  # into an Option<String>; this side reads untyped YAML. They disagree about
  # YAML null, so `image:` left blank — the ordinary way an author switches a
  # service to `build:` — is Some(Null) here and None there. Pinned as the
  # derivation: an arm naming the function survives the rule being inverted.
  if has "$I_RT" 'None \| Some\(serde_yaml_ng::Value::Null\)'; then
    ok "I1 the dropped-service predicate also treats a blanked image: line as no image"
  else
    bad "I1 the dropped-service predicate no longer matches what the agent skips — a compose file whose service says 'image:' with no value would deploy without that service again"
  fi

  # ⚠ NOT fnbody here: `compose_deploy_refusal` opens with a nested `fn
  # sentence`, and this file's fnbody is bounded by the next fn declaration at
  # ANY indent — so its window closes eight lines in, before the arm being
  # pinned. Counted instead: the name occurs exactly twice, its declaration and
  # its one call, so deleting the call reads 1 and goes red.
  D=$(count "$I_GD" 'dropped_service_refusal')
  if [ "$D" = "2" ]; then
    ok "I2 the compose door refuses a file whose services name no image (declared once, called once)"
  else
    bad "I2 expected the no-image refusal declared once and called once, found $D occurrences — a compose file that builds from source would deploy without those services again"
  fi

  # BOTH copies of the branch, counted. There are two, they diverged once
  # already, and fixing one is half a fix that reads exactly like a whole one.
  N=$(count "$I_GD" 'and_then\(compose_outcome\)')
  if [ "$N" = "2" ]; then
    ok "I3 both compose deploy call sites read the per-service outcome (2)"
  else
    bad "I3 expected 2 compose call sites reading the per-service outcome, found $N — a 200 whose services all failed would be written as status='running'"
  fi

  # The reader is SHARED with the stacks door rather than copied, so the two
  # paths cannot drift apart again.
  M=$(count "$I_GD" 'stacks::deployed_service_states')
  if [ "$M" = "1" ]; then
    ok "I4 the per-service reader is the stacks one, not a second copy"
  else
    bad "I4 expected exactly 1 use of the shared per-service reader, found $M"
  fi

  # An empty report is NOT a failure: an agent reporting no services cannot be
  # told from one too old to report them. Getting this backwards would fail every
  # deploy against an older agent.
  if has "$I_GD" 'total > 0 && running == 0'; then
    ok "I5 only a report of services that ALL failed counts as a failure"
  else
    bad "I5 the empty-report guard is gone — a deploy against an agent that reports no services would be recorded as failed"
  fi
fi

echo
echo "== §J  compose_validate is honest about what deploy_service silently drops =="

# s449: `user:`/`healthcheck:`/`entrypoint:` have no field on this deployer —
# `deploy_service` has never read any of the three — but `compose_validate`
# said a stack declaring one was simply "valid", with no warning that the
# directive would be dropped on the floor at deploy time.
# [[project_dockpanel_tech_debt_p185]] carry I.
if [ -n "$AR" ]; then
  CV=$(fnbody "$AR" "compose_validate")
  if [ -z "$CV" ]; then
    bad "J0 could not extract docker_apps::compose_validate"
  else
    ok "J0 docker_apps::compose_validate extracted"
    for flag in user_ignored healthcheck_ignored entrypoint_ignored; do
      if has "$CV" "svc\\.$flag"; then
        ok "J1 compose_validate reads svc.$flag"
      else
        bad "J1 compose_validate does not read svc.$flag — the author gets no warning that this directive is dropped"
      fi
    done
  fi
else
  bad "J $APPS_ROUTE is readable"
fi

# The flags themselves must actually be DERIVED from the parsed def, not a
# constant — `derives` also fails on a `let _unused` or `if false` shape.
CS=panel/agent/src/services/compose.rs
if CSS=$(subj "$CS"); then
  PC=$(fnbody "$CSS" "parse_compose")
  for pair in "user_ignored:def\.user\.is_some" "healthcheck_ignored:def\.healthcheck\.is_some" "entrypoint_ignored:def\.entrypoint\.is_some"; do
    field="${pair%%:*}"; pat="${pair#*:}"
    if derives "$PC" "${field}: ${pat}\(\)"; then
      ok "J2 $field is derived from the parsed definition, live"
    else
      bad "J2 $field is not derived from def.$field.is_some() in parse_compose, or is dead code"
    fi
  done
else
  bad "J2 $CS is readable"
fi

# ── summary ────────────────────────────────────────────────────────────
echo
if [ "$SKIP" -gt 0 ]; then
  echo "compose-stack pin: $PASS passed, $FAIL failed, $SKIP skipped"
else
  echo "compose-stack pin: $PASS passed, $FAIL failed"
fi
[ "$FAIL" -eq 0 ] || exit 1
