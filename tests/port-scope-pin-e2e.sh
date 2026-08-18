#!/usr/bin/env bash
# Regression pins for the s375 ship — a port allocator and the unique index
# behind it disagreed about what a port is unique WITHIN.
#
# `20260319000000_multi_server.sql` un-globalised five columns under the header
# "domain should be unique per server, not globally", including
# `git_deploys.host_port`. The NEXT migration re-introduced the global shape on
# two neighbouring columns, against allocators that were already server-scoped:
#
#   * `git_previews.host_port` — the allocator scopes through a JOIN and picks
#     the first free port in 8000-8999, so on a fleet the second server's used
#     set is empty, it picks 8000, and the INSERT collides with the first
#     server's row. The rejection was logged at warn and stepped over, and the
#     deploy ran anyway: container, port, vhost and certificate created with NO
#     ROW, and every consumer of a preview is row-driven.
#   * `sites.proxy_port` — same first-fit collision from `generate_series(5000,
#     5999)`, reported to the operator as CONFLICT "Domain already exists".
#
# The invariant these arms pin is NOT "a port index must name server_id".
# `databases.port` is globally unique and globally allocated, and that is
# correct — the table has no `server_id` to scope by. The invariant is that the
# INDEX and its ALLOCATOR agree, which is the thing that was false and which no
# textual arm elsewhere in the suite family could see: every line involved was
# correct on its own, in two files that never mention each other.
#
# §A derives its population from the migrations rather than listing it, because
# the list of port columns IS the thing that goes stale (lesson #551).
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

PASS=0; FAIL=0
ok()  { printf '\033[32m✓\033[0m %s\n' "$1"; PASS=$((PASS+1)); }
bad() { printf '\033[31m✗\033[0m %s — %s\n' "$1" "$2"; FAIL=$((FAIL+1)); }
eq()  { [ "$2" = "$3" ] && ok "$1" || bad "$1" "expected '$3', got '$2'"; }

# ugrep's --ignore-files shim honours .gitignore, so every count below uses the
# real binary explicitly (dockpanel-ops-p2, s357).
G=/usr/bin/grep

MIG=panel/backend/migrations
GD=panel/backend/src/routes/git_deploys.rs
ST=panel/backend/src/routes/sites.rs
DB=panel/backend/src/routes/databases.rs
PC=panel/backend/src/services/preview_cleanup.rs

for f in "$GD" "$ST" "$DB" "$PC"; do
  [ -f "$f" ] || { bad "SETUP" "$f missing"; exit 1; }
done
[ -d "$MIG" ] || { bad "SETUP" "$MIG missing"; exit 1; }

WORK=$(mktemp -d) || exit 1
trap 'rm -rf "$WORK"' EXIT

# An arm that measures an empty subject prints green for every absence below, so
# every subject is asserted before it is measured (lesson #143).
GD_LINES=$($G -c '' "$GD"); ST_LINES=$($G -c '' "$ST")
[ "$GD_LINES" -gt 3000 ] && ok "A0 subject extracted — $GD is $GD_LINES lines" \
  || bad "A0 subject extracted" "$GD is only $GD_LINES lines — every arm below examined nothing"
[ "$ST_LINES" -gt 2000 ] && ok "A0b subject extracted — $ST is $ST_LINES lines" \
  || bad "A0b subject extracted" "$ST is only $ST_LINES lines"

echo "── A. The effective port-bearing unique indexes, DERIVED from the migrations ─"

