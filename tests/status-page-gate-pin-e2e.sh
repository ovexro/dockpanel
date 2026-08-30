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

echo "== §F  the public surface does not republish what it must not ==="

# v2.149.0. Two leaks on the SAME unauthenticated response, both of them
# over-disclosure rather than a missing gate — the gate above was working.
#
#   §F1/F2/F3  a monitor's own URL. `reqwest::Error`'s Display ends with
#              " for url ({url})", and that string became `incidents.cause`,
#              `managed_incidents.description` and the "Auto-detected: …"
#              timeline entry — two of which this endpoint publishes, while
#              the guide states URLs are not published and the OTHER public
#              handler drops the URL deliberately.
#   §F4        `author_email`. The timeline was serialized whole, and the
#              struct carries the operator's address.
#
# ⚠ F2 is the arm the UNIT TESTS CANNOT REPLACE. `uptime.rs`'s tests exercise
# `redact_monitor_target` directly, so deleting its CALL from
# `describe_check_error` leaves all three of them green — measured, not
# assumed. F2 is what makes that mutation red.

UPT=panel/backend/src/services/uptime.rs

if [ ! -f "$UPT" ]; then
  bad "F0 $UPT exists"
elif U=$(subj "$UPT"); then
  HTTPBODY=$(fnbody "$U" check_http)
  DESCBODY=$(fnbody "$U" describe_check_error)
  REDBODY=$(fnbody "$U" redact_monitor_target)

  # FLOOR the three function subjects. An fnbody whose pattern stops matching
  # yields an EMPTY subject, and every absence arm below it then passes green
  # for a file that no longer contains the code at all.
  if [ "${#HTTPBODY}" -ge 600 ] && [ "${#DESCBODY}" -ge 200 ] && [ "${#REDBODY}" -ge 200 ]; then
    ok "F0 function subjects resolved (check_http ${#HTTPBODY}c, describe_check_error ${#DESCBODY}c, redact_monitor_target ${#REDBODY}c)"

    # The failing arm of check_http must not hand reqwest's Display straight to
    # the caller — that string is what gets stored and published.
    if grep -qE 'describe_check_error\(' <<< "$HTTPBODY" \
    && ! grep -qE 'Some\(e\.to_string\(\)\)' <<< "$HTTPBODY"; then
      ok "F1 check_http renders its error through describe_check_error, not reqwest's Display"
    else
      bad "F1 check_http renders its error through describe_check_error, not reqwest's Display"
    fi

    if grep -qE 'redact_monitor_target\(' <<< "$DESCBODY"; then
      ok "F2 describe_check_error is WIRED to the scrub (the unit tests cannot see this)"
    else
      bad "F2 describe_check_error is WIRED to the scrub (the unit tests cannot see this)"
    fi

    if grep -qE 'without_url\(\)' <<< "$DESCBODY"; then
      ok "F3 describe_check_error still strips the error's own URL slot"
    else
      bad "F3 describe_check_error still strips the error's own URL slot"
    fi
  else
    bad "F0 function subjects resolved — extraction is broken, F1-F3 would be vacuous"
    bad "F1 check_http renders its error through describe_check_error"
    bad "F2 describe_check_error is WIRED to the scrub"
    bad "F3 describe_check_error still strips the error's own URL slot"
  fi
fi

if I=$(subj "$INC"); then
  PUBBODY=$(fnbody "$I" public_status_page)
  if [ "${#PUBBODY}" -ge 1500 ]; then
    # `IncidentUpdate` is `SELECT *` over a table with an author_email column, so
    # serializing the Vec whole publishes it. The projection names its fields.
    if ! grep -qE '"updates": updates' <<< "$PUBBODY" \
    && grep -qE '"updates": public_updates' <<< "$PUBBODY"; then
      ok "F4 the public timeline is projected field by field, not serialized whole"
    else
      bad "F4 the public timeline is projected field by field, not serialized whole"
    fi

    if ! grep -qE 'author_email' <<< "$PUBBODY"; then
      ok "F5 author_email is named nowhere in the public handler"
    else
      bad "F5 author_email is named nowhere in the public handler"
    fi
  else
    bad "F4 public_status_page subject resolved (${#PUBBODY}c) — arms would be vacuous"
    bad "F5 public_status_page subject resolved"
  fi
fi

echo "== §G  the public read is scoped to ONE tenant (s418) =="

