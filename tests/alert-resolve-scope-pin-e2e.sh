#!/usr/bin/env bash
# alert-resolve-scope-pin-e2e.sh — s355 / v2.109.0
#
# Pins the fix for a recovery path that resolved alerts it was never told about,
# and for an alert type that could not be resolved at all.
#
#   §A  `alerts` carries the entity key, and the unresolvable rows were retired.
#   §B  `resolve_alert` is SCOPED BY THAT KEY, refuses to run unscoped, sends
#       nothing when it resolved nothing, and honours the per-type mute. This is
#       the section that matters: `alert_state` is keyed
#       (server_id, alert_type, state_key) and this UPDATE was keyed only
#       (user_id, server_id, alert_type), so one container recovering resolved
#       every other container's firing alert on that server — and because their
#       `alert_state` rows stayed 'firing', a transition-triggered engine never
#       re-announced them. The outages went silent permanently.
#   §C  every PER-ENTITY caller actually passes a key. A scoped UPDATE that all
#       its callers feed `""` is the same bug with more steps. The call list is
#       COMPUTED with a paren-balanced match, never a fixed `grep -A` window
#       (lessons #131/#172), and the enumeration is asserted before it is
#       trusted (#143).
#   §D  `slow_response` has a RESOLVE path. v2.107.0 gave it title-keyed dedup
#       and no clearing counterpart, so one slow check fired a row that stayed
#       'firing' for ever — visible to `check_escalations` through
#       idx_alerts_escalation_sweep, and read by the dedup guard itself, so the
#       stuck row also blinded every future slow-response check on that monitor.
#   §E  reopening an incident CLEARS `resolved_at`, on BOTH write paths, and
#       `postmortem` still keeps it. The stamp survived a reopen, so the public
#       status page rendered "Resolved <n> ago" over a live `investigating`.
#
# Pure source analysis: no box, no network, no build.
#
# MUTATION-TESTED at HEAD — 7 mutations, 7 kills, each by the arm it names, with
# the control asserted to print a real summary first (an empty one is not a pass,
# lesson #461) and every tree restored byte-identical:
#   drop `state_key = $4` from a resolve_alert arm        -> B1 red, counts 2 of 3
#   delete the `resolved == 0` early return               -> B3 red
#   pass `""` at the container resolve call site          -> C2 red, prints it
#   repoint the slow_response resolve at another type     -> D1 red
#   revert the POST reopen branch to the unconditional UPDATE -> E2 red
#   widen the PUT reopen arm to include "postmortem"      -> E1 red
#   add a SEPARATE `Some("postmortem")` clearing arm      -> E3 red
#
# ⚠ E3 was REWRITTEN because the mutation run caught it (#442 — the mutation run
# is the test OF the suite). Its first cut anchored on
# `"postmortem"[^)]*=> ", resolved_at = NULL"`, which cannot cross the `)` a real
# over-reach puts in the way: it stayed GREEN under the widened-arm mutation and
# only E1's exact-string arm caught that by accident. Restated as a property of
# the clearing SITES, it is killed by an isolating mutation that leaves E1's and
# E2's strings byte-identical.
#
# NO PIPES INTO `grep -q` — under `set -o pipefail` grep -q closes the pipe on
# its first match and the arm goes red on correct code. Every arm uses a
# here-string.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

NOTIF=panel/backend/src/services/notifications.rs
ENGINE=panel/backend/src/services/alert_engine.rs
HEALER=panel/backend/src/services/auto_healer.rs
UPTIME=panel/backend/src/services/uptime.rs
INC=panel/backend/src/routes/incidents.rs
MIG=panel/backend/migrations/20260813000000_alert_state_key.sql

for f in "$NOTIF" "$ENGINE" "$HEALER" "$UPTIME" "$INC" "$MIG"; do
  [ -f "$f" ] || bad "MISSING SUBJECT FILE: $f"
done

