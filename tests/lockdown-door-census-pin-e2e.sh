#!/usr/bin/env bash
# lockdown-door-census-pin-e2e.sh — s356 / v2.110.0
#
# Pins the fix for a lockdown that was enforced on some of the doors it names,
# and reported on the page as though it were enforced on all of them.
#
#   §A  every panel handler that proxies to an agent COMMAND-EXECUTION endpoint
#       consults the lockdown. The panel has two surfaces that hand an operator a
#       live shell; until v2.110.0 the lockdown closed one. The Terminal page is
#       refused at three doors, while the Apps console — which posts a string the
#       agent runs as `docker exec <id> sh -c <cmd>` — read no lockdown state and
#       COULD NOT HAVE: `State` was bound as `_state`, so the handler was
#       structurally incapable of asking. Meanwhile the security endpoint
#       reported terminals as blocked to the page.
#   §B  every panel handler that writes a `sites` row consults the lockdown, and
#       the three that are subject to the per-hour ceiling read it from ONE
#       definition. The rule was already written down, in `clone_site`, when a
#       previous fix gave that handler the guards its sibling had; the handler in
#       another module was left behind.
#   §C  the panel's own public URL is settable. It was read from `settings`,
#       absent from the allow-list, and absent from the settings page, so the
#       lookup fell through to an env var and then to the empty string and every
#       alert shipped without its runbook link.
#
# THE ARMS ARE A CENSUS, NOT A SPOT CHECK. §A and §B ENUMERATE their subjects
# from the source and judge each one, so a door added later is judged too — a
# suite naming today's three handlers would go green on tomorrow's fourth. Each
# enumeration is asserted non-empty AND against its expected size before it is
# trusted (lesson #143: an arm that enumerates its own subjects can examine zero
# of them and print green).
#
# ⚠ EVERY ARM READS COMMENT-STRIPPED SOURCE, and this suite is why that rule
# exists (lesson #469 — a census keyed on a hazard counts its own documentation).
# `services/domain_claim.rs` carries a doc comment reading "the four `INSERT INTO
# sites` sites", which is textually identical to the thing §B counts. Counted raw,
# §B finds five doors in four files and reports a defect in a comment.
#
# Pure source analysis: no box, no network, no build.
#
# MUTATION-TESTED at HEAD — 10 mutations, 10 kills, EACH BY THE ARM IT NAMES,
# with the control asserted to print a real summary first (an empty one is not a
# pass, lesson #461) and every tree restored byte-identical by sha256:
#   exec_command's State bound back to `_state` (the defect as it shipped) -> A2
#   exec_command's lockdown call deleted                                   -> A1
#   exec_command's gate moved BELOW the agent dispatch                     -> A3
#   a NEW ungated exec handler appended                                    -> A0
#   staging's lockdown block deleted                                       -> B1
#   staging's per-hour ceiling deleted                                     -> B3
#   the ceiling made private again                                         -> B5
#   base_url removed from the allow-list                                   -> C1
#   the base_url reader deleted                                            -> C3
#   the base_url operator control removed                                  -> C4
#
# ⚠ A2 EARNS ITS PLACE and the mutation run is how that was established. The
# first mutation reverts ONLY the `State` binding, leaving the call to the
# predicate byte-identical — so A1, which looks like the arm that owns this
# defect, stays GREEN. A source pin cannot compile the file; without A2 the exact
# shape this suite exists to prevent would have reproduced under a green §A.
#
# ⚠ The harness itself failed first, and uniformly: it reported 0 kills on 10
# mutations that had all gone red, because these suites print the mark and the
# arm label with an ANSI reset BETWEEN them, so `grep '✗ A1'` never matches
# (lesson #144). A uniform result is a result about the harness, not the pins —
# strip ANSI before reading any suite's output.
#
# NO PIPES INTO `grep -q` — under `set -o pipefail` grep -q closes the pipe on its
# first match and the arm goes red on correct code (lesson #414). Every arm uses a
# here-string.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

APPS=panel/backend/src/routes/docker_apps.rs
TERM=panel/backend/src/routes/terminal.rs
SITES=panel/backend/src/routes/sites.rs
STAGING=panel/backend/src/routes/staging.rs
MIGR=panel/backend/src/routes/migration.rs
SET=panel/backend/src/routes/settings.rs
NOTIF=panel/backend/src/services/notifications.rs
SETTINGS_TSX=panel/frontend/src/pages/Settings.tsx

for f in "$APPS" "$TERM" "$SITES" "$STAGING" "$MIGR" "$SET" "$NOTIF" "$SETTINGS_TSX"; do
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
# `grep -A n` window is not a function (lessons #131/#172).
fnbody() {
  awk -v name="$2" '
    /^[[:space:]]*(pub )?(pub\(crate\) )?(async )?fn / {
      if ($0 ~ "(pub |pub\\(crate\\) )?(async )?fn " name "\\(") { inside=1; next }
      inside=0
    }
    inside { print }
  ' <<< "$1"
}

