#!/usr/bin/env bash
# logs-scope-signed-pin-e2e.sh — s318 / v2.76.0
#
# THE SENTENCE THIS SUITE EXISTS TO PROVE:
#
#   "The log a stream serves is decided by the ownership check the panel
#    performed when it minted the ticket — carried inside the signature — not by
#    the `?domain=`/`?type=` parameters the browser sends to the agent."
#
# THE DEFECT (the terminal escalation's twin — v2.75.0 fixed the terminal, this
# fixes logs). The panel resolved a site's domain under an ownership predicate and
# returned it BESIDE the signed ticket, while the agent read BOTH the domain from
# `?domain=` AND the log type from `?type=`. Omitting the domain and asking for
# `type=auth` streamed `/var/log/auth.log`; any type outside access/error fell
# through the agent's `other => other.to_string()` arm straight into
# resolve_log_path's system allow-list (syslog, auth, …). So a non-admin who owned
# a single site minted an ownership-checked, site-scoped ticket and read the host's
# SSH auth log. The panel's "Admin access required for system log streaming" check
# only fires at mint time with no site — it never travelled to the agent.
#
# The mutations this suite must survive are WIDENING mutations — move the scope
# back out of the signature, trust the query param again, or let a site ticket
# request a system type — not deletions. Every arm was mutation-tested to redden
# ALONE at v2.75.0.
#
#   §A  the panel binds the scope INTO the signed ticket
#   §B  the agent takes the scope AND the log type FROM the ticket, not the params
#   §C  the two ends agree on the system marker
#
# Pure source analysis: no box, no network, no build.
#
# NO PIPES INTO `grep -q` (SIGPIPE + pipefail = flaky red). Every arm feeds grep a
# here-string.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

BE=panel/backend/src/routes/logs.rs
AG=panel/agent/src/routes/logs.rs

for f in "$BE" "$AG"; do [ -f "$f" ] || bad "MISSING SUBJECT FILE: $f"; done

# Comments out, CODE INTACT — this suite asserts the ABSENCE of a param read that
# the fix's OWN comments legitimately discuss.
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

# One function's / struct's body, bounded by its OWN closing brace in column 0.
fn_body() { awk -v pat="$1" '$0 ~ pat {f=1} f {print} f && /^\}/ {exit}' <<< "$2"; }
before() {
  local body="$1" first="$2" second="$3" a b
  a=$(grep -nE -- "$first" <<< "$body" | head -1 | cut -d: -f1)
  b=$(grep -nE -- "$second" <<< "$body" | head -1 | cut -d: -f1)
  [ -n "$a" ] && [ -n "$b" ] && [ "$a" -lt "$b" ]
}

BE_S=$(subj "$BE" || true)
AG_S=$(subj "$AG" || true)

echo "§A  the panel binds the scope INTO the signed ticket"

# A1 — the signed struct carries a scope field. The struct is what gets HS256'd.
if [ -z "$BE_S" ]; then
  bad "A1 SKIPPED: $BE did not yield source"
else
  TICKET=$(fn_body 'struct StreamTicket' "$BE_S")
  if [ -n "$TICKET" ] && has "$TICKET" 'scope: String'; then
    ok "A1 the signed StreamTicket carries a scope"
  else
    bad "A1 the signed ticket has no scope field — the log decision is not in the signature"
  fi
fi

# A2 — and it is set from the ownership-resolved domain, `@system` the only
# fallback (which the mint reaches only after the admin check for a site-less
# request). Keyed on the whole derivation, not the field name.
if [ -z "$BE_S" ]; then
  bad "A2 SKIPPED: $BE did not yield source"
else
  MINT=$(fn_body 'pub async fn stream_token' "$BE_S")
  MINT_FLAT=$(tr '\n' ' ' <<< "$MINT" | tr -s ' ')
  if [ -n "$MINT" ] \
    && has "$MINT_FLAT" 'let scope = domain\.clone\(\)\.unwrap_or_else\(\|\| "@system"'; then
    ok "A2 the scope is set from the ownership-resolved domain, @system as the only fallback"
  else
    bad "A2 the ticket scope is not derived from the resolved domain — an attacker-supplied value could reach it"
  fi
fi

# A3 — the ownership check still gates the site branch, AND the admin check still
# gates the site-less (system) request. The scope is only as good as these.
if [ -z "$BE_S" ]; then
  bad "A3 SKIPPED: $BE did not yield source"
elif has "$BE_S" 'SITE_CALLER_PREDICATE' \
  && has "$BE_S" 'Site not found' \
  && has "$BE_S" 'Admin access required for system log streaming'; then
  ok "A3 the site domain is resolved under the ownership predicate and system streaming still needs admin"
else
  bad "A3 the ownership/admin resolution behind the scope is gone — the signed scope would certify nothing"
