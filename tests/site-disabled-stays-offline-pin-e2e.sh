#!/usr/bin/env bash
# site-disabled-stays-offline-pin-e2e.sh — s317 / v2.74.0
#
# THE SENTENCE THIS SUITE EXISTS TO PROVE, written out so a mutation can be aimed
# at it rather than at whatever the arms happen to catch (lesson #237, and #243 —
# an arm that pins a NAME instead of the DECISION is green through the change it
# was written for):
#
#   "A site the operator disabled stays offline until somebody enables it — no
#    render, deploy, certificate or unattended loop may replace what nginx is
#    serving for that name — and what comes back when it is enabled is the site
#    as it is now, not as it was when it went offline."
#
# Both halves matter and they failed differently.
#
# The FIRST half broke because disabling does not remove a vhost: it parks the
# real body beside the live path and leaves a maintenance stub in service. Five
# writers each named the in-service path directly, so an ordinary settings change
# — PHP version, WAF mode, CSP, cache toggle, custom nginx, a git deploy,
# exposing a container, provisioning a certificate — wrote a working vhost over
# the stub and reloaded. The certificate-renewal loop did it with nobody
# watching. `sites.enabled` had exactly ONE reader in the whole backend: the
# idempotence guard inside the toggle that writes it.
#
# The SECOND half broke because enabling restores the body frozen at the moment
# the site went offline, so every setting changed in between was silently
# reverted — and a site that gained a certificate while offline came back on
# plain HTTP, because the frozen body has no TLS listener.
#
# So the mutations this suite must survive are WIDENING mutations — send a render
# into service for a site that has a parked body, or let an unattended loop push
# a vhost for a disabled site — not deletions. Every arm below was mutation-
# tested to redden ALONE.
#
#   §A  the decision is ONE function, reachable without a box
#   §B  every writer that renders a full vhost asks it where the render goes
#   §C  a write that did not go into service does not claim a reload
#   §D  what comes back when a site is enabled is the site as it is now
#   §E  the unattended loops decline for a disabled site
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

NGX_SVC=panel/agent/src/services/nginx.rs
NGX_RT=panel/agent/src/routes/nginx.rs
SSL_SVC=panel/agent/src/services/ssl.rs
GIT_SVC=panel/agent/src/services/git_build.rs
DAPPS_RT=panel/agent/src/routes/docker_apps.rs
SCANNER=panel/backend/src/services/security_scanner.rs
HEALER=panel/backend/src/services/auto_healer.rs

for f in "$NGX_SVC" "$NGX_RT" "$SSL_SVC" "$GIT_SVC" "$DAPPS_RT" "$SCANNER" "$HEALER"; do
  [ -f "$f" ] || bad "MISSING SUBJECT FILE: $f"
done

# Comments out, CODE INTACT.
#
# The obvious `s{/\*.*?\*/}{}gs` is wrong and shipped in four suites before the
# fix: `/*` occurs INSIDE string literals, so it opened a "block comment" that ran
# to the next `*/` and deleted real code. A truncated subject makes an ABSENCE arm
# pass on code the stripper merely removed, which is the worst way to be wrong.
#
# Load-bearing here for a second reason: §A3 asserts the ABSENCE of a filename
# suffix that this fix's own explanatory comments legitimately discuss, and one
# such comment is in a pinned file already (`feedback_source_pin_prose_trap` — a
# pin greps raw source, so it reads the prose that narrates the check as though
# it were the check).
code() {
  perl -0777 -pe '
    s{\{/\*.*?\*/\}}{}gs;
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
  ' "$1"
}

# An arm whose subject could not be extracted must SKIP, not print a confident
# green next to a red about the same subject.
subj() { local t; t=$(code "$1"); [ -n "$t" ] || return 1; printf '%s' "$t"; }

has()   { grep -qE -- "$2" <<< "$1"; }
count() { grep -cE -- "$2" <<< "$1" || true; }

# `grep -l` over CODE, not over raw bytes.
#
# ⚠ THE TRAP THIS EXISTS FOR (lesson #244). `grep -rl` reads a file as written, so
# it matches the prose that NARRATES a call exactly as readily as the call itself.
# Every census below strips first.
files_matching() {
  local pat="$1"; shift
  local f out=""
  for f in "$@"; do
    [ -f "$f" ] || continue
    if grep -qE -- "$pat" <<< "$(code "$f")"; then out="$out$f "; fi
  done
  printf '%s' "$out"
}

