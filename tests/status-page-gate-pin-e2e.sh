#!/usr/bin/env bash
# status-page-gate-pin-e2e.sh — s313 / v2.70.0
#
# Pins the fix for a panel that published its own alert engine's output to the
# internet from first boot, with no operator action, while the documented switch
# governed a different endpoint and the guide promised a 404.
#
#   §A  the GATE exists and FAILS CLOSED. Absent setting, unreadable setting,
#       any value that is not the literal "true" — all off. A decision to
#       publish operational state is never made by a default.
#   §B  EVERY route on the public /api/status-page surface reaches it. This is
#       the arm that matters, and it is the arm the old code would have failed:
#       four routes answered unauthenticated and exactly one checked anything.
#       The subject list is COMPUTED from the router, never written here — a
#       class arm that hardcodes its own members cannot see the fifth route
#       somebody adds next year (lesson #181).
#   §C  the public config read is DETERMINISTIC. An unordered LIMIT 1 over a
#       table that could hold duplicates let two readers of one flag disagree,
#       so an operator could untick "Enabled", get a 200, and still publish.
#   §D  one config row per operator — a real ON CONFLICT target, and the unique
#       index that makes it mean anything.
#
# Pure source analysis: no box, no network, no build.
#
# ⚠ §A2 and §B are ABSENCE arms over a subject that DID NOT EXIST at v2.69.0, so
# "red at the previous tag" cannot test them (lesson #222) — there was no gate to
# fail to call. Both were MUTATION-TESTED at HEAD instead: deleting the
# require_enabled call from incidents::subscribe turns §B red and names the
# handler; flipping the gate's unwrap_or(false) to (true) turns §A2 red.
#
# NO PIPES INTO `grep -q` — under `set -o pipefail` grep -q closes the pipe on
# its first match and the arm goes red on correct code, non-deterministically.
# Every arm feeds grep a here-string.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

GATE=panel/backend/src/services/public_status.rs
MOD=panel/backend/src/routes/mod.rs
INC=panel/backend/src/routes/incidents.rs
MON=panel/backend/src/routes/monitors.rs
SVCMOD=panel/backend/src/services/mod.rs

for f in "$GATE" "$MOD" "$INC" "$MON" "$SVCMOD"; do
  [ -f "$f" ] || bad "MISSING SUBJECT FILE: $f"
done

# Comments out, CODE INTACT. Copied from the FIXED stripper (lesson #136): the
# naive s{/\*.*?\*/}{}gs deletes real code, because `/*` occurs inside string
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

# The DECLARATION of one top-level fn: its line plus the parameter list, which is
# where the auth extractors live. Stops at the return arrow so it can never bleed
# into the body and read a gate call as an extractor.
fnsig() {
  awk -v name="$2" '
    /^[[:space:]]*(pub )?(pub\(crate\) )?(async )?fn / {
      inside = ($0 ~ "(pub |pub\\(crate\\) )?(async )?fn " name "\\(")
    }
    inside { print }
    inside && /\)[[:space:]]*->/ { inside=0 }
  ' <<< "$1"
}

echo "== §A  the gate exists, and it fails closed =="

if [ ! -f "$GATE" ]; then
  bad "A1 services/public_status.rs exists (the one place the publish decision is made)"
  bad "A2 the gate fails closed — absent or unreadable setting is OFF"
  bad "A3 the gate is registered in services/mod.rs"
elif G=$(subj "$GATE"); then
  if grep -qE 'pub async fn require_enabled' <<< "$G" \
  && grep -qE 'pub async fn is_enabled' <<< "$G"; then
    ok "A1 services/public_status.rs exposes is_enabled + require_enabled"
  else
    bad "A1 services/public_status.rs exposes is_enabled + require_enabled"
  fi

  # Both halves must be present: the query's error path collapses to None, and
  # the absent row collapses to false. Either one defaulting the other way
  # republishes the page.
  if grep -qE '\.unwrap_or\(None\)' <<< "$G" \
  && grep -qE 'unwrap_or\(false\)' <<< "$G" \
  && ! grep -qE 'unwrap_or(_else)?\((\|\| *)?true' <<< "$G"; then
    ok "A2 the gate fails closed — query error and absent row both read OFF"
  else
    bad "A2 the gate fails closed — query error and absent row both read OFF"
  fi

  if grep -qE '^pub mod public_status;' <<< "$(code "$SVCMOD")"; then
    ok "A3 the gate is registered in services/mod.rs"
  else
    bad "A3 the gate is registered in services/mod.rs"
  fi
fi

echo "== §B  every public status-page route reaches the gate =="

# Subject list COMPUTED from the router. Never written here.
ROUTES=$(grep -oE '\.route\("/api/status-page[^"]*",[^)]*\)' "$MOD" || true)
HANDLERS=$(grep -oE '(monitors|incidents)::[a-z_]+' <<< "$ROUTES" | sort -u || true)
NHANDLERS=$(grep -c . <<< "$HANDLERS" || true)

# An arm that enumerates its own subjects must assert the enumeration FIRST
# (lesson #143) — an empty list makes every check below pass having examined
# nothing. Four public + four admin handlers exist today; anything under 6 means
# the extraction broke, not that the surface shrank.
if [ "$NHANDLERS" -lt 6 ]; then
  bad "B0 router enumeration produced $NHANDLERS status-page handlers — extraction is broken, arms below are vacuous"
