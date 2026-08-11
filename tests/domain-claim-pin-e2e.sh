#!/usr/bin/env bash
# domain-claim-pin-e2e.sh — s294 / v2.52.0
#
# Pins the two halves of one mistake: nothing owned the question "may this domain
# be claimed?", and nothing owned the question "what was here before I wrote?"
#
#   §A  the GUARD exists and is complete — format, reserved, sites, git deploys
#       and, for the first time, DOCKER APPS, whose domain lives only in a
#       container label and which therefore no SQL guard could ever see.
#   §B  EVERY claim path calls it. This is the arm that matters. `sites.rs` had a
#       shared helper whose own comment said it existed "so the guard set create()
#       enforces cannot drift" — and two of eleven paths called it.
#   §C  the vhost write is NON-DESTRUCTIVE. Three writers replaced
#       /etc/nginx/sites-enabled/{domain}.conf and, when the whole-server
#       `nginx -t` failed, DELETED it — under a comment promising a restore.
#   §D  the Traefik route key is INJECTIVE. `.`→`-` mapped a.b.com and a-b.com
#       onto one file and one router name.
#
# Pure source analysis: no box, no network, no build.
#
# The arms are written against the CAPABILITY a regression must use, not today's
# spelling (lesson #122), and were attacked after they went green (lesson #132 —
# budget for the attack finding several, not one).
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

CLAIM=panel/backend/src/services/domain_claim.rs
SITES=panel/backend/src/routes/sites.rs
STAGING=panel/backend/src/routes/staging.rs
MIG=panel/backend/src/routes/migration.rs
GITDEP=panel/backend/src/routes/git_deploys.rs
APPS_BE=panel/backend/src/routes/docker_apps.rs
NGINX_ROUTE=panel/agent/src/routes/nginx.rs
NGINX_SVC=panel/agent/src/services/nginx.rs
APPS_ROUTE=panel/agent/src/routes/docker_apps.rs
GIT_BUILD=panel/agent/src/services/git_build.rs
TRAEFIK=panel/agent/src/services/traefik.rs

for f in "$CLAIM" "$SITES" "$STAGING" "$MIG" "$GITDEP" "$APPS_BE" \
         "$NGINX_ROUTE" "$NGINX_SVC" "$APPS_ROUTE" "$GIT_BUILD" "$TRAEFIK"; do
  [ -f "$f" ] || bad "MISSING SUBJECT FILE: $f"
done

# Comments out, CODE INTACT.
#
# The obvious `s{/\*.*?\*/}{}gs` is wrong and shipped in four suites before this
# one: `/*` occurs INSIDE string literals (a Dockerfile's
# `COPY --from=builder /app/target/release/*`, a glob, a regex), so it opened a
# "block comment" that ran to the next `*/` and deleted real code —
# git_build.rs 1214 -> 729 lines, agent/routes/nginx.rs 2263 -> 2145. A
# truncated subject makes an ABSENCE arm pass on code that was merely deleted by
# the stripper, which is the worst way for a pin to be wrong.
#
# So a block comment is only recognised where one is actually written: opening at
# the start of a line, closing at the end of one. A `/*` in the middle of a line
# is data.
code() {
  perl -0777 -pe '
    s{\{/\*.*?\*/\}}{}gs;
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
  ' "$1"
}

# An arm whose subject could not be extracted must SKIP, not print a confident
# green next to a red about the same subject (lesson #122b).
subj() { local t; t=$(code "$1"); [ -n "$t" ] || return 1; printf '%s' "$t"; }

has()   { grep -qE -- "$2" <<< "$1"; }
count() { grep -cE -- "$2" <<< "$1" || true; }

# Only DEAD-CODE markers belong here. An earlier suite also forbade `let _ =` and
# `return false;` and immediately reded three arms on code verified by hand
# minutes earlier — both are ordinary Rust (lesson #133). A guard added to make a
# pin stricter is itself new, untested code.
live() {
  ! grep -qE -- '(if false|&& false|\|\| true|let _unused)' <<< "$1"
}

# The body of one top-level fn, bounded by the NEXT top-level fn.
#
# A fixed `-A <n>` window is NOT a function (lesson #131): at s293 a per-parser
# arm stayed green because its 60-line window ran on into the next parser, whose
# line still matched. Every window below is a real body.
fnbody() {
  awk -v name="$2" '
    /^[[:space:]]*(pub )?(pub\(crate\) )?(async )?fn / {
      if ($0 ~ "(pub |pub\\(crate\\) )?(async )?fn " name "\\(") { inside=1; next }
      inside=0
    }
    inside { print }
  ' <<< "$1"
}

