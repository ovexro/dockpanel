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

# ⚠ NEVER call `bad` inside `cmd | while read`. A pipeline puts the loop in a
# SUBSHELL, so `FAIL=$((FAIL+1))` increments a copy that dies with it: the arm
# prints its ✗ marks, the summary still reads FAIL 0, and the suite exits 0.
# Found s322, in this file, on the §C4 violation loop — the one arm whose entire
# job is reporting violations had been unable to fail the build since it was
# written. Use `while read; do …; done < <(printf …)` so the loop runs in THIS
# shell. Any future arm that enumerates subjects must do the same.

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
    while ($src =~ /(?:pub )?async fn ([a-z_0-9]+)\s*\(/g) { push @at, [pos($src), $1, $-[0]]; }
    for my $i (0 .. $#at) {
      my ($start, $name) = @{$at[$i]};
      my $end  = ($i < $#at) ? $at[$i+1][2] : length($src);
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
    while IFS=: read -r file fn _; do
      [ -n "$fn" ] || continue
      bad "C4 $(basename "$file")::$fn resolves the site from the row and the host from the caller — the two questions are collapsed again"
    done < <(printf '%s\n' "$VIOL")
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
echo "-- E: the population comes from the SCHEMA, not from a spelling ------------"

# §C4 above censuses the site-resolving family, and it is real — but its
# membership test names six correct-form SPELLINGS, so a handler only enters the
# census once it already uses one of them. At s321 that left `mail.rs`,
# `git_deploys.rs`, `backup_orchestrator.rs` and `docker_apps.rs` contributing
# ZERO members between them while the tree held two dozen live defects in exactly
# those files, and C4 printed green over all of it. Worse, deleting a fixed
# handler's resolver call REMOVES it from the census — deletion reads as
# compliance, the failure mode §J5 next door was rewritten to close.
#
# So this section asks the question from the other end. A table that carries a
# `server_id` column is a table whose rows NAME A HOST. Any handler that reads
# such a row and then acts through an agent must take the host from the row. The
# population is derived from `migrations/*.sql`, so a table added tomorrow joins
# the census the day it lands, and no spelling can leave it.

# E1 — the table list is derived, and asserted non-empty before anything is judged.
export DP_TABLES=$(perl -0777 -ne '
  while (/ALTER\s+TABLE\s+(?:IF\s+EXISTS\s+)?(\w+)\s+ADD\s+COLUMN\s+(?:IF\s+NOT\s+EXISTS\s+)?server_id\b/gis) { print "$1\n"; }
  while (/CREATE\s+TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?(\w+)\s*\((.*?)\n\);/gis) {
    my ($t,$b)=($1,$2); print "$t\n" if $b =~ /^\s*server_id\s/mi;
  }
' panel/backend/migrations/*.sql | sort -u | grep -v '^migrations$' | paste -sd'|')
TABLE_N=$(printf '%s' "$DP_TABLES" | tr '|' '\n' | grep -c . || true)

# A HOSTED resource: a table whose server_id is NOT NULL. The schema says these
# rows cannot exist without naming a machine, which is precisely the property
# that makes "which host" a question the ROW answers and the caller must not.
# Tables with a nullable server_id are a mix -- telemetry, shared destinations,
# metadata -- where the id may legitimately be absent, so an unpinned read there
# is not on its own evidence of a misdispatch. E3 judges the NOT NULL set; the
# wider $DP_TABLES set still drives E1/E4.
export DP_HOSTED=$(perl -0777 -ne '
  while (/ALTER\s+TABLE\s+(?:IF\s+EXISTS\s+)?(\w+)\s+ALTER\s+COLUMN\s+server_id\s+SET\s+NOT\s+NULL/gis) { print "$1\n"; }
  while (/CREATE\s+TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?(\w+)\s*\((.*?)\n\);/gis) {
    my ($t,$b)=($1,$2); print "$t\n" if $b =~ /^\s*server_id\s+UUID[^,]*NOT\s+NULL/mi;
  }
' panel/backend/migrations/*.sql | sort -u | paste -sd"|")
HOSTED_N=$(printf '%s' "$DP_HOSTED" | tr '|' '\n' | grep -c . || true)
if [ "${HOSTED_N:-0}" -lt 3 ]; then
  bad "E1b derived only ${HOSTED_N:-0} tables whose server_id is NOT NULL — the extractor is broken"
else
  ok "E1b derived $HOSTED_N tables whose rows cannot exist without naming a host"
fi

if [ "${TABLE_N:-0}" -lt 10 ]; then
  bad "E1 derived only ${TABLE_N:-0} host-bearing tables from the schema — the extractor is broken, not the schema"
else
  ok "E1 derived $TABLE_N host-bearing tables from migrations/*.sql"
fi

# The census. Membership and compliance are BOTH transitive through file-local
# helpers: the shape that hid at s321 was a handler whose row read lived one call
# away (`get_db_info`), and an arm that only followed direct reads would miss it
# while an arm that followed reads but not resolutions would cry wolf at every
# delegator until nobody believed it.
dispatch_census() {
  for f in panel/backend/src/routes/*.rs; do
    perl -0777 -ne '
      BEGIN { $T = $ENV{DP_TABLES}; $H = $ENV{DP_HOSTED}; }
      my $src = $_;
      $src =~ s{^\s*///.*$}{}gm; $src =~ s{^\s*//.*$}{}gm;
      my @f;
      while ($src =~ /(?:pub )?(?:async )?fn ([a-z_0-9]+)\s*\(/g) { push @f, [pos($src), $1, $-[0]]; }
      my (%engages, %reader);
      for my $j (0 .. $#f) {
        my $e = ($j < $#f) ? $f[$j+1][2] : length($src);
        my $b = substr($src, $f[$j][0], $e - $f[$j][0]);
        $engages{$f[$j][1]} = 1 if $b =~ /agent_for_site_server\(|site_agent_for_caller\(|\bfor_server\(/;
        # A local helper only pulls its CALLERS into the census if it actually
        # surfaces the host — i.e. it reads a host-bearing row AND hands back a
        # server_id. `get_db_info` does, which is how `databases::query` (whose
        # own body contains no SQL at all) becomes a member; the shape that hid
        # at s321. A helper that reads such a table for some other purpose --
        # port holders, template lists -- answers a question about the box in
        # front of the caller and does not.
        $reader{$f[$j][1]}  = 1 if $b =~ /\b(?:FROM|JOIN)\s+(?:$T)\b/i && $b =~ /\bserver_id\b/;
      }
      my $ENG = join("|", map { quotemeta } keys %engages) || "\0NEVER\0";
      my $RD  = join("|", map { quotemeta } keys %reader)  || "\0NEVER\0";

      for my $i (0 .. $#f) {
        my $end = ($i < $#f) ? $f[$i+1][2] : length($src);
        my $body = substr($src, $f[$i][0], $end - $f[$i][0]);
        my $name = $f[$i][1];

        # MEMBER: reads a host-bearing row (directly or one call away) and then
        # acts through an agent. A READ, not a write — a handler that only
        # INSERTs into such a table never had a host to resolve.
        # ...and the read is keyed on the RESOURCE own id. That is what makes the
        # row an independent answer to which host: `WHERE id = $1` returns a row
        # that may name any machine, so the handler has to consult it. A read
        # keyed some other way (all images on the box, this server row itself) is
        # a question about the box in front of the caller, and the scope IS the
        # correct identity for it. This distinction is why the arm can be strict
        # about resolution without indicting every server-level handler.
        next unless ($body =~ /\b(?:FROM|JOIN)\s+(?:$H)\b/i || $body =~ /\b(?:$RD)\s*\(/);
        next unless ($body =~ /WHERE\s+[a-z_.]*\bid\s*=\s*\$\d/i || $body =~ /\b(?:$RD)\s*\(/);
        # ...and the row it reads yields the resource IDENTITY the agent is then
        # asked about -- a domain, or a helper that surfaced the host. Reading
        # `sites` to count something, or to build a template list, is not
        # resolving a resource whose host the row knows; those handlers act on the
        # box in front of the caller and the scope is their correct identity.
        # ...and the AGENT CALL ITSELF names that identity. This is the whole
        # concept in one line: the row answered WHICH resource, the scope
        # answered WHICH host, and the request carries the first to the second.
        # A handler that reads a host-bearing row and then asks its agent about a
        # container id or an image never carried a row-derived identity across,
        # so the scope is not the wrong answer to anything.
        next unless $body =~ /agent[\s\S]{0,400}?\bdomain\b/;
        next unless ($body =~ /(?:^|[^_a-z])agent\s*\n?\s*\./m || $body =~ /&agent\b/
                     || $body =~ /state\.agent\b/ || $body =~ /agents\.for_server/);

        # A function HANDED an already-resolved handle has no host to resolve --
        # its CALLER did, and the caller is judged on its own row. Anchored to the
        # SIGNATURE (window start to the first close-paren before -> or {), never
        # to the body: an axum handler takes extractors, so it cannot declare this
        # parameter, and an exemption that matched anywhere in the body would be a
        # hole any handler could climb through by naming the type in a comment.
        my ($sig) = $body =~ /^(.*?\)\s*(?:->|\{))/s;
        next if defined $sig && $sig =~ /AgentHandle/;

        # COMPLIANT: it has an opinion about WHICH host.
        # `\bserver_id\b` deliberately does not match `_server_id` — an underscore
        # is a statement about the compiler, not about the code, and every defect
        # this arm was built from spelled its scope `_server_id` and then
        # dispatched through the caller anyway.
        # COMPLIANCE IS ABOUT WHERE THE HANDLE CAME FROM, NOT ABOUT MENTIONING
        # A NAME. An earlier draft accepted any body containing server_id, and
        # mutation-testing killed it: reverting databases::query to the handle
        # supplied by the request left the binding from get_db_info in place, so
        # the arm printed GREEN over the exact defect it was written for. Merely
        # holding a row server id is the same non-evidence as spelling it
        # _server_id. The handle has to be RESOLVED from it.
        #
        # (No apostrophes in this block on purpose: the enclosing perl program is
        # single-quoted inside the shell, and one apostrophe ends it.)
        my $ok = 0;
        # `\bfor_server\(` rather than `agents\.for_server\(`: rustfmt wraps a
        # method chain, so the receiver and the call land on different lines and
        # a regex spanning the dot matched nothing. `servers::rotate_token`
        # resolves correctly and was reported as a violation for that reason
        # alone -- a false RED, which costs an arm its credibility as surely as a
        # false green costs it its purpose.
        $ok = 1 if $body =~ /agent_for_site_server\(|site_agent_for_caller\(|\bfor_server\(|for_server_or_local\(/;
        $ok = 1 if $body =~ /\b(?:$ENG)\s*\(/;
        # The read is PINNED to the server in scope, so the row and the handle
        # cannot disagree: a resource on another host simply is not found, which
        # is a correct answer rather than a misdispatch. This is what separates a
        # genuinely server-scoped resource (an image on this box, a scan of this
        # box) from a row that independently names a host.
        $ok = 1 if $body =~ /server_id = \$\d/;
        # Choosing a destination for a resource that does not exist yet. Two
        # spellings because two kinds of resource: one that becomes a row and
        # records the host it chose, and one that never becomes a row at all -- a
        # Docker app lives only as a labelled container, which is why
        # `docker_apps::deploy` needs the second clause. Claiming a domain as
        # `Holder::New` IS the statement that nothing held it anywhere in the
        # fleet, so there is no prior host to take the answer from.
        $ok = 1 if $body =~ /INSERT INTO \w+\s*\([^)]*\bserver_id\b/s;
        $ok = 1 if $body =~ /Holder::New/;
        # The host was decided by the CALLER and passed in. `sync_mail_config`
        # is the case: it takes `server_id: Uuid` and rebuilds that server maps.
        my ($sig) = $body =~ /^(.*?\)\s*(?:->|\{))/s;
        $ok = 1 if defined $sig && $sig =~ /\bserver_id\s*:/;
        $ok = 1 if $body =~ /require_local_agent_scope\(/;
        $ok = 1 if $body =~ /online_fleet\(/;

        print "$ARGV:$name:", ($ok ? "OK" : "VIOL"), "\n";
      }
    ' "$f"
  done | sort
}

CENSUS_E=$(dispatch_census)
CE_N=$(printf '%s\n' "$CENSUS_E" | grep -c . || true)
CE_V=$(printf '%s\n' "$CENSUS_E" | grep ':VIOL$' || true)
CE_VN=$(printf '%s\n' "$CE_V" | grep -c . || true)

# FLOOR RE-DERIVED s323, and the number moved for a reason worth recording: the
# window that carves each function body ended at the SUCCESSOR\047S pos() -- after
# the next `fn name(` -- so every body carried the next function\047s declaration.
# $RD and $ENG are alternations of function NAMES, so a handler inherited both
# MEMBERSHIP and COMPLIANCE from whatever happened to be declared below it. That
# inflated the census 31 -> 48 with 14 phantoms, and granted a free pass to the two
# real subjects whose successors resolved correctly. Windows now end at the
# successor\047s declaration START. Floor set below the measured 31 with headroom,
# not at it: this arm exists to catch an extractor that COLLAPSED, and a floor
# pinned to the exact count would go red every time a handler is legitimately added
# or removed.
if [ "${CE_N:-0}" -lt 25 ]; then
  bad "E2 censused only ${CE_N:-0} handlers that read a host-bearing row and dispatch — the extractor is broken, not the tree"
else
  ok "E2 censused $CE_N such handlers across every route module"
  # WHAT E3 DOES NOT COVER, stated because an arm that lets a reader assume it is
  # exhaustive defends the gap (s321 #268). Membership requires the agent call to
  # carry a row-derived DOMAIN. Handlers that carry a different identity across --
  # `databases::query` sends a db NAME, `image_scans` an image -- are outside it,
  # and were fixed this session by hand rather than by this arm. Extending E3 to
  # identities other than a domain is real work and is not done.
  if [ "${CE_VN:-0}" -eq 0 ]; then
    ok "E3 every one of them takes its host from the row it read"
  else
    while IFS=: read -r file fn _; do
      [ -n "$fn" ] || continue
      bad "E3 $(basename "$file")::$fn reads a row that names a host, then acts through whatever host the CALLER named"
    done < <(printf '%s\n' "$CE_V")
  fi
fi

# E4 — the population extends past routes/. An unattended service has no caller,
# so nothing downstream can catch it: at s321 `auto_healer::auto_renew_ssl` walked
# every site in the fleet holding the PANEL's own AgentClient and asked this box to
# renew certificates for domains it does not serve. HTTP-01 could not validate, so
# the certificate simply expired on live customer sites — and a local success wrote
# the wrong expiry back, pushing the site out of the renewal window so the loop
# stopped trying. No route-scoped arm can see that shape.
SVC_BAD=$(for f in panel/backend/src/services/*.rs; do
  perl -0777 -ne '
    BEGIN { $T = $ENV{DP_TABLES}; }
    my $src=$_; $src =~ s{^\s*///.*$}{}gm; $src =~ s{^\s*//.*$}{}gm;
    my @f; while ($src =~ /(?:pub )?(?:async )?fn ([a-z_0-9]+)\s*\(/g) { push @f, [pos($src), $1, $-[0]]; }
    for my $i (0 .. $#f) {
      my $e = ($i < $#f) ? $f[$i+1][2] : length($src);
      my $b = substr($src, $f[$i][0], $e - $f[$i][0]);
      next unless $b =~ /\b(?:FROM|JOIN)\s+(?:$T)\b/i;
      my ($sig) = $b =~ /^(.*?\)\s*(?:->|\{))/s;
      next unless defined $sig;
      print "$ARGV:$f[$i][1]\n" if $sig =~ /:\s*&?AgentClient\b/;
    }
  ' "$f"
done | sort)
SVC_N=$(printf '%s\n' "$SVC_BAD" | grep -c . || true)
if [ "${SVC_N:-0}" -eq 0 ]; then
  ok "E4 no background service that walks a host-bearing table holds the panel's local AgentClient"
else
  while IFS=: read -r file fn; do
    [ -n "$fn" ] || continue
    bad "E4 $(basename "$file")::$fn walks a host-bearing table with the LOCAL AgentClient — unattended, so nothing downstream can catch it"
  done < <(printf '%s\n' "$SVC_BAD")
fi

# E5 — the AGENTLESS shape. Publishing DNS reaches the wrong host with no agent
# anywhere in the call, so every arm above is blind to it by construction: a site
# created on a fleet member got an A record pointing at the PANEL, and the delete
# path only removes a record whose content matches, so a member's record outlived
# its site as a dangling A record. `helpers::public_ip_for_server` is the form
# that asks which host the resource is actually on.
# Through `code()`, because the arms in this suite match SOURCE and not the prose
# that narrates them. The fix commit's own comments explain what
# `detect_public_ip` used to do and where it moved, and a raw grep read every one
# of those sentences as a live call site — the false-RED twin of the false-GREEN
# trap this helper exists for.
DNS_BAD=$(for f in panel/backend/src/routes/*.rs; do
  code "$f" | grep -n 'detect_public_ip' | sed "s|^|$f:|"
done || true)
DNS_N=$(printf '%s\n' "$DNS_BAD" | grep -c . || true)
if [ "${DNS_N:-0}" -eq 0 ]; then
  ok "E5 no route publishes the PANEL's own address for a resource that may live elsewhere"
else
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    bad "E5 ${line%%:*} still reads the panel's own public IP — use helpers::public_ip_for_server"
  done < <(printf '%s\n' "$DNS_BAD")
fi


echo
echo "----------------------------------------------"
printf '  PASS %d   FAIL %d   SKIP %d\n' "$PASS" "$FAIL" "$SKIP"
echo
[ "$FAIL" -eq 0 ] || exit 1