# SQL statements span lines, so a line-based grep cannot see a CREATE and its
# column list together. Comments are stripped BEFORE the join, or a single `--`
# would swallow the rest of the file and shrink the census silently.
for f in $(ls "$MIG"/*.sql | sort); do
  sed 's/--.*$//' "$f" | tr '\n' ' ' | sed 's/;/;\n/g'
done > "$WORK/stmts.txt"

$G -oiE 'CREATE +UNIQUE +INDEX +(IF +NOT +EXISTS +)?[A-Za-z0-9_]+ +ON +[A-Za-z0-9_]+ *\([^)]*\)' \
  "$WORK/stmts.txt" > "$WORK/creates.txt"
$G -oiE 'DROP +INDEX +(IF +EXISTS +)?[A-Za-z0-9_]+' "$WORK/stmts.txt" \
  | $G -oE '[A-Za-z0-9_]+$' | sort -u > "$WORK/drops.txt"

# A0c/A0d: the parse itself. A malformed regex writes to stderr and returns
# nothing, and "no violations" is indistinguishable from "the grep never ran"
# (lesson #480). Both floors are controls, not assertions about the product.
NCREATE=$($G -c '' "$WORK/creates.txt")
NDROP=$($G -c '' "$WORK/drops.txt")
[ "$NCREATE" -ge 15 ] && ok "A0c migration parse is live — $NCREATE CREATE UNIQUE INDEX statements read" \
  || bad "A0c migration parse is live" "only $NCREATE parsed — the extraction failed, every arm in §A examined nothing"
[ "$NDROP" -ge 8 ] && ok "A0d DROP INDEX parse is live — $NDROP names read" \
  || bad "A0d DROP INDEX parse is live" "only $NDROP parsed"

# The surviving set: created and not later dropped. Every drop in this tree
# names an index that is not re-created under the same name, so a set difference
# is exact; if that ever stops being true A1 goes red on a name it cannot
# resolve, which is the loud direction.
: > "$WORK/live.txt"
while IFS= read -r st; do
  name=$(printf '%s' "$st" | $G -oiE 'INDEX +(IF +NOT +EXISTS +)?[A-Za-z0-9_]+' | $G -oE '[A-Za-z0-9_]+$')
  [ -n "$name" ] || continue
  $G -qxF "$name" "$WORK/drops.txt" && continue
  tbl=$(printf '%s' "$st" | sed -E 's/.*[Oo][Nn] +([A-Za-z0-9_]+) *\(.*/\1/')
  cols=$(printf '%s' "$st" | sed -E 's/.*\(([^)]*)\).*/\1/' | tr -d ' ')
  printf '%s\t%s\t%s\n' "$name" "$tbl" "$cols" >> "$WORK/live.txt"
done < "$WORK/creates.txt"
sort -u "$WORK/live.txt" -o "$WORK/live.txt"

# Port-bearing members only. `_port` and bare `port` both, because the three
# real columns are `host_port`, `proxy_port` and `port`.
awk -F'\t' '$3 ~ /(^|,)[a-z_]*port(,|$)/ {print}' "$WORK/live.txt" > "$WORK/portidx.txt"
NPORT=$($G -c '' "$WORK/portidx.txt")

# A1-control (positive): the derivation finds the index that has been correct
# since 2026-03-19. If this is absent the filter is wrong, not the tree.
$G -qF 'idx_git_deploys_host_port_server' "$WORK/portidx.txt" \
  && ok "A1-control the derivation finds the sibling that was scoped correctly in 2026-03" \
  || bad "A1-control" "idx_git_deploys_host_port_server is not in the derived set — the filter is broken, not the tree"

# A1-control-neg (negative): the global preview index must be GONE from the live
# set and PRESENT in the drop set. Asserting both directions is what separates
# "the fix landed" from "the parse missed it".
if $G -qxF 'idx_git_previews_host_port' "$WORK/drops.txt" \
   && ! $G -qE '^idx_git_previews_host_port\b' "$WORK/portidx.txt"; then
  ok "A1-control-neg the global preview index is dropped and no longer live"
else
  bad "A1-control-neg" "idx_git_previews_host_port is still live, or its DROP was not parsed"
fi

# A1: THE INVARIANT. For every live port-bearing unique index, the index's scope
# and its allocator's scope must be the same. A table with no `server_id` cannot
# be scoped and its allocator must therefore be global too — that is `databases`,
# and it is correct, which is why this arm is not "must contain server_id".
VIOL=""; AGREE=0
while IFS=$'\t' read -r name tbl cols; do
  case ",$cols," in *,server_id,*) idx_scoped=1 ;; *) idx_scoped=0 ;; esac
  # Each branch locates the allocator's BASE query first and only then asks
  # whether it carries a server predicate. A pattern that silently stops
  # matching therefore reports `unmapped` — red — instead of reporting "global",
  # which is a real answer and would be the reassuring direction.
  case "$tbl" in
    git_previews)  A=$GD; base='SELECT gp\.host_port FROM git_previews gp'; scoped='WHERE gd\.server_id = \$1' ;;
    git_deploys)   A=$GD; base='SELECT host_port FROM git_deploys';         scoped='FROM git_deploys WHERE server_id = \$1' ;;
    sites)         A=$ST; base='SELECT proxy_port FROM sites WHERE proxy_port IS NOT NULL'
                          scoped='FROM sites WHERE proxy_port IS NOT NULL AND server_id = \$1' ;;
    # Deliberately global, and correct: `databases` has no server_id column to
    # scope by, so a global index is the only shape its allocator can match.
    databases)     A=$DB; base='SELECT port FROM databases WHERE port IS NOT NULL'
                          scoped='FROM databases WHERE port IS NOT NULL AND server_id' ;;
    # A port index on a table this arm has never been taught about. Red on
    # purpose: whoever adds it must map its allocator here, which is exactly the
    # step that was skipped on 2026-03-20.
    *)             A=''; base=''; scoped='' ;;
  esac
  if [ -z "$A" ]; then
    alloc_scoped=unmapped
  elif ! $G -qE "$base" "$A"; then
    alloc_scoped=unmapped
  elif $G -qE "$scoped" "$A"; then
    alloc_scoped=1
  else
    alloc_scoped=0
  fi
  # Accumulated into ONE assertion rather than emitted per index. `docs-claims`
  # §3 re-runs every suite and compares its live tally to the number published in
  # four places, so a per-member arm would move the published count every time
  # the tree gains a port index — a false red on an unrelated change (#529).
  if [ "$alloc_scoped" = unmapped ]; then
    VIOL="$VIOL
    $name ($tbl.$cols): no allocator mapped — add it to this arm and state whether it is server-scoped"
  elif [ "$alloc_scoped" != "$idx_scoped" ]; then
    if [ "$idx_scoped" = 0 ]; then
      VIOL="$VIOL
    $name ($tbl.$cols): allocator is server-scoped, index is GLOBAL — every server after the first picks the bottom of the range for ever and its INSERT is rejected"
    else
      VIOL="$VIOL
    $name ($tbl.$cols): index is server-scoped, allocator is GLOBAL — the allocator lost its predicate"
    fi
  else
    AGREE=$((AGREE + 1))
  fi