# Assert a window both matches and is live, in one place so no arm forgets.
derives() {
  local w="$1" pat="$2"
  [ -n "$w" ] && grep -qE -- "$pat" <<< "$w" && live "$w"
}

echo "== §A  one guard, and it can see all three kinds of owner =="

if S=$(subj "$CLAIM"); then
  B=$(fnbody "$S" ensure_claimable)
  if derives "$B" 'is_valid_domain'; then
    ok "the guard rejects malformed domains"
  else
    bad "the guard no longer checks domain format"
  fi
  # `is_reserved_domain_for`, NOT `is_reserved_domain`: BASE_URL is empty on a
  # routine install, so only the header-reading variant knows the panel's host.
  if derives "$B" 'is_reserved_domain_for'; then
    ok "the guard blocks the panel's own control-plane domain (header variant)"
  else
    bad "the guard no longer blocks reserved domains via the header variant"
  fi
  if has "$B" 'is_reserved_domain\(' ; then
    bad "the guard fell back to the BASE_URL-only reserved check"
  else
    ok "the guard does not use the BASE_URL-only reserved check"
  fi

  O=$(fnbody "$S" find_occupant)
  if derives "$O" 'lower\(domain\) = \$1' && [ "$(count "$O" 'lower\(domain\) = \$1')" -ge 2 ]; then
    ok "both SQL owners are matched case-insensitively"
  else
    bad "a SQL owner query is case-sensitive again (EXAMPLE.com would walk past example.com)"
  fi
  if derives "$O" 'FROM sites'; then
    ok "the guard consults the sites table"
  else
    bad "the guard no longer consults the sites table"
  fi
  if derives "$O" 'FROM git_deploys'; then
    ok "the guard consults git_deploys"
  else
    bad "the guard no longer consults git_deploys"
  fi
  # THE new leg. A Docker app is not a row; the only way to learn its domain is
  # to ask the agent, which has been returning it all along.
  if derives "$O" 'get\("/apps"\)'; then
    ok "the guard asks the agent for Docker-app domains"
  else
    bad "the guard no longer asks the agent for Docker-app domains"
  fi
  if derives "$O" 'Occupant::DockerApp'; then
    ok "a Docker app is reported as an occupant"
  else
    bad "a Docker app is no longer reported as an occupant"
  fi
  # FLEET-WIDE. This arm used to pin the opposite property — that ONE agent was
  # asked and that an unreachable one failed the claim closed. Both halves were
  # correct for a single handle and both were replaced at s322, so the arm was
  # rewritten rather than deleted: the three SQL legs above are fleet-wide, an
  # app's domain is invisible to SQL, and asking only the caller's host meant a
  # domain held by an app on host B passed a claim made on host A.
  if derives "$O" 'online_fleet\('; then
    ok "the Docker-app leg asks every online member, like the SQL legs above it"
  else
    bad "the Docker-app leg is back to asking a single host — a domain held by an app on another member passes"
  fi
  # A member that will not answer is REPORTED, never silently skipped. Failing
  # closed across a whole fleet would let one sick box block every domain claim
  # everywhere, so the trade is deliberate — but it must be audible, because
  # failing open in silence is what produced the bug in the first place.
  # Flattened: `has` is line-based grep, and the match spans the `Err(e) => {`
  # newline. An arm that can never match is an arm that reports on nothing.
  if has "$(tr '\n' ' ' <<< "$O")" 'Err\(e\)[^}]*tracing::warn!'; then
    ok "a member that cannot be asked is logged by name rather than treated as free"
  else
    bad "the Docker-app leg skips an unreachable member without saying so — that is failing open in silence"
  fi
  # FLATTENED (#383). rustfmt breaks this chain at every `.`, so `get("/apps")`
  # and the `unwrap_or` that swallows its error land on different lines and a
  # line-oriented grep sees NEITHER — an ABSENCE arm that cannot match reports
  # "clean" about code it cannot see. The gap is BOUNDED to one statement
  # (`[^;]`), never `.*`: flattening widens matching, and a widened absence arm
  # must still not be able to leap a statement boundary.
  OFLAT=$(tr '\n' ' ' <<< "$O" | tr -s ' ')
  if has "$OFLAT" 'get\( *"/apps" *\)[^;]{0,160}unwrap_or'; then
    bad "the Docker-app leg swallows agent errors (fails OPEN)"
  else
    ok "the Docker-app leg does not swallow agent errors"
  fi

  N=$(fnbody "$S" normalise)
  if derives "$N" 'to_ascii_lowercase|to_lowercase'; then
    ok "normalise lowercases"
  else
    bad "normalise no longer lowercases — the case bypass is back"
  fi
