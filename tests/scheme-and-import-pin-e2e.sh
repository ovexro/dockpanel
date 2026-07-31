#!/usr/bin/env bash
# scheme-and-import-pin-e2e.sh — s293 / v2.51.0
#
# Pins two families of defect fixed this release. Pure source analysis: no box,
# no network, no build.
#
# They are one suite because they are one mistake made twice: the panel had two
# ends that disagreed about something each of them already knew.
#
#   §A  the SCHEME. Five places printed or fetched `https://<a user's domain>`
#       while the answer — a certificate, an ssl_email, an `ssl` flag in the very
#       response being read — was in scope. Issue #96.
#   §B  the DOCUMENT ROOT and the ARCHIVE ROOT. The migration importer wrote a
#       site's files one level above the directory its own vhost serves, and
#       resolved the archive root differently from the analyser that produced
#       the paths. Found while triaging issue #51.
#
# The arms are written against the CAPABILITY a regression must use, not the
# spelling the code happens to have today (lesson #122), and were attacked after
# they went green (lesson #125).
#
# NO PIPES INTO `grep -q`. Under `set -o pipefail` grep -q closes the pipe on its
# first match, the upstream dies of SIGPIPE (141), and pipefail reports the whole
# pipeline failed — so an arm goes red on correct code, non-deterministically.
# That bug has now shipped twice (backup-lands at s289, backup-truth at s292).
# Every arm here feeds grep a here-string.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

APPS_UI=panel/frontend/src/pages/Apps.tsx
APPS_BE=panel/backend/src/routes/docker_apps.rs
APPS_SVC=panel/agent/src/services/docker_apps.rs
NGINX_SVC=panel/agent/src/services/nginx.rs
NGINX_ROUTE=panel/agent/src/routes/nginx.rs
TRAEFIK=panel/agent/src/services/traefik.rs
GITDEP=panel/backend/src/routes/git_deploys.rs
SITES=panel/backend/src/routes/sites.rs
MIG_SVC=panel/agent/src/services/migration.rs
MIG_ROUTE=panel/agent/src/routes/migration.rs
MIG_BE=panel/backend/src/routes/migration.rs

for f in "$APPS_UI" "$APPS_BE" "$APPS_SVC" "$NGINX_SVC" "$NGINX_ROUTE" "$TRAEFIK" \
         "$GITDEP" "$SITES" "$MIG_SVC" "$MIG_ROUTE" "$MIG_BE"; do
  [ -f "$f" ] || bad "MISSING SUBJECT FILE: $f"
done

# Block comments first: stripping line comments first would let a `//` holding a
# `/*` eat the rest of the file.
code() { perl -0777 -pe 's{/\*.*?\*/}{}gs; s{^\s*//.*$}{}gm' "$1"; }

# An arm whose subject could not be extracted must SKIP, not print a confident
# green next to a red about the same subject (lesson #122b).
subj() { local t; t=$(code "$1"); [ -n "$t" ] || return 1; printf '%s' "$t"; }

has()   { grep -qE -- "$2" <<< "$1"; }
count() { grep -cE -- "$2" <<< "$1" || true; }

# The N lines following a marker — the window an arm is allowed to reason about.
win() { grep -A"${3:-20}" -E -- "$2" <<< "$1" || true; }

# A presence grep is not an arm. Every probe that beat the first draft of this
# suite kept the call in the file and made it not happen: `if false { … }`, a
# discarded `let _ = …`, a `&& false` bolted onto the condition. Same escape that
# beat backup-truth at s292 (lesson #125) — dead code satisfies a presence grep.
# So an arm asserts BOTH that the window derives its answer AND that the window
# is live.
# Only DEAD-CODE markers belong here. An earlier draft also forbade `let _ =`
# and `return false;`, and immediately reported three failures on code verified
# by hand minutes earlier — both are ordinary Rust (`domain_is_https` returns
# false when there is no vhost; the monitor INSERT discards its Result on
# purpose). That is the #127 tell: when a whole class of arms fails on code you
# have already verified, the harness is wrong, not the subject. Discards that
# DO matter are forbidden by name at the one arm that cares.
live() {
  ! grep -qE -- '(if false|&& false|\|\| true|let _unused)' <<< "$1"
}

