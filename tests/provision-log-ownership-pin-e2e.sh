#!/usr/bin/env bash
#
# Regression pin for the provisioning-log keyspace that anyone could read.
#
# `state.provision_logs` is ONE process-wide map shared by nine unrelated
# features — service installs, mail installs, system updates, site provisioning,
# backups, restores, container deploys, git deploys, migration imports. It is a
# single flat keyspace: nothing distinguishes a site id from a deploy id, so any
# endpoint that looks a caller-supplied uuid up in it reaches every other
# feature's stream unless it proves ownership itself.
#
# Five endpoints did that five different ways, and the divergence is the defect:
#
#   A  system::install_log took `AdminUser(_claims)` — the claims DISCARDED —
#      and did a bare lookup. Despite its path it is the panel's GENERAL progress
#      stream: the UI points backup, restore, site-deploy, mail-install and
#      system-update ids at it, so it could not authorize by feature and
#      authorized by nothing.
#
#   B  git_deploys::deploy_log consulted the owner table but fell through when
#      the id was absent, on the stated grounds that "the NOT_FOUND below handles
#      missing deploys". It does not: absent-from-owners and absent-from-logs are
#      different questions. Only 3 of 16 creation sites ever recorded an owner, so
#      absent was the COMMON case — and this route needs only `AuthUser`.
#
#   C  That stream deliberately carries a credential. sites.rs emits the
#      generated CMS admin password in cleartext on its "credentials" step,
#      justified by a comment claiming "the provisioning stream is owner-scoped" —
#      true of the endpoint its author was looking at, false of the siblings.
#
#   MEASURED on a throwaway box running the released v2.49.0 (s291): three
#   accounts attached to one site's provisioning stream. Owner, an unrelated
#   tenant with role "user", and a second admin all received the SAME 1458 bytes
#   containing the same cleartext WordPress admin password.
#
#   D  The two readers that DID check both exempted `role == "admin"`, while
#      /api/sites is user_id-scoped — so admin was never a licence to read
#      another tenant's log, and the exemption reopened A by another door.
#
#   E  A refused stream rendered as SUCCESS. ProvisionLog.tsx turned any error
#      with no steps yet into done + onComplete — a green check over an empty log.
#
# The fix is structural: ownership is carried by the KEY. `register_provision_log`
# creates a log and its owner under one lock scope; `open_provision_log` is the
# only way to read one and refuses an id whose owner is not recorded.
#
# ── WHY THE ARMS ARE SHAPED THE WAY THEY ARE ────────────────────────────────
#
# The first version of this suite was reviewed and found to be spelling-bound,
# and the review MEASURED the evasions rather than asserting them. It hunted
# `logs.insert(` and `logs.get(&` — incidental local-variable names — across a
# hardcoded list of 8 files. A new ownerless writer spelled
# `state.provision_logs.lock()…insert(…)`, or a new unchecked reader whose guard
# was called `guard` instead of `logs`, or ANY provisioning stream added to one
# of the other 55 route files, all passed it 30/30 green.
#
# So the negative arms no longer look for statements. They look for the two
# capabilities you cannot avoid naming:
#
#   * to touch the map at all you must name `state.provision_logs`
#   * to serve its contents to a client you must call `.subscribe()`
#
# Both are checked across EVERY file in routes/, and both are checked by what a
# violation must contain rather than by how somebody happened to write it.
#
# Pure source analysis: no box, no network, no build.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

ROUTES=panel/backend/src/routes
HELPERS=panel/backend/src/helpers.rs
MAIN=panel/backend/src/main.rs
PROVLOG_TSX=panel/frontend/src/components/ProvisionLog.tsx

for f in "$HELPERS" "$MAIN" "$PROVLOG_TSX"; do
  [ -f "$f" ] || { echo "missing $f"; exit 1; }
done
[ -d "$ROUTES" ] || { echo "missing $ROUTES"; exit 1; }

