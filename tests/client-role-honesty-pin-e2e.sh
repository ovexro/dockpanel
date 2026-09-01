#!/usr/bin/env bash
# client-role-honesty-pin-e2e.sh — s319 / v2.77.0
#
# THE SENTENCE THIS SUITE EXISTS TO PROVE, written out so a mutation can be aimed
# at it rather than at whatever the arms happen to catch:
#
#   "A non-admin is shown what it actually has, and is not shown what it cannot
#    have — neither an empty promise (a menu entry, tile or checkbox that leads
#    to a refusal) nor a false statement (a host number rendered as zero to an
#    account that was never allowed to ask for it)."
#
# THE DEFECT, in three shapes that were all the same shape.
#
#  1. THE DASHBOARD NEVER DREW. `system` is written by exactly two paths and both
#     are admin-only (`fetchRealtimeData` returns early; `ws_metrics.rs` 403s a
#     non-admin socket). The whole render body was keyed on `{!system ? skeleton :
#     body}`, so a client sat in a six-card pulsing SKELETON for ever — a loading
#     state for data that was never going to load. Reported from the field as
#     "the Dashboard also have no stats".
#
#  2. THE MENUS LIED IN BOTH DIRECTIONS. The sidebar's role filter was inlined in
#     `useLayoutState` and nowhere else, so the command palette — a second menu
#     over the same pages — offered /users, /secrets, /security and /settings to
#     every account. Meanwhile the Terminal page auto-dialled the ADMIN-ONLY
#     server shell for everyone, producing an immediate refusal for an account
#     that does own a working per-site shell. The capability a client HAD was
#     invisible; the one it could NOT have was advertised.
#
#  3. THE DASHBOARD ENDPOINT ANSWERED HOST-WIDE. `dashboard::intelligence`
#     forwarded the agent's `/diagnostics` verbatim (that endpoint is AdminUser on
#     its own route), counted every tenant's stale backups, and returned the host
#     security-scan totals; `dashboard::docker_summary` made the same agent call
#     `docker_apps::list_apps` guards with `require_admin`, and guarded nothing.
#
# So the mutations this suite must survive are WIDENING mutations — put the body
# back behind `system`, drop a role gate, let a menu offer everything again — not
# deletions. Every §A–§D arm below was mutation-tested to redden ALONE, and the
# suite as a whole is RED at v2.76.0 and GREEN at HEAD.
#
# §E is the opposite: CONTEXT GUARDS that must be GREEN IN BOTH STATES. This ship
# makes a client's terminal easier to REACH, so §E pins the v2.75.0/v2.76.0
# escalation fixes it must not have loosened on the way.
#
#   §A  the dashboard renders for the account looking at it
#   §B  the dashboard endpoint answers per-tenant, not per-host
#   §C  one menu-visibility decision, in one place
#   §D  the terminal opens the shell the caller actually has
#   §E  CONTEXT — the server shell is still admin-only and still signed
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

DASH=panel/frontend/src/pages/Dashboard.tsx
TERM=panel/frontend/src/pages/Terminal.tsx
PAL=panel/frontend/src/components/CommandPalette.tsx
NAV=panel/frontend/src/data/navItems.ts
LAY=panel/frontend/src/hooks/useLayoutState.ts
API=panel/backend/src/routes/dashboard.rs
BTERM=panel/backend/src/routes/terminal.rs
ATERM=panel/agent/src/routes/terminal.rs

for f in "$DASH" "$TERM" "$PAL" "$NAV" "$LAY" "$API" "$BTERM" "$ATERM"; do
  [ -f "$f" ] || bad "MISSING SUBJECT FILE: $f"
done

# Comments out, CODE INTACT. This matters more here than in most suites: the fix
# ITSELF is largely explanatory, and its comments name every pattern asserted
# below ("admin-only read", "require_admin", "Server root"). Without the strip,
# most arms would pass by matching the prose that narrates them.
code() {
  perl -0777 -pe '
    s{\{/\*.*?\*/\}}{}gs;
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
  ' "$1"
}
subj() { local t; t=$(code "$1"); [ -n "$t" ] || return 1; printf '%s' "$t"; }
has()   { grep -qE -- "$2" <<< "$1"; }

