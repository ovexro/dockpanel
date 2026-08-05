#!/usr/bin/env bash
# site-transfer-visibility-pin-e2e.sh — s314 / v2.71.0
#
# Pins the fix for a handover that was a ONE-WAY DOOR, and for the reason the
# role it created could not be used at all.
#
# v2.69.0 shipped `client` + an admin-only site Transfer. Ownership is a single
# axis, so `list` and every per-site read are `WHERE user_id = $1` with no role
# branch — correct, and the whole reason the role needed no other change. But it
# meant that the instant an admin handed a site over, the row left their list,
# the detail page answered 404, and the Transfer control — rendered ONLY on that
# page — became unreachable. There was no way back through the panel. The guide
# and the handler's own doc comment both promised the opposite. Reported from the
# field on #51 by the operator who had just used the feature.
#
#   §A  the admin all-sites READ exists, is admin-gated, is server-scoped, and is
#       a narrow PROJECTION — never `Site`, which is FromRow over `SELECT *` and
#       is bound at three dozen call sites.
#   §B  and ownership is NOT widened anywhere else. This is the arm that matters
#       in the long run: the temptation, next time, is to add `OR role='admin'`
#       to the reads themselves. Subject list COMPUTED, never written here.
#   §C  the way back is REACHABLE — the all-sites list can transfer, and the
#       recipient is PICKED, not typed.
#   §D  the cross-account server-id leak is closed at both ends.
#   §E  the two prose surfaces that promised admin visibility no longer do, and
#       do describe the way back.
#
# Pure source analysis: no box, no network, no build.
#
# ⚠ §A and §C1/§C2 are arms over subjects that DID NOT EXIST at v2.70.0, so "red
# at the previous tag" is trivially true for them and proves little (lesson
# #222). The load-bearing ones are §B (a NEGATIVE arm, green at both tags, whose
# job is to go red on a future widening) and §D/§E (red at v2.70.0 on the real
# defect). §B was MUTATION-TESTED at HEAD instead — see the note above it.
#
# ⚠ Every arm whose subject is a STATEMENT reads the file WHOLE (lesson #230):
# `grep -E` is line-based and this suite's subjects — a Rust fn signature, a SQL
# string, a JSX attribute — are all routinely written across lines.
#
# NO PIPES INTO `grep -q` — under `set -o pipefail` grep -q closes the pipe on
# its first match and the arm goes red on correct code. Every arm uses a
# here-string.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

SITES=panel/backend/src/routes/sites.rs
MOD=panel/backend/src/routes/mod.rs
SITESTSX=panel/frontend/src/pages/Sites.tsx
DETAIL=panel/frontend/src/pages/SiteDetail.tsx
SRVCTX=panel/frontend/src/context/ServerContext.tsx
AUTHCTX=panel/frontend/src/context/AuthContext.tsx
GUIDE=docs/guides/roles-and-ownership.md

for f in "$SITES" "$MOD" "$SITESTSX" "$DETAIL" "$SRVCTX" "$AUTHCTX" "$GUIDE"; do
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
    s{^\s*///.*$}{}gm;
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
# into the body and read a query as an extractor.
fnsig() {
  awk -v name="$2" '
    /^[[:space:]]*(pub )?(pub\(crate\) )?(async )?fn / {
      inside = ($0 ~ "(pub |pub\\(crate\\) )?(async )?fn " name "\\(")
    }
    inside { print }
    inside && /\)[[:space:]]*->/ { inside=0 }
  ' <<< "$1"
}

S=$(subj "$SITES") || bad "sites.rs stripped to nothing"

echo "== §A  the admin all-sites read: gated, scoped, and a projection =="

A_SIG=$(fnsig "$S" list_for_admin)
A_BODY=$(fnbody "$S" list_for_admin)

if [ -z "$A_SIG" ]; then
  bad "A1 sites::list_for_admin exists"
  bad "A2 list_for_admin is ADMIN-gated (AdminUser, not AuthUser)"
  bad "A3 list_for_admin is server-scoped and deliberately NOT user-scoped"
  bad "A4 list_for_admin returns a narrow projection, never Site"
