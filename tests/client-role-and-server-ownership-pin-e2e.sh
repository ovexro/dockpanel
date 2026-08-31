#!/usr/bin/env bash
# client-role-and-server-ownership-pin-e2e.sh — s324
#
# TWO PROPERTIES, one theme: a machine is not one administrator's property, and a
# control the panel shows must be one the caller can actually use.
#
#   §A  DELETING AN ACCOUNT MUST NOT DELETE A SERVER'S SITES.
#       `servers.user_id` is `NOT NULL REFERENCES users(id) ON DELETE CASCADE`
#       and `sites.server_id` is `REFERENCES servers(id) ON DELETE CASCADE`.
#       Those two edges compose, so `DELETE FROM users` on whoever happened to
#       hold a machine removed EVERY site on it — every owner's — and answered
#       `{"ok": true}`. The local row belongs to the FIRST admin by `created_at`,
#       so retiring the founding administrator was the trigger.
#
#   §B  A SECOND ADMINISTRATOR CAN SEE THE MACHINES.
#       `servers::list` was `WHERE user_id = $1` for everyone, so a panel's second
#       admin read "No servers found. The local server should appear
#       automatically." — which it never could.
#
#   §C  TRANSFER MOVES THE WHOLE SITE, INCLUDING ITS STAGING CHILD.
#       Staging is a second `sites` row carrying `parent_site_id`. Transfer moved
#       `WHERE id = $2` alone, so the departed owner kept a `www-data` shell inside
#       a full clone of the new owner's document root — and kept push-to-production,
#       which writes that clone OVER the new owner's live site.
#
#   §D  THE SITE SHELL'S CROSS-SITE GUARD SEES THE RELATIVE SPELLING.
#       `/var/www/` matched only the absolute form while the shell's own cwd is
#       inside `/var/www`, so `cat ../other-site/wp-config.php` walked past it.
#
#   §E  A GATE MUST NOT REFUSE A CALLER THEIR OWN ROWS.
#       Four handlers in `monitors.rs` called `require_admin` over SQL already
#       scoped `WHERE user_id = $1`, so the gate decided WHO was refused, never
#       WHAT was returned — while the Dashboard tile beside them showed a client
#       the very same certificates.
#
#   §F  A RESELLER MAY ACT ON EVERY ACCOUNT ITS OWN TABLE LISTS.
#       `list_users` scopes on `reseller_id` alone; update/delete added
#       `AND role = 'user'`, so a sub-account moved to `client` or `suspended`
#       stayed on screen and answered "User not found" to every button.
#
#   §G  A CONTROL A ROLE CAN NEVER USE IS NOT SHOWN TO IT.
#
#   §H  CONTEXT — arms that must be green at BOTH tags, so a harness measuring
#       nothing cannot read as a pass.
#
# EVERY ARM IN §A–§G WAS RUN AGAINST v2.81.0 AND REQUIRED TO BE RED.

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

echo
echo "=============================================="
echo "  client role + server ownership — source pins (s324)"
echo "=============================================="
echo

PASS=0; FAIL=0; SKIP=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

# ⚠ NEVER call `bad` inside `cmd | while read` — the loop runs in a SUBSHELL and
# the FAIL increment dies with it, so the arm prints ✗ and the suite still exits 0
# (s322, wrong-host-dispatch §C4). Use `while read; do …; done < <(…)`.