else
  ok "B0 router enumeration found $NHANDLERS status-page handlers"

  UNGUARDED=""
  for h in $HANDLERS; do
    mod="${h%%::*}"; fn="${h##*::}"
    src="panel/backend/src/routes/${mod}.rs"
    [ -f "$src" ] || { UNGUARDED="$UNGUARDED $h(no-file)"; continue; }
    S=$(subj "$src") || { UNGUARDED="$UNGUARDED $h(unreadable)"; continue; }

    SIG=$(fnsig "$S" "$fn")
    BODY=$(fnbody "$S" "$fn")
    [ -n "$SIG" ] || { UNGUARDED="$UNGUARDED $h(no-fn)"; continue; }

    # Authenticated is fine — the gate is for the routes that answer to anybody.
    if grep -qE '(AdminUser|AuthUser)' <<< "$SIG"; then continue; fi
    if grep -qE 'public_status::require_enabled' <<< "$BODY"; then continue; fi
    UNGUARDED="$UNGUARDED $h"
  done

  if [ -z "$UNGUARDED" ]; then
    ok "B1 every unauthenticated /api/status-page route calls public_status::require_enabled"
  else
    bad "B1 status-page routes that are neither authenticated nor gated:$UNGUARDED"
  fi
fi

# Named arms for the three that were open, so a regression says WHICH one.
if S=$(subj "$INC"); then
  for fn in public_status_page subscribe unsubscribe; do
    B=$(fnbody "$S" "$fn")
    if [ -n "$B" ] && grep -qE 'public_status::require_enabled' <<< "$B"; then
      ok "B2 incidents::$fn is gated"
    else
      bad "B2 incidents::$fn is gated"
    fi
  done
fi

if S=$(subj "$MON"); then
  B=$(fnbody "$S" status_page)
  # The documented endpoint always failed closed, but on its own inline copy of
  # the check. Two copies of one decision is how the surface drifted apart in the
  # first place, so it must read the shared gate now.
  if [ -n "$B" ] && grep -qE 'public_status::require_enabled' <<< "$B" \
     && ! grep -qE "SELECT value FROM settings WHERE key = 'status_page_enabled'" <<< "$B"; then
    ok "B3 monitors::status_page reads the shared gate, not its own copy"
  else
    bad "B3 monitors::status_page reads the shared gate, not its own copy"
  fi
fi

echo "== §C  the public read is deterministic =="

if S=$(subj "$INC"); then
  B=$(fnbody "$S" public_status_page)
  if [ -n "$B" ] && grep -qE 'FROM status_page_config ORDER BY' <<< "$B"; then
    ok "C1 the public config read is ordered"
  else
    bad "C1 the public config read is ordered"
  fi
  # The specific shape that let two readers disagree.
  if [ -n "$B" ] && ! grep -qE 'FROM status_page_config LIMIT 1' <<< "$B"; then
    ok "C2 no unordered LIMIT 1 over status_page_config"
  else
    bad "C2 no unordered LIMIT 1 over status_page_config"
  fi
fi

echo "== §D  one config row per operator =="

if S=$(subj "$INC"); then
  B=$(fnbody "$S" update_config)
  if [ -n "$B" ] && grep -qE 'ON CONFLICT \(user_id\) DO NOTHING' <<< "$B"; then
    ok "D1 update_config names a real conflict target"
  else
    bad "D1 update_config names a real conflict target"
  fi
  # A targetless clause on this table can only absorb a uuid PK collision, so it
  # reads as a guard and is inert.
  if [ -n "$B" ] && ! grep -qE 'INSERT INTO status_page_config[^;]*ON CONFLICT DO NOTHING' <<< "$B"; then
    ok "D2 no targetless ON CONFLICT on status_page_config"
  else
    bad "D2 no targetless ON CONFLICT on status_page_config"
  fi
fi

# Read each migration WHOLE, not line-wise: `CREATE UNIQUE INDEX ... ON
# status_page_config(user_id)` is written across two lines, and a line-based
# grep reported the migration missing when it was sitting right there. An arm
# that cannot see a correct migration would have sent the next reader looking
# for a bug in the wrong place.
MIGS=$(for m in panel/backend/migrations/*.sql; do
  perl -0777 -ne 'exit(1) unless /CREATE\s+UNIQUE\s+INDEX[^;]*?status_page_config\s*\(user_id\)/si' "$m" \
    && printf '%s\n' "$m"
done)
if [ -n "$MIGS" ]; then
  ok "D3 a migration makes status_page_config.user_id unique"
  # The index is what makes D1 fire; a dedupe that runs after it would deadlock
  # on its own duplicates, so assert the migration removes them first.
  if grep -qE 'DELETE FROM status_page_config' $MIGS; then
    ok "D4 that migration dedupes before adding the constraint"
  else
    bad "D4 that migration dedupes before adding the constraint"
  fi
else
  bad "D3 a migration makes status_page_config.user_id unique"
  bad "D4 that migration dedupes before adding the constraint"
fi

echo "== §E  the guide no longer promises what the code did not do =="

GUIDE=docs/guides/status-page.md
if [ -f "$GUIDE" ]; then
  if grep -qE 'off by default' "$GUIDE" && grep -qE 'master switch' "$GUIDE"; then
    ok "E1 the guide states the switch is off by default and is the master"
  else
    bad "E1 the guide states the switch is off by default and is the master"
  fi
  # The guide told operators a disabled page 404s. It was false for 4.5 months,
  # and an operator who read it had no reason to check. Saying so is the fix.
  if grep -qE 'Upgrade note \(v2\.70\.0\)' "$GUIDE"; then
    ok "E2 the guide discloses that the promise was previously untrue"
  else
    bad "E2 the guide discloses that the promise was previously untrue"
  fi
else
  bad "E1 docs/guides/status-page.md exists"
  bad "E2 docs/guides/status-page.md exists"
fi

echo
printf 'status-page-gate: \033[32m%d passed\033[0m, \033[31m%d failed\033[0m\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
