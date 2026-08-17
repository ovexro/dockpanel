#!/usr/bin/env bash
# site-runtime-workdir-pin-e2e.sh — s370 / v2.123.0
#
# A managed process must start where its code is.
#
# `create_app_service` hardcoded `WorkingDirectory=/var/www/{domain}/public` for
# every runtime, and it only ever runs for node and python — the two runtimes
# whose files live at the SITE ROOT. `document_root_for` says so ("proxy" |
# "node" | "python" => site_dir), the git clone lands there, and the unit's own
# `EnvironmentFile` and `ReadWritePaths` both point there. `public/` was the one
# path in the whole arrangement that disagreed, and the function CREATED it, two
# lines before `systemctl enable --now`, so the process started in an empty
# directory the panel had just made for it.
#
# Measured rather than reasoned, one variable, controls from the site root:
#
#   node server.js   cwd=public/  -> rc=1  Cannot find module   | site root rc=0
#   python3 app.py   cwd=public/  -> rc=2  can't open file      | site root rc=0
#   npm start        cwd=public/  -> rc=0                       | site root rc=0
#
# With `Restart=always` the first two are a permanent crash loop. The third is
# why this survived since 749ae2f: `npm start` walks up to the nearest
# package.json and re-roots the cwd there, and it is the FIRST of the three
# examples the site form offers ("e.g., npm start, node server.js, npx next
# start"). One of the three suggestions worked, so the runtime looked merely
# fussy rather than broken.
#
#   §A  THE PROCESS STARTS WHERE ITS CODE IS. The working directory is DERIVED
#       from the runtime through the one primitive that knows, never spelled
#       again — a second hardcoded copy is how the two drifted apart.
#   §B  THE UPGRADE CANNOT STRAND THE UNITS IT EXISTS TO REPAIR. Unit ownership
#       is keyed on the `WorkingDirectory` line, so the pre-v2.123.0 `/public`
#       form must STAY accepted. Recognising only the new form would hand every
#       unit already on disk to `Owner::Theirs`, and `owned_service_name` refuses
#       to stop, restart or delete one of those.
#   §C  THE SAME CONFUSION AT THE OTHER DOOR. `optimize_images` hardcoded the
#       same `public/`, so the WebP/AVIF buttons scanned a directory a node or
#       python site does not fill — and before this release that directory
#       existed and was empty, so it reported a clean run over zero images.
#       Its fix needs the runtime from the backend, so the reaches-arm ships
#       with it (lesson #555).
#
# NO PIPES INTO `grep -q`: under pipefail grep -q closes the pipe on first match
# and the arm goes red on correct code. Every arm feeds grep a here-string.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0; FAIL=0; SKIP=0
ok()   { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }
skip() { SKIP=$((SKIP+1)); printf '  \033[33m-\033[0m SKIP %s\n' "$1"; }

AP=panel/agent/src/services/app_process.rs
NGX=panel/agent/src/services/nginx.rs
OWN=panel/agent/src/services/ownership.rs
ANR=panel/agent/src/routes/nginx.rs
BSITES=panel/backend/src/routes/sites.rs

for f in "$AP" "$NGX" "$OWN" "$ANR" "$BSITES"; do
  [ -f "$f" ] || bad "MISSING SUBJECT FILE: $f"
done

# Comments out, CODE INTACT. Copied from the FIXED stripper (lesson #136) — the
# original opened a block comment on a `/*` inside a string literal and ran to
# the next `*/`, which makes an ABSENCE arm pass on code it merely deleted.
code() {
  perl -0777 -pe '
    s{\{/\*.*?\*/\}}{}gs;
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
  ' "$1"
}

