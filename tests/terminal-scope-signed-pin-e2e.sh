#!/usr/bin/env bash
# terminal-scope-signed-pin-e2e.sh — s317 / v2.75.0
#
# THE SENTENCE THIS SUITE EXISTS TO PROVE, written out so a mutation can be aimed
# at it rather than at whatever the arms happen to catch:
#
#   "The shell a terminal ticket opens is decided by the ownership check the
#    panel performed when it minted the ticket — carried inside the signature —
#    not by a parameter the browser sends to the agent."
#
# THE DEFECT. The panel resolved a site's domain under an ownership predicate and
# returned it BESIDE the signed ticket, while the agent read the domain from the
# `?domain=` query string. An empty domain there opens a root shell with no
# privilege drop (no setuid to www-data, no restricted shell). So a non-admin who
# owned a single site minted an ownership-checked, site-scoped ticket, then dialled
# the agent with the domain omitted and got an unrestricted root shell on the host.
# The panel's "Admin access required for server terminal" check only fires when the
# ticket is minted with no site at all — it never travelled to the agent.
#
# So the mutations this suite must survive are WIDENING mutations — move the scope
# back out of the signature, or make the agent trust the query param again — not
# deletions. Every arm below was mutation-tested to redden ALONE.
#
#   §A  the panel binds the scope INTO the signed ticket
#   §B  the agent takes the scope FROM the ticket, not the query param
#   §C  the two ends agree on the server-shell marker
#
# Pure source analysis: no box, no network, no build.
#
# NO PIPES INTO `grep -q`. Under `set -o pipefail` grep -q closes the pipe on its
# first match, the upstream dies of SIGPIPE (141), and pipefail reports the whole
# pipeline failed — so an arm goes red on correct code, non-deterministically.
# Every arm here feeds grep a here-string.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

BE=panel/backend/src/routes/terminal.rs
AG=panel/agent/src/routes/terminal.rs

for f in "$BE" "$AG"; do [ -f "$f" ] || bad "MISSING SUBJECT FILE: $f"; done

# Comments out, CODE INTACT. The `/*` in a string-literal trap and the reason:
# this suite asserts the ABSENCE of a query-param read that the fix's OWN comments
# legitimately discuss.
code() {
  perl -0777 -pe '
    s{\{/\*.*?\*/\}}{}gs;
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
  ' "$1"
}
subj() { local t; t=$(code "$1"); [ -n "$t" ] || return 1; printf '%s' "$t"; }
has()   { grep -qE -- "$2" <<< "$1"; }
count() { grep -cE -- "$2" <<< "$1" || true; }

# One function's body, bounded by its OWN closing brace in column 0.
fn_body() { awk -v pat="$1" '$0 ~ pat {f=1} f {print} f && /^\}/ {exit}' <<< "$2"; }
# Order predicate: does `first` appear before `second` in `body`?
before() {
  local body="$1" first="$2" second="$3" a b
  a=$(grep -nE -- "$first" <<< "$body" | head -1 | cut -d: -f1)
  b=$(grep -nE -- "$second" <<< "$body" | head -1 | cut -d: -f1)
  [ -n "$a" ] && [ -n "$b" ] && [ "$a" -lt "$b" ]
}

BE_S=$(subj "$BE" || true)
AG_S=$(subj "$AG" || true)

echo "§A  the panel binds the scope INTO the signed ticket"

# A1 — the signed struct carries a scope field. The struct is what gets HS256'd,
# so a field here is a field the agent can trust.
if [ -z "$BE_S" ]; then
  bad "A1 SKIPPED: $BE did not yield source"
else
  TICKET=$(fn_body 'struct TerminalTicket' "$BE_S")
  if [ -n "$TICKET" ] && has "$TICKET" 'scope: String'; then
    ok "A1 the signed TerminalTicket carries a scope"
  else
    bad "A1 the signed ticket has no scope field — the shell decision is not in the signature"
  fi
fi

# A2 — and it is set from the ownership-resolved domain, not from anything the
# caller sent. The mint reads `domain` (the SITE_CALLER_PREDICATE result) and
# falls back to the server marker. Keyed on the decision, not the field name.
if [ -z "$BE_S" ]; then
  bad "A2 SKIPPED: $BE did not yield source"
