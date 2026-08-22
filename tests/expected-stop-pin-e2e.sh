#!/usr/bin/env bash
# expected-stop-pin-e2e.sh — s391 / v2.144.0
#
# Pins the fix for an EXPECTED container stop publishing a CRITICAL incident on
# the operator's PUBLIC status page.
#
#   §A  the severity the engine FIRES and the severity the product DECLARES are
#       the same string. This is the arm that would have caught the original
#       defect: `alert_runbook_defaults.rs` has always listed container_down
#       under "Info" and the runbook attached to every one of those
#       notifications says "we don't page on it", while the engine raised it at
#       "critical" — which is the exact string the incident branch gates on. The
#       arm derives BOTH sides and compares them, so it fails whichever one drifts.
#   §B  the suppression sits at the CHECK, never in the fire funnel. The
#       `alert_state` row is stamped 'firing' immediately above the fire and the
#       branch is guarded by `!= Some("firing")`, so a funnel-level skip would
#       stamp a row claiming a page that never went out and then suppress every
#       future container_down for that name — the tombstone this engine's own
#       comment records as having run for four months.
#   §C  the expectation is cleared from OBSERVATION, ABOVE the state ladder and
#       on `not exited/dead` — never inside the recovery arm, which is
#       `running && health != unhealthy` and sits UNDER the unhealthy arm. A
#       container that comes back up with a failing healthcheck (the ordinary
#       case for something stopped because it was misbehaving) never reaches
#       that arm, would keep its expectation for ever, and would be silenced
#       when it finally died.
#   §D  the clear cannot touch the auto-sleep clock. `wake_container` writes
#       is_sleeping/last_woken_at/last_activity_at together, and that statement
#       is the obvious one to copy — but the engine runs every 120s against every
#       running container, so copying it would bump `last_activity_at` for ever
#       and silently disable auto-sleep, whose idle test is that column alone.
#   §E  every deliberate stop path records, and every start path clears —
#       including the STACK path, where one click stops N containers.
#   §F  the caller's container id is CANONICALISED before it is used as a key.
#       `is_valid_container_id` accepts any 1-64 hex string and Docker resolves a
#       prefix, so the short id `docker ps` prints stops the container fine and
#       would otherwise write a row the engine can never match.
#   §G  the suppression predicate is membership and nothing broader (narrowing).
#   §H  the migration carries the server term in its UNIQUE and does NOT backfill.
#   §I  the SPA stops calling the panel's own deliberate action a crash.
#
# Pure source analysis: no box, no network, no build.
#
# NO PIPES INTO `grep -q` — under `set -o pipefail` grep -q closes the pipe on
# its first match and the arm goes red on correct code. Every arm uses a
# here-string.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

ENGINE=panel/backend/src/services/alert_engine.rs
HEALER=panel/backend/src/services/auto_healer.rs
APPS=panel/backend/src/routes/docker_apps.rs
STACKS=panel/backend/src/routes/stacks.rs
SVC=panel/backend/src/services/expected_stops.rs
DEFAULTS=panel/backend/src/services/alert_runbook_defaults.rs
MIG=panel/backend/migrations/20260822000000_container_expected_stops.sql
SPA=panel/frontend/src/pages/Apps.tsx

for f in "$ENGINE" "$HEALER" "$APPS" "$STACKS" "$SVC" "$DEFAULTS" "$MIG" "$SPA"; do
  [ -f "$f" ] || bad "MISSING SUBJECT FILE: $f"
done

# Comments out, CODE INTACT. Copied from the FIXED stripper (lesson #136): the
# naive s{/\*.*?\*/}{}gs deletes real code because `/*` occurs inside string
# literals. Every arm below reads a stripped subject, so the explanatory prose in
# these files — which necessarily quotes the very shapes being pinned — cannot
# satisfy an arm (the s386 trap, which shipped a suite 43/43 green over the
# headline defect restored).
code() {
  perl -0777 -pe '
    s{\{/\*.*?\*/\}}{}gs;
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
  ' "$1"
}
subj() { local t; t=$(code "$1"); [ -n "$t" ] || return 1; printf '%s' "$t"; }

# SQL comments are `--`, which the Rust stripper does not touch.
sqlsubj() { sed -E 's/^[[:space:]]*--.*$//' "$1"; }

# The body of one top-level fn, bounded by the NEXT top-level fn.
fnbody() {
  awk -v name="$2" '
    /^[[:space:]]*(pub )?(pub\(crate\) )?(async )?fn / {
      if ($0 ~ "(pub |pub\\(crate\\) )?(async )?fn " name "\\(") { inside=1; next }
      inside=0
    }
    inside { print }
  ' <<< "$1"
}