# v2.171.0. §A-§F all guarded WHETHER the page publishes and WHAT fields it
# publishes — none of them checked WHOSE data it publishes. Every query below
# `status_page_config`'s ORDER BY was unscoped: on any multi-tenant/reseller
# install, one tenant's components/incidents/legacy-incidents were served to
# every anonymous visitor of every OTHER tenant's /status page. Confirmed
# live-reachable on this box before the fix (10 real incident rows, 1 user).
#
# Each arm names the ONE query it pins, not a bag of substrings, so a
# regression says which query lost its scope.

if I=$(subj "$INC"); then
  PUBBODY=$(fnbody "$I" public_status_page)
  if [ "${#PUBBODY}" -ge 1500 ]; then
    if grep -qE 'SELECT user_id, title, description' <<< "$PUBBODY" \
    && grep -qE 'let owner_id: Option<Uuid> = config\.as_ref\(\)\.map' <<< "$PUBBODY"; then
      ok "G1 the config read carries user_id forward as owner_id"
    else
      bad "G1 the config read carries user_id forward as owner_id"
    fi

    if grep -qE 'FROM status_page_components WHERE user_id = \$1' <<< "$PUBBODY"; then
      ok "G2 the components read is scoped to owner_id"
    else
      bad "G2 the components read is scoped to owner_id"
    fi

    if grep -qE 'WHERE cm\.component_id = \$1 AND m\.user_id = \$2 AND m\.enabled = TRUE' <<< "$PUBBODY"; then
      ok "G3 the per-component monitor-status read is ALSO scoped (defense in depth — the component is already owner-scoped, but a monitor linked cross-tenant into it must not leak status)"
    else
      bad "G3 the per-component monitor-status read is ALSO scoped"
    fi

    if grep -qE 'FROM managed_incidents WHERE user_id = \$1 AND visible_on_status_page = TRUE' <<< "$PUBBODY"; then
      ok "G4 the managed-incidents read is scoped to owner_id"
    else
      bad "G4 the managed-incidents read is scoped to owner_id"
    fi

    if grep -qE 'FROM incidents i JOIN monitors m ON m\.id = i\.monitor_id \\' <<< "$PUBBODY" \
    && grep -qE 'WHERE m\.user_id = \$1 AND i\.started_at > NOW\(\)' <<< "$PUBBODY"; then
      ok "G5 the legacy auto-detected-incidents read is scoped to owner_id"
    else
      bad "G5 the legacy auto-detected-incidents read is scoped to owner_id"
    fi

    # The negative control: NONE of the four data queries may be reachable
    # with no owner_id in hand. An `if let Some(uid) = owner_id` / `match
    # owner_id { Some(uid) => ... None => Vec::new() }` around each is what
    # makes G2-G5 real gates rather than filters bolted on after an unscoped
    # fetch already ran.
    if grep -qE 'if let Some\(uid\) = owner_id' <<< "$PUBBODY" \
    && grep -qE 'match owner_id \{' <<< "$PUBBODY"; then
      ok "G6 the components/incidents queries are structurally UNREACHABLE without an owner_id, not merely filtered"
    else
      bad "G6 the components/incidents queries are structurally UNREACHABLE without an owner_id, not merely filtered"
    fi
  else
    bad "G1 public_status_page subject resolved (${#PUBBODY}c) — G1-G6 would be vacuous"
    bad "G2 components read scoped"; bad "G3 monitor-status read scoped"
    bad "G4 managed-incidents read scoped"; bad "G5 legacy-incidents read scoped"
    bad "G6 queries structurally gated on owner_id"
  fi
fi

echo "== §H  the subscriber list and its fan-out are ALSO scoped (s427) =="

# v2.180.0. §G scoped every READ of the public page to one tenant. The
# subscriber table (and the worker that mails it) was the one piece of this
# surface §G never touched — s418's own fan-out found it, deferred it, and it
# sat as a documented, zero-owner-column carry until now. Same failure shape as
# §G: a real address list, readable/mailable across every tenant on the
# install, not just the one whose page a visitor actually subscribed through.

NOTICES=panel/backend/src/services/status_notices.rs
UPT=panel/backend/src/services/uptime.rs

