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
  # FAILS CLOSED. `unwrap_or_default()` here would turn an unreachable agent into
  # "no apps own this domain" — a guard that cannot see is the whole bug.
  if has "$O" 'get\("/apps"\)[^;]*\.await[^;]*\n?[[:space:]]*\.map_err' \
     || derives "$O" 'map_err\(\|e\| agent_error'; then
    ok "an unreachable agent fails the claim rather than allowing it"
  else
    bad "the Docker-app leg no longer fails closed"
  fi
  if has "$O" 'get\("/apps"\).*unwrap_or'; then
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
check_writer() {
  local file="$1" fn="$2" label="$3"
  local S B
  S=$(subj "$file") || { bad "$label: subject unreadable"; return; }
  B=$(fnbody "$S" "$fn")
  if [ -z "$B" ]; then
    bad "$label: could not extract fn $fn — the arm is not measuring anything"
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
check_writer "$APPS_ROUTE"  deploy     "the Docker-app auto-proxy write"

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
echo "──────────────────────────────────────────"
echo "PASS: $PASS   FAIL: $FAIL"
[ "$FAIL" -eq 0 ]