done < "$WORK/portidx.txt"

if [ -z "$VIOL" ]; then
  ok "A1 all $AGREE port indexes agree with their allocator about what a port is unique within"
else
  bad "A1 a port index and its allocator disagree" "$VIOL"
fi

[ "$NPORT" -ge 4 ] && ok "A2 the port-index population is $NPORT — all four port-bearing tables reached" \
  || bad "A2 the port-index population" "only $NPORT derived — expected at least 4 (databases, git_deploys, git_previews, sites)"

echo "── B. git_previews carries the host its allocator scopes by ────────────────"

MIGF="$MIG/20260818000000_port_uniqueness_server_scope.sql"
[ -f "$MIGF" ] && ok "B1 the scoping migration exists" \
  || bad "B1 the scoping migration exists" "$MIGF is missing"

eq "B2 git_previews gains server_id with the sibling tables' ON DELETE CASCADE" \
   "$($G -cE 'ADD COLUMN IF NOT EXISTS server_id UUID REFERENCES servers\(id\) ON DELETE CASCADE' "$MIGF")" "1"

# The backfill is what makes SET NOT NULL safe. Without it the column is added
# nullable and every pre-existing preview becomes a row the new index cannot
# constrain, because NULLs are distinct in a unique index.
eq "B3 existing previews are backfilled from their deploy before the column is tightened" \
   "$($G -cE 'SET server_id = d\.server_id' "$MIGF")" "1"
$G -qE 'ALTER COLUMN server_id SET NOT NULL' "$MIGF" \
  && ok "B4 the column is tightened to NOT NULL" \
  || bad "B4 the column is tightened to NOT NULL" "a nullable server_id makes the unique index unenforceable"
$G -qE 'RAISE WARNING' "$MIGF" \
  && ok "B4b the tightening is guarded — a migration that aborts is worse than a nullable column" \
  || bad "B4b the tightening is guarded" "an unguarded SET NOT NULL can abort the upgrade on an install this backfill cannot reach"

# B5: the INSERT must name the column, or every new row is NULL and the index
# stops constraining anything while every arm above stays green.
eq "B5 the preview upsert writes server_id" \
   "$($G -cE 'INSERT INTO git_previews \(git_deploy_id, server_id,' "$GD")" "1"
eq "B5b the conflict arm repairs server_id too, so a legacy row is corrected on its next push" \
   "$($G -cE 'DO UPDATE SET status = .deploying., server_id = \$2' "$GD")" "1"

echo "── C. A failed record is a refusal, not a warning ──────────────────────────"

# C1 is the defect. Everything else in this file guards the collision; this arm
# guards what the code DID about it. Keyed on the return, not on the absence of
# the warn — the warn is still there and still correct, it just no longer stands
# alone.
# The two anchors are the log line and the spawn. NEITHER is anything this
# section asserts about, which is the point: an earlier draft anchored on the
# refusal's own sentence, so deleting the refusal — the exact defect — made C1
# and C2 go MISSING rather than RED. A check whose anchor is the thing it checks
# for cannot fail ([[feedback_verifier_shares_source]]); it can only vanish.
# Verified by mutation: P1 and P5 now turn C1 and C2 red directly.
GD_UPSERT_WARN=$($G -n 'Failed to upsert git preview record' "$GD" | head -1 | cut -d: -f1)
GD_SPAWN=$($G -n 'tokio::spawn' "$GD" | awk -F: -v w="${GD_UPSERT_WARN:-0}" '$1 > w {print $1; exit}')