# Does FIRST appear, followed within GAP characters by SECOND? Used to tie a
# rendered label to the gate that governs it without depending on indentation.
within() { # $1=body $2=first $3=second $4=maxgap
  BODY="$1" FIRST="$2" SECOND="$3" GAP="$4" perl -0777 -e '
    exit($ENV{BODY} =~ /$ENV{FIRST}.{0,$ENV{GAP}}$ENV{SECOND}/s ? 0 : 1);
  '
}

DASH_S=$(subj "$DASH" || true)
TERM_S=$(subj "$TERM" || true)
PAL_S=$(subj  "$PAL"  || true)
NAV_S=$(subj  "$NAV"  || true)
LAY_S=$(subj  "$LAY"  || true)
API_S=$(subj  "$API"  || true)
BT_S=$(subj   "$BTERM" || true)
AT_S=$(subj   "$ATERM" || true)

# Every arm below asserts against a STRIPPED body. If the strip produced nothing
# the arm must SKIP loudly rather than pass — an empty haystack makes an absence
# arm green for the wrong reason.
need() { [ -n "$1" ] || { bad "$2 SKIPPED: subject yielded no source"; return 1; }; }

echo "§A  the dashboard renders for the account looking at it"

# A1 — the skeleton is a LOADING state, so it is gated on the account that has
# something loading. This is the whole defect in one line.
if need "$DASH_S" A1; then
  if has "$DASH_S" 'isAdmin && !system \?'; then
    ok "A1 the skeleton ternary is gated on isAdmin, not on system alone"
  else
    bad "A1 the render body is keyed on an admin-only read again — a non-admin never leaves the skeleton"
  fi
fi

# A2 — the WIDENING guard. `{!system ? (` with no role term is the exact prior
# state; this arm is what reddens if someone "simplifies" A1 away.
if need "$DASH_S" A2; then
  if has "$DASH_S" '\{!system \? \('; then
    bad "A2 a bare {!system ? ( gate is back — that is the pre-v2.77.0 shape verbatim"
  else
    ok "A2 no bare {!system ? ( gate remains"
  fi
fi

# A3/A4 — the blocks that genuinely dereference `system` carry their own guard,
# now that the outer ternary no longer supplies it.
if need "$DASH_S" A3; then
  if has "$DASH_S" 'system && isVisible\("system_info"\)'; then
    ok "A3 the System Information grid guards its own system deref"
  else
    bad "A3 the System Information grid dereferences system with no guard"
  fi
fi
if need "$DASH_S" A4; then
  if has "$DASH_S" 'system && isVisible\("metrics"\)'; then
    ok "A4 the CPU/memory/disk tiles guard their own system deref"
  else
    bad "A4 the resource-metric tiles dereference system with no guard"
  fi
fi

# A5 — the single host-derived cell inside an otherwise tenant-safe status grid.
# It is the reason that bar could not simply be shown or hidden as a whole.
if need "$DASH_S" A5; then
  if within "$DASH_S" '\{system && \(' 'Uptime' 200; then
    ok "A5 the Uptime cell is gated on system"
  else
    bad "A5 the Uptime cell is not gated on system — it deref's system.uptime_secs"
  fi
fi