# The stripper is code, and code can be wrong. Measure it before trusting it.
stripper_self_check() {
  local bad_files=0 f raw stripped
  for f in "$@"; do
    [ -f "$f" ] || continue
    raw=$(grep -cE '^[[:space:]]*(pub )?(pub\(crate\) )?(async )?fn ' "$f" || true)
    stripped=$(grep -cE '^[[:space:]]*(pub )?(pub\(crate\) )?(async )?fn ' <<< "$(code "$f")" || true)
    if [ "$raw" != "$stripped" ]; then
      bad "STRIPPER ATE CODE in $f: $raw fn declarations before, $stripped after"
      bad_files=$((bad_files+1))
    fi
  done
  [ "$bad_files" -eq 0 ] && ok "comment stripper preserves every fn declaration in all $# subjects"
}

subj()  { local t; t=$(code "$1"); [ -n "$t" ] || return 1; printf '%s' "$t"; }
has()   { grep -qE -- "$2" <<< "$1"; }
count() { grep -cE -- "$2" <<< "$1" || true; }
flat()  { tr '\n' ' ' <<< "$1" | tr -s ' '; }

# Extract one function body by brace balance. A fixed `-A n` window is NOT a
# function (p31) — a regression that moves a line past the window makes the arm
# silently measure nothing.
fnbody() {
  local src="$1" name="$2"
  awk -v fn="$name" '
    index($0, "fn " fn) && !started { started=1 }
    started {
      n=gsub(/\{/,"{"); m=gsub(/\}/,"}"); depth += n - m; print
      if (opened || n>0) opened=1
      if (opened && depth<=0) exit
    }
  ' <<< "$src"
}

echo
echo "site-runtime-workdir-pin-e2e — s370 / v2.123.0"
echo "=============================================="
echo
echo "§0 harness self-check"
stripper_self_check "$AP" "$NGX" "$OWN" "$ANR" "$BSITES"

AP_S=$(subj "$AP")         || { bad "cannot read $AP"; AP_S=""; }
NGX_S=$(subj "$NGX")       || { bad "cannot read $NGX"; NGX_S=""; }
OWN_S=$(subj "$OWN")       || { bad "cannot read $OWN"; OWN_S=""; }
ANR_S=$(subj "$ANR")       || { bad "cannot read $ANR"; ANR_S=""; }
BSITES_S=$(subj "$BSITES") || { bad "cannot read $BSITES"; BSITES_S=""; }

echo
echo "§A the process starts where its code is"

CAS=$(fnbody "$AP_S" "create_app_service")
if [ -n "$CAS" ]; then
  # A1 — DERIVED, not spelled. The primitive is the single place that knows
  # which runtimes nest under public/.
  if has "$CAS" 'document_root_for'; then
    ok "A1 the working directory is derived from document_root_for"
  else
    bad "A1 create_app_service no longer derives its working directory from the runtime"
  fi

  # A2 — ABSENCE. No `public/` may be spelled inside this function at all; that
  # literal is exactly what disagreed with every other path in the unit.
  # Control first: an absence arm with no positive control cannot tell "clean"
  # from "the pattern never ran" (#480).
  WWW_HITS=$(count "$CAS" '/var/www/')
  if [ "$WWW_HITS" -lt 1 ]; then
    bad "A2 CONTROL FAILED — '/var/www/' matched nothing in create_app_service, so the absence arm proves nothing"
  elif has "$CAS" '/public'; then
    bad "A2 create_app_service spells a public/ path again — the empty-directory crash loop is back"
  else
    ok "A2 no public/ path spelled in create_app_service ($WWW_HITS site-root references — control non-vacuous)"
  fi
else
  skip "A1/A2 — create_app_service body not extractable"
fi

# A3 — the primitive this fix leans on must keep saying what it says. If
# document_root_for ever nests node/python under public/, A1 stays green while
# the defect returns through the primitive.
DRF=$(flat "$(fnbody "$NGX_S" "document_root_for")")
if [ -n "$DRF" ]; then
  if has "$DRF" '"proxy" \| "node" \| "python" => site_dir'; then
    ok "A3 document_root_for still serves node/python/proxy from the site root"
  else
    bad "A3 document_root_for no longer maps node/python/proxy to the site root — A1 now derives the wrong path"
  fi