# Comments out, CODE INTACT. Copied from the FIXED stripper (lesson #136): the
# naive s{/\*.*?\*/}{}gs deletes real code because `/*` occurs inside string
# literals, and a truncated subject makes an ABSENCE arm pass on code the
# stripper merely removed — failure in the reassuring direction.
code() {
  perl -0777 -pe '
    s{\{/\*.*?\*/\}}{}gs;
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
  ' "$1"
}

subj() { local t; t=$(code "$1"); [ -n "$t" ] || return 1; printf '%s' "$t"; }

# The body of one top-level fn, bounded by the NEXT top-level fn. A fixed
# `grep -A n` window is not a function (lesson #131/#172).
fnbody() {
  awk -v name="$2" '
    /^[[:space:]]*(pub )?(pub\(crate\) )?(async )?fn / {
      if ($0 ~ "(pub |pub\\(crate\\) )?(async )?fn " name "\\(") { inside=1; next }
      inside=0
    }
    inside { print }
  ' <<< "$1"
}

# Every call to $2 in file $1, one per line, arguments PAREN-BALANCED and
# whitespace-flattened. Immune to rustfmt reflowing a call across lines — the
# defect class that published 813 routes against a real 821 (lesson #462).
calls() {
  perl -0777 -ne '
    my $n = shift @ARGV;
    while (/\b\Q'"$2"'\E\s*(\((?:[^()]++|(?1))*\))/gs) {
      my $c = $1; $c =~ s/\s+/ /g; print "$c\n";
    }
  ' "$1"
}

echo "== §A  the entity key exists on alerts, and the dead rows were retired =="

if M=$(subj "$MIG"); then
  if grep -qE 'ALTER TABLE alerts ADD COLUMN IF NOT EXISTS state_key' <<< "$M"; then
    ok "A1 migration adds alerts.state_key"
  else
    bad "A1 migration adds alerts.state_key"
  fi

  # NOT NULL DEFAULT '' rather than nullable: `alert_state.state_key` already
  # uses '' for "about the whole server", so the two tables agree on the absent
  # value. A nullable column would silently match nothing under plain `=`.
  if grep -qE "state_key VARCHAR\(100\) NOT NULL DEFAULT ''" <<< "$M"; then
    ok "A2 the key is NOT NULL DEFAULT '' — same absent-value convention as alert_state"
  else
    bad "A2 the key is NOT NULL DEFAULT '' — same absent-value convention as alert_state"
  fi

  if grep -qE "alert_type = 'slow_response'" <<< "$M" \
  && grep -qE "SET status = 'resolved'" <<< "$M"; then
    ok "A3 migration retires the slow_response rows that could never resolve"
  else
    bad "A3 migration retires the slow_response rows that could never resolve"
  fi

  # The retirement must stay scoped to the one type that had no resolver. A
  # general "resolve every firing alert" sweep would clear live outages.
  if grep -qE 'UPDATE alerts' <<< "$M" \
  && [ "$(grep -cE 'UPDATE alerts' <<< "$M")" -eq 1 ]; then
    ok "A4 exactly ONE corrective UPDATE, scoped to slow_response — not a general sweep"
  else
    bad "A4 exactly ONE corrective UPDATE, scoped to slow_response — not a general sweep"
  fi
else
  for a in A1 A2 A3 A4; do bad "$a migration $MIG is readable"; done
fi

if N=$(subj "$NOTIF"); then
  FIRE=$(fnbody "$N" try_fire_alert)
  if [ -n "$FIRE" ] && grep -qE 'INSERT INTO alerts .*state_key' <<< "$FIRE"; then
    ok "A5 try_fire_alert records the key it will later be resolved by"
  else
    bad "A5 try_fire_alert records the key it will later be resolved by"
  fi
else
  bad "A5 $NOTIF is readable"
fi

echo
echo "== §B  resolve_alert is scoped, refuses to run unscoped, and stays quiet =="