fi

echo
echo "== §B  every path that can claim a domain calls it =="

# The anti-drift arm. Each of these is a path that causes a vhost to be written.
# A new one that does not appear here is exactly the regression this suite is for.
check_calls_guard() {
  local file="$1" fn="$2" label="$3"
  local S B
  S=$(subj "$file") || { bad "$label: subject unreadable"; return; }
  B=$(fnbody "$S" "$fn")
  if [ -z "$B" ]; then
    bad "$label: could not extract fn $fn — the arm is not measuring anything"
    return
  fi
  if ! derives "$B" 'ensure_claimable|ensure_domain_available'; then
    bad "$label no longer claims through the shared guard"
    return
  fi
  # Calling it is not using it. Probe P2 kept the call and threw the result away
  # (`.await.ok()`, then stored the raw body value) and walked through a presence
  # arm — the discarded-result class that also beat the s293 draft (lesson #132).
  # The guard returns the NORMALISED domain and propagates rejection through `?`,
  # so `.await?` is the thing that makes the claim actually govern this path.
  # Flattened, because the call is formatted across many lines.
  local F; F=$(tr '\n' ' ' <<< "$B")
  if grep -qE -- '(let[[:space:]]+_[[:space:]]*=[[:space:]]*[a-z_:]*ensure_claimable|ensure_claimable\([^;]*\)[[:space:]]*\.await[[:space:]]*\.[[:space:]]*(ok|unwrap))' <<< "$F"; then
    bad "$label discards the guard's result instead of using it"
  elif grep -qE -- '(ensure_claimable|ensure_domain_available)\([^;]*\)[[:space:]]*\.await\?' <<< "$F"; then
    ok "$label claims through the shared guard, and uses what it returns"
  else
    bad "$label does not propagate the guard's verdict"
  fi
}

check_calls_guard "$SITES"   create          "sites::create"
check_calls_guard "$SITES"   rename_domain   "sites::rename_domain"
check_calls_guard "$SITES"   clone_site      "sites::clone_site"
check_calls_guard "$SITES"   add_alias       "sites::add_alias"
check_calls_guard "$STAGING" create          "staging::create"
check_calls_guard "$GITDEP"  create          "git_deploys::create"
check_calls_guard "$GITDEP"  update          "git_deploys::update"
check_calls_guard "$APPS_BE" deploy          "docker_apps::deploy"

# The two that are not plain fn bodies: the migration import's work happens in a
# spawned task, and the preview path reports rather than rejects.
if S=$(subj "$MIG"); then
  if derives "$S" 'ensure_claimable'; then
    ok "migration::import claims through the shared guard"
  else
    bad "migration::import no longer claims through the shared guard"
  fi
fi
if S=$(subj "$GITDEP"); then
  B=$(fnbody "$S" handle_preview_deploy)
  if derives "$B" 'find_occupant'; then
    ok "the preview path checks the synthesised domain before taking it"
  else
    bad "the preview path takes {branch}.{domain} without checking it"
  fi
  # It must ALSO not trust the webhook caller's Host header to decide what is
  # reserved — that request is unauthenticated.
  if has "$B" 'is_reserved_domain_for'; then
    bad "the preview path trusts an attacker-supplied Host header for reserved checks"
  else
    ok "the preview path does not trust the webhook's Host header"
  fi
fi

# The delegation itself: sites.rs must not have grown its own copy back.
if S=$(subj "$SITES"); then
  E=$(fnbody "$S" ensure_domain_available)
  if derives "$E" 'domain_claim::ensure_claimable'; then
    ok "sites' local helper delegates to the shared guard"
  else
    bad "sites' local helper stopped delegating — the guards can drift again"
  fi
fi

echo
echo "== §B2  no claim path keeps a private conflict query =="