else
  ok "A1 sites::list_for_admin exists"

  # AdminUser, and NOT AuthUser. An AuthUser signature here would hand every
  # account every site on the box — the exact inversion this route must not be.
  if grep -qE 'AdminUser\(' <<< "$A_SIG" && ! grep -qE 'AuthUser\(' <<< "$A_SIG"; then
    ok "A2 list_for_admin is ADMIN-gated (AdminUser, not AuthUser)"
  else
    bad "A2 list_for_admin is ADMIN-gated (AdminUser, not AuthUser)"
  fi

  # Server-scoped yes; user-scoped no. Both halves matter: without the server
  # scope a fleet admin sees another box's rows, and WITH a user scope the route
  # is pointless because it returns exactly what `list` already did.
  if grep -qE 'ServerScope\(' <<< "$A_SIG" \
  && grep -qE 's\.server_id = \$1' <<< "$A_BODY" \
  && ! grep -qE 'user_id = \$' <<< "$A_BODY"; then
    ok "A3 list_for_admin is server-scoped and deliberately NOT user-scoped"
  else
    bad "A3 list_for_admin is server-scoped and deliberately NOT user-scoped"
  fi

  # The projection is a SEPARATE struct. `Site` is FromRow over `SELECT *`; a
  # field with no column breaks every `Site`-typed binding in the crate at
  # runtime, and a Site carrying an owner would eventually be handed to a
  # handler whose whole job is to ask "is this yours?".
  if grep -qE 'Vec<AdminSiteRow>' <<< "$A_SIG" \
  && grep -qE 'pub struct AdminSiteRow' <<< "$S" \
  && ! grep -qE 'Vec<Site>' <<< "$A_SIG"; then
    ok "A4 list_for_admin returns a narrow projection, never Site"
  else
    bad "A4 list_for_admin returns a narrow projection, never Site"
  fi
fi

# Registered, and at an admin-namespaced path. Read whole: a .route(...) call
# routinely wraps.
M=$(subj "$MOD") || bad "mod.rs stripped to nothing"
if grep -qE '\.route\("/api/admin/sites",[[:space:]]*get\(sites::list_for_admin\)\)' <<< "$M"; then
  ok "A5 GET /api/admin/sites is registered to sites::list_for_admin"
else
  bad "A5 GET /api/admin/sites is registered to sites::list_for_admin"
fi

echo "== §B  ownership is NOT widened anywhere else =="

# ⚠ MUTATION TEST (run at HEAD, s314): adding ` OR $1 = $1` to get_one's
# predicate — the shape a "let admins open it too" change would take — turns B2
# red while B1 and B3 stay green, so the arm is per-handler and non-vacuous.
#
# Subject list COMPUTED from the file, not written here: every top-level fn in
# sites.rs whose signature takes AuthUser AND whose body reads `FROM sites`.
# A class arm that hardcodes its members cannot see the handler somebody adds
# next year (lesson #181).
OWNED=""
for fn in $(grep -oE '^pub async fn [a-z_]+' <<< "$S" | awk '{print $4}'); do
  sig=$(fnsig "$S" "$fn")
  body=$(fnbody "$S" "$fn")
  grep -qE 'AuthUser\(' <<< "$sig" || continue
  grep -qE 'FROM sites' <<< "$body" || continue
  OWNED="$OWNED $fn"
done
NOWNED=$(wc -w <<< "$OWNED")

# An arm that enumerates its own subjects must assert the enumeration FIRST
# (lesson #143) — an empty list makes every check below pass having examined
# nothing. There are well over a dozen such handlers today; under 8 means the
# extraction broke, not that the file shrank.
if [ "$NOWNED" -lt 8 ]; then
  bad "B0 handler enumeration produced $NOWNED owner-scoped site readers — extraction is broken, arms below are vacuous"
else
  ok "B0 handler enumeration found $NOWNED AuthUser handlers reading FROM sites"

  # The three that carry the whole ownership guarantee for the transfer story:
  # the list the operator reads, the detail page, and delete. Each must keep a
  # user_id predicate. Named explicitly because these three are what a future
  # "make it convenient for admins" change would reach for first.
  for fn in list get_one remove; do
    body=$(fnbody "$S" "$fn")
    if [ -z "$body" ]; then
      bad "B-$fn sites::$fn exists"
    elif grep -qE 'user_id = \$' <<< "$body"; then
      ok "B-$fn sites::$fn is still ownership-scoped"
    else
      bad "B-$fn sites::$fn is still ownership-scoped — a site read lost its user_id predicate"
    fi
  done

  # And no role branch has crept into the reads. `transfer` legitimately checks
  # claims.role, so the arm is scoped to the three readers above rather than to
  # the file.
  LEAKED=""
  for fn in list get_one remove; do
    body=$(fnbody "$S" "$fn")
    grep -qE "claims\.role" <<< "$body" && LEAKED="$LEAKED $fn"
  done
  if [ -z "$LEAKED" ]; then
    ok "B4 none of list/get_one/remove branches on claims.role"
  else
    bad "B4 role branch appeared in site reads:$LEAKED — ownership must stay one axis"
  fi
fi

echo "== §C  the way back is reachable, and the recipient is picked =="

ST=$(subj "$SITESTSX") || bad "Sites.tsx stripped to nothing"
DT=$(subj "$DETAIL")    || bad "SiteDetail.tsx stripped to nothing"

if grep -qE '"/admin/sites"' <<< "$ST"; then
  ok "C1 Sites.tsx fetches the admin all-sites view"