if N=$(subj "$NOTIF"); then
  BODY=$(fnbody "$N" resolve_alert)

  if [ -z "$BODY" ]; then
    for b in B1 B2 B3 B4; do bad "$b resolve_alert body could not be extracted"; done
  else
    # THE headline arm. Count the UPDATE arms and require EVERY one to carry the
    # key — asserting the enumeration first (#143), because "0 of 0 arms are
    # unscoped" is the shape that passes on a deleted subject.
    ARMS=$(grep -cE "UPDATE alerts SET status = 'resolved'" <<< "$BODY")
    SCOPED=$(grep -cE 'state_key = \$4' <<< "$BODY")
    if [ "$ARMS" -ge 3 ] && [ "$SCOPED" -eq "$ARMS" ]; then
      ok "B1 all $ARMS resolve arms scope by state_key (none resolves a sibling entity)"
    else
      bad "B1 all resolve arms scope by state_key — found $ARMS arms, $SCOPED scoped"
    fi

    # Both ids NULL *and* an empty key would match every server-wide alert the
    # user has. That case must refuse, not guess.
    if grep -qE 'refusing to resolve unscoped' <<< "$BODY"; then
      ok "B2 no server, no site and no key is a REFUSAL, not an unscoped UPDATE"
    else
      bad "B2 no server, no site and no key is a REFUSAL, not an unscoped UPDATE"
    fi

    # "Resolved: X" for something that was never firing is a false all-clear,
    # and two auto_healer callers reach here without checking first.
    if grep -qE 'rows_affected\(\)' <<< "$BODY" \
    && grep -qE 'if resolved == 0' <<< "$BODY"; then
      ok "B3 a recovery notice is sent only if something was actually resolved"
    else
      bad "B3 a recovery notice is sent only if something was actually resolved"
    fi

    # A muted type that pages on recovery is still a page.
    if grep -qE 'is_type_muted' <<< "$BODY"; then
      ok "B4 the recovery notice honours the per-type mute"
    else
      bad "B4 the recovery notice honours the per-type mute"
    fi
  fi

  # The mute must be SINGLE-SOURCED. It drifted apart once already: applied on
  # the firing path, skipped on the resolve path.
  USERS=$(grep -cE '\bis_type_muted\(' <<< "$N")
  if grep -qE 'fn is_type_muted' <<< "$N" && [ "$USERS" -ge 3 ]; then
    ok "B5 is_type_muted is one function used by both the firing and resolving paths"
  else
    bad "B5 is_type_muted is one function used by both paths — $USERS references"
  fi
else
  for b in B1 B2 B3 B4 B5; do bad "$b $NOTIF is readable"; done
fi

echo
echo "== §C  every per-entity caller passes a real key =="

ALLCALLS=$( { calls "$ENGINE" notifications::resolve_alert
              calls "$HEALER" notifications::resolve_alert
              calls "$UPTIME" notifications::resolve_alert; } 2>/dev/null )
NCALLS=$(grep -c . <<< "$ALLCALLS")

# Assert the enumeration BEFORE trusting it (#143): an empty call list would
# make every arm below vacuously green.
if [ "$NCALLS" -ge 11 ]; then
  ok "C1 enumeration is sound — $NCALLS resolve_alert call sites found"
else
  bad "C1 enumeration is sound — only $NCALLS resolve_alert call sites found (expected >= 11)"
fi

# A call about a NAMED entity that also passes a literal "" is the original bug
# reintroduced at the call site: the UPDATE is scoped, and scoped to nothing.
ENTITY=$(grep -E "service_down|container_down|container_crashloop|container_unhealthy|Container '|slow_response" <<< "$ALLCALLS")
NENT=$(grep -c . <<< "$ENTITY")
UNKEYED=$(grep -E ', "", ' <<< "$ENTITY")
NUNKEYED=$(grep -c . <<< "$UNKEYED")

if [ "$NENT" -ge 4 ] && [ "$NUNKEYED" -eq 0 ]; then
  ok "C2 all $NENT per-entity resolve calls pass a real key, none passes \"\""
else
  bad "C2 per-entity resolve calls pass a real key — $NENT found, $NUNKEYED pass \"\""
  while IFS= read -r c; do [ -n "$c" ] && printf '        unkeyed: %.120s\n' "$c"; done <<< "$UNKEYED"