# A6-A9 — the cells that render a NUMBER from state a non-admin never fetches.
# An unhidden blank is honest; an unhidden zero is a claim about a machine the
# reader was never allowed to ask about. These four were the false statements.
# ⚠ s442: A6's gap grew from 177 to 267 chars when Updates became a Link with
# its own conditional background (matching Alerts/Incidents/Backups' existing
# shape) — 300 keeps ~30 chars of headroom for that cell while staying far
# tighter than A10's 700 below, so this still means "the SAME gate", not "gated
# somewhere in the file".
for pair in "A6:Updates:up to date" "A7:Mail Queue:messages" "A8:Bandwidth:formatSize" "A9:Docker:running"; do
  arm=${pair%%:*}; rest=${pair#*:}; label=${rest%%:*}; tail=${rest#*:}
  if need "$DASH_S" "$arm"; then
    if within "$DASH_S" '\{isAdmin && \(' "$label" 300 && within "$DASH_S" "$label" "$tail" 400; then
      ok "$arm the $label cell is admin-gated"
    else
      bad "$arm the $label cell renders to a non-admin — a host figure stated as zero"
    fi
  fi
done

# A10 — the dead-end link. Apps.tsx bounces any non-admin straight back, and the
# sibling "Add Site" link three lines below was already gated for exactly this
# reason: the fix landed on one of the two links.
if need "$DASH_S" A10; then
  if within "$DASH_S" '\{isAdmin && \(' 'Deploy App' 700; then
    ok "A10 the Deploy App shortcut is admin-gated"
  else
    bad "A10 the Deploy App shortcut is offered to a role that Apps.tsx redirects away"
  fi
fi

echo "§B  the dashboard endpoint answers per-tenant, not per-host"

if need "$API_S" B1; then
  if has "$API_S" 'use crate::error::\{.*require_admin.*\}'; then
    ok "B1 dashboard.rs imports require_admin"
  else
    bad "B1 dashboard.rs no longer imports require_admin"
  fi
fi

# B2 — the ungated door to the list next door: this makes the SAME agent call
# `docker_apps::list_apps` guards with require_admin.
if need "$API_S" B2; then
  if within "$API_S" 'pub async fn docker_summary' 'require_admin' 400; then
    ok "B2 docker_summary requires admin"
  else
    bad "B2 docker_summary is ungated — it proxies the agent /apps that list_apps guards"
  fi
fi

# B3 — /diagnostics is AdminUser on its own route; forwarding it verbatim from
# here was the way around that.
if need "$API_S" B3; then
  if within "$API_S" 'if host_scoped \{' 'agent\.get\("/diagnostics"\)' 200; then
    ok "B3 the agent /diagnostics call is admin-scoped"
  else
    bad "B3 the agent /diagnostics blob is fetched for every role again"
  fi
fi

# B4 — stale_backups counted every tenant's sites on the box.
if need "$API_S" B4; then
  if has "$API_S" '\$2::uuid IS NULL OR s\.user_id = \$2'; then
    ok "B4 the stale-backup count carries a tenant predicate"
  else
    bad "B4 the stale-backup count is server-scoped only — it counts other tenants' sites"
  fi
fi

# B5 — security_scans has NO user column to filter by, so the only correct
# tenant answer is not to answer.
if need "$API_S" B5; then
  if within "$API_S" 'let scan_findings.*=\s*if host_scoped' 'security_scans' 400; then
    ok "B5 the host security-scan totals are admin-only"
  else
    bad "B5 the host security-scan totals reach every role again"
  fi
fi

echo "§C  one menu-visibility decision, in one place"

# C1 — the decision has a NAME and a home. While it was inlined in one hook, the
# second menu over the same pages simply never asked.
if need "$NAV_S" C1; then
  if has "$NAV_S" 'export function isNavVisible'; then
    ok "C1 navItems.ts owns the visibility predicate"
  else
    bad "C1 the visibility predicate has no shared home — it will drift again"
  fi
fi
if need "$NAV_S" C2; then
  if has "$NAV_S" 'export function navFlagsFor'; then
    ok "C2 navItems.ts owns the path -> flags lookup"
  else
    bad "C2 no shared path->flags lookup; a caller must keep its own copy"
  fi
fi
if need "$LAY_S" C3; then
  if has "$LAY_S" 'isNavVisible\(item, user\.role\)'; then
    ok "C3 the sidebar reads the shared predicate"
  else
    bad "C3 the sidebar has re-inlined its own copy of the role filter"
  fi
fi
if need "$PAL_S" C4; then
  if has "$PAL_S" 'isNavVisible\(navFlagsFor\(c\.path\)'; then
    ok "C4 the command palette reads the same predicate"
  else
    bad "C4 the command palette filters by nothing — it offers every admin page to every account"
  fi
fi

# C5 — the empty-query branch IS the default view. Filtering only the searched
# list would leave the palette's opening screen unfiltered, which is the state
# most users would actually see.
if need "$PAL_S" C5; then
  if has "$PAL_S" ': commands;'; then
    bad "C5 the unqueried palette falls back to the unfiltered list — its default view is the leak"
  else
    ok "C5 the unqueried palette shows the filtered list"
  fi
fi

echo "§D  the terminal opens the shell the caller actually has"

# D1 — the default option was the one shell a non-admin can never open, so the
# page refused itself before the user touched anything.
if need "$TERM_S" D1; then
  if has "$TERM_S" 'isAdmin && <option value="">Server root</option>'; then
    ok "D1 the Server root option is admin-only"
  else
    bad "D1 Server root is offered to every role — selecting it is a guaranteed 403"
  fi
fi

# D2 — and the page dialled that option on mount without being asked.
if need "$TERM_S" D2; then
  if has "$TERM_S" 'if \(isAdmin \|\| selectedSite\) connect\('; then
    ok "D2 mount does not dial the server shell for a non-admin"
  else
    bad "D2 mount auto-connects to the server shell again — the refusal the field report hit"
  fi
fi

# D3 — a site shell is www-data under a restricted bash with a command blocklist;
# the server snippet list is a menu of refusals there.
if need "$TERM_S" D3; then
  if has "$TERM_S" 'selectedSite \? siteSnippets : serverSnippets'; then
    ok "D3 the snippet bar follows the session, not the role"
  else
    bad "D3 the snippet bar offers root commands inside a restricted site shell"
  fi
fi

# D4 — the SSH panel prints credentials for the host's ROOT account.
if need "$TERM_S" D4; then
  if has "$TERM_S" 'isAdmin && showSshInfo'; then
    ok "D4 the root SSH panel is admin-only"
  else
    bad "D4 the root SSH connection panel renders for every role"
  fi
fi

echo "§E  CONTEXT — the server shell is still admin-only and still signed"
echo "    (these must be GREEN AT BOTH TAGS: this ship makes a client terminal"
echo "     easier to REACH, and must not have loosened what it reaches)"

# E1 — the gate v2.75.0 exists to enforce. This ship changed its MESSAGE; if it
# ever changes its CONDITION, that is the root-shell escalation returning.
if need "$BT_S" E1; then
  if has "$BT_S" 'q\.site_id\.is_none\(\) && claims\.role != "admin"'; then
    ok "E1 a site-less ticket is still refused to a non-admin"
  else
    bad "E1 THE SERVER-SHELL GATE IS GONE — this is the v2.75.0 root-shell escalation"
  fi
fi

# E2 — and the decision still travels INSIDE the signature, which was the actual
# v2.75.0 fix: the gate above only ever guarded minting.
if need "$BT_S" E2; then
  # Spans lines: the binding is rustfmt'd across three. Assert the DECISION
  # (scope derived from the ownership-resolved domain, falling back to the
  # server marker) and that it reaches the signed struct.
  if within "$BT_S" 'let scope = domain' 'SERVER_SHELL_SCOPE' 200 \
     && within "$BT_S" 'let ticket = TerminalTicket' 'scope,' 400; then
    ok "E2 the panel still binds the resolved scope into the signed ticket"
  else
    bad "E2 the scope is no longer bound into the signed ticket"
  fi
fi
if need "$AT_S" E3; then
  if has "$AT_S" 'match ticket_scope\.as_deref\(\)'; then
    ok "E3 the agent still takes the shell scope from the signed ticket"
  else
    bad "E3 the agent no longer reads the scope from the ticket — check for a query-param read"
  fi
fi

# E4 — ownership, not role, is what grants a site shell. This is why a client has
# a terminal at all, and the premise the whole §D section rests on.
if need "$BT_S" E4; then
  if has "$BT_S" 'SITE_CALLER_PREDICATE'; then
    ok "E4 the site branch is still authorised by the ownership predicate"
  else
    bad "E4 the site branch no longer uses the shared ownership predicate"
  fi
fi

echo
echo "  PASS=$PASS  FAIL=$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
