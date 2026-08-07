#!/usr/bin/env bash
# wrong-host-dispatch-pin-e2e.sh — s320
#
# Two questions that look like one: WHICH SITE, and WHICH HOST.
#
# `site_domain_for_caller` answers the first from a row the caller is authorised
# on. The server-scope extractor answers the second from a header the browser
# chooses, falling back to the local agent when that header is absent. On a
# single-box install the two always agree, which is why the difference was
# invisible for the whole life of the fleet feature.
#
# They part company the moment a second server exists. A caller who owns no
# `servers` row — every client, by construction, since the only INSERT is
# admin-gated — sends no header and is handed the LOCAL agent, while their site's
# row may name a fleet member. An operator with two servers gets the same result
# by reaching a site on server B while the switcher says A: both list routes are
# scoped so the UI will not link there, but `get_one` is not, so a bookmark, a
# deep link or a second tab renders the page normally and every control on it
# acts on A.
#
# The rule was already written down twice in this repo before this suite existed.
# The webhook deploy path resolves `for_server` from the row and says why in a
# comment; the git-deploy update path does the same; every background service
# walks these tables and routes per row. It was the authenticated HTTP handlers —
# the buttons a human presses — that never adopted it.
#
#   §A  THE WEBROOT IS NOT A DELETABLE CHILD. Containment and identity are not
#       the same question. `.`, `""` and `/` all canonicalise to the site root and
#       a path always starts with itself, so the traversal guard passed them and
#       a delete became `remove_dir_all` on the whole site — reported as success.
#       This is live on a single-box install and has nothing to do with fleets.
#   §B  LISTING DOES NOT CREATE. The list verb used to make the site root before
#       resolving, so a request aimed at a host that does not serve that domain
#       got a root-owned directory and a 200 with an empty array, which then
#       unblocked write/create/upload into a folder no vhost serves. Every other
#       verb already failed loudly. Silence was the only reason a misdispatch
#       could go unnoticed.
#   §C  THE ROW NAMES THE HOST. A handler that then talks to an agent resolves it
#       from the row's server, and an unreachable server is REFUSED — never
#       silently swapped for this machine, because substituting is how one
#       tenant's files get written onto another tenant's box.
#   §D  CONTEXT. Arms that must be green at BOTH tags, so a harness that measures
#       nothing cannot read as a pass.

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

echo
echo "=============================================="
echo "  wrong-host dispatch — source pins (s320)"
echo "=============================================="
echo