fi

echo
echo "== §D  slow_response can be resolved =="

if U=$(subj "$UPTIME"); then
  SLOWRESOLVE=$(grep -E '"slow_response"' <<< "$(calls "$UPTIME" notifications::resolve_alert)")
  if [ -n "$SLOWRESOLVE" ]; then
    ok "D1 uptime.rs resolves slow_response when the response time recovers"
  else
    bad "D1 uptime.rs resolves slow_response when the response time recovers"
  fi

  # Dedup keyed on the monitor's id, not on the title. The title carries the
  # monitor NAME, so a rename raised a second firing alert for the same subject
  # and made the first unreachable.
  if grep -qE "AND state_key = \\\$5::text" <<< "$U"; then
    ok "D2 the slow_response dedup guard keys on state_key, not on the title"
  else
    bad "D2 the slow_response dedup guard keys on state_key, not on the title"
  fi

  # Fire and resolve must agree on the key or the resolve silently matches
  # nothing — a green build with the bug intact.
  NKEY=$(grep -cE 'monitor\.id\.to_string\(\)' <<< "$U")
  if [ "$NKEY" -ge 2 ]; then
    ok "D3 fire and resolve use the SAME key expression ($NKEY uses of monitor.id)"
  else
    bad "D3 fire and resolve use the same key expression — only $NKEY use(s) of monitor.id"
  fi
else
  for d in D1 D2 D3; do bad "$d $UPTIME is readable"; done
fi

echo
echo "== §E  reopening an incident clears the resolution stamp, on both paths =="

if I=$(subj "$INC"); then
  # PUT path: the match must name all three live statuses.
  if grep -qE 'Some\("investigating"\) \| Some\("identified"\) \| Some\("monitoring"\) => ", resolved_at = NULL"' <<< "$I"; then
    ok "E1 the PUT path clears resolved_at for all three live statuses"
  else
    bad "E1 the PUT path clears resolved_at for all three live statuses"
  fi

  # POST path: the sibling that would have been left behind. Both endpoints
  # write this column, so fixing one leaves the defect reachable.
  if grep -qE 'matches!\(req\.status\.as_str\(\), "investigating" \| "identified" \| "monitoring"\)' <<< "$I" \
  && grep -qE 'SET status = \$2, resolved_at = NULL, updated_at = NOW\(\)' <<< "$I"; then
    ok "E2 the POST path clears resolved_at for all three live statuses"
  else
    bad "E2 the POST path clears resolved_at for all three live statuses"
  fi

  # The arm that stops an over-broad fix: `postmortem` FOLLOWS a resolution, so
  # clearing the stamp there would destroy the very timestamp a postmortem is
  # written about.
  #
  # Stated as a property of the CLEARING SITES themselves rather than as one
  # spelled-out match arm. The first cut of this arm anchored on
  # `"postmortem"[^)]*=> ", resolved_at = NULL"`, which cannot cross the `)` that
  # a real over-reach puts in the way — so it stayed GREEN under the very
  # mutation it names, and only E1's exact-string arm caught it by accident
  # (lesson #442: the mutation run is the test OF the suite).
  CLEARERS=$(grep -nE 'resolved_at = NULL' <<< "$I")
  NCLEAR=$(grep -c . <<< "$CLEARERS")
  TAINTED=$(grep -c 'postmortem' <<< "$CLEARERS")
  if [ "$NCLEAR" -ge 2 ] && [ "$TAINTED" -eq 0 ]; then
    ok "E3 postmortem KEEPS its resolution stamp — $NCLEAR clearing sites, none mentions it"
  else
    bad "E3 postmortem KEEPS its resolution stamp — $NCLEAR clearing sites, $TAINTED mention it"
  fi
else
  for e in E1 E2 E3; do bad "$e $INC is readable"; done
fi

echo
printf 'alert-resolve-scope: \033[32m%d passed\033[0m, \033[31m%d failed\033[0m\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