# The negative that makes §B stick. A domain-conflict SELECT anywhere but the
# guard means some path can answer the question its own way again, which is how
# `create` and `update` came to disagree in the first place.
for f in "$SITES" "$STAGING" "$GITDEP" "$APPS_BE" "$MIG"; do
  if S=$(subj "$f"); then
    # Anchored on the opening quote: sites.rs:883 has
    #   VALUES ((SELECT id FROM sites WHERE domain = $1), 'mysql', ...)
    # which is a value lookup inside a database-record INSERT, not a conflict
    # guard. An unanchored pattern reads it as one and reds a correct file.
    n=$(count "$S" '"SELECT id FROM (sites|git_deploys) WHERE (lower\()?domain')
    if [ "$n" -eq 0 ]; then
      ok "$(basename "$f") holds no private domain-conflict query"
    else
      bad "$(basename "$f") has $n private domain-conflict quer(y|ies) again"
    fi
  fi
done

echo
echo "== §C  a write never destroys what it replaced =="

if S=$(subj "$NGINX_SVC"); then
  R=$(fnbody "$S" restore_or_remove)
  if [ -z "$R" ]; then
    bad "restore_or_remove is gone — nothing can put a replaced vhost back"
  else
    # It must actually WRITE the previous content. A version that only logs, or
    # only removes, satisfies a presence grep and restores nothing.
    if derives "$R" 'std::fs::write\(&tmp_path, prev\)'; then
      ok "restore_or_remove writes the previous content back"
    else
      bad "restore_or_remove no longer writes the previous content back"
    fi
    if derives "$R" 'rename\(&tmp_path, config_path\)'; then
      ok "the restore is atomic (tmp + rename), like the write it undoes"
    else
      bad "the restore is no longer atomic"
    fi
  fi
fi