# Strip comments from the SUBJECT FILES before matching.
#
# Not this suite's own header — that text is never searched. It is the sources
# that are the trap, and on this change an unusually loaded one: the fix's own
# comments spell `register_provision_log` and `open_provision_log` in prose, and
# the negative arms forbid literals a commented-out copy of the old block would
# reproduce exactly.
#
# `//` alone is not enough. The review MEASURED both evasions: wrapping
# deploy.rs's registration in `/* … */` left its positive arm green while that
# feature created an ownerless log, and reverting ProvisionLog.tsx with the
# pinned strings left only inside `/* … */` kept both §6 arms green. Block
# comments are the idiomatic way to disable a multi-line Rust call and a normal
# TSX comment form, so they are stripped too.
# A `/*` inside a string literal (a Dockerfile glob, a regex) is DATA, not a
# comment opener — the naive form deleted 485 lines of git_build.rs and 118 of
# agent/routes/nginx.rs, and a truncated subject makes an ABSENCE arm pass on
# code the stripper merely removed. Likewise `//` only opens a comment at the
# start of a line: stripping it anywhere ate the `//` of every `https://` URL.
strip() {
  perl -0777 -pe '
    s{\{/\*.*?\*/\}}{}gs;
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
  ' "$1"
}

# Every route file, comments stripped. ALL of them — the previous version read 8
# of 63, so five plausible future homes for a provisioning stream (stacks.rs,
# staging.rs, wordpress.rs, databases.rs, backup_orchestrator.rs) were unscanned.
ROUTE_FILES=$(ls "$ROUTES"/*.rs)
NROUTES=$(printf '%s\n' $ROUTE_FILES | wc -l)
SRC_ROUTES=""
for f in $ROUTE_FILES; do SRC_ROUTES+=$(strip "$f")$'\n'; done

SRC_HELPERS=$(strip "$HELPERS")
SRC_MAIN=$(strip "$MAIN")
SRC_TSX=$(strip "$PROVLOG_TSX")

# Here-strings, never pipelines: `grep -q` closing a pipe kills the upstream
# with SIGPIPE and `pipefail` turns that into a failed arm non-deterministically.
has()  { grep -q  -- "$2" <<< "$1"; }
hasE() { grep -qE -- "$2" <<< "$1"; }
cnt()  { grep -c  -- "$2" <<< "$1" || true; }
cntE() { grep -cE -- "$2" <<< "$1" || true; }
fnbody() { awk "/pub async fn $2\(/,/^}/" <<< "$1"; }

# Every awk range extraction anchors on an incidental spelling. If one stops
# matching, the variable is EMPTY and every arm reading it goes red blaming the
# code — indistinguishable from the regression.
readable() { [ -n "$2" ] || bad "could not extract $1 — the arms reading it mean nothing"; }

if [ "$NROUTES" -ge 50 ]; then
  ok "scanning all $NROUTES route files"
else
  bad "only $NROUTES route files found — the scan lost its subjects"
fi

echo
echo "§1 the map cannot be touched except through the two helpers"

# THE structural arm. You cannot reach the map without naming it, so enumerate
# every occurrence and allow exactly two forms: an argument to a helper, and the
# Arc clone the spawned emit closures capture. Anything else — notably
# `state.provision_logs.lock()` — is direct access, whatever the local is called.
forms=$(grep -ohE "state\.provision_logs[.a-zA-Z_()]*" <<< "$SRC_ROUTES" | sort -u)
illegal=$(grep -vE "^state\.provision_logs(\.clone\(\))?$" <<< "$forms" || true)
if [ -z "$illegal" ]; then
  ok "every mention of the map in routes/ is a helper argument or an Arc clone"
else
  bad "routes/ reaches the map directly: $(tr '\n' ' ' <<< "$illegal")"
fi

forms=$(grep -ohE "state\.deploy_owners[.a-zA-Z_()]*" <<< "$SRC_ROUTES" | sort -u)
illegal=$(grep -vE "^state\.deploy_owners$" <<< "$forms" || true)
if [ -z "$illegal" ]; then
  ok "no route manipulates the owner table itself"
else
  bad "routes/ touches deploy_owners directly: $(tr '\n' ' ' <<< "$illegal")"
fi

echo
echo "§2 no route can serve the map's contents to a client"

# The capability arm. A provisioning stream is a broadcast Receiver, and the only
# way to obtain one is `.subscribe()`. `open_provision_log` is the sole place
# that may. The one legitimate exception is the notifications SSE, a different
# channel entirely (state.notif_tx) that never touches provision_logs.
subs=$(grep -ohE "[a-zA-Z_.]*\.subscribe\(\)" <<< "$SRC_ROUTES" | sort -u)
rogue=$(grep -vE "^state\.notif_tx\.subscribe\(\)$" <<< "$subs" || true)
if [ -z "$rogue" ]; then
  ok "the only subscribe() in routes/ is the notifications channel"
else
  bad "a route subscribes to a provisioning channel itself: $(tr '\n' ' ' <<< "$rogue")"
fi

if has "$SRC_HELPERS" '.subscribe()'; then
  ok "the helper is where subscription happens"
else
  bad "open_provision_log no longer subscribes — the readers cannot be going through it"
fi

echo
echo "§3 every creation site records an owner"

# Exact counts, not ">= 1". The previous version accepted one registration per
# file, so git_deploys could lose 3 of its 4 and stay green — which is how the
# pre-change ratio (3 of 16) could be re-established one site at a time. Adding a
# provisioning log to a feature is expected to change the number here.
census="backups:2 deploy:1 sites:1 migration:1 git_deploys:4 mail:2 system:4 docker_apps:2"
total=0
for e in $census; do
  f=${e%%:*}; want=${e##*:}
  got=$(cntE "$(strip "$ROUTES/$f.rs")" 'register_provision_log\(')
  total=$((total+got))
  if [ "$got" -eq "$want" ]; then
    ok "$f.rs registers $got of $want"
  else
    bad "$f.rs registers $got owners, expected $want — a creation site lost its owner, or a new one needs adding here"
  fi
done
if [ "$total" -eq 17 ]; then
  ok "17 creation sites, all owned"
else
  bad "$total registrations across the census, expected 17"
fi

echo
echo "§4 every reader proves ownership"

READERS="sites:provision_log docker_apps:deploy_log git_deploys:deploy_log system:install_log migration:progress"
for pair in $READERS; do
  file=${pair%%:*}; fn=${pair##*:}
  body=$(fnbody "$(strip "$ROUTES/$file.rs")" "$fn")
  readable "$file::$fn" "$body"
  [ -n "$body" ] || continue
  if has "$body" 'open_provision_log('; then
    ok "$file::$fn reads the map through the owner check"
  else
    bad "$file::$fn reaches the map without proving ownership"
  fi
done

# Narrow claim, deliberately. This says only that no reader carries the
# `claims.role != "admin"` COMPARISON that let both broken readers skip the
# ownership test. `require_admin(&claims.role)` NARROWS a route and is fine —
# migration::progress does exactly that and must keep it. Three of these five
# were already green before the fix (sites and migration never had the
# exemption; install_log's defect was a different shape, covered below), so
# treat this as a guard against reintroduction, not as evidence any of them is
# correct — §4's first loop is what establishes that.
for pair in $READERS; do
  file=${pair%%:*}; fn=${pair##*:}
  body=$(fnbody "$(strip "$ROUTES/$file.rs")" "$fn")
  [ -n "$body" ] || continue
  if hasE "$body" 'claims\.role *[!=]= *"admin"'; then
    bad "$file::$fn exempts admins from the ownership test"
  else
    ok "$file::$fn carries no admin exemption"
  fi
done

SYS_LOG=$(fnbody "$(strip "$ROUTES/system.rs")" 'install_log')
readable "system::install_log" "$SYS_LOG"
if [ -n "$SYS_LOG" ]; then
  if has "$SYS_LOG" 'AdminUser'; then
    bad "install_log is gated by role again — it must be owner-gated, per key"
  else
    ok "install_log no longer answers on the strength of the admin role alone"
  fi
  if has "$SYS_LOG" 'AuthUser(claims)'; then
    ok "install_log binds the caller it has to compare against"
  else
    bad "install_log discards its claims — there is nothing to check ownership against"
  fi
fi

echo
echo "§5 the door itself fails closed"

# Behaviour, not spelling. The previous arm forbade the literal `None => true`,
# a spelling that never existed in this codebase in either state: it stayed
# green against the genuinely broken tree, and the realistic reintroduction
# `map_or(true, …)` did not trip it. What actually matters is that the owner
# lookup is a total comparison, so enumerate the idioms that turn absence into
# permission instead of guessing one.
if has "$SRC_HELPERS" 'owners.get(&id).copied() == Some(caller)'; then
  ok "the owner comparison treats absence as a mismatch"
else
  bad "the owner comparison changed shape — check that a missing owner cannot pass"
fi

# Scoped to the reader's own body. Against the whole file this matched a `None
# =>` arm in the unrelated IP-allowlist parser — a red that named the wrong code
# and would have been "fixed" by weakening the arm.
OPEN_FN=$(awk '/pub fn open_provision_log\(/,/^}/' <<< "$SRC_HELPERS")
readable "open_provision_log body" "$OPEN_FN"
# Guarded, not merely reported. With no such function the extraction is empty and
# every idiom arm below "passes" against nothing — five confident greens sitting
# next to the one red that actually knows. Skip them instead.
if [ -n "$OPEN_FN" ]; then
  for idiom in 'map_or(true' 'map_or_else' 'unwrap_or(true' 'is_none_or' 'None =>'; do
    if has "$OPEN_FN" "$idiom"; then
      bad "open_provision_log contains '$idiom' — absence may now mean permission"
    else
      ok "no '$idiom' fall-through in open_provision_log"
    fi
  done
fi

# The header delegates the door's behaviour to these unit tests, so the arm has
# to prove they RUN. Grepping the function name alone left `#[test]` deletable
# in silence — the review measured the suite dropping from 6 tests to 5 with
# this arm still green.
for t in an_ownerless_log_is_refused_not_served a_stranger_cannot_read_someone_elses_log; do
  if grep -Pzoq "#\[test\]\s*\n\s*fn $t\(" "$HELPERS"; then
    ok "$t is an executed test"
  else
    bad "$t is missing, or is no longer marked #[test] and therefore never runs"
  fi
done

echo
echo "§6 the owner map stays bounded"

if hasE "$SRC_MAIN" 'if map\.len\(\) *< *before'; then
  bad "owner pruning is conditional on a TTL eviction again — features self-remove in 30-60s, so that tick almost never fires and the map grows for the life of the process"
else
  ok "owner pruning is not gated on the TTL sweep having evicted something"
fi

if has "$SRC_MAIN" 'owners.retain(|id, _| map.contains_key(id))'; then
  ok "owners are pruned against live logs"
else
  bad "nothing prunes the owner map — it now gains an entry per provisioning job"
fi

# The pruner is the one component that must survive; every other accessor of
# these maps recovers from poisoning with into_inner(). An `if let Ok` here
# would let a single panic disable pruning permanently while writers carried on.
if hasE "$SRC_MAIN" 'if let Ok\(mut map\) = provision\.lock\(\)'; then
  bad "the cleanup task gives up on a poisoned lock — writers recover, so only the pruner would stop"
else
  ok "the cleanup task recovers from a poisoned lock like everything else"
fi

echo
echo "§7 a refused stream is not drawn as a completed one"

if has "$SRC_TSX" 'setSevered(true)'; then
  ok "a stream that ends before reporting anything is marked, not assumed complete"
else
  bad "an empty stream is silently treated as success — every arm above becomes invisible in the UI"
fi

if hasE "$SRC_TSX" 'isComplete *=.*!severed'; then
  ok "the severed case is excluded from the success state"
else
  bad "severed is not excluded from isComplete — the green check comes back"
fi

# Without the delay the component unmounts before its own message paints: every
# mount but the PHP picker clears the gating state inside onComplete, so a
# synchronous call replaced the false green check with nothing at all.
# Extracted as a block: grep is line-oriented, so a multi-line pattern spanning
# the two statements silently never matched and reported the code broken.
ONERR=$(awk '/es\.onerror = \(\) => \{/,/^    \};/' <<< "$SRC_TSX")
readable "es.onerror block" "$ONERR"
if [ -n "$ONERR" ]; then
  # Anchoring on "no line STARTS with onComplete" passed the pre-fix code, which
  # called it mid-line inside the state updater. Require instead that the block's
  # single mention of onComplete is the deferred one.
  mentions=$(cnt "$ONERR" 'onComplete')
  if has "$ONERR" 'setTimeout(() => onComplete' && [ "$mentions" -eq 1 ]; then
    ok "the severed path defers onComplete, so its message can render first"
  else
    bad "onComplete fires synchronously on a severed stream ($mentions mention(s)) — the warning never reaches the screen"
  fi
fi

# A retry reuses this component on a new url; without a reset the previous run's
# verdict and steps survive into it.
if hasE "$SRC_TSX" 'setSevered\(false\)'; then
  ok "state is reset per stream, so a retry is not judged by the last attempt"
else
  bad "severed is never cleared — one refused stream pins the component to 'Progress unavailable' forever"
fi

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