if [ -n "$GD_UPSERT_WARN" ] && [ -n "$GD_SPAWN" ] && [ "$GD_SPAWN" -gt "$GD_UPSERT_WARN" ]; then
  ok "C0 both anchors resolved (upsert warn $GD_UPSERT_WARN, spawn $GD_SPAWN)"
  BLOCK=$(sed -n "${GD_UPSERT_WARN},$((GD_SPAWN - 1))p" "$GD")

  # C1 is the defect itself. Between logging the failed write and spawning the
  # build there must be a return: otherwise the container, the port, the vhost
  # and the certificate are created for a row that does not exist.
  printf '%s\n' "$BLOCK" | $G -qE 'return Err\(' \
    && ok "C1 the failed upsert RETURNS before the spawn — no build starts without a row" \
    || bad "C1 the failed upsert returns before the spawn" \
           "nothing between line $GD_UPSERT_WARN and the spawn at $GD_SPAWN returns — the deploy proceeds with no row"

  # C2: the reason is echoed verbatim into the webhook's HTTP body at :1684 and
  # from there into GitHub's delivery log, and that door has no auth extractor.
  # An interpolated sqlx error there publishes the constraint name, the table,
  # and on some paths the conflicting value.
  eq "C2 the refusal interpolates nothing — it cannot carry database detail to an unauthenticated caller" \
     "$(printf '%s\n' "$BLOCK" | $G -c 'format!')" "0"
  eq "C2b the one interpolation in the block is the log's, not the refusal's" \
     "$(printf '%s\n' "$BLOCK" | $G -c '{e}')" "1"
  eq "C2b-control there is exactly one log line in the block to own it" \
     "$(printf '%s\n' "$BLOCK" | $G -c 'tracing::warn!')" "1"
else
  bad "C0 anchors" "warn='$GD_UPSERT_WARN' spawn='$GD_SPAWN' — §C cannot be evaluated"
fi

# C3: the operator still gets the detail the pusher does not.
eq "C3 the sqlx error is logged for the operator, with the branch that produced it" \
   "$($G -cE 'Failed to upsert git preview record for branch .\{branch\}.: \{e\}' "$GD")" "1"

echo "── D. sites.proxy_port — the collision the operator was told was a domain ──"

eq "D1 the site proxy_port index is scoped to the server" \
   "$($G -cE 'idx_sites_proxy_port_server' "$MIGF")" "1"
$G -qE 'ON sites\(proxy_port, server_id\) WHERE proxy_port IS NOT NULL' "$MIGF" \
  && ok "D1b the partial predicate survives — a NULL proxy_port means the runtime does not proxy" \
  || bad "D1b the partial predicate survives" "dropping WHERE proxy_port IS NOT NULL makes every non-proxying site collide"
$G -qE 'DROP INDEX IF EXISTS idx_sites_proxy_port;' "$MIGF" \
  && ok "D1c the global site port index is dropped" \
  || bad "D1c the global site port index is dropped" "both indexes live means the global one still rejects"

# D2: the message. Two unique indexes can reject that INSERT and they mean
# opposite things; collapsing them sent the operator to look at DNS.
eq "D2 a port collision is reported as a port collision, not as a taken domain" \
   "$($G -cE 'msg\.contains\("idx_sites_proxy_port_server"\)' "$ST")" "1"
eq "D2-control the domain arm still exists below it" \
   "$($G -cE 'msg\.contains\("duplicate key"\) \|\| msg\.contains\("unique"\)' "$ST")" "1"

# D3: the range comment. It said 4000-4999 while the query said 5000-5999, in
# the two lines that have to agree for anyone to reason about this at all.
eq "D3 the allocator comment names the range the query actually walks" \
   "$($G -cE 'first free port in the 5000-5999 range' "$ST")" "1"
eq "D3-control the query walks that range" \
   "$($G -cE 'generate_series\(5000, 5999\)' "$ST")" "1"

echo "── E. The sweeps still resolve the host they act on ────────────────────────"

# The teardown's authority comes from the DEPLOY's server_id, reached through the
# JOIN. Adding a column to git_previews must not tempt anyone into dropping that
# JOIN — `unattended-host-scope-pin-e2e.sh` F8 pins the same thing from the other
# side, and this arm states why it must stay.
# Keyed on the SELECT LIST, not on the bare token: `d.server_id` also appears in
# the comment that explains why it is there, so a bare count measures the prose
# as well as the code and moves whenever the comment is edited (ops-p2, s296).
eq "E1 both preview sweeps still carry the deploy's server_id through the JOIN" \
   "$($G -cE 'p\.host_port, d\.server_id' "$PC")" "2"
$G -qE 'JOIN git_deploys d' "$PC" \
  && ok "E1-control the JOIN those columns come from is present" \
  || bad "E1-control" "the JOIN is gone — E1 counted something else"

echo
echo "PASS $PASS / FAIL $FAIL"
[ "$FAIL" -eq 0 ]