fi

echo "§B  the agent takes the scope AND the log type FROM the ticket, not the params"

# B1 — the agent's decode struct carries the scope, so the value it acts on is
# the signed one.
if [ -z "$AG_S" ]; then
  bad "B1 SKIPPED: $AG did not yield source"
else
  ATICKET=$(fn_body 'struct StreamTicket' "$AG_S")
  if [ -n "$ATICKET" ] && has "$ATICKET" 'scope: Option<String>'; then
    ok "B1 the agent decodes the ticket's scope"
  else
    bad "B1 the agent does not read a scope off the ticket — it can only be trusting the query params"
  fi
fi

# B2 — THE ARM THAT MATTERS. The log_type is chosen by matching on the TICKET
# scope, reached before any q.domain read. A regression that binds from q.domain
# up front is the defect.
if [ -z "$AG_S" ]; then
  bad "B2 SKIPPED: $AG did not yield source"
else
  WS=$(fn_body 'async fn stream_handler' "$AG_S")
  if [ -z "$WS" ]; then
    bad "B2 SKIPPED: could not bound stream_handler"
  elif has "$WS" 'let log_type = match claims\.scope' \
    && before "$WS" 'match claims\.scope' 'q\.domain'; then
    ok "B2 the log is chosen from the signed scope; the query param is not read up front"
  else
    bad "B2 the log is bound from the query param, not the ticket scope — the escalation is restored"
  fi
fi

# B3 — a SITE scope may stream only that site's own log types. The refusal of a
# system type is the arm that stops `type=auth`. The pass-through `other =>
# other.to_string()` must NOT live in the site branch — it is legacy-only.
if [ -z "$AG_S" ]; then
  bad "B3 SKIPPED: $AG did not yield source"
else
  WS=$(fn_body 'async fn stream_handler' "$AG_S")
  N=$(count "$WS" 'other => other\.to_string\(\)')
  if [ -n "$WS" ] \
    && has "$WS" 'Log type not permitted for a site-scoped stream' \
    && has "$WS" 'Some\(domain\) =>' \
    && [ "$N" = "1" ]; then
    ok "B3 a site scope maps only its own log types and refuses a system type (one legacy pass-through only)"
  else
    bad "B3 a site scope can pass an arbitrary type through (${N} pass-throughs) — type=auth reaches the host logs"
  fi
fi

# B4 — the system logs are reachable ONLY through the signed @system sentinel,
# and the legacy param fallback is confined to the no-scope branch AND logged.
if [ -z "$AG_S" ]; then
  bad "B4 SKIPPED: $AG did not yield source"
else
  WS=$(fn_body 'async fn stream_handler' "$AG_S")
  ND=$(count "$WS" 'q\.domain\.as_deref\(\)')
  if [ -n "$WS" ] \
    && has "$WS" 'Some\("@system"\) =>' \
    && has "$WS" 'None =>' \
    && before "$WS" 'None =>' 'q\.domain\.as_deref\(\)' \
    && [ "$ND" = "1" ] \
    && has "$WS" 'tracing::warn'; then
    ok "B4 host logs need the signed @system marker; the param fallback is legacy-only and logged"
  else
    bad "B4 the param is read outside the legacy branch (${ND} reads) or an unscoped stream is silent"
  fi
fi

echo "§C  the two ends agree on the system marker"

# C1 — both files use the SAME sentinel. If they drift, an admin system ticket
# minted by the panel is treated as a site named '@system' by the agent.
if has "$BE_S" '"@system"' && has "$AG_S" '"@system"'; then
  ok "C1 both ends agree the system marker is @system"
else
  bad "C1 the system marker differs: panel=$(has "$BE_S" '"@system"' && echo @system || echo none) agent=$(has "$AG_S" '"@system"' && echo @system || echo none)"
fi

# C2 — the marker is not a legal domain, so no site can claim it. The agent's own
# validate_domain rejects '@'.
if [[ "@system" == *@* ]]; then
  ok "C2 the system marker (@system) is not a valid domain, so no site can claim it"
else
  bad "C2 the system marker could be a real domain — a site of that name would open the host logs"
fi

# C3 — a site-scoped stream still reaches its OWN logs (guards a fix that carries
# the scope but forgets to serve the site): the site branch still builds
# nginx_access:{domain} and the handler still resolves through resolve_log_path.
if [ -z "$AG_S" ]; then
  bad "C3 SKIPPED: $AG did not yield source"
elif has "$AG_S" 'nginx_access:\{domain\}' && has "$AG_S" 'resolve_log_path'; then
  ok "C3 a site-scoped stream still serves that site's own logs"
else
  bad "C3 the site-scoped stream no longer serves the site's own logs"
fi

echo
echo "PASS $PASS / FAIL $FAIL"
[ "$FAIL" -eq 0 ]