else
  skip "A3 — document_root_for body not extractable"
fi

# A4 — the blast radius. create_app_service is reached only for node|python, so
# deriving from the runtime can never move a static or php site off public/.
CALLWIN=$(flat "$(grep -B 6 -E 'create_app_service\(' <<< "$ANR_S" || true)")
if [ -n "$CALLWIN" ]; then
  if has "$CALLWIN" 'runtime == "node"' && has "$CALLWIN" 'runtime == "python"'; then
    ok "A4 the managed-process unit is still written only for node and python"
  else
    bad "A4 create_app_service is reached for other runtimes now — the derived path must be re-judged"
  fi
else
  skip "A4 — no create_app_service call site found in $ANR"
fi

echo
echo "§B the upgrade cannot strand the units it exists to repair"

SU=$(fnbody "$OWN_S" "systemd_unit")
if [ -n "$SU" ]; then
  # B1 — the NEW form is recognised, so units written from v2.123.0 are ours.
  if has "$SU" 'WorkingDirectory=/var/www/\{domain\}"'; then
    ok "B1 the site-root WorkingDirectory marker identifies units written from v2.123.0"
  else
    bad "B1 units written from v2.123.0 are not matched by their WorkingDirectory line"
  fi

  # B2 — the OLD form is STILL recognised. This is the arm that matters on an
  # upgrade: every unit already on disk carries `/public`, and dropping the
  # marker hands all of them to Owner::Theirs, which owned_service_name refuses
  # to stop, restart or delete. A "tidy-up" that deletes one line here strands
  # exactly the sites this release repairs.
  if has "$SU" 'WorkingDirectory=/var/www/\{domain\}/public"'; then
    ok "B2 the pre-v2.123.0 public/ marker is still accepted, so existing units stay ours"
  else
    bad "B2 the pre-v2.123.0 WorkingDirectory marker was dropped — every existing app unit becomes undeletable"
  fi

  # B3 — the description marker is the independent second identifier, and the
  # whole reason two markers exist is that either one alone can be edited away.
  if has "$SU" 'Description=DockPanel App: \{domain\}'; then
    ok "B3 the description marker still identifies the unit independently"
  else
    bad "B3 the description marker is gone — a single edited line now decides ownership"
  fi
else
  skip "B1/B2/B3 — systemd_unit body not extractable"
fi

echo
echo "§C the same confusion at the other door"

OI=$(fnbody "$ANR_S" "optimize_images")
if [ -n "$OI" ]; then
  # C1 — follow the code.
  if has "$OI" 'document_root_for'; then
    ok "C1 image optimization scans the runtime's own document root"
  else
    bad "C1 image optimization hardcodes a root again — it scans where a node/python site has no files"
  fi
else
  skip "C1 — optimize_images body not extractable"
fi

# C2 — REACHES. C1 alone can be perfectly green about a function that is handed
# nothing to derive from: without the runtime in the request the agent falls back
# to the previous behaviour and C1 still passes (lesson #555).
# Bounded on the handler's own braces, not on a fixed -A window. `sites.rs` posts
# `"runtime": site.runtime` from THREE different handlers, so a window wide enough
# to be safe is also wide enough to read a sibling's payload and call it this
# one's (p31). Proven necessary: the first cut of this arm used -A 14 and a
# mutation deleting the runtime from a DIFFERENT handler left it green.
BOI=$(flat "$(fnbody "$BSITES_S" "optimize_images")")
if [ -n "$BOI" ]; then
  if has "$BOI" '"runtime"'; then
    ok "C2 the backend sends the site's runtime, so C1 has something to derive from"
  else
    bad "C2 the backend posts no runtime — the agent silently falls back and C1 measures nothing"
  fi
else
  skip "C2 — no optimize-images call found in $BSITES"
fi

echo
echo "----------------------------------------------"
printf '  PASS %d   FAIL %d   SKIP %d\n' "$PASS" "$FAIL" "$SKIP"
echo
[ "$FAIL" -eq 0 ]