if G=$(subj "$GATE"); then
  if grep -qE 'pub async fn resolve_current_status_page_owner' <<< "$G"; then
    ok "H1 public_status.rs exposes resolve_current_status_page_owner"
  else
    bad "H1 public_status.rs exposes resolve_current_status_page_owner"
  fi

  # The fallback that makes this reachable even before an admin has ever
  # visited status-page settings — see resolve_current_status_page_owner's own
  # doc comment for why `status_page_enabled` can be true with zero config rows.
  RCSPO=$(fnbody "$G" resolve_current_status_page_owner)
  if [ "${#RCSPO}" -ge 200 ] && grep -qE 'FROM users ORDER BY created_at ASC' <<< "$RCSPO"; then
    ok "H2 the owner resolver falls back to the install's first user when no config row exists"
  else
    bad "H2 the owner resolver falls back to the install's first user when no config row exists"
  fi
fi

if I=$(subj "$INC"); then
  SUBBODY=$(fnbody "$I" subscribe)
  UNSUBBODY=$(fnbody "$I" unsubscribe)
  LISTBODY=$(fnbody "$I" list_subscribers)

  if [ "${#SUBBODY}" -ge 300 ] && [ "${#UNSUBBODY}" -ge 100 ] && [ "${#LISTBODY}" -ge 100 ]; then
    ok "H3 function subjects resolved (subscribe ${#SUBBODY}c, unsubscribe ${#UNSUBBODY}c, list_subscribers ${#LISTBODY}c)"

    if grep -qE 'resolve_current_status_page_owner' <<< "$SUBBODY" \
    && grep -qE 'INSERT INTO status_page_subscribers \(owner_id, email' <<< "$SUBBODY"; then
      ok "H4 subscribe resolves and stamps an owner_id on the new row"
    else
      bad "H4 subscribe resolves and stamps an owner_id on the new row"
    fi

    if grep -qE 'resolve_current_status_page_owner' <<< "$UNSUBBODY" \
    && grep -qE 'WHERE email = \$1 AND owner_id IS NOT DISTINCT FROM \$2' <<< "$UNSUBBODY"; then
      ok "H5 unsubscribe only removes the row for the page the visitor is looking at"
    else
      bad "H5 unsubscribe only removes the row for the page the visitor is looking at"
    fi

    # The negative control: list_subscribers must not be install-wide. Before
    # s427 this endpoint had no WHERE clause at all — any admin's JWT could
    # read every OTHER tenant's subscriber email list.
    if grep -qE 'WHERE owner_id = \$1' <<< "$LISTBODY" \
    && grep -qE '\.bind\(claims\.sub\)' <<< "$LISTBODY"; then
      ok "H6 list_subscribers is scoped to the calling admin's own owner_id"
    else
      bad "H6 list_subscribers is scoped to the calling admin's own owner_id"
    fi
  else
    bad "H3 subscribe/unsubscribe/list_subscribers subjects resolved — H4-H6 would be vacuous"
    bad "H4 subscribe stamps owner_id"; bad "H5 unsubscribe scoped"; bad "H6 list_subscribers scoped"
  fi
fi

echo "== §I  the notify fan-out cannot reach a subscriber of a DIFFERENT tenant (s427) =="

if [ ! -f "$NOTICES" ]; then
  bad "I1 $NOTICES exists"
  bad "I2 enqueue requires an owner_id"
  bad "I3 the worker's subscriber SELECT is scoped to owner_id"
else
  N=$(subj "$NOTICES")
  if grep -qE 'pub owner_id: Uuid' <<< "$N"; then
    ok "I1 StatusNotice carries an owner_id field"
  else
    bad "I1 StatusNotice carries an owner_id field"
  fi

  ENQSIG=$(fnsig "$N" enqueue)
  if grep -qE 'owner_id: Uuid' <<< "$ENQSIG"; then
    ok "I2 enqueue() requires an owner_id — a call site missing it cannot compile"
  else
    bad "I2 enqueue() requires an owner_id — a call site missing it cannot compile"
  fi

  # The one query that actually enforces isolation between two tenants' mail.
  # H4-H6 prove the DATA is tagged; this proves the FAN-OUT respects the tag.
  if grep -qE 'WHERE owner_id = \$1 AND verified = TRUE AND notify_incidents = TRUE' <<< "$N" \
  && grep -qE '\.bind\(notice\.owner_id\)' <<< "$N"; then
    ok "I3 the worker's subscriber SELECT is scoped to the notice's own owner_id"
  else
    bad "I3 the worker's subscriber SELECT is scoped to the notice's own owner_id"
  fi
fi