else
  bad "C1 Sites.tsx fetches the admin all-sites view"
fi

# Transfer must be reachable from the LIST, not only from the detail page. That
# is the entire defect: a control rendered only on a page the admin can no
# longer open can give a site away and never take it back.
if grep -qE 'sites/\$\{transferFor\.id\}/transfer' <<< "$ST"; then
  ok "C2 Sites.tsx can transfer a site it does not own (the way back)"
else
  bad "C2 Sites.tsx can transfer a site it does not own (the way back)"
fi

# Picked, not typed, on BOTH surfaces. `transfer` answers 404 on an unknown
# address, so a text field can only fail after the fact.
if grep -qE 'transferTargets\.map' <<< "$DT" \
&& ! grep -qE 'type="email"[^>]*transferEmail|transferEmail[^>]*type="email"' <<< "$(perl -0777 -pe 's/\s+/ /g' <<< "$DT")"; then
  ok "C3 SiteDetail picks the recipient from the account list, not free text"
else
  bad "C3 SiteDetail picks the recipient from the account list, not free text"
fi

echo "== §D  the cross-account server-id leak is closed at both ends =="

SC=$(subj "$SRVCTX")  || bad "ServerContext.tsx stripped to nothing"
AC=$(subj "$AUTHCTX") || bad "AuthContext.tsx stripped to nothing"

# The self-heal: an account whose server list cannot back the stored id must
# DROP it. Without this branch a client inherits the admin's server id from
# localStorage and ServerScope refuses every request.
#
# ⚠ SCOPED TO fetchServers, and that scoping is the whole arm. The file-wide
# version of this check was GREEN at v2.70.0 — on the defect — because
# `setActiveServerId` has always had a removeItem for the manual clear path. It
# matched the wrong occurrence and tested nothing. Caught by running the suite
# at the previous tag, which is the only reason it is written this way.
FETCH=$(perl -0777 -ne 'print $1 if /const fetchServers\s*=\s*useCallback\(async \(\)\s*=>\s*\{(.*?)\n\s*\}, \[\]\);/s' <<< "$SC")
if [ -z "$FETCH" ]; then
  bad "D1 ServerContext clears a stored server id its own list cannot back — fetchServers closure not found"
elif grep -qE 'removeItem\("dp-active-server"\)' <<< "$FETCH"; then
  ok "D1 ServerContext clears a stored server id its own list cannot back"
else
  bad "D1 ServerContext clears a stored server id its own list cannot back"
fi

# And the source: the value must not survive a sign-out into the next account.
# Scoped to the logout closure rather than the file, so the arm cannot be
# satisfied by an unrelated removeItem elsewhere in the context.
LOGOUT=$(perl -0777 -ne 'print $1 if /const logout\s*=\s*\(\)\s*=>\s*\{(.*?)\n\s*\};/s' <<< "$AC")
if [ -z "$LOGOUT" ]; then
  bad "D2 AuthContext drops the selected server on logout — logout closure not found"
elif grep -qE 'removeItem\("dp-active-server"\)' <<< "$LOGOUT"; then
  ok "D2 AuthContext drops the selected server on logout"
else
  bad "D2 AuthContext drops the selected server on logout"
fi

echo "== §E  the prose no longer promises what the code does not do =="

# Read WHOLE: the retired claim was a sentence spanning two lines, which is
# exactly what a line-wise grep cannot see (lesson #230).
G=$(perl -0777 -pe 's/\s+/ /g' "$GUIDE")
SDOC=$(perl -0777 -pe 's/\s+/ /g' "$SITES")

# ⚠ These are ABSENCE arms, so they match COMMENTS as readily as code
# (feedback_source_pin_prose_trap). Neither the guide nor sites.rs may spell the
# retired sentence, even to say it was retired — both were reworded to describe
# it instead.
if ! grep -qE 'admins see everything' <<< "$G"; then
  ok "E1 the guide no longer promises an admin sees every site"
else
  bad "E1 the guide no longer promises an admin sees every site"
fi

if grep -qE 'All sites on this server' <<< "$G"; then
  ok "E2 the guide names the control that makes a transfer reversible"
else
  bad "E2 the guide names the control that makes a transfer reversible"
fi

if ! grep -qE 'sees them by being' <<< "$SDOC"; then
  ok "E3 transfer's doc comment no longer claims the role preserves the view"
else
  bad "E3 transfer's doc comment no longer claims the role preserves the view"
fi

# The reseller row promised sub-accounts' SITES; the dashboard returns a COUNT.
if ! grep -qE 'Its own sub-accounts and their sites' <<< "$G"; then
  ok "E4 the guide no longer promises a reseller sees its sub-accounts' sites"
else
  bad "E4 the guide no longer promises a reseller sees its sub-accounts' sites"
fi

echo
echo "site-transfer-visibility: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