PASS=0; FAIL=0; SKIP=0
ok()   { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

# Strip comments before matching. Without this every arm below could be satisfied
# by the prose that NARRATES it — the trap that has produced false greens here
# before, and the reason no comment in the fixed files spells these tokens.
code() {
  perl -0777 -pe '
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
    s{^\s*///.*$}{}gm;
  ' "$1"
}

has()  { grep -qE -- "$2" <<< "$1"; }

# One function body by brace balance. A fixed -A window is not a function: a
# regression that moves a line past the window makes the arm measure nothing.
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

AGENT_FILES_SVC=panel/agent/src/services/files.rs
AGENT_FILES_RT=panel/agent/src/routes/files.rs
HELPERS=panel/backend/src/helpers.rs
SITES=panel/backend/src/routes/sites.rs
DEPLOY=panel/backend/src/routes/deploy.rs
STACKS=panel/backend/src/routes/stacks.rs

for f in "$AGENT_FILES_SVC" "$AGENT_FILES_RT" "$HELPERS" "$SITES" "$DEPLOY" "$STACKS"; do
  [ -f "$f" ] || { echo "  FATAL: $f missing — wrong tree?"; exit 1; }
done

SVC=$(code "$AGENT_FILES_SVC")
RT=$(code "$AGENT_FILES_RT")
HLP=$(code "$HELPERS")
SIT=$(code "$SITES")
DEP=$(code "$DEPLOY")
STK=$(code "$STACKS")

# ── §A the webroot is not a deletable child ──────────────────────────────────
echo "§A  the site root is not a deletable child"

if has "$SVC" 'fn resolve_safe_child'; then
  ok "A1 a root-refusing resolver exists"
else
  bad "A1 no root-refusing resolver — '.' still canonicalises to the webroot and deletes the site"
fi

CHILD=$(fnbody "$SVC" "resolve_safe_child")
if [ -z "$CHILD" ]; then
  bad "A2 could not extract the resolver body — the arm measured nothing"
elif has "$CHILD" 'resolved == canon_base|canon_base == resolved'; then
  ok "A2 it compares the resolved path against the root by IDENTITY, not containment"
else
  bad "A2 the resolver does not compare resolved against the site root — containment alone lets '.' through"
fi

DEL=$(fnbody "$RT" "delete_entry")
if [ -z "$DEL" ]; then
  bad "A3 could not extract the delete handler — the arm measured nothing"
elif has "$DEL" 'resolve_safe_child'; then
  ok "A3 delete resolves through the root-refusing resolver"
else
  bad "A3 delete still uses the permissive resolver — DELETE ?path=. wipes the whole webroot"
fi

REN=$(fnbody "$RT" "rename_entry")
if [ -z "$REN" ]; then
  bad "A4 could not extract the rename handler — the arm measured nothing"
elif has "$REN" 'resolve_safe_child'; then
  ok "A4 rename refuses to move the site root itself"
else
  bad "A4 rename can still take the site root as its source"
fi

# The over-fix guard. Listing the root IS the file manager's default view, so a
# resolver that refused the root everywhere would break the normal case. This arm
# must stay green at BOTH tags.
LIST=$(fnbody "$RT" "list_dir")
if [ -z "$LIST" ]; then
  bad "A5 could not extract the list handler — the arm measured nothing"
elif has "$LIST" 'resolve_safe_path'; then
  ok "A5 list still uses the permissive resolver, so the default view of the root still works"
else
  bad "A5 list no longer resolves permissively — the root listing, which is the default view, is broken"
fi

# ── §B listing does not create ───────────────────────────────────────────────
echo
echo "§B  listing does not create"

if has "$SVC" 'fn ensure_site_root'; then
  bad "B1 the create-on-list helper is back — a misdirected list will root-mkdir on the wrong host"
else
  ok "B1 the create-on-list helper is gone from the service"
fi

if has "$RT" 'ensure_site_root'; then
  bad "B2 a route still calls the create-on-list helper"
else
  ok "B2 no route creates the site root before resolving"
fi

if has "$SVC" 'Site root does not exist'; then
  ok "B3 an absent site root still reports itself loudly"
else
  bad "B3 nothing reports an absent site root — the failure went quiet again"
fi

# ── §C the row names the host ────────────────────────────────────────────────
echo
echo "§C  the row names the host"

if has "$HLP" 'fn agent_for_site_server'; then
  ok "C1 a row-driven agent resolver exists"
else
  bad "C1 no row-driven agent resolver — handlers must each re-derive the host"
fi

RESOLVER=$(fnbody "$HLP" "agent_for_site_server")
if [ -z "$RESOLVER" ]; then
  bad "C2 could not extract the resolver body — the arm measured nothing"
elif has "$RESOLVER" 'AgentHandle::Local|local_server_id|for_server_or_local'; then
  bad "C2 the row-driven resolver can still fall back to this machine — substituting is the defect"
else
  ok "C2 the resolver never substitutes the local agent"
fi

if has "$RESOLVER" 'for_server'; then
  ok "C3 it resolves the agent from the row's server"
else
  bad "C3 it does not resolve per-server at all"
fi

# C4 — NO handler answers WHICH SITE from the row and WHICH HOST from the caller.
#
# ⚠ s321: THE LINE ABOVE THIS ONE USED TO READ "Subjects derived FROM SOURCE, not
# from a literal list" — sitting directly on top of a hardcoded three-element
# array. It was not derived from anything. It judged THREE handlers while sixty-two
# call sites in seven modules spelled the pattern, and because the comment used the
# vocabulary of the lesson, the gap read as already audited. That is the sharper
# form of §J5's mistake in the sibling suite: a literal list under a comment
# FORBIDDING literal lists is a missed application, but a literal list under a
# comment CLAIMING to be derived is a false statement about coverage, and it was
# written into the very suite that exists to police this defect class.
#
# The census below is real. A handler is IN the class when it resolves a site the
# caller may act on — through a module resolver or a shared-predicate row read —
# and then names that site's domain on an agent call. Those two facts together ARE
# the defect: the row answered which site, the scope answered which host, and on a
# fleet they disagree. Membership is computed per handler across every route
# module, so a handler added tomorrow is judged the day it lands.
#
# Per lesson #143 the census is asserted BEFORE it is judged. An empty scan means
# the extractor broke, not that the tree is clean, and it must fail loudly — a
# violation count of zero over zero subjects is the reassuring direction.
CENSUS=$(for f in panel/backend/src/routes/*.rs; do
  perl -0777 -ne '
    my $src = $_;
    $src =~ s{^\s*///.*$}{}gm; $src =~ s{^\s*//.*$}{}gm;
    my @at;
    while ($src =~ /(?:pub )?async fn ([a-z_0-9]+)\s*\(/g) { push @at, [pos($src), $1]; }
    for my $i (0 .. $#at) {
      my ($start, $name) = @{$at[$i]};
      my $end  = ($i < $#at) ? $at[$i+1][0] : length($src);
      my $body = substr($src, $start, $end - $start);
      # The FIXED form counts as a member too. Keying membership only on the
      # unfixed spellings would shrink the denominator as handlers are converted,
      # so the control would weaken exactly as the tree improved — and a handler
      # that later lost its site resolution altogether would leave the census
      # silently, which is the reassuring direction.
      next unless $body =~ /SITE_CALLER_PREDICATE|SITE_FOR_CALLER_ALL|site_domain_for_caller|site_agent_for_caller|agent_for_site_server|let domain = (?:site_domain|get_site_domain|get_site)\(/;
      my $scoped   = ($body =~ /ServerScope\(/)                    ? 1 : 0;
      # Any use of the handle is enough. Keying on a literal "{domain}" missed
      # `logs::site_logs`, which interpolates the domain into a log-type string
      # first and passes THAT — the site is still the subject of the call.
      my $usesagent = ($body =~ /(?:^|[^_a-z])agent\s*\n?\s*\./m || $body =~ /&agent\b/) ? 1 : 0;
      # …unless the row read is ALSO constrained to the scoped server. Then the
      # site set and the handle agree by construction and a remote site simply
      # reports "not found" — correct, not a misdispatch. This is what separates
      # `wordpress::bulk_update` and `all_wp_sites` from the real members.
      my $pinned = ($body =~ /server_id = \$\d/) ? 1 : 0;
      # A streaming-ticket mint is not a dispatcher: its handle is the SIGNING key
      # and its scope id is the input to the local-only guard, so taking either
      # from the row would delete the guard rather than aim it. Excluded by the
      # PROPERTY of carrying that guard, never by name — a third mint written
      # tomorrow is excluded for the same reason, and one written WITHOUT the
      # guard is judged here (and by J4/J5 next door).
      $pinned = 1 if $body =~ /require_local_agent_scope\(/;
      # A handler that CREATES a site row is choosing a destination, not resolving
      # an existing host. There is no row to take the answer from yet.
      $pinned = 1 if $body =~ /INSERT INTO sites\b/;
      $scoped = 0 if $pinned;
      print "$ARGV:$name:$scoped$usesagent\n";
    }
  ' "$f"
done | sort)
CENSUS_N=$(printf '%s\n' "$CENSUS" | grep -c . || true)
VIOL=$(printf '%s\n' "$CENSUS" | grep ':11$' || true)
VIOL_N=$(printf '%s\n' "$VIOL" | grep -c . || true)

if [ "$CENSUS_N" -lt 20 ]; then
  bad "C4 the census found only $CENSUS_N site-resolving handlers — the extractor is broken, not the tree (this family spans seven modules)"
else
  ok "C4 censused $CENSUS_N site-resolving handlers across every route module, computed from source"
  if [ "$VIOL_N" -eq 0 ]; then
    ok "C4 none of them takes its agent from the caller's server selection"
  else
    printf '%s\n' "$VIOL" | while IFS=: read -r file fn _; do
      [ -n "$fn" ] || continue
      bad "C4 $(basename "$file")::$fn resolves the site from the row and the host from the caller — the two questions are collapsed again"
    done
  fi
fi

STKBODY=$(fnbody "$STK" "remove")
if [ -z "$STKBODY" ]; then
  bad "C5 could not extract the stack remove body — the arm measured nothing"
elif has "$STKBODY" 'server_id FROM docker_stacks|SELECT id, name, domain, server_id'; then
  ok "C5 the stack delete reads the server its row names"
else
  bad "C5 the stack delete does not read its row's server — it deletes the record unconditionally, so a misdispatch orphans the real host"
fi

# ── §D context: the arms that must be green at BOTH tags ─────────────────────
echo
echo "§D  context (green at both tags — proves the harness measures something)"

WEBHOOK=$(fnbody "$DEP" "webhook")
if [ -z "$WEBHOOK" ]; then
  bad "D1 could not extract the webhook body — the arm measured nothing"
elif has "$WEBHOOK" 'for_server'; then
  ok "D1 the webhook deploy path still resolves per-server (the fix this suite generalises)"
else
  bad "D1 the webhook deploy path lost its per-server resolution"
fi

if has "$HLP" 'SITE_CALLER_PREDICATE'; then
  ok "D2 the shared ownership predicate is still the one place that decides WHICH SITE"
else
  bad "D2 the shared ownership predicate is gone — ownership decided somewhere new"
fi

# The predicate must NOT gain a server term. Filtering by server would hide a
# client's own remote site behind an empty result instead of an error, which is
# the wrong fix and the one the original author explicitly warned against.
PRED=$(grep -A 4 'SITE_CALLER_PREDICATE' "$HELPERS" | grep -v '^\s*///')
if grep -qE 's\.server_id *=' <<< "$PRED"; then
  bad "D3 the ownership predicate now filters by server — that HIDES a client's own remote site rather than routing to it"
else
  ok "D3 the ownership predicate still does not filter by server"
fi

echo
echo "----------------------------------------------"
printf '  PASS %d   FAIL %d   SKIP %d\n' "$PASS" "$FAIL" "$SKIP"
echo
[ "$FAIL" -eq 0 ] || exit 1