else
  MINT=$(fn_body 'async fn ws_token' "$BE_S")
  # Flatten: the derivation spans several lines and the invariant is the whole
  # expression `scope = domain … unwrap_or_else(… SERVER_SHELL_SCOPE)`.
  MINT_FLAT=$(tr '\n' ' ' <<< "$MINT" | tr -s ' ')
  if [ -n "$MINT" ] \
    && has "$MINT_FLAT" 'let scope = domain \.clone\(\) \.unwrap_or_else\(\|\| SERVER_SHELL_SCOPE'; then
    ok "A2 the scope is set from the ownership-resolved domain, server marker as the only fallback"
  else
    bad "A2 the ticket scope is not derived from the resolved domain — an attacker-supplied value could reach it"
  fi
fi

# A3 — the ownership check still gates the site branch (the scope is only as good
# as the resolution behind it). SITE_CALLER_PREDICATE must still bind the domain.
if [ -z "$BE_S" ]; then
  bad "A3 SKIPPED: $BE did not yield source"
elif has "$BE_S" 'SITE_CALLER_PREDICATE' && has "$BE_S" 'not owned by you'; then
  ok "A3 the site domain is still resolved under the ownership predicate before it becomes the scope"
else
  bad "A3 the ownership resolution behind the scope is gone — the signed scope would certify nothing"
fi

echo "§B  the agent takes the scope FROM the ticket, not the query param"

# B1 — the agent's decode struct carries the scope, so the value it acts on is
# the signed one.
if [ -z "$AG_S" ]; then
  bad "B1 SKIPPED: $AG did not yield source"
else
  ATICKET=$(fn_body 'struct TerminalTicket' "$AG_S")
  if [ -n "$ATICKET" ] && has "$ATICKET" 'scope: Option<String>'; then
    ok "B1 the agent decodes the ticket's scope"
  else
    bad "B1 the agent does not read a scope off the ticket — it can only be trusting the query param"
  fi
fi

# B2 — THE ARM THAT MATTERS. The domain the shell opens under is chosen from the
# TICKET scope, and the query param is used ONLY on the no-scope (legacy) branch.
# A regression that reads `q.domain` up front, before the scope, is the defect.
if [ -z "$AG_S" ]; then
  bad "B2 SKIPPED: $AG did not yield source"
else
  WS=$(fn_body 'async fn ws_handler' "$AG_S")
  if [ -z "$WS" ]; then
    bad "B2 SKIPPED: could not bound ws_handler"
  # The scope match must be reached BEFORE the domain binding, and the domain
  # binding must be the match itself, not a bare `q.domain.clone()` assignment.
  elif before "$WS" 'ticket_scope' 'let domain =' \
    && has "$WS" 'let domain = match ticket_scope'; then
    ok "B2 the shell's domain is chosen from the signed scope; the query param is not read up front"
  else
    bad "B2 the domain is bound from the query param, not the ticket scope — the escalation is restored"
  fi
fi

# B3 — the legacy fallback to the query param is confined to the no-scope branch
# AND announces itself. An unscoped root shell that is silent is how this would
# hide on a mixed-version fleet.
if [ -z "$AG_S" ]; then
  bad "B3 SKIPPED: $AG did not yield source"
else
  WS=$(fn_body 'async fn ws_handler' "$AG_S")
  # Exactly one q.domain read, and it sits after a `None =>` arm with a warn.
  N=$(count "$WS" 'q\.domain\.clone\(\)')
  if [ "$N" = "1" ] && has "$WS" 'None =>' && before "$WS" 'None =>' 'q\.domain\.clone\(\)' && has "$WS" 'tracing::warn'; then
    ok "B3 the query param is read once, only on the no-scope legacy branch, and it is logged"
  else
    bad "B3 the query param is read outside the legacy branch (${N} reads) or an unscoped root shell is silent"
  fi
fi

echo "§C  the two ends agree on the server-shell marker"