AGENT_RS=$(find panel/agent/src -name '*.rs' | sort)
# An arm that ENUMERATES its own subjects must assert the enumeration first:
# `find` outside a checkout returns nothing, and a census over zero files is
# green by construction.
[ "$(wc -w <<< "$AGENT_RS")" -ge 20 ] \
  && ok "enumerated $(wc -w <<< "$AGENT_RS") agent source files to census" \
  || bad "the agent source enumeration came back with $(wc -w <<< "$AGENT_RS") files — every census below would be vacuous"

# One function's body, bounded by its OWN closing brace — a `}` in column 0 —
# and never by a comment or a fixed line offset.
fn_body() { awk -v pat="$1" '$0 ~ pat {f=1} f {print} f && /^\}/ {exit}' <<< "$2"; }

# Does `first` appear before `second` inside `body`? Used where ORDER is the
# invariant: an early return that lands AFTER the reload it exists to skip is not
# an early return.
before() {
  local body="$1" first="$2" second="$3" a b
  a=$(grep -nE -- "$first" <<< "$body" | head -1 | cut -d: -f1)
  b=$(grep -nE -- "$second" <<< "$body" | head -1 | cut -d: -f1)
  [ -n "$a" ] && [ -n "$b" ] && [ "$a" -lt "$b" ]
}

NGX_SVC_S=$(subj "$NGX_SVC" || true)
NGX_RT_S=$(subj "$NGX_RT" || true)
SSL_SVC_S=$(subj "$SSL_SVC" || true)
GIT_SVC_S=$(subj "$GIT_SVC" || true)
DAPPS_RT_S=$(subj "$DAPPS_RT" || true)
SCANNER_S=$(subj "$SCANNER" || true)
HEALER_S=$(subj "$HEALER" || true)

echo "§A  the decision is ONE function, reachable without a box"

# A1 — the decision itself, not a name. It takes BOTH candidate paths as
# arguments, which is what makes it reachable from a unit test: while it lived
# inline in five writers there was nothing a test could hold (lesson #243).
if [ -z "$NGX_SVC_S" ]; then
  bad "A1 SKIPPED: $NGX_SVC did not yield source"
elif has "$NGX_SVC_S" 'fn vhost_target_between\(' \
  && has "$NGX_SVC_S" 'Path::new\(parked\)\.exists\(\)'; then
  ok "A1 the choice between an in-service path and a parked one is a function taking both, deciding on the parked file's existence"
else
  bad "A1 the vhost destination is no longer decided by a function taking both paths — an inline check per writer is what this suite exists to prevent"
fi

# A2 — the two outcomes are distinguishable by callers, and the distinction is
# what gates the reload. An enum whose variants collapse would let §C pass
# vacuously.
#      ⚠ `fn is_live` PRESENT is not the assertion — a body rewritten to `true`
#      keeps the name and turns every §C guard vacuous. The arm reads the body.
if [ -z "$NGX_SVC_S" ]; then
  bad "A2 SKIPPED: $NGX_SVC did not yield source"
else
  IS_LIVE=$(fn_body 'fn is_live' "$NGX_SVC_S")
  if has "$NGX_SVC_S" 'enum VhostTarget' \
    && has "$NGX_SVC_S" 'Live\(String\)' \
    && has "$NGX_SVC_S" 'Parked\(String\)' \
    && [ -n "$IS_LIVE" ] \
    && has "$IS_LIVE" 'VhostTarget::Live'; then
    ok "A2 a caller can tell an in-service write from a parked one, and the answer is derived from the variant"
  else
    bad "A2 the in-service and parked outcomes are no longer distinguishable — every reload guard downstream becomes vacuous"
  fi
fi

# A3 — CENSUS over stripped code: the parked filename is spelled in exactly one
# file. Three call sites building it by hand is how the parked body ended up with
# no reader outside the pair that wrote it.
PARKED_SPELLERS=$(files_matching 'conf\.disabled' $AGENT_RS)
if [ "$(tr -s ' ' '\n' <<< "$PARKED_SPELLERS" | grep -c .)" = "1" ] \
  && [[ "$PARKED_SPELLERS" == *"$NGX_SVC"* ]]; then
  ok "A3 the parked filename is spelled in $NGX_SVC and nowhere else in the agent"
else
  bad "A3 the parked filename is spelled in: ${PARKED_SPELLERS:-nowhere} — it belongs in $NGX_SVC alone"