# All three writers: snapshot before, restore after. Named per writer so a
# regression says WHICH one lost it.
# $2 is a |-separated list of the function the write may live in. The vhost
# write moved out of `deploy` and into the shared `expose_domain` at v2.54.0 so
# the Compose-stack path could not grow a second, unguarded copy of it; the arm
# follows the write rather than the name it used to sit under. The FIRST
# non-empty body wins, and an empty result across all of them still reds.
check_writer() {
  local file="$1" fns="$2" label="$3"
  local S B fn
  S=$(subj "$file") || { bad "$label: subject unreadable"; return; }
  B=""
  for fn in ${fns//|/ }; do
    B=$(fnbody "$S" "$fn")
    [ -n "$B" ] && break
  done
  if [ -z "$B" ]; then
    bad "$label: could not extract any of [$fns] — the arm is not measuring anything"
    return
  fi
  # A body with no vhost write in it is not the writer, and every arm below
  # would pass on it vacuously — the third one especially, since a body with no
  # deletes has no stray deletes.
  if ! derives "$B" 'fs::write\(&tmp_path|fs::write\(&config_path'; then
    bad "$label: the extracted body writes no vhost — the arm is pointed at the wrong function"
    return
  fi
  if derives "$B" 'read_to_string\(&config_path\)'; then
    ok "$label snapshots the existing vhost before replacing it"
  else
    bad "$label replaces a vhost without reading what was there"
  fi
  if derives "$B" 'restore_or_remove\(&config_path'; then
    ok "$label restores through restore_or_remove on failure"
  else
    bad "$label no longer restores on failure"
  fi
  # The defect itself: a delete of the SHARED path. Keyed on the property, not on
  # one spelling — probe P8 renamed the variable (`let doomed = config_path.clone()`)
  # and walked straight through an arm that grepped `remove_file(&config_path)`.
  # So: every remove_file in this body must be cleaning up a tmp/restore file.
  # Nothing else may be deleted here, whatever it is called.
  local strays
  strays=$(grep -oE 'remove_file\([^)]*\)' <<< "$B" | grep -cvE 'tmp_path|\.tmp|\.restore' || true)
  if [ "$strays" -eq 0 ]; then
    ok "$label deletes nothing but its own temporary files"
  else
    bad "$label has $strays delete(s) of something other than a temp file"
  fi
}

check_writer "$NGINX_ROUTE" put_site   "put_site"
check_writer "$APPS_ROUTE"  "expose_domain|deploy" "the Docker-app auto-proxy write"

# ...and the extraction must stay reachable. A shared writer nothing calls is a
# refactor that quietly deleted a feature.
ARD=$(subj "$APPS_ROUTE") || ARD=""
if [ -n "$ARD" ]; then
  DEP=$(fnbody "$ARD" "deploy")
  if derives "$DEP" 'expose_domain' || derives "$DEP" 'fs::write\(&tmp_path'; then
    ok "the app deploy still reaches the vhost writer"
  else
    bad "the app deploy no longer writes a vhost at all"
  fi
fi

# git_build's writer returns a String error rather than an HTTP response, so it
# is checked by name rather than through check_writer.
#
# Scoped to setup_nginx_proxy's BODY, not the file: git_build.rs:643 holds a
# Dockerfile template containing `collectstatic --noinput 2>/dev/null || true`,
# and a file-wide liveness check reads that shell idiom as a neutralised
# condition. Lesson #131's window bleed, widened from a function to a file.
if S=$(subj "$GIT_BUILD"); then
  B=$(fnbody "$S" setup_nginx_proxy)
  if [ -z "$B" ]; then
    bad "could not extract setup_nginx_proxy — the arm is not measuring anything"
  elif derives "$B" 'read_to_string\(&config_path\)' \
       && derives "$B" 'restore_or_remove\(&config_path'; then
    ok "the git-deploy writer snapshots and restores"
  else
    bad "the git-deploy writer no longer snapshots and restores"
  fi
  strays=$(grep -oE 'remove_file\([^)]*\)' <<< "$B" | grep -cvE 'tmp_path|\.tmp|\.restore' || true)
  if [ "$strays" -eq 0 ]; then
    ok "the git-deploy writer deletes nothing but its own temporary files"
  else
    bad "the git-deploy writer has $strays delete(s) of something other than a temp file"
  fi
fi

# The comment that lied for as long as the code did. If a future edit reinstates
# a promise of a restore, the restore had better be there — but the arms above
# already assert that, so this one only forbids the exact stale sentence.
if S=$(subj "$NGINX_ROUTE"); then
  if grep -qE 'remove it and restore' panel/agent/src/routes/nginx.rs; then
    bad "the 'remove it and restore' comment is back over code that only removes"
  else
    ok "no comment promises a restore that the code does not perform"
  fi
fi

echo
echo "== §D  the Traefik route key is injective =="

if S=$(subj "$TRAEFIK"); then
  K=$(fnbody "$S" route_key)
  if [ -z "$K" ]; then
    bad "route_key is gone — the route filename mangling is unowned again"
  else
    # Escaping the literal '-' BEFORE mapping '.' is what makes it injective.
    if derives "$K" "'-' => out.push_str\(\"--\"\)"; then
      ok "a literal hyphen is escaped, so a lone hyphen can only be a dot"
    else
      bad "route_key no longer escapes literal hyphens — a.b.com and a-b.com collide again"
    fi
  fi
  # Only the deliberately-kept legacy path may use the ambiguous mangling.
  n=$(count "$S" "replace\('\.', \"-\"\)")
  if [ "$n" -le 1 ]; then
    ok "the ambiguous mangling survives only in the legacy-cleanup path"
  else
    bad "$n live uses of the ambiguous dot-to-hyphen mangling"
  fi
  W=$(fnbody "$S" write_route_config)
  if derives "$W" 'route_key\(domain\)'; then
    ok "the writer uses the injective key"
  else
    bad "the writer no longer uses the injective key"
  fi
  P=$(fnbody "$S" route_config_path)
  if derives "$P" 'route_key\(domain\)'; then
    ok "the reader derives the same key as the writer"
  else
    bad "reader and writer can disagree about where a route file lives"
  fi
fi

echo
echo "== §E  the guard runs BEFORE the side effects =="

# docker_apps::deploy creates a DNS A record inside its spawned task. A domain
# rejected after that point still leaves a live record behind, so the claim has
# to be settled before the spawn — ordering, which no presence grep can see.
if S=$(subj "$APPS_BE"); then
  B=$(fnbody "$S" deploy)
  g=$(grep -nE 'ensure_claimable' <<< "$B" | head -1 | cut -d: -f1)
  s=$(grep -nE 'tokio::spawn' <<< "$B" | head -1 | cut -d: -f1)
  if [ -n "$g" ] && [ -n "$s" ] && [ "$g" -lt "$s" ]; then
    ok "the app deploy settles its domain claim before spawning (and before DNS)"
  else
    bad "the app deploy's domain claim no longer precedes the spawned side effects"
  fi
fi

echo
echo "== §F  the client gate lives at the choke point, and no caller can fake it =="

# s312 / GitHub #51. A `client` holds sites and may not bring a NEW domain into
# service. The gate is one check inside ensure_claimable rather than a role check
# at each creating handler, for the reason this whole file exists: git_deploys,
# docker_apps and stacks all materialise a served vhost WITHOUT inserting into
# `sites`, so gates bolted onto the four `INSERT INTO sites` sites would have left
# three doors open. These arms pin the choke point, not the spelling.
if S=$(subj "$CLAIM"); then
  B=$(fnbody "$S" ensure_claimable)
  if [ -z "$B" ]; then
    bad "§F: could not extract ensure_claimable — the arms are not measuring anything"
  else
    if derives "$B" 'may_claim_new'; then
      ok "the shared guard consults the claim-permission rule"
    else
      bad "the shared guard no longer consults the claim-permission rule"
    fi
    # Ordering: refusing AFTER the occupancy lookup would let a client probe which
    # domains are free by reading which error comes back.
    g=$(grep -nE 'may_claim_new' <<< "$B" | head -1 | cut -d: -f1)
    o=$(grep -nE 'find_occupant' <<< "$B" | head -1 | cut -d: -f1)
    if [ -n "$g" ] && [ -n "$o" ] && [ "$g" -lt "$o" ]; then
      ok "the permission check precedes the occupancy lookup"
    else
      bad "a refused claimant still learns whether the domain was free"
    fi
  fi

  R=$(fnbody "$S" may_claim_new)
  if [ -z "$R" ]; then
    bad "§F: could not extract may_claim_new"
  elif derives "$R" 'Holder::New'; then
    ok "the rule is keyed on the NEW-domain transition, not on a resource kind"
  else
    bad "the rule stopped keying on Holder::New — a rename may now be refused, or a create allowed"
  fi
fi

# The arm that matters most. Every call site must pass a role it READ from the
# request; a literal makes the gate vacuous while leaving every other arm green.
# Flattened, because these calls are formatted across many lines.
BYPASS=0
for f in "$SITES" "$STAGING" "$MIG" "$GITDEP" "$APPS_BE" panel/backend/src/routes/stacks.rs; do
  S=$(subj "$f") || continue
  F=$(tr '\n' ' ' <<< "$S")
  # A string literal in the role position of a call to the guard.
  while IFS= read -r hit; do
    [ -z "$hit" ] && continue
    BYPASS=$((BYPASS+1))
    bad "$(basename "$f") passes a hardcoded role to the guard: $hit"
  done < <(grep -oE 'ensure_claimable\([^;]*"(admin|reseller|user|client)"[^;]*\)' <<< "$F")
done
[ "$BYPASS" -eq 0 ] && ok "no claim path hardcodes the role the gate reads"

# And the rule must actually be reachable from a real role value: the string the
# gate compares against has to be the same one the database CHECK permits.
MIGR=$(ls panel/backend/migrations/*client_role.sql 2>/dev/null | head -1)
if [ -n "$MIGR" ] && [ -f "$MIGR" ]; then
  if grep -qE "CHECK \(role IN \(.*'client'.*\)\)" "$MIGR"; then
    ok "the database permits the role value the gate refuses on"
  else
    bad "the migration does not admit 'client' — the gate can never fire"
  fi
else
  bad "no client-role migration found — the gate refuses a value no account can hold"
fi

echo "== §G  a mail domain is a domain HOLDER, and only an admin may claim over it =="

# s347 / GitHub #106. Client-scoped mail matches `mail_domains.domain` against the
# domain of a site the caller owns — which makes `sites.domain` an authorisation
# key, and that column is writable by the very account being authorised. Before
# this, the occupancy oracle asked sites, git deploys, stacks and each agent's
# apps, and never asked about mail: so a non-admin could point a site they already
# owned at a name whose mailboxes existed and mint their own access to them. The
# collision had no nginx symptom, which is why it survived — a mail-only domain
# has no vhost to overwrite.

if S=$(subj "$CLAIM"); then
  B=$(fnbody "$S" find_occupant)
  if [ -z "$B" ]; then
    bad "§G: could not extract find_occupant"
  else
    if derives "$B" 'mail_domains'; then
      ok "the occupancy oracle asks whether a mail domain holds the name"
    else
      bad "find_occupant no longer asks about mail domains — a non-admin can claim a name whose mailboxes exist"
    fi

    # Ordering is load-bearing, not cosmetic. `ensure_claimable` TOLERATES this
    # occupant for an administrator, so a mail domain reported IN PLACE OF a vhost
    # holder would let an admin claim over a live site, git deploy, stack or app.
    # Asking last makes that impossible by construction.
    m=$(grep -nE 'mail_domains' <<< "$B" | head -1 | cut -d: -f1)
    a=$(grep -nE '"/apps"' <<< "$B" | head -1 | cut -d: -f1)
    if [ -n "$m" ] && [ -n "$a" ] && [ "$m" -gt "$a" ]; then
      ok "the mail question is asked LAST, after every holder that must always refuse"
    else
      bad "the mail leg no longer runs last — a tolerated occupant can now mask a vhost collision"
    fi

    # Fleet-wide and case-folded, like the three SQL legs above it. A server term
    # here would let the same name be claimed on a second host while its mailboxes
    # live on the first.
    q=$(grep -oE 'SELECT id FROM mail_domains[^"]*' <<< "$B" | head -1)
    if [ -n "$q" ] && ! grep -qE 'server_id' <<< "$q"; then
      ok "the mail leg is fleet-wide, matching the sites/git/stack legs"
    else
      bad "the mail leg grew a server term — a name can be claimed on one host while its mail lives on another"
    fi
    if [ -n "$q" ] && grep -qE 'lower\(domain\)' <<< "$q"; then
      ok "the mail leg folds case rather than trusting the writer's convention"
    else
      bad "the mail comparison is case-sensitive — EXAMPLE.com walks past a row holding example.com"
    fi
  fi

  E=$(fnbody "$S" ensure_claimable)
  if [ -z "$E" ]; then
    bad "§G: could not extract ensure_claimable"
  else
    if derives "$E" 'may_claim_mail_held'; then
      ok "the guard consults the mail-claim rule before tolerating an occupant"
    else
      bad "the guard stopped consulting the mail-claim rule — either every claim is refused, or every claim is allowed"
    fi
    # The tolerance must name exactly ONE variant. Folding a second occupant into
    # that branch would let an administrator claim over a live vhost, and it would
    # look like a one-word change.
    #
    # ⚠ OCCURRENCES, not lines. The shared `count` is `grep -c`, which counts
    # matching LINES — and the natural way to write this regression puts the
    # second variant on the SAME line (`!= A && != B`), so a line count reports
    # one and the arm passes. Measured: that mutation SURVIVED this arm until it
    # was rewritten to count occurrences. `wc -l` on `grep -o` output is safe
    # under pipefail — wc consumes all input, so there is no SIGPIPE (unlike a
    # pipe into `grep -q`).
    n=$(grep -oE 'Occupant::' <<< "$E" | wc -l)
    if [ "$n" -eq 1 ]; then
      ok "exactly one occupant is tolerated, and it is named explicitly"
    else
      bad "the tolerance branch names $n occupants — it must name only the mail one"
    fi
    if derives "$E" 'Occupant::MailDomain'; then
      ok "the tolerated occupant is the mail domain"
    else
      bad "the tolerance is no longer keyed on the mail occupant"
    fi
  fi

  # Fails CLOSED, unlike its sibling. `may_claim_new` returns true for a role it
  # does not recognise, so drift merely un-restricts. This one must return false,
  # so drift refuses — an unknown role string must never be able to take a mailbox.
  G=$(fnbody "$S" may_claim_mail_held)
  if [ -z "$G" ]; then
    bad "§G: could not extract may_claim_mail_held"
  elif derives "$G" 'role == "admin"'; then
    ok "the mail gate admits the administrator role and refuses everything else"
  else
    bad "the mail gate stopped failing closed — a role it does not recognise may now claim a name whose mailboxes exist"
  fi

  # The refusal has to say WHAT holds the name. "In use" with no owner is the
  # sentence this whole module was written to stop printing.
  if derives "$S" 'Occupant::MailDomain =>'; then
    ok "the mail occupant carries a message of its own"
  else
    bad "the mail occupant has no message — the refusal cannot name what holds the domain"
  fi
fi

echo
echo "──────────────────────────────────────────"
echo "PASS: $PASS   FAIL: $FAIL"
[ "$FAIL" -eq 0 ]