# Every real producer must pass its OWN tenant's id, never a borrowed one.
# Named per call site so a regression says which producer stopped scoping.
if U=$(subj "$UPT"); then
  DOWNCALLS=$(grep -c 'notify_status_subscribers(&monitor\.name, "investigating"' <<< "$U" || true)
  UPCALLS=$(grep -c 'notify_status_subscribers(&monitor\.name, "resolved"' <<< "$U" || true)
  if grep -qE 'notify_status_subscribers\(&monitor\.name, "investigating"[^;]*monitor\.user_id\);' <<< "$U" \
  && [ "${DOWNCALLS:-0}" -eq 1 ]; then
    ok "I4 the down-transition call passes the monitor's own user_id"
  else
    bad "I4 the down-transition call passes the monitor's own user_id"
  fi
  if grep -qE 'notify_status_subscribers\(&monitor\.name, "resolved"[^;]*monitor\.user_id\);' <<< "$U" \
  && [ "${UPCALLS:-0}" -eq 1 ]; then
    ok "I4b the recovery call passes the monitor's own user_id"
  else
    bad "I4b the recovery call passes the monitor's own user_id"
  fi
  if grep -qE 'owner_id: uuid::Uuid' <<< "$(fnsig "$U" notify_status_subscribers)"; then
    ok "I5 notify_status_subscribers requires an owner_id"
  else
    bad "I5 notify_status_subscribers requires an owner_id"
  fi
fi

if I=$(subj "$INC"); then
  # Named per call site rather than one loose substring match: a `grep -q`
  # over the whole file passes the instant ANY ONE of the three happens to
  # match, so a regression in two of three sites would read as healthy.
  CALLSITES=$(grep -oE 'notify_subscribers\([^;]*\);' <<< "$I" | grep -v 'fn notify_subscribers')
  NCALLS=$(grep -c . <<< "$CALLSITES" 2>/dev/null || echo 0)
  if [ "$NCALLS" -eq 3 ]; then
    ok "I6 exactly 3 notify_subscribers call sites found (a 4th needs its own arm here)"
  else
    bad "I6 expected 3 notify_subscribers call sites, found $NCALLS — I7-I9 would be vacuous or miscounted"
  fi

  if grep -qE 'notify_subscribers\(&incident\.title, status, req\.description\.as_deref\(\)\.unwrap_or\(""\), incident\.user_id\);' <<< "$I"; then
    ok "I7 create_incident passes the just-inserted incident's own user_id"
  else
    bad "I7 create_incident passes the just-inserted incident's own user_id"
  fi

  if grep -qE 'notify_subscribers\(&incident\.title, update_status, message, incident\.user_id\);' <<< "$I"; then
    ok "I8 the PUT update path passes the already-fetched incident's own user_id"
  else
    bad "I8 the PUT update path passes the already-fetched incident's own user_id"
  fi

  if grep -qE 'SELECT title, user_id FROM managed_incidents WHERE id = \$1' <<< "$I" \
  && grep -qE 'notify_subscribers\(&title, &req\.status, &req\.message, owner_id\);' <<< "$I"; then
    ok "I9 the POST update path fetches THIS incident's own owner before enqueuing, then passes it"
  else
    bad "I9 the POST update path fetches THIS incident's own owner before enqueuing, then passes it"
  fi
fi

echo "== §J  the schema backs the scoping, not just the query strings =="

SUBMIG=$(for m in panel/backend/migrations/*.sql; do
  grep -qE 'ALTER TABLE status_page_subscribers' "$m" && grep -qE 'ADD COLUMN IF NOT EXISTS owner_id' "$m" \
    && printf '%s\n' "$m"
done)
if [ -n "$SUBMIG" ]; then
  ok "J1 a migration adds status_page_subscribers.owner_id"
  if grep -qE 'UNIQUE \(owner_id, email\)' $SUBMIG; then
    ok "J2 that migration replaces the global email-only uniqueness with (owner_id, email)"
  else
    bad "J2 that migration replaces the global email-only uniqueness with (owner_id, email)"
  fi
  # The negative control for J2: the OLD single-column constraint must actually
  # be dropped, not just shadowed by a new one sitting alongside it.
  if grep -qE 'DROP CONSTRAINT IF EXISTS status_page_subscribers_email_key' $SUBMIG; then
    ok "J3 the old install-wide UNIQUE(email) constraint is dropped"
  else
    bad "J3 the old install-wide UNIQUE(email) constraint is dropped"
  fi
else
  bad "J1 a migration adds status_page_subscribers.owner_id"
  bad "J2 replaces global uniqueness with (owner_id, email)"
  bad "J3 drops the old UNIQUE(email) constraint"
fi

echo
printf 'status-page-gate: \033[32m%d passed\033[0m, \033[31m%d failed\033[0m\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