# The body of one top-level fn, bounded by the NEXT top-level fn.
#
# A fixed `-A <n>` window is not a function. Probe P9 removed the directory from
# parse_plesk's database path and the per-parser arm stayed green, because the
# 90-line window ran on into parse_hestiacp — whose line still matched. A window
# that can be satisfied by the code AFTER the subject is not scoped to the
# subject.
fnbody() {
  awk -v name="$2" '
    /^(pub )?(async )?fn / {
      if ($0 ~ "^(pub )?(async )?fn " name "\\(") { inside=1; next }
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

echo "== §A  the scheme is derived, never assumed =="

if S=$(subj "$APPS_UI"); then
  # The link must consume a per-app boolean. Requiring the ternary INSIDE the
  # href template is what stops "compute app.ssl, then ignore it" — the shape
  # that walked through the backup suite at s292.
  if has "$S" 'href=\{`\$\{app\.ssl \? "https" : "http"\}://\$\{app\.domain\}`\}'; then
    ok "the app link picks its scheme from the app's own ssl flag"
  else
    bad "the app link no longer derives its scheme from app.ssl"
  fi
  if has "$S" 'href=\{`https://\$\{app\.'; then
    bad "an app link hardcodes https:// again"
  else
    ok "no app link hardcodes https://"
  fi
  # The opt-out has to reach the wire, not merely exist in component state.
  if has "$S" 'external_tls' && has "$S" 'externalTls'; then
    ok "the deploy form sends external_tls"
  else
    bad "the deploy form no longer sends external_tls"
  fi
  # …and it must suppress the certificate request, not sit beside it.
  if has "$S" '&& !externalTls \? \{ ssl_email'; then
    ok "ticking 'TLS terminated upstream' withholds ssl_email"
  else
    bad "ssl_email is sent even when TLS is terminated upstream"
  fi
fi

if S=$(subj "$APPS_SVC"); then
  if has "$S" 'pub ssl: bool'; then
    ok "DeployedApp carries the served scheme"
  else
    bad "DeployedApp lost its ssl field"
  fi
  # Must be FILLED, not merely declared. A struct field defaulted to false is a
  # different bug wearing the same shape.
  W=$(win "$S" 'let ssl = domain' 6)
  if derives "$W" 'domain_is_https' && has "$S" '^ *ssl,$'; then
    ok "the app list fills ssl by asking what the box serves"
  else
    bad "the app list no longer derives ssl from the served config"
  fi
fi

if S=$(subj "$NGINX_SVC"); then
  # The nginx answer must come from the vhost, and Traefik must be consulted
  # first — when Traefik owns a domain there is no nginx vhost to read, and
  # "no file" would otherwise be reported as "no TLS".
  W=$(win "$S" 'pub fn domain_is_https' 26)
  if derives "$W" 'route_is_tls' && derives "$W" 'lines\(\)\.any' && derives "$W" '443'; then
    ok "domain_is_https reads Traefik first, then the nginx vhost's listen line"
  else
    bad "domain_is_https no longer derives from the served configuration"
  fi
fi

if S=$(subj "$TRAEFIK"); then
  # One path builder, used by the writer's siblings and both readers, so a
  # rename of the mangling cannot desynchronise them.
  if has "$S" 'fn route_config_path' && has "$S" 'pub fn route_is_tls'; then
    ok "the Traefik route path has one builder and a TLS reader"
  else
    bad "the Traefik route path builder or its TLS reader is gone"
  fi
  # None means "not proxied by Traefik" and must stay distinguishable from false.
  if has "$S" 'pub fn route_is_tls\(domain: &str\) -> Option<bool>'; then
    ok "route_is_tls keeps 'not mine' distinct from 'no TLS'"
  else
    bad "route_is_tls collapsed 'not proxied here' into 'no TLS'"
  fi
fi

if S=$(subj "$APPS_BE"); then
  if has "$S" 'let url = format!\("https://\{domain\}"\)'; then
    bad "the auto-created app monitor hardcodes https again"
  else
    ok "the auto-created app monitor does not hardcode https"
  fi
  # It must read the deploy's own answer. Requiring `result.get("ssl")` in the
  # same file is not enough — the SSE step already does that — so require the
  # scheme binding to exist and the URL to interpolate it.
  W=$(win "$S" 'let scheme = if result\.get\("ssl"\)' 8)
  if derives "$W" 'https' && derives "$W" 'http' \
     && has "$S" 'format!\("\{scheme\}://\{domain\}"\)'; then
    ok "the monitor URL is built from the scheme the deploy reported"
  else
    bad "the monitor URL no longer follows the deploy's reported scheme"
  fi
  if has "$S" 'pub external_tls: bool'; then
    ok "the deploy request can decline a certificate"
  else
    bad "DeployRequest lost external_tls"
  fi
  # Declining must actually suppress the fallback to the operator's address.
  W=$(win "$S" 'let deploy_ssl_email = if body\.external_tls \{$' 6)
  if derives "$W" 'None' && derives "$W" 'or_else\(\|\| Some\(claims\.email'; then
    ok "external_tls suppresses the ssl_email fallback"
  else
    bad "external_tls no longer gates the ssl_email fallback"
  fi
fi

if S=$(subj "$GITDEP"); then
  W=$(win "$S" 'fn deploy_url\(domain: &str, ssl_email: Option<&str>\)' 6)
  if derives "$W" 'if ssl_email\.is_some\(\)'; then
    ok "git deploys resolve their URL in one place, from the ssl_email"
  else
    bad "deploy_url is gone or no longer branches on ssl_email"
  fi
  N=$(count "$S" 'format!\("https://\{?\}?"?, ?domain')
  if [ "$N" = "0" ]; then
    ok "no git-deploy path hardcodes https://<domain>"
  else
    bad "$N git-deploy path(s) hardcode https://<domain> again"
  fi
  # Taking the finished URL is what prevents a NEW caller reintroducing the
  # hardcode: there is no domain parameter left to build one from.
  if has "$S" 'async fn set_github_status\(.*target_url: Option<String>\)'; then
    ok "set_github_status takes a finished URL, not a domain"
  else
    bad "set_github_status takes a domain again — a caller can hardcode the scheme"
  fi
  # Every call site must route through the resolver.
  CALLS=$(count "$S" 'set_github_status\(')
  RESOLVED=$(count "$S" 'deploy_url\(d, config\.ssl_email\.as_deref\(\)\)')
  # CALLS counts the definition too, so callers = CALLS - 1.
  if [ "$RESOLVED" -ge "$((CALLS - 1))" ]; then
    ok "all $((CALLS - 1)) set_github_status callers resolve through deploy_url"
  else
    bad "only $RESOLVED of $((CALLS - 1)) set_github_status callers resolve through deploy_url"
  fi
fi

if S=$(subj "$SITES"); then
  # A domain rename must change the domain and nothing else. Rebuilding the URL
  # from a literal scheme is the defect; preserving the existing one is the fix.
  if has "$S" 'let new_url = format!\("https://\{new_domain\}"\)'; then
    bad "the rename path rewrites monitor URLs to https again"
  else
    ok "the rename path does not rewrite the monitor's scheme"
  fi
  if has "$S" "WHEN url LIKE 'http://%'"; then
    ok "the rename preserves whichever scheme the monitor already had"
  else
    bad "the rename no longer preserves the monitor's existing scheme"
  fi
  # NEGATIVE-CONTROL, deliberately kept. sites.rs' OTHER auto-monitor is created
  # disabled and is only ever enabled by the SSL-provisioning paths, so its
  # https:// is correct by construction and must NOT be "fixed". This arm fails
  # if someone deletes the enabled=FALSE that makes it correct.
  if has "$S" "'pending', FALSE, 'http'"; then
    ok "the site-creation monitor is still created disabled (its https is earned)"
  else
    bad "the site-creation monitor is no longer created disabled — its https is now a guess"
  fi
fi

echo
echo "== §B  the importer and the vhost agree on where a site lives =="

if S=$(subj "$NGINX_SVC"); then
  if has "$S" 'pub fn document_root_for'; then
    ok "there is one answer to 'which directory does nginx serve'"
  else
    bad "document_root_for is gone — the rule is spelled per-caller again"
  fi
fi

# Both callers must ASK, rather than each re-deriving the runtime match.
for f in "$NGINX_ROUTE" "$MIG_SVC"; do
  if S=$(subj "$f"); then
    # Bound, not merely called: a discarded `let _ = document_root_for(..)`
    # beside a hand-built path satisfies a presence grep (probe P4). Read with a
    # line of leading context, because one caller wraps the binding onto its own
    # line — a single-line pattern misses it and reds correct code.
    W=$(grep -B1 -E 'document_root_for' <<< "$S" || true)
    if [ -n "$W" ] && has "$W" 'let [a-z_]+ *=' && ! has "$W" 'let _ *='; then
      ok "$(basename "$f") binds its document root from document_root_for"
    else
      bad "$(basename "$f") derives the document root itself again"
    fi
    if has "$S" '"proxy" \| "node" \| "python"'; then
      bad "$(basename "$f") re-spells the runtime rule instead of asking"
    else
      ok "$(basename "$f") does not re-spell the runtime rule"
    fi
  fi
done

if S=$(subj "$MIG_SVC"); then
  # The analyser steps into a nested archive; both importers must step in too.
  N=$(count "$S" 'find_backup_root\(&format!\("\{MIGRATION_DIR\}-\{migration_id\}"\)\)')
  if [ "$N" -ge 2 ]; then
    ok "both importers resolve the archive root the way the analyser did"
  else
    bad "only $N of 2 importers resolve the archive root through find_backup_root"
  fi
  if has "$S" 'let extract_root = format!\("\{MIGRATION_DIR\}-\{migration_id\}"\);'; then
    bad "an importer rebuilds the archive root without stepping into the nest"
  else
    ok "no importer rebuilds the archive root by hand"
  fi
  # Absolute doc_roots cannot survive import_site_files' own traversal guard, so
  # no parser may emit one.
  # Keyed on the CAPABILITY, not on one spelling of the defect. The first draft
  # forbade `doc_root: doc_root.to_string_lossy`, and probe P6 walked straight
  # past it by writing `doc_root.display().to_string()` instead — the same
  # spelling-bound-negative disease as lesson #122. What a correct parser must
  # DO is pass the path through `relative_to`; what it must never do is hand a
  # `Path` conversion straight to the field.
  PARSERS_OK=1
  for fn in parse_plesk parse_hestiacp; do
    W=$(fnbody "$S" "$fn")
    derives "$W" 'relative_to\(&doc_root, root\)' || PARSERS_OK=0
  done
  if [ "$PARSERS_OK" = "1" ]; then
    ok "every path-walking parser makes its doc_root archive-relative"
  else
    bad "a path-walking parser no longer makes its doc_root archive-relative"
  fi
  if has "$S" 'doc_root:[^,]*\.(display\(\)|to_string_lossy\(\)|to_str\(\)|into_os_string\(\))'; then
    bad "a parser hands a filesystem path straight to doc_root — import rejects it as traversal"
  else
    ok "no parser hands a filesystem path straight to doc_root"
  fi
  # The general property behind both arms above, and the one probe P14 used to
  # get past them: an inventory path is joined ONTO the archive root by the
  # importer, so it must never already contain it. Interpolating `{root}` is
  # absolute by another name.
  if has "$S" '(doc_root|file):[^,]*\{root\}'; then
    bad "a parser embeds the archive root in a path the importer will join onto it"
  else
    ok "no parser embeds the archive root in an inventory path"
  fi
  if has "$S" 'fn relative_to'; then
    ok "there is one helper making inventory paths archive-relative"
  else
    bad "relative_to is gone"
  fi
  # A bare filename resolves one level above the directory it was found in.
  # Property, not spelling: the emitted value must CARRY A DIRECTORY. The first
  # draft forbade the literal `file: name,` and probe P9 walked past it with
  # `file: name.clone()`. What matters is that a separator is present, because
  # `import_database` joins this onto the archive root.
  DBS_OK=1
  for fn in parse_cpanel parse_plesk parse_hestiacp; do
    W=$(fnbody "$S" "$fn")
    derives "$W" 'file: format!\("[a-z_]+/' || DBS_OK=0
  done
  if [ "$DBS_OK" = "1" ]; then
    ok "every parser carries the database's directory"
  else
    bad "a parser emits a database path with no directory — import looks one level too high"
  fi
fi

if S=$(subj "$MIG_BE"); then
  # Scoped to the IMPORT body. `"runtime": runtime` also appears in the nginx
  # body a few lines up, so an unscoped grep stays green with the import body
  # empty — which is exactly what it did on the pre-fix tree when this arm was
  # first written (lesson #125: an arm satisfied by unrelated code is not an arm).
  IMPORT_BODY=$(grep -A8 -E 'let import_body = serde_json::json!' <<< "$S" || true)
  if [ -n "$IMPORT_BODY" ] && has "$IMPORT_BODY" '"runtime": runtime'; then
    ok "the panel tells the importer which runtime the vhost was created with"
  else
    bad "the import request no longer carries the runtime"
  fi
fi

if S=$(subj "$MIG_ROUTE"); then
  # The s288 lesson: a key the sender spells differently from the reader is a
  # silent default, not an error. Pin the reader's key name.
  if has "$S" 'body\["runtime"\]'; then
    ok "the agent reads the same runtime key the panel sends"
  else
    bad "the agent no longer reads the runtime key the panel sends"
  fi
  if has "$S" 'import_site_files\(migration_id, domain, source_dir, runtime\)'; then
    ok "the runtime reaches import_site_files"
  else
    bad "the runtime does not reach import_site_files"
  fi
fi

echo
echo "──────────────────────────────────────────"
echo "PASS: $PASS   FAIL: $FAIL"
[ "$FAIL" -eq 0 ]