# C1 — both files define the SAME marker string. If they drift, an admin server
# shell minted by the panel is treated as a site named '@server' by the agent, or
# worse a site scope is mistaken for the server shell.
BE_MARK=$(grep -oE 'SERVER_SHELL_SCOPE[^;]*"[^"]+"' <<< "$BE_S" | grep -oE '"[^"]+"' | head -1)
AG_MARK=$(grep -oE 'SERVER_SHELL_SCOPE[^;]*"[^"]+"' <<< "$AG_S" | grep -oE '"[^"]+"' | head -1)
if [ -n "$BE_MARK" ] && [ "$BE_MARK" = "$AG_MARK" ]; then
  ok "C1 both ends agree the server-shell marker is $BE_MARK"
else
  bad "C1 the server-shell marker differs: panel=${BE_MARK:-none} agent=${AG_MARK:-none}"
fi

# C2 — the marker is not a legal domain, so it can never collide with a site's
# name. The agent's own domain validation rejects it (contains '@', or '/').
if [ -n "$BE_MARK" ]; then
  m=${BE_MARK//\"/}
  if [[ "$m" == *@* || "$m" == */* ]]; then
    ok "C2 the server-shell marker ($BE_MARK) is not a valid domain, so no site can claim it"
  else
    bad "C2 the server-shell marker ($BE_MARK) could be a real domain — a site of that name would open a root shell"
  fi
else
  bad "C2 SKIPPED: no marker found to check"
fi

# C3 — the agent still drops privilege for a site scope: a non-empty domain must
# reach the setuid path. Guards against a fix that carries the scope but forgets
# to act on it.
if [ -z "$AG_S" ]; then
  bad "C3 SKIPPED: $AG did not yield source"
elif has "$AG_S" 'site_domain\.is_some\(\)' && has "$AG_S" 'setuid'; then
  ok "C3 a site-scoped shell still drops to the site's user"
else
  bad "C3 the privilege drop for a site shell is gone"
fi

# C4 — and the drop must be VERIFIED, not merely attempted.
#
# ⚠ WHY C3 IS NOT ENOUGH, written down because C3 reads as though it were.
# C3 asserts two tokens that merely COEXIST in the file: `site_domain.is_some()`
# and `setuid`. Both survive a tree in which `setuid` FAILS and the child falls
# through to `execv` anyway — which is exactly what this code did until v2.88.0,
# where all three of setgid/initgroups/setuid had their results discarded. So C3
# would have certified "a site-scoped shell still drops to the site's user" on a
# tree that hands the SITE'S OWNER a root shell in their own webroot. That is the
# project's #321 shape — a context arm must name the PROPERTY whose change would
# invalidate the decision it guards, never two tokens a rewrite keeps.
#
# The property: a failed privilege drop must not reach `execv`. `getuid`/`geteuid`
# are the half that cannot be fooled by a return code nobody read.
if [ -z "$AG_S" ]; then
  bad "C4 SKIPPED: $AG did not yield source"
else
  DROP=$(fn_body 'fn open_pty_shell' "$AG_S")
  DROP_FLAT=$(tr '\n' ' ' <<< "$DROP" | tr -s ' ')
  if [ -z "$DROP" ]; then
    bad "C4 SKIPPED: could not bound open_pty_shell"
  elif has "$DROP_FLAT" 'libc::setgid\(gid\) != 0 \|\| libc::initgroups\(username, gid\) != 0 \|\| libc::setuid\(uid\) != 0' \
    && has "$DROP_FLAT" 'libc::getuid\(\) != uid \|\| libc::geteuid\(\) != uid' \
    && has "$DROP_FLAT" '_exit\(1\)'; then
    ok "C4 a FAILED privilege drop exits instead of reaching execv, and the result is re-read from the kernel"
  else
    bad "C4 the privilege drop's syscalls are unchecked — a failed setuid would exec a ROOT shell for the site's owner"
  fi
fi

echo "§D  a terminal share is scoped to the admin who created it (s428)"
#
# THE DEFECT THIS SECTION CLOSES. `list_shares` (GET /api/terminal/shares) ran
# `SELECT key, value FROM settings WHERE key LIKE 'terminal_share_%'` with no
# owner filter at all — on a multi-admin/reseller install, any admin could
# enumerate every OTHER admin's shares and, via the exposed share_id, read
# their root terminal output through the unauthenticated public viewer.
# `revoke_share` had the same gap: any admin could DELETE any admin's share by
# id alone. Both are settings-table rows (no dedicated owner column), so the
# fix threads the creating admin's id through the existing pipe-delimited
# value string share_lifetime already parses.

# D1 — share_output stores the CREATING ADMIN's id alongside the timestamp,
# not just content. Three pipe-delimited fields, the middle one bound from
# claims — not two.
if [ -z "$BE_S" ]; then
  bad "D1 SKIPPED: $BE did not yield source"
else
  SO=$(fn_body 'async fn share_output' "$BE_S")
  if [ -n "$SO" ] && has "$SO" 'format!\("\{\}\|\{\}\|\{\}"' && has "$SO" 'claims\.sub'; then
    ok "D1 share_output stores the creating admin's id in the share value"
  else
    bad "D1 share_output does not store an owner — every admin's shares are indistinguishable"
  fi
fi

# D2 — share_lifetime parses out an owner field alongside timestamp/content,
# so the two readers below can actually scope on it.
if [ -z "$BE_S" ]; then
  bad "D2 SKIPPED: $BE did not yield source"
else
  LIFETIME=$(fn_body 'fn share_lifetime' "$BE_S")
  if [ -n "$LIFETIME" ] && has "$LIFETIME" 'Option<\(i64, i64, &str, &str\)>'; then
    ok "D2 share_lifetime returns an owner field, not just (created_at, remaining, content)"
  else
    bad "D2 share_lifetime's signature lost the owner field — list_shares/revoke_share cannot scope"
  fi
fi

# D3 — THE ARM THAT MATTERS. list_shares skips any share whose owner is not
# the calling admin's own id.
if [ -z "$BE_S" ]; then
  bad "D3 SKIPPED: $BE did not yield source"
else
  LIST=$(fn_body 'async fn list_shares' "$BE_S")
  LIST_FLAT=$(tr '\n' ' ' <<< "$LIST" | tr -s ' ')
  if [ -n "$LIST" ] && has "$LIST" 'claims\.sub\.to_string\(\)' \
    && has "$LIST_FLAT" 'if owner != my_id \{ continue; \}'; then
    ok "D3 list_shares skips any share not owned by the calling admin"
  else
    bad "D3 list_shares returns every admin's shares — any admin can enumerate another's root terminal output"
  fi
fi

# D4 — revoke_share's DELETE is scoped to the calling admin's own share, not
# just the share id. A key match alone lets any admin revoke — or, paired with
# a D3 regression, first enumerate then read via the share_id — another
# admin's share.
if [ -z "$BE_S" ]; then
  bad "D4 SKIPPED: $BE did not yield source"
else
  REVOKE=$(fn_body 'async fn revoke_share' "$BE_S")
  if [ -n "$REVOKE" ] && has "$REVOKE" 'split_part\(value, .\|., 2\) = \$2' \
    && has "$REVOKE" 'claims\.sub\.to_string\(\)'; then
    ok "D4 revoke_share's DELETE is scoped to the calling admin's own share"
  else
    bad "D4 revoke_share can delete any admin's share by id alone — no ownership check"
  fi
fi

# D5 — view_shared (the public, unauthenticated link) must NOT gain an owner
# check — its whole design is "the id is the credential." Bounded on the
# signature alone: fn_body's closing-brace-at-column-0 heuristic cannot bound
# this function (its body is a raw HTML/JS template with braces starting at
# column 0), so this confirms the fix didn't overcorrect without relying on it.
if [ -z "$BE_S" ]; then
  bad "D5 SKIPPED: $BE did not yield source"
else
  VIEW_SIG=$(awk '/pub async fn view_shared\(/{f=1} f{print} f && /-> Result</{exit}' <<< "$BE_S")
  if [ -n "$VIEW_SIG" ] && ! has "$VIEW_SIG" 'AuthUser'; then
    ok "D5 view_shared stays unauthenticated — the link itself remains the only credential"
  else
    bad "D5 view_shared gained an auth requirement, or its signature could not be found — legitimate share links would break"
  fi
fi

echo
echo "PASS $PASS / FAIL $FAIL"
[ "$FAIL" -eq 0 ]