# Names of the top-level fns in $1 (stripped source) whose body matches $2.
# The enclosing-fn map is computed, never assumed: this is what makes §A and §B
# censuses rather than lists.
fns_matching() {
  awk -v pat="$2" '
    /^[[:space:]]*(pub )?(pub\(crate\) )?(async )?fn / {
      cur=$0
      sub(/^.*fn[[:space:]]+/, "", cur)
      sub(/[(<].*$/, "", cur)
    }
    $0 ~ pat { if (cur != "" && !seen[cur]++) print cur }
  ' <<< "$1"
}

echo "== §A  every door onto a container shell consults the lockdown =="

if A=$(subj "$APPS"); then
  # The agent endpoints that RUN something in a container. `shell-info` belongs
  # here because the agent answers it by running `docker exec <id> which bash`:
  # it is execution on the host and it is the step the console takes immediately
  # before opening a shell, so closing the exec and leaving its probe open would
  # be hardening that stops one step short (lesson #449).
  EXEC_DOORS=$(fns_matching "$A" '/apps/[{][a-z_]+[}]/(exec|shell-info)')
  DOOR_N=$(grep -c . <<< "$EXEC_DOORS")

  if [ "$DOOR_N" -eq 2 ]; then
    ok "A0 enumeration found $DOOR_N execution doors in $(basename $APPS): $(tr '\n' ' ' <<< "$EXEC_DOORS")"
  else
    bad "A0 expected 2 execution doors, enumerated $DOOR_N — a door was added or renamed, judge it: $(tr '\n' ' ' <<< "$EXEC_DOORS")"
  fi

  # Judge EVERY enumerated door, not a named pair.
  while read -r fn; do
    [ -n "$fn" ] || continue
    B=$(fnbody "$A" "$fn")

    if grep -qE 'terminals_locked_down\(&state\.db\)' <<< "$B"; then
      ok "A1 $fn consults the lockdown"
    else
      bad "A1 $fn consults the lockdown"
    fi

    # The handler must be ABLE to ask. An underscore-bound State compiles, reads
    # as a deliberate "this handler needs no database", and is the exact form in
    # which this defect shipped — so a future edit that reverts the binding puts
    # the door back structurally, and the arm above would go red for a reason
    # that looks like a missing call rather than a missing capability.
    SIG=$(awk -v n="$fn" '$0 ~ "fn " n "\\(" {f=1} f {print} f && /^\) ->/ {exit}' <<< "$A")
    if grep -qE 'State\(state\): State<AppState>' <<< "$SIG"; then
      ok "A2 $fn binds State so it can consult it"
    else
      bad "A2 $fn binds State so it can consult it"
    fi

    # Order: the refusal must precede the call it is refusing. A guard below the
    # agent call is not a guard.
    GLINE=$(grep -nE 'terminals_locked_down' <<< "$B" | head -1 | cut -d: -f1)
    CLINE=$(grep -nE '\.(get|post)\(&format!\("/apps/' <<< "$B" | head -1 | cut -d: -f1)
    if [ -n "$GLINE" ] && [ -n "$CLINE" ] && [ "$GLINE" -lt "$CLINE" ]; then
      ok "A3 $fn refuses before it dispatches (gate@$GLINE < agent@$CLINE)"
    else
      bad "A3 $fn refuses before it dispatches (gate@${GLINE:-none} agent@${CLINE:-none})"
    fi
  done <<< "$EXEC_DOORS"
else
  bad "A- could not read $APPS"
fi

# The predicate is single-sourced. Two doors answering "is the panel locked
# down" from two different expressions is how the page and the gate drift apart,
# which is the condition this whole suite exists to prevent.
if A=$(subj "$APPS") && T=$(subj "$TERM"); then
  if grep -qE 'pub async fn terminals_locked_down' <<< "$T"; then
    ok "A4 the shared predicate is defined once, in $(basename $TERM)"
  else
    bad "A4 the shared predicate is defined once, in $(basename $TERM)"
  fi
  # No local re-derivation of the predicate in the apps module.
  if ! grep -qE 'lockdown_state|lockdown_terminal_state' <<< "$A"; then
    ok "A5 $(basename $APPS) does not re-derive the predicate"
  else
    bad "A5 $(basename $APPS) does not re-derive the predicate"
  fi
fi

echo "== §B  every door that creates a site consults the lockdown =="

SITE_DOORS=""
for f in "$SITES" "$STAGING" "$MIGR"; do
  if S=$(subj "$f"); then
    for fn in $(fns_matching "$S" 'INSERT INTO sites'); do
      SITE_DOORS="${SITE_DOORS}${f}:${fn}"$'\n'
    done
  else
    bad "B- could not read $f"
  fi
done
SITE_N=$(grep -c . <<< "$SITE_DOORS")

# Four, and the count is asserted before the members are judged. If a fifth
# handler starts writing `sites` rows this arm names the shortfall rather than
# silently judging the four it knows.
if [ "$SITE_N" -eq 4 ]; then
  ok "B0 enumeration found $SITE_N site-creating handlers: $(tr '\n' ' ' <<< "$SITE_DOORS")"
else
  bad "B0 expected 4 site-creating handlers, enumerated $SITE_N: $(tr '\n' ' ' <<< "$SITE_DOORS")"
fi

while read -r entry; do
  [ -n "$entry" ] || continue
  f=${entry%%:*}; fn=${entry##*:}
  B=$(fnbody "$(subj "$f")" "$fn")

  if grep -qE 'is_locked_down\(&state\.db\)' <<< "$B"; then
    ok "B1 $(basename $f)::$fn refuses during a lockdown"
  else
    bad "B1 $(basename $f)::$fn refuses during a lockdown"
  fi
done <<< "$SITE_DOORS"

# The per-hour ceiling: three of the four, and the fourth is an exclusion that is
# WRITTEN DOWN rather than merely true (lesson #426 — an exclusion you did not
# record is revocable by any feature). A bulk import is an operator asking for
# many sites from a source they named; a ceiling of three would break the
# feature rather than guard it.
CEIL=0
while read -r entry; do
  [ -n "$entry" ] || continue
  f=${entry%%:*}; fn=${entry##*:}
  B=$(fnbody "$(subj "$f")" "$fn")
  if grep -qE 'site_rate_limit\(&state\.db\)' <<< "$B"; then
    CEIL=$((CEIL+1))
  elif [ "$(basename $f)" = "migration.rs" ]; then
    if grep -qE 'ceiling|per-hour' <<< "$(sed -n '/pub async fn import(/,/^}/p' "$f")"; then
      ok "B2 $(basename $f)::$fn is the recorded exclusion from the ceiling"
    else
      bad "B2 $(basename $f)::$fn omits the ceiling with no recorded reason"
    fi
  fi
done <<< "$SITE_DOORS"

if [ "$CEIL" -eq 3 ]; then
  ok "B3 the per-hour ceiling is enforced by $CEIL of the 4 doors"
else
  bad "B3 expected the ceiling on 3 doors, found $CEIL"
fi

# ONE definition of the number. A second copy is how two doors start answering
# differently, and it is why the helper became crate-visible instead of being
# duplicated into the module that could not reach it.
if S=$(subj "$SITES"); then
  DEFS=$(grep -cE '^(pub\(crate\) )?async fn site_rate_limit' <<< "$S")
  if [ "$DEFS" -eq 1 ]; then
    ok "B4 the ceiling has exactly one definition"
  else
    bad "B4 the ceiling has $DEFS definitions"
  fi
  if grep -qE 'pub\(crate\) async fn site_rate_limit' <<< "$S"; then
    ok "B5 the ceiling is reachable from other modules"
  else
    bad "B5 the ceiling is reachable from other modules"
  fi
fi

echo "== §C  the panel's own URL is settable by an operator =="

if S=$(subj "$SET"); then
  ALLOWED=$(awk '/pub const ALLOWED_KEYS/,/\];/' <<< "$S")
  if grep -qE '"base_url"' <<< "$ALLOWED"; then
    ok "C1 base_url is in the settings allow-list"
  else
    bad "C1 base_url is in the settings allow-list"
  fi

  # A relative value is useless: the reader concatenates it into a link that
  # leaves the panel, where nothing supplies an origin.
  if grep -qE 'base_url must be an absolute HTTP' <<< "$S"; then
    ok "C2 a relative base_url is refused before it is stored"
  else
    bad "C2 a relative base_url is refused before it is stored"
  fi
fi

# The reader must still exist. A setting whose only consumer has been deleted is
# the same severed pair pointing the other way, and it would leave §C green over
# a control that configures nothing.
if N=$(subj "$NOTIF"); then
  if grep -qE "key = 'base_url'" <<< "$N"; then
    ok "C3 the reader that makes the setting mean something is still there"
  else
    bad "C3 the reader that makes the setting mean something is still there"
  fi
fi

# And the operator can actually reach it. The whole defect was a value that was
# readable and unsettable, so an allow-list entry with no control repeats it.
if grep -qE 'id="base_url"' "$SETTINGS_TSX"; then
  ok "C4 the settings page renders a control for it"
else
  bad "C4 the settings page renders a control for it"
fi
if grep -qE 'base_url:' "$SETTINGS_TSX"; then
  ok "C5 the settings page submits it"
else
  bad "C5 the settings page submits it"
fi

echo
echo "  PASS $PASS   FAIL $FAIL"
[ "$FAIL" -eq 0 ] || exit 1