# Whitespace-squeezed, so rustfmt reflowing a call across lines cannot retire an
# arm (lesson #585/#638 — it has happened twice).
flat() { tr -d ' \n' <<< "$1"; }

echo "== §A  the severity FIRED and the severity DECLARED are the same string =="

if EG=$(subj "$ENGINE"); then
  CH=$(fnbody "$EG" check_container_health)
  CHF=$(flat "$CH")

  if [ -n "$CH" ]; then
    ok "A0-control check_container_health extracted, $(wc -l <<< "$CH") lines"
  else
    bad "A0-control check_container_health extracted"
  fi

  # DERIVED on both sides. The declared value is read out of the defaults table
  # rather than spelled here, so this arm goes red if EITHER side moves.
  DECLARED=$(perl -0777 -ne '
    while (/alert_type:\s*"container_down"\s*,\s*severity:\s*"([a-z]+)"/gs) { print "$1\n" }
    while (/severity:\s*"([a-z]+)"\s*,\s*alert_type:\s*"container_down"/gs) { print "$1\n" }
  ' "$DEFAULTS" | head -1)
  FIRED=$(perl -0777 -ne 'while (/"container_down",name,"([a-z]+)",/gs) { print "$1\n" }' <<< "$CHF" | head -1)

  if [ -n "$DECLARED" ]; then
    ok "A1-control the declared severity was parsed out of alert_runbook_defaults: '$DECLARED'"
  else
    bad "A1-control the declared severity was parsed out of alert_runbook_defaults — parsed nothing"
  fi
  if [ -n "$FIRED" ]; then
    ok "A2-control the fired severity was parsed out of the engine: '$FIRED'"
  else
    bad "A2-control the fired severity was parsed out of the engine — parsed nothing"
  fi
  if [ -n "$DECLARED" ] && [ "$DECLARED" = "$FIRED" ]; then
    ok "A3 container_down is fired at the severity the product declares ($FIRED)"
  else
    bad "A3 container_down is fired at the severity the product declares — declared '$DECLARED', fired '$FIRED'"
  fi

  # The incident branch gates on this exact string, so the regression has a name.
  if [ "$FIRED" = "critical" ]; then
    bad "A4 container_down is not raised at the severity that opens a PUBLIC incident"
  else
    ok "A4 container_down is not raised at the severity that opens a PUBLIC incident"
  fi

  echo
  echo "== §B  the suppression is at the CHECK, never in the fire funnel =="

  FWR=$(fnbody "$EG" fire_alert_with_retry)
  if grep -qF 'try_fire_alert(' <<< "$FWR"; then
    ok "B1-control the fire funnel was extracted and is non-empty"
    if grep -qF 'expected' <<< "$FWR"; then
      bad "B2 the expectation is NOT consulted at the fire — it is, and every alert_state stamp beside a fire then claims a page that never went out"
    else
      ok "B2 the expectation is not consulted at the fire"
    fi
  else
    bad "B1-control the fire funnel was extracted and is non-empty"
  fi

  # The skip must precede the alert_state stamp, or the row is written 'firing'
  # while nothing is sent and the key is deaf for ever. Ordering by line number
  # with BOTH anchors asserted present (lesson #582).
  # Anchored on the SKIP ITSELF — the predicate guarding a bare `continue` — and
  # not on "the last occurrence of the predicate". The first cut used the latter
  # and passed over its own mutation: deleting the skip left the CLEAR guard as
  # the last match, which is also above the stamp (#626, an arm that inspects the
  # wrong token cannot fail). Found by the mutation run, not by review.
  NSKIP=$(grep -oF 'ifexpected_stopped.contains(name){continue;}' <<< "$CHF" | wc -l)
  if [ "$NSKIP" -eq 1 ]; then
    ok "B3a the exited arm carries exactly one unconditional skip"
  else
    bad "B3a the exited arm carries exactly one unconditional skip — found $NSKIP"
  fi
  # The skip's line = the first predicate use AFTER the ladder opens, so this
  # cannot silently fall back onto the clear guard above it.
  SKIPLINE=$(awk '/state == "exited" \|\| state == "dead"/{seen=1} seen && /expected_stopped\.contains\(name\)/{print NR; exit}' <<< "$CH")
  STAMP=$(grep -n "INSERT INTO alert_state" <<< "$CH" | head -1 | cut -d: -f1)
  if [ -n "$SKIPLINE" ] && [ -n "$STAMP" ] && [ "$SKIPLINE" -lt "$STAMP" ]; then
    ok "B3b the skip precedes the alert_state stamp (skip $SKIPLINE < stamp $STAMP)"
  else
    bad "B3b the skip precedes the alert_state stamp (skip '$SKIPLINE', stamp '$STAMP')"
  fi

  echo
  echo "== §C  cleared from OBSERVATION, above the ladder, not gated on health =="

  CLEAR=$(grep -n 'clear_if_older_than' <<< "$CH" | head -1 | cut -d: -f1)
  LADDER=$(grep -n 'state == "exited" || state == "dead"' <<< "$CH" | head -1 | cut -d: -f1)
  if [ -n "$CLEAR" ] && [ -n "$LADDER" ] && [ "$CLEAR" -lt "$LADDER" ]; then
    ok "C1 the clear runs ABOVE the state ladder (clear $CLEAR < ladder $LADDER)"
  else
    bad "C1 the clear runs ABOVE the state ladder (clear '$CLEAR', ladder '$LADDER')"
  fi

  # Keyed on liveness, not on health. `running` alone would miss restarting /
  # paused / created, and `running && healthy` would miss the unhealthy comeback.
  if grep -qF 'state!="exited"&&state!="dead"&&expected_stopped.contains(name)' <<< "$CHF"; then
    ok "C2 the clear is keyed on not-exited/not-dead, not on running-and-healthy"
  else
    bad "C2 the clear is keyed on not-exited/not-dead, not on running-and-healthy"
  fi

  NCLEAR=$(grep -oF 'clear_if_older_than' <<< "$CHF" | wc -l)
  if [ "$NCLEAR" -eq 1 ]; then
    ok "C3 exactly one clear site in the health check"
  else
    bad "C3 exactly one clear site in the health check — found $NCLEAR"
  fi
else
  for a in A0 A1 A2 A3 A4 B1 B2 B3 C1 C2 C3; do bad "$a $ENGINE is readable"; done
fi

echo
echo "== §D  the clear cannot move the auto-sleep clock =="

if SV=$(subj "$SVC"); then
  CLEARFN=$(fnbody "$SV" clear_if_older_than)
  if grep -qF 'DELETE FROM container_expected_stops' <<< "$CLEARFN"; then
    ok "D1-control the clear statement was extracted"
    BUMPS=""
    for col in last_activity_at last_woken_at total_sleeps is_sleeping; do
      if grep -qF "$col" <<< "$CLEARFN"; then BUMPS="$BUMPS $col"; fi
    done
    if [ -z "$BUMPS" ]; then
      ok "D2 the clear writes none of the auto-sleep columns"
    else
      bad "D2 the clear writes none of the auto-sleep columns — found:$BUMPS"
    fi
  else
    bad "D1-control the clear statement was extracted"
  fi

  # Positive control for D2: those column names DO occur elsewhere in the tree,
  # so "found none" above is a real absence and not a pattern that cannot match.
  if AP=$(subj "$APPS"); then
    if grep -qF 'last_activity_at' <<< "$AP"; then
      ok "D3-control last_activity_at is present elsewhere, so D2's absence is real"
    else
      bad "D3-control last_activity_at is present elsewhere, so D2's absence is real"
    fi
  fi
else
  for a in D1 D2 D3; do bad "$a $SVC is readable"; done
fi

echo
echo "== §E  every deliberate stop records, every start clears =="

if AP=$(subj "$APPS"); then
  for fn in stop_app sleep_container; do
    B=$(flat "$(fnbody "$AP" "$fn")")
    if grep -qF 'expected_stops::record(' <<< "$B"; then
      ok "E1 $fn records the expectation"
    else
      bad "E1 $fn records the expectation"
    fi
  done
  for fn in start_app restart_app wake_container; do
    B=$(flat "$(fnbody "$AP" "$fn")")
    if grep -qF 'expected_stops::clear(' <<< "$B"; then
      ok "E2 $fn clears the expectation"
    else
      bad "E2 $fn clears the expectation"
    fi
  done

  # An expectation recorded before the stop succeeded would mark a RUNNING
  # container. Assert the record sits after the agent call in stop_app.
  SB=$(fnbody "$AP" stop_app)
  POST=$(grep -n 'agent_error("Container stop"' <<< "$SB" | head -1 | cut -d: -f1)
  REC=$(grep -n 'expected_stops::record' <<< "$SB" | head -1 | cut -d: -f1)
  if [ -n "$POST" ] && [ -n "$REC" ] && [ "$POST" -lt "$REC" ]; then
    ok "E3 stop_app records only after the stop succeeded (post $POST < record $REC)"
  else
    bad "E3 stop_app records only after the stop succeeded (post '$POST', record '$REC')"
  fi
else
  bad "E $APPS is readable"
fi

if ST=$(subj "$STACKS"); then
  SA=$(flat "$(fnbody "$ST" stack_action)")
  if grep -qF 'expected_stops::record(' <<< "$SA" && grep -qF 'expected_stops::clear(' <<< "$SA"; then
    ok "E4 the stack path both records and clears — one click stops N containers"
  else
    bad "E4 the stack path both records and clears — one click stops N containers"
  fi
else
  bad "E4 $STACKS is readable"
fi

if HL=$(subj "$HEALER"); then
  HF=$(flat "$HL")
  if grep -qF 'expected_stops::record(' <<< "$HF"; then
    ok "E5 the auto-sleep leg records the expectation"
  else
    bad "E5 the auto-sleep leg records the expectation"
  fi
  # The restart leg must READ it, or the panel restarts the operator's own Stop
  # within 120s — silently, now that the spurious alert is gone.
  if grep -qF 'expected_stopped.contains(name)' <<< "$HF"; then
    ok "E6 the auto-heal restart leg skips a container stopped on purpose"
  else
    bad "E6 the auto-heal restart leg skips a container stopped on purpose"
  fi
else
  for a in E5 E6; do bad "$a $HEALER is readable"; done
fi

echo
echo "== §F  the caller's container id is canonicalised before it becomes a key =="

if AP=$(subj "$APPS"); then
  RC=$(flat "$(fnbody "$AP" resolve_container_name)")
  if grep -qF '[only]=>Some(' <<< "$RC"; then
    ok "F1 an ambiguous id prefix resolves to nothing rather than to the first hit"
  else
    bad "F1 an ambiguous id prefix resolves to nothing rather than to the first hit"
  fi

  # No writer may key on the raw path parameter. Extracted paren-balanced so a
  # reflowed call cannot hide from the arm.
  RAWKEY=$(perl -0777 -ne '
    while (/expected_stops::(?:record|clear)\s*(\((?:[^()]++|(?1))*\))/gs) {
      my $c = $1; $c =~ s/\s+//g;
      print "$c\n" if $c =~ /&container_id/;
    }
  ' <<< "$AP" | wc -l)
  NCALLS=$(perl -0777 -ne '
    while (/expected_stops::(?:record|clear)\s*(\((?:[^()]++|(?1))*\))/gs) { print "x\n" }
  ' <<< "$AP" | wc -l)
  if [ "$NCALLS" -ge 5 ]; then
    ok "F2-control found $NCALLS record/clear call sites to inspect"
  else
    bad "F2-control found $NCALLS record/clear call sites to inspect — too few, the extractor is not matching"
  fi
  if [ "$RAWKEY" -eq 0 ]; then
    ok "F3 no writer keys on the raw path parameter"
  else
    bad "F3 no writer keys on the raw path parameter — $RAWKEY call site(s) do"
  fi
fi

echo
echo "== §G  the suppression is membership and nothing broader =="

if EG=$(subj "$ENGINE"); then
  NPRED=$(grep -oF 'expected_stopped.contains(' <<< "$(flat "$EG")" | wc -l)
  if [ "$NPRED" -eq 2 ]; then
    ok "G1 exactly two uses of the predicate — one clear guard, one skip"
  else
    bad "G1 exactly two uses of the predicate — one clear guard, one skip — found $NPRED"
  fi
fi

echo
echo "== §H  the migration carries the server term and does not backfill =="

if MG=$(sqlsubj "$MIG"); then
  MGF=$(flat "$MG")
  if grep -qF 'UNIQUE(server_id,container_name)' <<< "$MGF"; then
    ok "H1 the UNIQUE carries the server term"
  else
    bad "H1 the UNIQUE carries the server term"
  fi
  if grep -qF 'REFERENCESservers(id)ONDELETECASCADE' <<< "$MGF"; then
    ok "H2 rows die with their server"
  else
    bad "H2 rows die with their server"
  fi
  if grep -qF 'INSERTINTOcontainer_expected_stops' <<< "$MGF"; then
    bad "H3 the migration does NOT backfill — it does, which would silence containers that are down right now"
  else
    ok "H3 the migration does not backfill"
  fi
fi

echo
echo "== §I  the SPA stops calling a deliberate stop a crash =="

if SP=$(subj "$SPA"); then
  SPAF=$(flat "$SP")
  if grep -qF 'stoppedApps.filter(a=>!a.expected_stop_reason)' <<< "$SPAF"; then
    ok "I1 the crashed list excludes containers the panel stopped"
  else
    bad "I1 the crashed list excludes containers the panel stopped"
  fi
  if grep -qF 'intentionallyStopped' <<< "$SPAF"; then
    ok "I2 the deliberate stops are rendered as their own group"
  else
    bad "I2 the deliberate stops are rendered as their own group"
  fi
fi

echo
printf 'expected-stop: \033[32m%d passed\033[0m, \033[31m%d failed\033[0m\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