# Strip comments before matching, or every arm can be satisfied by the prose that
# NARRATES it — and this file's own header spells each defect, which is the trap.
# ⚠ This is the FIXED stripper (s294): a block comment is recognised only where one
# is actually written — opening at line start, closing at line end — because a `/*`
# inside a string literal used to open a comment that ran to the next `*/` and
# deleted real code, making ABSENCE arms pass on code the stripper had removed.
code() {
  perl -0777 -pe '
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
    s{^\s*///.*$}{}gm;
  ' "$1"
}

has() { grep -qE -- "$2" <<< "$1"; }

# Collapse to one line: grep is line-oriented, so an arm pinning a multi-line SQL
# string or an ordered pair of builder calls needs the newlines gone.
flat() { tr '\n' ' ' <<< "$1" | tr -s ' '; }

# Brace depth immediately BEFORE the first occurrence of fixed-string $2 in
# flattened body $1. Depth 1 = a top-level statement directly inside the
# function's own opening brace, not nested in any if/for/match. A text-
# presence-and-ordering pin (has/grep -bo) cannot tell live code from code
# wrapped in a dead branch (`if false { ... }`) — both contain the same text
# in the same relative order. This can: the extra `{` from the wrapper pushes
# everything inside it to depth 2. Prints -1 if $2 is not found at all.
depth_before() {
  local off
  off=$(grep -boF -- "$2" <<< "$1" | head -1 | cut -d: -f1)
  if [ -z "$off" ]; then echo -1; return; fi
  local prefix="${1:0:$off}"
  local opens closes
  opens=$(tr -cd '{' <<< "$prefix" | wc -c)
  closes=$(tr -cd '}' <<< "$prefix" | wc -c)
  echo $((opens - closes))
}

# A function body, bounded on ITS OWN braces.
# ⚠ s323: a window that ends at the SUCCESSOR'S match position swallows the next
# function's declaration, so a body inherits its neighbour's tokens. This counts
# braces instead and stops at depth 0.
fnbody() {
  awk -v fn="$2" '
    index($0, "fn " fn) && !started { started=1 }
    started {
      n=gsub(/\{/,"{"); m=gsub(/\}/,"}"); depth += n - m; print
      if (opened || n>0) opened=1
      if (opened && depth<=0) exit
    }
  ' <<< "$1"
}

USERS=panel/backend/src/routes/users.rs
SERVERS=panel/backend/src/routes/servers.rs
SITES=panel/backend/src/routes/sites.rs
DASH=panel/backend/src/routes/dashboard.rs
DRIFT=panel/backend/src/routes/drift.rs
DRIFT_SVC=panel/backend/src/services/drift.rs
ALERTS=panel/backend/src/routes/alerts.rs
FILTER=panel/agent/src/services/command_filter.rs
MONITORS=panel/backend/src/routes/monitors.rs
RESELLER=panel/backend/src/routes/reseller_dashboard.rs
AUTH=panel/backend/src/routes/auth.rs
LAYOUT=panel/frontend/src/hooks/useLayoutState.ts
SITESTSX=panel/frontend/src/pages/Sites.tsx
DETAIL=panel/frontend/src/pages/SiteDetail.tsx

for f in "$USERS" "$SERVERS" "$SITES" "$DRIFT" "$DRIFT_SVC" "$ALERTS" "$FILTER" "$MONITORS" "$RESELLER" "$AUTH" \
         "$LAYOUT" "$SITESTSX" "$DETAIL"; do
  [ -f "$f" ] || { bad "SETUP subject missing: $f"; }
done

# ── §A  deleting an account must not delete a server's sites ──────────────────

USERS_SRC=$(code "$USERS")
REMOVE=$(flat "$(fnbody "$USERS_SRC" "remove")")

if [ -z "$REMOVE" ]; then
  bad "A0 could not extract users::remove — the extractor is broken, not the tree"
elif ! has "$REMOVE" 'DELETE FROM users'; then
  bad "A0 extracted a users::remove that does not delete a user — wrong window"
else
  ok "A0 users::remove extracted and contains its DELETE ($(wc -c <<< "$REMOVE") bytes)"

  # A1 — the servers are re-pointed BEFORE the user row goes.
  if has "$REMOVE" 'UPDATE servers SET user_id'; then
    ok "A1 users::remove re-points the deleted account's servers"
  else
    bad "A1 users::remove does not re-point servers — deleting an admin CASCADEs through servers into every site on the box"
  fi

  # A2 — ORDER. The UPDATE must precede the DELETE in the source, or the row is
  # already gone (and with it, by cascade, the sites) before anything is saved.
  UPD_AT=$(grep -bo 'UPDATE servers SET user_id' <<< "$REMOVE" | head -1 | cut -d: -f1)
  DEL_AT=$(grep -bo 'DELETE FROM users' <<< "$REMOVE" | head -1 | cut -d: -f1)
  if [ -n "$UPD_AT" ] && [ -n "$DEL_AT" ] && [ "$UPD_AT" -lt "$DEL_AT" ]; then
    ok "A2 the re-point precedes the delete"
  else
    bad "A2 the re-point does not precede the delete (update@${UPD_AT:-none} delete@${DEL_AT:-none}) — a cascade cannot be undone afterwards"
  fi

  # A3 — one transaction. Two statements that can half-apply are how a machine
  # ends up owned by a user row that no longer exists.
  if has "$REMOVE" '\.begin\(\)' && has "$REMOVE" '\.commit\(\)'; then
    ok "A3 the re-point and the delete share one transaction"
  else
    bad "A3 users::remove does not wrap the re-point + delete in a transaction"
  fi

  # A4 — the operator is TOLD. A silent change of machine ownership is its own
  # defect, milder than the cascade and easier to ship by accident.
  if has "$REMOVE" 'reassigned'; then
    ok "A4 the reassignment is reported back, not done silently"
  else
    bad "A4 users::remove re-points servers without reporting it"
  fi
fi

# A5 — THE SCHEMA CLAIM THIS ALL RESTS ON. If either edge stops being CASCADE the
# arms above are pinning a fix for a defect that no longer exists, and this arm is
# how the next reader finds that out instead of assuming.
MIG=$(cat panel/backend/migrations/*.sql 2>/dev/null)
if grep -qE 'user_id UUID NOT NULL REFERENCES users\(id\) ON DELETE CASCADE' <<< "$MIG" \
   && grep -qE 'sites ADD COLUMN IF NOT EXISTS server_id UUID REFERENCES servers\(id\) ON DELETE CASCADE' <<< "$MIG"; then
  ok "A5 both cascade edges are still in the schema (users→servers→sites)"
else
  bad "A5 the users→servers→sites cascade chain is no longer what §A pins — re-derive before trusting A1-A4"
fi

# ── §B  a second administrator can see the machines ───────────────────────────

SERVERS_SRC=$(code "$SERVERS")
SLIST=$(flat "$(fnbody "$SERVERS_SRC" "list")")

if [ -z "$SLIST" ]; then
  bad "B0 could not extract servers::list"
else
  ok "B0 servers::list extracted"
  # Not "mentions admin somewhere" — the QUERY must stop being unconditionally
  # owner-scoped. Pin the shape of the WHERE clause itself.
  if has "$SLIST" 'FROM servers WHERE user_id = \$1'; then
    bad "B1 servers::list is still unconditionally WHERE user_id = \$1 — a panel's second admin sees no servers at all"
  elif has "$SLIST" 'FROM servers WHERE \(\$2 OR user_id = \$1\)'; then
    ok "B1 servers::list admits an administrator to every machine"
  else
    bad "B1 servers::list's WHERE clause is neither the old shape nor the pinned one — re-read it"
  fi
  if has "$SLIST" 'claims\.role == "admin"'; then
    ok "B2 the widening is bound to the admin role, not to a bare truth"
  else
    bad "B2 servers::list widened without binding the condition to the admin role"
  fi
fi

# B3 — THE WIDENING MUST NOT LEAK THE AGENT TOKEN. `SELECT *` feeds this struct,
# and the agent token is root on the agent (the terminal ticket is signed with it).
if has "$(flat "$(code "$SERVERS")")" 'serde\(skip_serializing\)\] +pub agent_token'; then
  ok "B3 Server::agent_token is still skip_serializing — widening the list cannot disclose it"
else
  bad "B3 Server::agent_token is no longer skip_serializing — a widened list would hand out the agent's root credential"
fi

# ── §C  transfer moves the staging child ──────────────────────────────────────

SITES_SRC=$(code "$SITES")
XFER=$(flat "$(fnbody "$SITES_SRC" "transfer")")

if [ -z "$XFER" ] || ! has "$XFER" 'UPDATE sites SET user_id'; then
  bad "C0 could not extract sites::transfer with its UPDATE"
else
  ok "C0 sites::transfer extracted"
  if has "$XFER" 'UPDATE sites SET user_id = \$1 WHERE id = \$2 OR parent_site_id = \$2'; then
    ok "C1 transfer moves the site AND its staging children"
  else
    bad "C1 transfer does not move parent_site_id children — the departed owner keeps a shell in the clone, and push-to-production over the new owner's live site"
  fi
  # C2 — the dependent-table sweep must follow the ids that actually moved. Keying
  # it on the parent alone re-creates the same split one level down.
  if has "$XFER" 'WHERE site_id = ANY\(\$2\)'; then
    ok "C2 the dependent-table sweep keys on the moved id set"
  elif has "$XFER" 'WHERE site_id = \$2'; then
    bad "C2 the dependent-table sweep still keys on the parent id — a staging row's alerts, monitors and vaults stay with the departed owner"
  else
    bad "C2 the dependent-table sweep matches neither shape — re-read it"
  fi
fi

# C3 — THE CONSTANT THAT CLAIMS TO BE DERIVED. `OWNERSHIP_DENORMALIZED_TABLES`
# says it is "every table in the schema carrying BOTH columns". Derive that set
# from the migrations and require the constant to equal it, so the claim cannot
# quietly become false when a migration adds a table.
DERIVED=$(perl -0777 -ne '
  while (/CREATE\s+TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?"?(\w+)"?\s*\(/gi) {
    my ($n,$p)=($1,pos($_)-1); my $d=0; my $s=$p;
    while ($p < length($_)) {
      my $c = substr($_,$p,1);
      $d++ if $c eq "("; if ($c eq ")") { $d--; last if $d==0 } $p++;
    }
    my $b = substr($_,$s+1,$p-$s-1);
    print "$n\n" if $b =~ /(^|[\s,(])user_id\b/m && $b =~ /(^|[\s,(])site_id\b/m;
  }
' <<< "$MIG" | sort -u | tr '\n' ' ')
# ⚠ NOT `sed -n '/NAME/,/\];/p'`. A sed range RESTARTS on every later line matching
# the start pattern, and this constant is also NAMED at its use site — so the range
# reopened at `for table in OWNERSHIP_DENORMALIZED_TABLES` and ran to the next `];`,
# harvesting the JSON keys of the response (`dependent_rows_moved`, `new_owner`,
# `previous_owner`, …) as though they were table names. That printed a confident
# RED against a correct tree. Take the FIRST block only.
DECLARED=$(awk '/OWNERSHIP_DENORMALIZED_TABLES/ { inblk=1 } inblk { print; if (/\];/) exit }' "$SITES" \
           | grep -oE '"[a-z_]+"' | tr -d '"' | sort -u | tr '\n' ' ')
if [ -z "$DERIVED" ]; then
  bad "C3 derived ZERO tables carrying both user_id and site_id — the deriver is broken, not the tree"
elif [ "$DERIVED" = "$DECLARED" ]; then
  ok "C3 OWNERSHIP_DENORMALIZED_TABLES still equals the derived set ($DERIVED)"
else
  bad "C3 OWNERSHIP_DENORMALIZED_TABLES has drifted from the schema — declared [$DECLARED] vs derived [$DERIVED]"
fi

# ── §D  the site shell's cross-site guard ─────────────────────────────────────

FILTER_SRC=$(code "$FILTER")
TERMPAT=$(flat "$(sed -n '/TERMINAL_BLOCKED_PATTERNS/,/^\];/p' <<< "$FILTER_SRC")")

if [ -z "$TERMPAT" ]; then
  bad "D0 could not extract TERMINAL_BLOCKED_PATTERNS"
else
  ok "D0 TERMINAL_BLOCKED_PATTERNS extracted"
  if has "$TERMPAT" '"/var/www/"'; then
    ok "D1 the absolute cross-site spelling is still blocked"
  else
    bad "D1 /var/www/ is no longer blocked in the site terminal"
  fi
  if has "$TERMPAT" '"\.\."'; then
    ok "D2 the relative spelling is blocked too"
  else
    bad "D2 '..' is not blocked — cat ../other-site/wp-config.php walks past the /var/www/ pattern from a cwd inside /var/www"
  fi
fi

# D3 — the guard applies to SITE terminals only. If this ever stops being true an
# administrator's server shell loses `cd ..`, which is a real regression in the
# opposite direction.
TERM_RS=panel/agent/src/routes/terminal.rs
# ⚠ NOT a count of `is_site_terminal` mentions anywhere in the file. That was the
# first draft and a mutation SURVIVED it: replacing one call site's `is_site_terminal`
# with `true` dropped the file-wide count 3 -> 2, which still satisfied
# `N_GATED >= N_CALLS` and printed green over an ungated server shell. Counting two
# populations and comparing the totals is not the same as checking each member.
#
# ⚠ REPOINTED s330. This arm used to count CALL SITES and required at least two,
# because the reconstruct-and-classify loop was written out twice — once per
# WebSocket frame shape. v2.88.0 collapsed both copies into `observe_input`, so
# the subject moved and this arm went red on correct code: the s296 lesson,
# "extracting a helper moves the subject of every pin that measured it". The arm
# follows the code, and D3b below is the companion that lesson demands — the one
# that proves the ORIGINAL callers still REACH the helper, so the gate cannot be
# quietly reintroduced per-branch.
N_CALLS=$(grep -c 'is_safe_terminal_command' "$TERM_RS")
# The block check must live in ONE place and be gated on the SAME line, so there
# is no window between the gate and the check for a later edit to slip into.
if [ "$N_CALLS" -ne 1 ]; then
  bad "D3 expected exactly 1 is_safe_terminal_command call site, found $N_CALLS — the reader or the shape changed"
elif grep -qE 'is_site_terminal && !command_filter::is_safe_terminal_command' "$TERM_RS"; then
  ok "D3 the single is_safe_terminal_command call is gated on is_site_terminal in the same condition"
else
  bad "D3 the block check is no longer gated on is_site_terminal — an administrator's SERVER shell would lose 'cd ..'"
fi

# D3b — BOTH input arms must go through the one classifier. This is the arm that
# would have caught the defect v2.88.0 fixed: the JSON arm classified and the raw
# arm did not, so the ONE input path with no detection was the one the product's
# own UI never uses and only a scripted client takes. A unit test cannot see this
# — it calls `observe_input` directly and would stay green with a branch reverted.
N_OBSERVE=$(grep -c 'observe_input(' "$TERM_RS")
# 1 declaration + 2 call sites (+ the test helpers, which live in the same file).
N_OBSERVE_CALLS=$(grep -cE '= observe_input\(&?(data|text),' "$TERM_RS")
if [ "$N_OBSERVE" -lt 2 ]; then
  bad "D3b observe_input is not present in $TERM_RS — the reader is broken, not the tree"
elif [ "$N_OBSERVE_CALLS" -eq 2 ]; then
  ok "D3b both input arms (JSON frame and raw text) reach the single observe_input path"
else
  bad "D3b $N_OBSERVE_CALLS of 2 input arms reach observe_input — a frame shape is being classified differently again"
fi

# ── §E  a gate must not refuse a caller their own rows ────────────────────────

MON_SRC=$(code "$MONITORS")
E_VIOL=0; E_N=0
while read -r fn; do
  [ -n "$fn" ] || continue
  B=$(flat "$(fnbody "$MON_SRC" "$fn")")
  [ -n "$B" ] || continue
  # Only handlers whose SQL is already per-caller are subjects. A handler with no
  # user-scoping is not evidence of anything and must not be counted.
  #
  # ⚠ NOT `user_id = \$`. That spelling is the WHERE form, and `create_maintenance`
  # names the column in an INSERT column list instead — so the first draft of this
  # arm censused 3 of 4 and reported the extractor broken. Membership is "the SQL
  # names user_id AND the handler binds its own caller", which both forms satisfy.
  { has "$B" 'user_id' && has "$B" 'claims\.sub'; } || continue
  E_N=$((E_N+1))
  has "$B" 'require_admin' && E_VIOL=$((E_VIOL+1)) && printf '%s\n' "$fn" >> /tmp/.s324_e_viol
done < <(printf '%s\n' certificate_dashboard create_maintenance list_maintenance delete_maintenance)

if [ "$E_N" -lt 4 ]; then
  bad "E0 censused only $E_N of 4 user-scoped monitors handlers — the extractor is broken, not the tree"
elif [ "$E_VIOL" -eq 0 ]; then
  ok "E1 all $E_N user-scoped monitors handlers answer their own caller"
else
  while read -r fn; do
    [ -n "$fn" ] || continue
    bad "E1 monitors::$fn gates with require_admin over SQL already scoped WHERE user_id = \$1 — it refuses a caller their own rows"
  done < <(cat /tmp/.s324_e_viol 2>/dev/null)
fi
rm -f /tmp/.s324_e_viol

# ── §F  a reseller may act on every account its own table lists ───────────────

RES_SRC=$(code "$RESELLER")
RES_FLAT=$(flat "$RES_SRC")

if has "$RES_FLAT" "reseller_id = \\\$2 AND role = 'user'"; then
  bad "F1 reseller update/delete still scope AND role = 'user' — an account moved to client or suspended stays listed and answers 'User not found'"
elif has "$RES_FLAT" 'reseller_id = \$2 AND role = ANY\(\$3\)'; then
  ok "F1 reseller update/delete resolve against the shared manageable-role list"
else
  bad "F1 the reseller predicate matches neither the old shape nor the pinned one — re-read it"
fi

# F2 — an ALLOW-list, and privileged roles must be absent from it. A future
# `role <> 'admin'` here would invert the property the moment a new role lands.
# Same restart trap as C3, and it bit harder here: the constant is NAMED at both
# `.bind(RESELLER_MANAGEABLE_ROLES)` call sites, each of which reopened the range
# into surrounding code containing `claims.role == "admin"` — so the arm reported
# that the allow-list names a privileged role when the declaration names three
# unprivileged ones. First block only, and it must be the `const` line.
ALLOW=$(awk '/const RESELLER_MANAGEABLE_ROLES/ { inblk=1 } inblk { print; if (/;/) exit }' <<< "$RES_SRC" | tr '\n' ' ')
if [ -z "$ALLOW" ]; then
  bad "F2 RESELLER_MANAGEABLE_ROLES not found"
elif has "$ALLOW" '"admin"' || has "$ALLOW" '"reseller"'; then
  bad "F2 RESELLER_MANAGEABLE_ROLES names a privileged role — a reseller could reset an administrator's password"
elif has "$ALLOW" '"user"' && has "$ALLOW" '"client"'; then
  ok "F2 RESELLER_MANAGEABLE_ROLES admits user + client and no privileged role"
else
  bad "F2 RESELLER_MANAGEABLE_ROLES does not admit both user and client"
fi

# F3 — the two halves must agree. `list_users` scoping on reseller_id alone is
# fine ONLY while the action predicate covers what it lists; this arm is what
# makes them one decision instead of two.
LIST_U=$(flat "$(fnbody "$RES_SRC" "list_users")")
if has "$LIST_U" "role = 'user'"; then
  bad "F3 list_users now filters by role while the action predicate uses an allow-list — the two halves have drifted again, in the other direction"
else
  ok "F3 list_users still lists by reseller_id alone, matching what the action predicate now admits"
fi

# ── §G  a control a role can never use is not shown to it ─────────────────────

for pair in "$SITESTSX:Sites.tsx" "$DETAIL:SiteDetail.tsx"; do
  f=${pair%%:*}; label=${pair##*:}
  SRC=$(code "$f")
  if has "$(flat "$SRC")" 'role === "client"'; then
    ok "G1 $label derives a client predicate from the role"
  else
    bad "G1 $label has no client predicate — the new-domain controls are offered to the one role that can never use them"
  fi
done

# G2 — the create control specifically. Not "the file mentions isClient" — the
# button must be inside the guard.
S_FLAT=$(flat "$(code "$SITESTSX")")
if has "$S_FLAT" '\{!isClient && \( *<button'; then
  ok "G2 the Create Site control is behind the client guard"
else
  bad "G2 the Create Site control is not behind the client guard"
fi

# G3 — the health indicator. A refusal rendered as a fault is the shape this ship
# is about; pin that the admin-only read is not made by everyone.
L_FLAT=$(flat "$(code "$LAYOUT")")
if has "$L_FLAT" 'if \(!isAdmin\) return;'; then
  ok "G3 the admin-only health poll is not issued by a non-admin"
else
  bad "G3 useLayoutState still polls admin-only /settings/health for every role — its 403 renders as a pulsing red 'Disconnected' on every page"
fi

# G4 — 2FA enforcement must come from a read every role can make.
# L_FLAT is ALREADY flattened — and that was not enough, which is the whole of
# lesson #383. Flattening leaves a SPACE at every break point, so the pattern has
# to absorb it too: prettier breaks a 3-call member chain before `.get` (the head
# `api` is longer than tabWidth, so it is not merged onto it) and breaks a long
# call before its argument, adding a trailing comma. Both spellings of this exact
# call already exist in this tree. Gaps BOUNDED (' *'), never '.*'.
if has "$L_FLAT" 'api *\. *get<Record<string, *string>> *\( *"/settings" *,? *\)'; then
  bad "G4 useLayoutState still reads enforcement from admin-only /settings — the banner can only render for admins, who are not who it is for"
elif has "$L_FLAT" 'enforced'; then
  ok "G4 2FA enforcement is read from a per-caller endpoint"
else
  bad "G4 2FA enforcement is read from neither shape — re-read it"
fi

if has "$(flat "$(fnbody "$(code "$AUTH")" "twofa_status")")" 'enforced'; then
  ok "G5 twofa_status serves the enforcement flag it is read from"
else
  bad "G5 twofa_status does not return 'enforced' — G4's reader has no writer"
fi

# ── §I  fleet_overview is one of §B's own named siblings, now widened (s430) ──
# `servers::list` (§B) documented seven sibling reads still resolving a machine
# through `servers.user_id` — `routes/dashboard.rs`'s four fleet aggregations
# among them, though its cited line numbers had drifted by s430. A completeness
# critic re-derived the live one: `fleet_overview` (`GET /api/dashboard/fleet`,
# `AdminUser`-gated) filtered `WHERE s.user_id = $1` — the exact §B bug, on the
# ONE dashboard route whose entire purpose is showing an admin every server.

DASH_SRC=$(code "$DASH")
FLEET=$(flat "$(fnbody "$DASH_SRC" "fleet_overview")")

if [ -z "$FLEET" ]; then
  bad "I0 could not extract dashboard::fleet_overview"
else
  ok "I0 dashboard::fleet_overview extracted"
  if has "$FLEET" 'FROM servers s.*WHERE s\.user_id = \$1'; then
    bad "I1 fleet_overview still filters WHERE s.user_id = \$1 — a second admin sees an empty fleet"
  elif ! has "$FLEET" 'WHERE s\.user_id'; then
    ok "I1 fleet_overview's server query is fleet-wide, not owner-scoped"
  else
    bad "I1 fleet_overview's WHERE clause is neither the old shape nor fleet-wide — re-read it"
  fi
  if has "$FLEET" 'FROM managed_incidents WHERE user_id = \$1'; then
    bad "I2 active_incidents is still scoped to the calling admin, not the whole fleet"
  else
    ok "I2 active_incidents counts fleet-wide, matching the server query's own widening"
  fi
  # Negative control: this route takes ONLY AdminUser (unlike servers::list,
  # which serves both roles and needs the bound `$2 OR` widening) — so a
  # correct fix has no reason to bind claims.sub into either query at all.
  if has "$FLEET" '\.bind\(claims\.sub\)'; then
    bad "I3 fleet_overview still binds claims.sub into a query — the widening is incomplete"
  else
    ok "I3 no query in fleet_overview binds claims.sub — genuinely fleet-wide, not a hidden per-admin filter"
  fi
fi

# ── §J  drift.rs's two admin-only reads are fleet-wide too (s432) ────────────
# `servers::list` (§B) documented seven sibling reads still resolving a
# machine through `servers.user_id`. `fleet_overview` closed one (§I, s430).
# `routes/drift.rs`'s `servers()` and `report()` are two more — both
# `AdminUser`-only, same fix shape as `fleet_overview`: drop the filter.

DRIFT_SRC=$(code "$DRIFT")
DSERVERS=$(flat "$(fnbody "$DRIFT_SRC" "servers")")
DREPORT=$(flat "$(fnbody "$DRIFT_SRC" "report")")

if [ -z "$DSERVERS" ]; then
  bad "J0 could not extract drift::servers"
else
  ok "J0 drift::servers extracted"
  if has "$DSERVERS" 'FROM servers WHERE user_id = \$1'; then
    bad "J1 drift::servers still filters WHERE user_id = \$1 — a second admin sees fewer comparable servers than exist"
  elif ! has "$DSERVERS" 'WHERE user_id'; then
    ok "J1 drift::servers's query is fleet-wide, not owner-scoped"
  else
    bad "J1 drift::servers's WHERE clause is neither the old shape nor fleet-wide — re-read it"
  fi
fi

if [ -z "$DREPORT" ]; then
  bad "J2 could not extract drift::report"
else
  ok "J2 drift::report extracted"
  if has "$DREPORT" 'FROM servers WHERE user_id = \$1'; then
    bad "J3 drift::report's candidate-server query still filters WHERE user_id = \$1"
  elif ! has "$DREPORT" 'WHERE user_id'; then
    ok "J3 drift::report's candidate-server query is fleet-wide"
  else
    bad "J3 drift::report's candidate-server query matches neither shape — re-read it"
  fi
  # J4 — negative control, same reasoning as I3: an AdminUser-only route has
  # no reason to bind a caller id into a query at all.
  if has "$DREPORT" '\.bind\(claims\.sub\)' || has "$DREPORT" '\.bind\(user_id\)'; then
    bad "J4 drift::report still binds a caller id into a query — the widening is incomplete"
  else
    ok "J4 no query in drift::report binds a caller id — genuinely fleet-wide"
  fi
fi

# ── §K  services/drift.rs's report fetchers are fleet-wide too (s432) ────────
# The one remaining logical sibling from §B's original seven: `build_report`
# and its five DB fetchers, reachable ONLY from `routes/drift.rs::report`
# (§J), which is `AdminUser`-gated — so there is no narrower case to preserve.

DRIFT_SVC_SRC=$(code "$DRIFT_SVC")
BUILD_REPORT=$(flat "$(fnbody "$DRIFT_SVC_SRC" "build_report")")

if [ -z "$BUILD_REPORT" ]; then
  bad "K0 could not extract services::drift::build_report"
else
  ok "K0 services::drift::build_report extracted"
  if has "$BUILD_REPORT" 'user_id: *Uuid'; then
    bad "K1 build_report still takes a user_id parameter — a caller-scoped id has no reason to exist on an AdminUser-only call chain"
  else
    ok "K1 build_report no longer takes a user_id parameter"
  fi
fi

K_VIOL=0; K_N=0
while read -r fn; do
  [ -n "$fn" ] || continue
  B=$(flat "$(fnbody "$DRIFT_SVC_SRC" "$fn")")
  [ -n "$B" ] || continue
  K_N=$((K_N+1))
  # Placeholder-agnostic: a caller-id filter re-added as $2 (or any other
  # position) must still be caught, not just the $1 shape these 5 fetchers
  # happen to use today. A JOIN condition comparing two COLUMNS (e.g.
  # `s.user_id = ar.user_id`, fetch_alert_rules's own fix below) has no `$`
  # and so is correctly NOT a violation — only a comparison against a bound
  # query parameter is.
  if has "$B" 'user_id *= *\$[0-9]'; then
    K_VIOL=$((K_VIOL+1))
    printf '%s\n' "$fn" >> /tmp/.s432_k_viol
  fi
done < <(printf '%s\n' fetch_server_meta fetch_alert_rules fetch_sites fetch_crons fetch_backup_coverage)

if [ "$K_N" -ne 5 ]; then
  bad "K2 censused only $K_N of 5 drift report fetchers — the extractor is broken, not the tree"
elif [ "$K_VIOL" -eq 0 ]; then
  ok "K2 all 5 drift report fetchers query by server id alone, no owner filter"
else
  while read -r fn; do
    [ -n "$fn" ] || continue
    bad "K2 services::drift::$fn still filters by user_id — a second admin's drift report silently drops rows for servers it didn't register"
  done < <(cat /tmp/.s432_k_viol 2>/dev/null)
fi
rm -f /tmp/.s432_k_viol

# ── §L  three more dashboard.rs fleet aggregations, widened for admins (s432) ─
# `metrics_history`, `gpu_metrics_history`, and `timeline` are the three
# remaining §B siblings alongside `fleet_overview` (§I) — all four are what
# the original comment called "the four fleet aggregations in
# routes/dashboard.rs". Unlike `fleet_overview`, these three serve every
# role, so the fix is `dashboard::intelligence`'s own conditional-widen idiom
# (a bound `$N::uuid IS NULL OR` predicate) rather than an unconditional drop.

MHIST=$(flat "$(fnbody "$DASH_SRC" "metrics_history")")

if [ -z "$MHIST" ]; then
  bad "L0 could not extract dashboard::metrics_history"
else
  ok "L0 dashboard::metrics_history extracted"
  if has "$MHIST" 'servers WHERE user_id = \$1\)'; then
    bad "L1 metrics_history still filters WHERE user_id = \$1 — a second admin's chart is empty for every server they didn't register"
  elif has "$MHIST" 'servers WHERE \$1::uuid IS NULL OR user_id = \$1\)'; then
    ok "L1 metrics_history admits an admin to fleet-wide history"
  else
    bad "L1 metrics_history's server subquery matches neither shape — re-read it"
  fi
  if has "$MHIST" 'claims\.role == "admin"'; then
    ok "L2 metrics_history's tenant scope is bound to the admin role"
  else
    bad "L2 metrics_history widened without binding the condition to the admin role"
  fi
  if has "$MHIST" '\.bind\(claims\.sub\)'; then
    bad "L2b metrics_history still binds claims.sub directly — the widening is incomplete"
  else
    ok "L2b metrics_history binds only the derived tenant value, not claims.sub directly"
  fi
fi

GHIST=$(flat "$(fnbody "$DASH_SRC" "gpu_metrics_history")")

if [ -z "$GHIST" ]; then
  bad "L3 could not extract dashboard::gpu_metrics_history"
else
  ok "L3 dashboard::gpu_metrics_history extracted"
  if has "$GHIST" 'servers WHERE user_id = \$1\)'; then
    bad "L4 gpu_metrics_history still filters WHERE user_id = \$1"
  elif has "$GHIST" 'servers WHERE \$1::uuid IS NULL OR user_id = \$1\)'; then
    ok "L4 gpu_metrics_history admits an admin to fleet-wide history"
  else
    bad "L4 gpu_metrics_history's server subquery matches neither shape — re-read it"
  fi
  if has "$GHIST" '\.bind\(claims\.sub\)'; then
    bad "L4b gpu_metrics_history still binds claims.sub directly — the widening is incomplete"
  else
    ok "L4b gpu_metrics_history binds only the derived tenant value, not claims.sub directly"
  fi
fi

TIMELINE=$(flat "$(fnbody "$DASH_SRC" "timeline")")

if [ -z "$TIMELINE" ]; then
  bad "L5 could not extract dashboard::timeline"
else
  ok "L5 dashboard::timeline extracted"
  if has "$TIMELINE" 'claims\.role == "admin"'; then
    ok "L6 timeline's tenant scope is bound to the admin role"
  else
    bad "L6 timeline widened without binding the condition to the admin role"
  fi
  # L7 — every one of the five sub-queries' old unconditional shape must be gone.
  if has "$TIMELINE" 's\.user_id = \$1 ORDER|WHERE user_id = \$1 ORDER BY created_at DESC LIMIT 10|servers WHERE user_id = \$1\)'; then
    bad "L7 timeline still has an unconditional user_id = \$1 predicate on at least one sub-query — a second admin's activity feed is missing that source's fleet-wide events"
  else
    ok "L7 no sub-query in timeline has the old unconditional owner-only predicate"
  fi
  # L8 — the widened shape must actually be present, not just absent-by-some-
  # unrelated-rewrite: count the IS NULL escape hatch. Five sub-queries, one
  # (alerts) uses it twice (its own predicate + the servers subquery it joins
  # against) — six occurrences total, an exact count so a partial fix reds too.
  L_WIDENS=$(grep -o '\$1::uuid IS NULL' <<< "$TIMELINE" | wc -l)
  if [ "$L_WIDENS" -eq 6 ]; then
    ok "L8 timeline's widened predicate appears exactly 6 times — all five sub-queries fixed (alerts uses it twice)"
  else
    bad "L8 timeline's widened predicate appears $L_WIDENS times, expected 6 — at least one sub-query was not fixed"
  fi
  # L9 — negative control, same reasoning as I3/J4: only the derived `tenant`
  # value may be bound now, never claims.sub directly.
  if has "$TIMELINE" '\.bind\(claims\.sub\)'; then
    bad "L9 timeline still binds claims.sub directly into a query — the widening is incomplete"
  else
    ok "L9 timeline binds only the derived tenant value, not claims.sub directly"
  fi
fi

# ── §M  alerts.rs's per-server write routes check ownership before acting (s433) ─
# `update_server_rules`/`delete_server_rules` take `server_id` from the URL
# with no check that the caller owns it — the opposite defect from §B/§I/§J/
# §K/§L (those widened WHO sees fleet-wide data; this is a caller acting on a
# server_id entirely outside its own scope). `upsert_rules`'s own existence
# lookup (`WHERE user_id = $1 AND server_id = $2`) meant a non-owner's write
# never touched the real owner's row — it silently INSERTed a stray second
# row for that server_id instead (`services/drift.rs::fetch_alert_rules` had
# to defend against exactly this stray row, §K, s432).

ALERTS_SRC=$(code "$ALERTS")
USRV=$(flat "$(fnbody "$ALERTS_SRC" "update_server_rules")")

if [ -z "$USRV" ]; then
  bad "M0 could not extract alerts::update_server_rules"
else
  ok "M0 alerts::update_server_rules extracted"
  if has "$USRV" 'FROM servers WHERE id = \$1 AND user_id = \$2'; then
    ok "M1 update_server_rules checks the caller owns server_id"
  else
    bad "M1 update_server_rules has no ownership check on server_id — any authenticated caller can write alert rules for a server they don't own"
  fi
  # M2 — ORDER. The ownership check must precede the write, or a stray row
  # can still be created before the check ever runs.
  CHK_AT=$(grep -bo 'FROM servers WHERE id = \$1 AND user_id = \$2' <<< "$USRV" | head -1 | cut -d: -f1)
  UPS_AT=$(grep -bo 'upsert_rules(&state' <<< "$USRV" | head -1 | cut -d: -f1)
  if [ -n "$CHK_AT" ] && [ -n "$UPS_AT" ] && [ "$CHK_AT" -lt "$UPS_AT" ]; then
    ok "M2 the ownership check precedes the write"
  else
    bad "M2 the ownership check does not precede the write (check@${CHK_AT:-none} write@${UPS_AT:-none})"
  fi
  if has "$USRV" 'owns\.is_none\(\)' && has "$USRV" 'NOT_FOUND'; then
    ok "M3 a non-owned/nonexistent server_id is rejected, not silently accepted"
  else
    bad "M3 update_server_rules performs the ownership lookup but never rejects on failure"
  fi
  # M2b — BIND ORDER for the ownership query specifically ($1=id must bind
  # server_id, $2=user_id must bind claims.sub). Swapping the two .bind()
  # calls leaves the pinned SQL string and M1/M2/M3 all unchanged — the
  # query would compare $1 (id) against the CALLER's own id and $2 (user_id)
  # against the TARGET server_id, which is not the check this claims to be.
  BS_AT=$(grep -boF -- '.bind(server_id)' <<< "$USRV" | head -1 | cut -d: -f1)
  BC_AT=$(grep -boF -- '.bind(claims.sub)' <<< "$USRV" | head -1 | cut -d: -f1)
  if [ -n "$BS_AT" ] && [ -n "$BC_AT" ] && [ "$BS_AT" -lt "$BC_AT" ]; then
    ok "M2b bind order is server_id then claims.sub, matching \$1=id \$2=user_id"
  else
    bad "M2b bind order is wrong or missing (server_id@${BS_AT:-none} claims.sub@${BC_AT:-none}) — \$1/\$2 would compare the wrong columns"
  fi
  # M2c — LIVENESS. A text-presence-and-ordering check (M1-M3, M2b) cannot
  # tell live code from the identical text sitting inside a dead branch —
  # `if false { <the whole ownership check> }` leaves every arm above green
  # while the write two lines later runs completely unguarded. The wrapper's
  # extra `{` pushes the check to depth 2; correct code has it at depth 1
  # (a top-level statement, directly inside the function body).
  D_CHK=$(depth_before "$USRV" 'FROM servers WHERE id = $1 AND user_id = $2')
  if [ "$D_CHK" = "1" ]; then
    ok "M2c the ownership check is live (depth 1 — not wrapped in a dead branch)"
  else
    bad "M2c the ownership check is at depth $D_CHK, not 1 — it may be wrapped in unreachable code (e.g. 'if false { ... }') while the write after it runs unconditionally"
  fi
fi

DSRV=$(flat "$(fnbody "$ALERTS_SRC" "delete_server_rules")")

if [ -z "$DSRV" ]; then
  bad "M4 could not extract alerts::delete_server_rules"
else
  ok "M4 alerts::delete_server_rules extracted"
  if has "$DSRV" 'FROM servers WHERE id = \$1 AND user_id = \$2'; then
    ok "M5 delete_server_rules checks the caller owns server_id"
  else
    bad "M5 delete_server_rules has no ownership check on server_id — a request naming a server the caller doesn't own silently no-ops and still answers ok:true"
  fi
  CHK_AT2=$(grep -bo 'FROM servers WHERE id = \$1 AND user_id = \$2' <<< "$DSRV" | head -1 | cut -d: -f1)
  DEL_AT2=$(grep -bo 'DELETE FROM alert_rules' <<< "$DSRV" | head -1 | cut -d: -f1)
  if [ -n "$CHK_AT2" ] && [ -n "$DEL_AT2" ] && [ "$CHK_AT2" -lt "$DEL_AT2" ]; then
    ok "M6 the ownership check precedes the delete"
  else
    bad "M6 the ownership check does not precede the delete (check@${CHK_AT2:-none} delete@${DEL_AT2:-none})"
  fi
  if has "$DSRV" 'owns\.is_none\(\)' && has "$DSRV" 'NOT_FOUND'; then
    ok "M7 a non-owned/nonexistent server_id is rejected, not silently answered ok:true"
  else
    bad "M7 delete_server_rules performs the ownership lookup but never rejects on failure"
  fi
  # M6b/M6c — same bind-order and liveness checks as M2b/M2c above, applied
  # to delete_server_rules's own ownership check.
  BS_AT2=$(grep -boF -- '.bind(server_id)' <<< "$DSRV" | head -1 | cut -d: -f1)
  BC_AT2=$(grep -boF -- '.bind(claims.sub)' <<< "$DSRV" | head -1 | cut -d: -f1)
  if [ -n "$BS_AT2" ] && [ -n "$BC_AT2" ] && [ "$BS_AT2" -lt "$BC_AT2" ]; then
    ok "M6b bind order is server_id then claims.sub, matching \$1=id \$2=user_id"
  else
    bad "M6b bind order is wrong or missing (server_id@${BS_AT2:-none} claims.sub@${BC_AT2:-none}) — \$1/\$2 would compare the wrong columns"
  fi
  D_CHK2=$(depth_before "$DSRV" 'FROM servers WHERE id = $1 AND user_id = $2')
  if [ "$D_CHK2" = "1" ]; then
    ok "M6c the ownership check is live (depth 1 — not wrapped in a dead branch)"
  else
    bad "M6c the ownership check is at depth $D_CHK2, not 1 — it may be wrapped in unreachable code (e.g. 'if false { ... }') while the delete after it runs unconditionally"
  fi
fi

# M8 — negative control: the GLOBAL list route (no server_id in the URL) is
# unaffected and stays scoped to the caller's own rows — this fix must not
# have widened or narrowed it.
GRULES=$(flat "$(fnbody "$ALERTS_SRC" "get_rules")")
if has "$GRULES" 'WHERE user_id = \$1 ORDER BY server_id'; then
  ok "M8 alerts::get_rules (the global list) is unchanged — still scoped to the caller's own rows"
else
  bad "M8 alerts::get_rules no longer matches its known shape — re-read it, this fix should not have touched it"
fi

# ── §N  on_call.rs: on_call_schedules carried ZERO tenant scoping (s437) ──────
# Every route here gated on AdminUser (role) alone — any admin on the install
# could read/write/delete every OTHER tenant's on-call rotations. Same shape
# as §M: a single resource resolved by a caller-supplied ID never widens for
# admin, admin included.

ON_CALL=panel/backend/src/routes/on_call.rs
[ -f "$ON_CALL" ] || bad "SETUP subject missing: $ON_CALL"
ON_CALL_SRC=$(code "$ON_CALL")

NLIST=$(flat "$(fnbody "$ON_CALL_SRC" "list_schedules")")
if has "$NLIST" 'FROM on_call_schedules WHERE user_id = \$1 ORDER BY name ASC' && has "$NLIST" '\.bind\(claims\.sub\)'; then
  ok "N0 on_call::list_schedules is scoped to the caller's own rows"
else
  bad "N0 on_call::list_schedules no longer matches its known scoped shape"
fi

NGET=$(flat "$(fnbody "$ON_CALL_SRC" "get_schedule")")
if has "$NGET" 'FROM on_call_schedules WHERE id = \$1 AND user_id = \$2'; then
  ok "N1 on_call::get_schedule checks the caller owns the schedule"
else
  bad "N1 on_call::get_schedule has no ownership check — any admin could fetch any tenant's schedule"
fi
BID_AT=$(grep -boF -- '.bind(id)' <<< "$NGET" | head -1 | cut -d: -f1)
BCS_AT=$(grep -boF -- '.bind(claims.sub)' <<< "$NGET" | head -1 | cut -d: -f1)
if [ -n "$BID_AT" ] && [ -n "$BCS_AT" ] && [ "$BID_AT" -lt "$BCS_AT" ]; then
  ok "N1b bind order is id then claims.sub, matching \$1=id \$2=user_id"
else
  bad "N1b bind order is wrong or missing (id@${BID_AT:-none} claims.sub@${BCS_AT:-none})"
fi

NCREATE=$(flat "$(fnbody "$ON_CALL_SRC" "create_schedule")")
if has "$NCREATE" 'INSERT INTO on_call_schedules \(user_id, name, members, cadence_days, anchor_at\)' \
   && has "$NCREATE" '\.bind\(claims\.sub\)\s*\.bind\(input\.name\.trim\(\)\)'; then
  ok "N2 on_call::create_schedule stamps the creating admin's own user_id, bound first"
else
  bad "N2 on_call::create_schedule no longer inserts/binds user_id as \$1"
fi

NUPDATE=$(flat "$(fnbody "$ON_CALL_SRC" "update_schedule")")
if has "$NUPDATE" 'WHERE id = \$1 AND user_id = \$6' && has "$NUPDATE" 'WHERE id = \$1 AND user_id = \$5'; then
  ok "N3 on_call::update_schedule scopes BOTH the with-anchor and no-anchor branches"
else
  bad "N3 on_call::update_schedule is missing the ownership scope on one or both branches"
fi
CS_COUNT=$(grep -o '\.bind(claims\.sub)' <<< "$NUPDATE" | wc -l)
if [ "$CS_COUNT" -eq 2 ]; then
  ok "N3b claims.sub is bound in BOTH update_schedule branches (found $CS_COUNT)"
else
  bad "N3b expected claims.sub bound exactly twice in update_schedule, found $CS_COUNT — a branch may have lost its scope"
fi

NDELETE=$(flat "$(fnbody "$ON_CALL_SRC" "delete_schedule")")
if has "$NDELETE" 'DELETE FROM on_call_schedules WHERE id = \$1 AND user_id = \$2'; then
  ok "N4 on_call::delete_schedule checks the caller owns the schedule before deleting"
else
  bad "N4 on_call::delete_schedule has no ownership check on the delete itself"
fi
if has "$NDELETE" 'FROM escalation_policies WHERE user_id = \$1 FOR UPDATE'; then
  ok "N5 the orphan-route sweep is scoped to the SAME tenant as the deleted schedule"
else
  bad "N5 the orphan-route sweep scans every tenant's escalation_policies — a delete could rewrite another tenant's policy"
fi
BID2_AT=$(grep -boF -- '.bind(id)' <<< "$NDELETE" | head -1 | cut -d: -f1)
BCS2_AT=$(grep -boF -- '.bind(claims.sub)' <<< "$NDELETE" | head -1 | cut -d: -f1)
if [ -n "$BID2_AT" ] && [ -n "$BCS2_AT" ] && [ "$BID2_AT" -lt "$BCS2_AT" ]; then
  ok "N4b bind order is id then claims.sub for the delete's ownership check"
else
  bad "N4b bind order is wrong or missing (id@${BID2_AT:-none} claims.sub@${BCS2_AT:-none})"
fi

# N6 — negative control: `whoami` stays intentionally UNSCOPED (a caller
# checking their OWN membership, not reading another tenant's rotation
# layout) — this fix must not have widened it into a leak, or narrowed it
# into a false negative for a legitimate on-call operator.
NWHOAMI=$(flat "$(fnbody "$ON_CALL_SRC" "whoami")")
if has "$NWHOAMI" 'SELECT id, name, members, cadence_days, anchor_at FROM on_call_schedules "' \
   || has "$NWHOAMI" "SELECT id, name, members, cadence_days, anchor_at FROM on_call_schedules"; then
  if has "$NWHOAMI" 'FROM on_call_schedules WHERE user_id'; then
    bad "N6 on_call::whoami now scopes by user_id — it must scan every tenant's schedules to answer 'am I on call anywhere', not just the caller's own"
  else
    ok "N6 on_call::whoami is unchanged — still scans every schedule, matching self-membership semantics"
  fi
else
  bad "N6 on_call::whoami no longer matches its known shape — re-read it, this fix should not have touched it"
fi

# ── §O  escalation_policies.rs: same ZERO-scoping defect, same fix shape (s437) ─

ESCALATION=panel/backend/src/routes/escalation_policies.rs
[ -f "$ESCALATION" ] || bad "SETUP subject missing: $ESCALATION"
ESCALATION_SRC=$(code "$ESCALATION")

OLIST=$(flat "$(fnbody "$ESCALATION_SRC" "list_policies")")
if has "$OLIST" 'FROM escalation_policies p WHERE p\.user_id = \$1 ORDER BY p\.name ASC'; then
  ok "O0 escalation_policies::list_policies is scoped to the caller's own rows"
else
  bad "O0 escalation_policies::list_policies no longer matches its known scoped shape"
fi

OGET=$(flat "$(fnbody "$ESCALATION_SRC" "get_policy")")
if has "$OGET" 'FROM escalation_policies p WHERE p\.id = \$1 AND p\.user_id = \$2'; then
  ok "O1 escalation_policies::get_policy checks the caller owns the policy"
else
  bad "O1 escalation_policies::get_policy has no ownership check"
fi

OCREATE=$(flat "$(fnbody "$ESCALATION_SRC" "create_policy")")
if has "$OCREATE" 'INSERT INTO escalation_policies \(user_id, name, steps\)' \
   && has "$OCREATE" 'validate_schedule_routes\(&state\.db, &input, claims\.sub\)'; then
  ok "O2 escalation_policies::create_policy stamps user_id AND checks schedule-route ownership"
else
  bad "O2 escalation_policies::create_policy no longer inserts user_id or passes claims.sub to validate_schedule_routes"
fi

OUPDATE=$(flat "$(fnbody "$ESCALATION_SRC" "update_policy")")
if has "$OUPDATE" 'WHERE id = \$1 AND user_id = \$4' \
   && has "$OUPDATE" 'validate_schedule_routes\(&state\.db, &input, claims\.sub\)'; then
  ok "O3 escalation_policies::update_policy is scoped AND checks schedule-route ownership"
else
  bad "O3 escalation_policies::update_policy is missing its ownership scope or the schedule-route check"
fi

ODELETE=$(flat "$(fnbody "$ESCALATION_SRC" "delete_policy")")
if has "$ODELETE" 'DELETE FROM escalation_policies WHERE id = \$1 AND user_id = \$2'; then
  ok "O4 escalation_policies::delete_policy checks the caller owns the policy before deleting"
else
  bad "O4 escalation_policies::delete_policy has no ownership check on the delete itself"
fi

# O5 — the Finding-3 closer: a policy's `on_call_schedule:<uuid>` step must
# not be storable pointing at a schedule owned by a DIFFERENT tenant. Before
# s437 this only checked EXISTENCE, so a policy could legitimately reference
# — and therefore page — another tenant's on-call rotation.
OVALIDATE=$(flat "$(fnbody "$ESCALATION_SRC" "validate_schedule_routes")")
if has "$OVALIDATE" 'FROM on_call_schedules WHERE id = \$1 AND user_id = \$2'; then
  ok "O5 validate_schedule_routes checks the referenced schedule is owned by the SAME tenant, not just that it exists"
else
  bad "O5 validate_schedule_routes only checks existence — a policy could still reference another tenant's schedule"
fi
BSID_AT=$(grep -boF -- '.bind(schedule_id)' <<< "$OVALIDATE" | head -1 | cut -d: -f1)
BOWN_AT=$(grep -boF -- '.bind(owner_id)' <<< "$OVALIDATE" | head -1 | cut -d: -f1)
if [ -n "$BSID_AT" ] && [ -n "$BOWN_AT" ] && [ "$BSID_AT" -lt "$BOWN_AT" ]; then
  ok "O5b bind order is schedule_id then owner_id, matching \$1=id \$2=user_id"
else
  bad "O5b bind order is wrong or missing (schedule_id@${BSID_AT:-none} owner_id@${BOWN_AT:-none})"
fi

# ── §P  alerts.rs::attach_escalation_policy had NO ownership check at all (s437) ─
# The one write path that COUPLES alert_rules to escalation_policies took
# rule_id from the URL under plain AdminUser (role only) — any admin could
# re-point ANY tenant's alert rule at ANY policy. Same check-then-act shape
# §M proved defeatable two ways (dead-code wrap, bind-order swap), so this
# gets the same full treatment: liveness AND bind-order, not just presence.

PATTACH=$(flat "$(fnbody "$ALERTS_SRC" "attach_escalation_policy")")

if [ -z "$PATTACH" ]; then
  bad "P0 could not extract alerts::attach_escalation_policy"
else
  ok "P0 alerts::attach_escalation_policy extracted"

  if has "$PATTACH" 'FROM alert_rules WHERE id = \$1 AND user_id = \$2'; then
    ok "P1 attach_escalation_policy checks the caller owns rule_id"
  else
    bad "P1 attach_escalation_policy has no ownership check on rule_id — any admin can re-point any tenant's alert rule"
  fi
  if has "$PATTACH" '\.bind\(rule_id\)\s*\.bind\(claims\.sub\)'; then
    ok "P1b bind order is rule_id then claims.sub for the rule-ownership check"
  else
    bad "P1b the rule-ownership check's bind order is wrong or missing"
  fi
  D_RULE=$(depth_before "$PATTACH" 'FROM alert_rules WHERE id = $1 AND user_id = $2')
  if [ "$D_RULE" = "1" ]; then
    ok "P1c the rule-ownership check is live (depth 1 — not wrapped in a dead branch)"
  else
    bad "P1c the rule-ownership check is at depth $D_RULE, not 1 — may be wrapped in unreachable code"
  fi
  if has "$PATTACH" 'owns_rule\.is_none\(\)' && has "$PATTACH" 'NOT_FOUND'; then
    ok "P2 a non-owned/nonexistent rule_id is rejected"
  else
    bad "P2 attach_escalation_policy performs the rule-ownership lookup but never rejects on failure"
  fi

  if has "$PATTACH" 'FROM escalation_policies WHERE id = \$1 AND user_id = \$2'; then
    ok "P3 attach_escalation_policy checks the target policy is owned by the SAME tenant, not just that it exists"
  else
    bad "P3 the policy-existence check is unscoped — a rule could still be attached to another tenant's policy"
  fi
  if has "$PATTACH" '\.bind\(pid\)\s*\.bind\(claims\.sub\)'; then
    ok "P3b bind order is pid then claims.sub for the policy-ownership check"
  else
    bad "P3b the policy-ownership check's bind order is wrong or missing"
  fi

  if has "$PATTACH" 'UPDATE alert_rules SET escalation_policy_id' && has "$PATTACH" 'WHERE id = \$1 AND user_id = \$3'; then
    ok "P4 the final UPDATE itself is ALSO scoped to the caller's own tenant (defense-in-depth)"
  else
    bad "P4 the final UPDATE lost its own ownership scope"
  fi

  CHK_R_AT=$(grep -bo 'FROM alert_rules WHERE id = \$1 AND user_id = \$2' <<< "$PATTACH" | head -1 | cut -d: -f1)
  CHK_P_AT=$(grep -bo 'FROM escalation_policies WHERE id = \$1 AND user_id = \$2' <<< "$PATTACH" | head -1 | cut -d: -f1)
  UPD_AT=$(grep -bo 'UPDATE alert_rules SET escalation_policy_id' <<< "$PATTACH" | head -1 | cut -d: -f1)
  if [ -n "$CHK_R_AT" ] && [ -n "$CHK_P_AT" ] && [ -n "$UPD_AT" ] \
     && [ "$CHK_R_AT" -lt "$CHK_P_AT" ] && [ "$CHK_P_AT" -lt "$UPD_AT" ]; then
    ok "P5 both ownership checks precede the write, in order: rule, then policy, then update"
  else
    bad "P5 the checks are missing or out of order (rule@${CHK_R_AT:-none} policy@${CHK_P_AT:-none} update@${UPD_AT:-none})"
  fi
fi

# ── §Q  schema backstop: the migration actually adds and enforces both columns (s437) ─

if [ "$(grep -c 'ADD COLUMN IF NOT EXISTS user_id UUID REFERENCES users(id) ON DELETE CASCADE' <<< "$MIG")" -eq 2 ]; then
  ok "Q0 exactly 2 tables gain a user_id column via the s437 migration (escalation_policies + on_call_schedules)"
else
  bad "Q0 expected exactly 2 ADD COLUMN user_id lines from the s437 migration, found a different count"
fi
if grep -qF 'ALTER TABLE escalation_policies ALTER COLUMN user_id SET NOT NULL' <<< "$MIG"; then
  ok "Q1 escalation_policies.user_id is enforced NOT NULL (guarded on a clean backfill)"
else
  bad "Q1 escalation_policies.user_id is never set NOT NULL — a future row could ship ownerless"
fi
if grep -qF 'ALTER TABLE on_call_schedules ALTER COLUMN user_id SET NOT NULL' <<< "$MIG"; then
  ok "Q2 on_call_schedules.user_id is enforced NOT NULL (guarded on a clean backfill)"
else
  bad "Q2 on_call_schedules.user_id is never set NOT NULL — a future row could ship ownerless"
fi
if grep -qF 'idx_escalation_policies_user' <<< "$MIG" && grep -qF 'idx_on_call_schedules_user' <<< "$MIG"; then
  ok "Q3 both new user_id columns are indexed"
else
  bad "Q3 one or both new user_id columns are missing their index"
fi

# ── §H  context: green at BOTH tags ───────────────────────────────────────────
# Without these, a harness that has stopped reading real bytes prints an all-green
# §A-§G and looks exactly like a clean tree.

if [ "$(grep -c 'pub async fn ' "$SERVERS")" -ge 3 ]; then
  ok "H1 servers.rs still parses as a route module (control)"
else
  bad "H1 servers.rs yielded almost no handlers — the reader is broken"
fi

if has "$(flat "$(code "$SITES")")" 'OWNERSHIP_DENORMALIZED_TABLES'; then
  ok "H2 the transfer constant is still present (control)"
else
  bad "H2 OWNERSHIP_DENORMALIZED_TABLES is gone — §C is measuring nothing"
fi

if [ "${#MIG}" -gt 10000 ]; then
  ok "H3 the migration corpus read $((${#MIG}/1024))KB (control — A5 and C3 are non-vacuous)"
else
  bad "H3 the migration corpus is implausibly small (${#MIG} bytes) — A5 and C3 prove nothing"
fi

echo
echo "----------------------------------------------"
printf '  PASS %d  FAIL %d  SKIP %d\n' "$PASS" "$FAIL" "$SKIP"
echo "----------------------------------------------"
echo

[ "$FAIL" -eq 0 ] || exit 1
exit 0
