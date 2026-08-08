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
FILTER=panel/agent/src/services/command_filter.rs
MONITORS=panel/backend/src/routes/monitors.rs
RESELLER=panel/backend/src/routes/reseller_dashboard.rs
AUTH=panel/backend/src/routes/auth.rs
LAYOUT=panel/frontend/src/hooks/useLayoutState.ts
SITESTSX=panel/frontend/src/pages/Sites.tsx
DETAIL=panel/frontend/src/pages/SiteDetail.tsx

for f in "$USERS" "$SERVERS" "$SITES" "$FILTER" "$MONITORS" "$RESELLER" "$AUTH" \
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
if has "$L_FLAT" 'api\.get<Record<string, string>>\("/settings"\)'; then
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