fi

echo "§B  every writer that renders a full vhost asks where the render goes"

# B1 — THE CENSUS THIS SUITE TURNS ON. Rendering a site config is the choke point
# the writers DO share; choosing a destination is the one they did not. So the set
# of files that render must equal the set that ask. A sixth writer added later
# fails this arm on the day it is written, which is the only way this class of bug
# stays fixed.
RENDERERS=$(files_matching 'render_site_config\(' $AGENT_RS)
ASKERS=$(files_matching 'vhost_target(_between)?\(' $AGENT_RS)
if [ "$RENDERERS" = "$ASKERS" ] && [ -n "$RENDERERS" ]; then
  ok "B1 every file that renders a site vhost also asks where it goes ($(tr -s ' ' '\n' <<< "$RENDERERS" | grep -c .) files, sets identical)"
else
  bad "B1 renders-a-vhost and asks-where-it-goes disagree — renders: [${RENDERERS:-none}] asks: [${ASKERS:-none}]"
fi

# B2–B5 — each writer named SEPARATELY. An arm that ORs the four lets one file
# reaching the helper count for all of them (lesson #244).
b_writer() {
  local label="$1" src="$2" fnpat="$3" file="$4"
  if [ -z "$src" ]; then
    bad "$label SKIPPED: $file did not yield source"
    return
  fi
  local body; body=$(fn_body "$fnpat" "$src")
  if [ -z "$body" ]; then
    bad "$label SKIPPED: could not bound the function matching /$fnpat/ in $file"
  elif has "$body" 'vhost_target\(' && has "$body" 'target\.path\(\)'; then
    ok "$label the writer in $file asks where its render goes, and writes where it was told"
  else
    bad "$label the writer in $file writes a rendered vhost without asking, or asks and ignores the answer — a disabled site is back on the internet"
  fi
}

b_writer "B2" "$NGX_RT_S"   'async fn put_site'              "$NGX_RT"
b_writer "B3" "$SSL_SVC_S"  'async fn enable_ssl_for_site'   "$SSL_SVC"
b_writer "B4" "$GIT_SVC_S"  'async fn setup_nginx_proxy'     "$GIT_SVC"
b_writer "B5" "$DAPPS_RT_S" 'async fn expose_domain'         "$DAPPS_RT"

echo "§C  a write that did not go into service does not claim a reload"

# The ORDER is the invariant, not the presence. A guard that runs after the
# reload it exists to skip is not a guard — and `is_live` appearing anywhere in
# the body would satisfy a presence check.
c_order() {
  local label="$1" src="$2" fnpat="$3" file="$4"
  if [ -z "$src" ]; then
    bad "$label SKIPPED: $file did not yield source"
    return
  fi
  local body; body=$(fn_body "$fnpat" "$src")
  if [ -z "$body" ]; then
    bad "$label SKIPPED: could not bound the function matching /$fnpat/ in $file"
  elif before "$body" 'is_live\(\)' 'nginx::reload\(\)|reload\(\)\.await'; then
    ok "$label $file stops before reloading when the write was parked"
  else
    bad "$label $file reloads nginx without first checking the write reached it — the parked branch reports a success nobody can see"
  fi
}

c_order "C1" "$NGX_RT_S"  'async fn put_site'            "$NGX_RT"
c_order "C2" "$SSL_SVC_S" 'async fn enable_ssl_for_site' "$SSL_SVC"
c_order "C3" "$GIT_SVC_S" 'async fn setup_nginx_proxy'   "$GIT_SVC"

echo "§D  what comes back when a site is enabled is the site as it is now"

# D1 — the pair that parks and restores reads its two names from the one
# function, so the writer of the parked body and every writer that must notice it
# cannot drift apart. Named separately, not ORed.
d_pair() {
  local label="$1" fnpat="$2"
  if [ -z "$NGX_RT_S" ]; then
    bad "$label SKIPPED: $NGX_RT did not yield source"
    return
  fi
  local body; body=$(fn_body "$fnpat" "$NGX_RT_S")
  if [ -z "$body" ]; then
    bad "$label SKIPPED: could not bound the function matching /$fnpat/"
  elif has "$body" 'vhost_paths\('; then
    ok "$label the function matching /$fnpat/ takes both paths from the one place that spells them"
  else
    bad "$label the function matching /$fnpat/ builds a vhost path by hand — the parked name now has two spellings"
  fi
}

d_pair "D1" 'async fn disable_site'
d_pair "D2" 'async fn enable_site'

# D3 — renaming a disabled site carries its parked body across. Without this the
# real body stays parked under the OLD name, where enabling will never look, and
# the site cannot be brought back through the panel at all.
if [ -z "$NGX_RT_S" ]; then
  bad "D3 SKIPPED: $NGX_RT did not yield source"
else
  RENAME_BODY=$(fn_body 'async fn rename_site' "$NGX_RT_S")
  if [ -z "$RENAME_BODY" ]; then
    bad "D3 SKIPPED: could not bound rename_site"
  elif [ "$(count "$RENAME_BODY" 'vhost_paths\(')" -ge 2 ] \
    && has "$RENAME_BODY" 'fs::write\(&new_parked' \
    && has "$RENAME_BODY" 'fs::remove_file\(&old_parked'; then
    ok "D3 renaming a disabled site carries its parked body to the new name"
  else
    bad "D3 renaming a disabled site strands its parked body under the old name — the site can no longer be enabled from the panel"
  fi
fi

echo "§E  the unattended loops decline for a disabled site"

# The panel-side half of the fix, and the one that reaches a fleet member whose
# agent has not been updated. Scoped to the function that holds the rebuild, not
# to the file: both of these files mention `enabled` in unrelated queries.
e_loop() {
  local label="$1" src="$2" fnpat="$3" file="$4"
  if [ -z "$src" ]; then
    bad "$label SKIPPED: $file did not yield source"
    return
  fi
  local body; body=$(fn_body "$fnpat" "$src")
  if [ -z "$body" ]; then
    bad "$label SKIPPED: could not bound the function matching /$fnpat/ in $file"
  elif ! has "$body" 'build_nginx_body\('; then
    bad "$label SKIPPED: the function matching /$fnpat/ in $file no longer rebuilds a vhost — re-aim this arm"
  elif before "$body" '!site\.enabled' 'build_nginx_body\('; then
    ok "$label $file skips the unattended vhost rebuild for a disabled site"
  else
    bad "$label $file pushes a vhost for a disabled site on an unattended schedule — this is the resurrection nobody is watching"
  fi
}

e_loop "E1" "$SCANNER_S" 'async fn auto_fix_safe_findings' "$SCANNER"
e_loop "E2" "$HEALER_S"  'async fn auto_renew_ssl'         "$HEALER"

echo "§F  the maintenance response answers where the site answered"

# THE ARM FOR THE DEFECT THE BOX FOUND AND THE SOURCE REVIEW DID NOT. The stub
# named `listen 80` while every template binds the server's address whenever the
# panel knows it, so nginx attached the stub to a different socket and never
# consulted it: disabling a site did nothing at all. Proven by A/B on one box —
# same stub, address-bound listener, 200 became 503.
if [ -z "$NGX_SVC_S" ]; then
  bad "F1 SKIPPED: $NGX_SVC did not yield source"
elif has "$NGX_SVC_S" 'fn listener_lines\(' \
  && has "$NGX_SVC_S" 'starts_with\("listen "\)' \
  && has "$NGX_SVC_S" 'starts_with\("ssl_certificate "\)'; then
  ok "F1 the listeners a maintenance response must answer on are derived from a vhost, certificates included"
else
  bad "F1 nothing derives a vhost's listeners — a maintenance response that names its own addresses is attached to a socket the site never used"
fi

# F2 — and the writer USES it. F1 passing while disable_site keeps its own
# hardcoded block is the shape lesson #243 is about.
if [ -z "$NGX_RT_S" ]; then
  bad "F2 SKIPPED: $NGX_RT did not yield source"
else
  DIS_BODY=$(fn_body 'async fn disable_site' "$NGX_RT_S")
  if [ -z "$DIS_BODY" ]; then
    bad "F2 SKIPPED: could not bound disable_site"
  elif has "$DIS_BODY" 'listener_lines\(' && before "$DIS_BODY" 'listener_lines\(' 'server_name \{domain\}'; then
    ok "F2 disabling a site builds its maintenance response on the addresses the site was answering on"
  else
    bad "F2 disable_site names its own listen addresses — the maintenance response will not be consulted and the site stays up"
  fi
fi

echo
echo "PASS $PASS / FAIL $FAIL"
[ "$FAIL" -eq 0 ]
