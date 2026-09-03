#!/usr/bin/env bash
# Regression pins for the s258 SSL-correctness ship.
#
# Everything here had already shipped broken once, in a way a green unit suite
# could not see — none of these assertions can be made by a test that only knows
# about Rust:
#
#   A. The npm audit gate must WAIVE the reviewed advisory and still FAIL on
#      anything else. An always-red gate and an always-green gate fail the same
#      way — neither can report a new advisory — so both directions are pinned,
#      along with there being exactly ONE allowlist.
#   B. A site is installed at the scheme it can actually serve.
#   C. …and the promotion to HTTPS only ever touches the URL DockPanel itself set.
#   D. A certificate that stops renewing is announced, not just logged.
#   E. The installer does not read "I could not ask systemd" as "the service
#      failed" — which aborted a real fresh install four steps from the end.
#   F. A renewal records the certificate it installed.
#
# ── s338: every arm above was comment-blind, and eight of them were measured ──
#
# A pin greps RAW source, so a line that has been turned into an explanation
# still satisfies it. That is not a hypothetical here — it was executed against
# this file, one mutation at a time:
#
#   * commenting out the single call site that promotes the canonical URL after
#     SSL is enabled left the suite at 47 passed / 0 failed;
#   * so did commenting out the guard that ties the promotion to this vhost's own
#     domain, and the signature of the function holding it;
#   * so did commenting out the deduped alert path, the plain-HTTP install URL
#     and the late-certificate promotion;
#   * the two EXACT-COUNT arms in §D were the worst of the set: commenting a line
#     out conserves the count, so the arm cannot tell three live call sites from
#     two live ones and a note about the third;
#   * and §A's pre-push arm needed nobody to plant anything at all. Blanking the
#     executable line that runs the shared gate left the arm green, satisfied by
#     a comment two sections earlier that happens to name the same script.
#
# Two arms failed in the other direction: a NEGATIVE arm (match ⇒ bad) reading
# raw source turns RED when someone documents the thing that was removed. Both
# were reproduced by editing nothing but a comment.
#
# So every arm that reads SOURCE now reads it through `code_lines`, defined at
# the top of this file rather than beside the section that first needed it — a
# check placed below its call sites inherits that position as a defect however
# good the check is. Arms that read PROSE by design (a JSON manifest, a lockfile)
# stay raw and say so where they are.
#
# §G proves the stripper strips comments and nothing else. §H is the s338
# mutation encoded, so it re-runs on every push instead of dying with the session
# that did it by hand. §I plants the real defect back into a copy for each
# negative arm, because narrowing a negative arm fails silently. §J stops the
# next arm from being written the old way.
#
# No running panel needed.
#   run: bash tests/ssl-correctness-pin-e2e.sh
set -u

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GATE="$REPO/scripts/npm-audit-gate.mjs"
HOOK="$REPO/scripts/hooks/pre-push"
SITES="$REPO/panel/backend/src/routes/sites.rs"
WP="$REPO/panel/agent/src/services/wordpress.rs"
AGENT_SSL="$REPO/panel/agent/src/services/ssl.rs"
HEALER="$REPO/panel/backend/src/services/auto_healer.rs"
SCAN="$REPO/panel/backend/src/services/security_scanner.rs"
NOTIF="$REPO/panel/backend/src/services/notifications.rs"
SETUP="$REPO/scripts/setup.sh"
BE_SSL="$REPO/panel/backend/src/routes/ssl.rs"
PASS=0; FAIL=0
ok()   { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (expected [$3], got [$2])"; fi; }

TMP=$(mktemp -d)
FIXDIR=$(mktemp -d)
trap 'rm -rf "$TMP" "$FIXDIR"' EXIT

# ── the comment strippers, ABOVE every arm that needs them (s337) ────────────
#
# A check placed below the thing it must run before inherits that position as a
# defect even when the check itself is perfect — the s336 lesson, and the reason
# these functions are the first thing in the file rather than the middle. The
# draft this replaced defined its own stripper two thirds of the way down, with
# eighteen raw arms above it.
#
# The comment marker is chosen by EXTENSION, never guessed. Getting it wrong is
# not symmetric: over-stripping turns a positive arm loudly red, but turns a
# NEGATIVE arm (match ⇒ bad) silently green, which is the direction that ships.
comment_marker_for() {
  case "$1" in
    *.rs|*.mjs|*.js|*.ts|*.tsx)                echo '//' ;;
    *.json)                                    echo ''   ;;  # JSON has no comments
    *)                                         echo '#'  ;;  # .sh, .service, .gitignore, .toml
  esac
}

# BLANK the comment, do not delete it. The self-scan below reads line numbers out
# of this stream and compares them; a stripper that removes lines renumbers the
# file and would silently change what that arm measures. §E also slices a shell
# helper out of the installer by line range, and every slice boundary has to land
# where it did before.
code_lines() {
  local f="$1" m
  m=$(comment_marker_for "$f")
  if [ -z "$m" ]; then cat "$f"; return; fi
  if [ "$m" = '//' ]; then
    # A block comment is recognised ONLY where one is actually written: opening
    # at the start of a line and closing at the end of one. s294: a `/*` inside a
    # string literal (a Dockerfile's `COPY … /app/target/release/*`) opened a
    # comment that swallowed 485 lines of git_build.rs, and every ABSENCE arm
    # over it passed on code the stripper had merely removed.
    #
    # A trailing `//` is stripped, but only when the text before it has BALANCED
    # double quotes — otherwise `let u = "https://host"` would lose its tail.
    # Backslash escapes are skipped so `\"` does not flip the parity.
    awk '
      /^[[:space:]]*\/\*/ &&  /\*\/[[:space:]]*$/ { print ""; next }
      /^[[:space:]]*\/\*/                         { inblk = 1; print ""; next }
      inblk { if ($0 ~ /\*\/[[:space:]]*$/) inblk = 0; print ""; next }
      /^[[:space:]]*\/\//                         { print ""; next }
      {
        line = $0; out = line; n = length(line); q = 0; i = 1
        while (i <= n) {
          c = substr(line, i, 1)
          if (c == "\\") { i += 2; continue }
          if (c == "\"") { q = 1 - q; i++; continue }
          if (q == 0 && c == "/" && substr(line, i + 1, 1) == "/") { out = substr(line, 1, i - 1); break }
          i++
        }
        print out
      }
    ' "$f"
  else
    # Shell keeps to FULL-LINE comments only, and the asymmetry with `//` above is
    # deliberate. A trailing `#` cannot be stripped safely here: `${VAR#prefix}`,
    # `$#`, a colour literal and `sed 's/#//'` are all ordinary code. The
    # convention in this tree is that explanation goes on its own line, and §G
    # asserts the stripper leaves a `#` inside a string alone.
    awk '/^[[:space:]]*#/ { print ""; next } { print }' "$f"
  fi
}

# Counted, never `grep -q`, and the reason is a trap this suite walked straight
# into twice. Under `set -o pipefail`, `code_lines f | grep -q PATTERN` reports
# FAILURE on a successful match: grep -q exits at the first hit, the producer
# upstream dies of SIGPIPE (141), and pipefail takes the pipeline's status from
# it. The effect is silent and selective — a file whose match is near the top gets
# dropped while one whose match is near the bottom survives, because the producer
# had already finished. `grep -c` consumes all of its input, so there is no early
# exit and no signal. (s336 #365 is the same operator lying in the other
# direction: a failed PRODUCER read as a clean no-match.)
code_count() { local f="$1"; shift; code_lines "$f" | grep -c "$@" 2>/dev/null || true; }
has_code()   { local f="$1"; shift; [ "$(code_count "$f" "$@")" -gt 0 ]; }

# True file line numbers, because code_lines blanks rather than deletes.
code_lineno() { local f="$1"; shift; code_lines "$f" | grep -n "$@" 2>/dev/null | head -1 | cut -d: -f1 || true; }

# §F's subjects carry unit tests in-file, and a fixture is not the shipping path.
# Blank from the test attribute down rather than cutting, for the same reason
# code_lines blanks: height is part of the contract.
prod_lines() {
  code_lines "$1" | awk '/^#\[cfg\(test\)\]/ { intest = 1 } { if (intest) print ""; else print }'
}
prod_count() { local f="$1"; shift; prod_lines "$f" | grep -c "$@" 2>/dev/null || true; }

# Build an npm-audit-shaped report for one advisory.
mkreport() { # $1=ghsa $2=severity $3=package
  cat > "$TMP/report.json" <<EOF
{
  "auditReportVersion": 2,
  "vulnerabilities": {
    "$3": {
      "name": "$3",
      "severity": "$2",
      "via": [
        {
          "source": 1234567,
          "name": "$3",
          "title": "Synthetic advisory for the gate's own test",
          "url": "https://github.com/advisories/$1",
          "severity": "$2"
        }
      ]
    }
  },
  "metadata": { "vulnerabilities": { "total": 1 } }
}
EOF
}

run_gate() { node "$GATE" --input "$TMP/report.json" >"$TMP/out" 2>&1; echo $?; }

echo "── A: the npm audit gate waives what was reviewed and blocks what was not ──"

# ⚠ RETIRED at s326, and the arm INVERTED rather than deleted. This advisory was
# waived from 2026-07-27; `live-surfaces-check.sh` §4 watched for the condition
# the waiver named, fired when react-router 7.18.2 landed, and both frontends were
# bumped. What must now be true is the OPPOSITE of what this arm used to assert:
# the advisory is no longer excused, so it blocks like any other.
#
# Deleting the arm was the tempting move and the wrong one — a retired waiver
# needs the same evidence a live one does, or nothing distinguishes "we fixed it"
# from "we stopped looking".
mkreport "GHSA-qwww-vcr4-c8h2" "high" "react-router"
check "the retired react-router waiver no longer excuses it" "$(run_gate)" "1"

# ...and the reason it no longer needs excusing: we ship the fixed version. Read
# from the LOCKFILES, which are what actually gets installed, in BOTH frontends.
#
# RAW ON PURPOSE: a lockfile is JSON, it has no comment syntax, and this arm
# parses it as data rather than grepping it. There is nothing here for a stripper
# to remove and running one would only risk corrupting the parse.
for lock in panel/frontend/package-lock.json website/client/package-lock.json; do
  v=$(node -e "try{process.stdout.write(require('$REPO/$lock').packages['node_modules/react-router'].version)}catch(e){}" 2>/dev/null)
  if [ -n "$v" ] && [ "$(printf '%s\n7.18.2\n' "$v" | sort -V | head -1)" = "7.18.2" ]; then
    ok "$(basename "$(dirname "$lock")") locks react-router $v (>= 7.18.2, the fix)"
  else
    bad "$(basename "$(dirname "$lock")") locks react-router ${v:-nothing} — below the fix, and the waiver that covered it is gone"
  fi
done

# The point of the whole exercise: it must still be able to fail.
mkreport "GHSA-0000-0000-0001" "high" "some-lib"
check "un-reviewed HIGH advisory fails the build"     "$(run_gate)" "1"
mkreport "GHSA-0000-0000-0002" "critical" "some-lib"
check "un-reviewed CRITICAL advisory fails the build" "$(run_gate)" "1"

# Below the threshold is not a silent pass by accident — it is the same
# --audit-level=high contract the job had before.
mkreport "GHSA-0000-0000-0003" "moderate" "some-lib"
check "moderate advisory does not fail at --level high" "$(run_gate)" "0"

echo '{"vulnerabilities":{},"metadata":{"vulnerabilities":{"total":0}}}' > "$TMP/report.json"
check "a clean report passes" "$(run_gate)" "0"

# The stale-entry report cannot be EXERCISED while the allowlist is empty, so what
# is pinned is that the machinery is still there for the next waiver, and that
# there is no waiver today. An allowlist nobody is watching is how one entry
# becomes a blanket.
if has_code "$GATE" -F -e 'stale '; then
  ok "the gate still reports an allowlist entry that matches nothing"
else
  bad "the stale-entry report was removed along with the entry it was written for"
fi

# The entry count is read from the STRIPPED gate. The allowlist is currently a
# block of explanation with no entries in it, which is exactly the shape where a
# raw read cannot tell a commented-out entry from a live one.
code_lines "$GATE" > "$TMP/gate-code.mjs"
if [ "$(node -e "const s=require('fs').readFileSync('$TMP/gate-code.mjs','utf8');const m=s.match(/const ALLOWLIST = \\[([\\s\\S]*?)\\n\\];/);process.stdout.write(String(m?(m[1].match(/^\\s*\\{/gm)||[]).length:'no-block'))")" = "0" ]; then
  ok "nothing is currently waived — the allowlist is empty"
else
  printf '  \033[0;33m•\033[0m %s\n' "an advisory is currently waived; check its reason is still true"
fi

# A gate that cannot parse its input must not report success.
echo 'not json at all' > "$TMP/report.json"
check "unparseable audit output is an error, not a pass" "$(run_gate)" "2"
# Distinct from a parse failure on purpose: the pre-push hook warns on this and
# blocks on the other. A gate that blocks on a DNS blip is one that gets
# disabled with --no-verify, so the two must stay tellable apart.
printf '{"error":{"summary":"registry unreachable"}}' > "$TMP/report.json"
check "an unreachable registry is its own outcome, not a pass" "$(run_gate)" "3"

# One allowlist, not two. This hook used to carry its own copy and waive the
# advisory locally while CI — which had none — failed on it for six releases.
#
# The strongest measured case in the s338 batch, and the one that needed no
# planted prose: the hook already carries a line of explanation naming the shared
# gate, two sections above the line that runs it. Blanking the executable line
# left this arm green on the explanation alone.
if has_code "$HOOK" -F -e 'npm-audit-gate.mjs'; then
  ok "the pre-push hook uses the same gate CI does"
else
  bad "the pre-push hook audits npm its own way again"
fi
# NEGATIVE arm, and reading it raw made it noisy rather than blind: a one-line
# note recording that the local override had been removed turned this red on a
# correct tree. Controlled in §I.
if has_code "$HOOK" -F -e 'NPM_AUDIT_ALLOWLIST'; then
  bad "a second npm allowlist is back in the pre-push hook"
else
  ok "there is exactly one reviewed-advisory allowlist"
fi

# The react-router waiver's OWN PREMISES, which are facts about us, not upstream.
#
# live-surfaces.yml already re-checks the upstream half daily (has a fix landed
# inside ^7?). Nothing checked the half the waiver actually rests on: that no
# frontend here runs the server runtime GHSA-qwww-vcr4-c8h2 needs. Add
# @react-router/node or framework mode and the waiver silently becomes false
# while still suppressing a HIGH advisory — the worst direction for a security
# gate to fail in, and invisible to every other check we own.
#
# Discover the frontends from the manifests that actually depend on
# react-router-dom rather than naming them here: a third frontend must join this
# pin by existing, not by somebody remembering to add it.
#
# RAW ON PURPOSE: package.json is JSON. There is no comment syntax to strip, and
# `comment_marker_for` returns an empty marker for it precisely so that a scan
# over a manifest cannot quietly lose a line.
RR_MANIFESTS=$(find "$REPO" -name package.json \
                 -not -path '*/node_modules/*' -not -path '*/dist/*' -not -path '*/target/*' \
                 -exec grep -l '"react-router-dom"' {} + 2>/dev/null | sort)
RR_COUNT=$(printf '%s' "$RR_MANIFESTS" | grep -c . || true)

# An arm that discovered nothing would pass while proving nothing at all.
if [ "$RR_COUNT" -gt 0 ]; then
  ok "found $RR_COUNT frontend(s) depending on react-router-dom to check the waiver against"
else
  bad "no manifest depends on react-router-dom — either the dependency is gone (drop the waiver) or this discovery broke"
fi

# The source scan behind the third arm below. `grep -r` reads raw, so it is
# replaced by a per-file walk through the stripper.
#
# Only extensions the marker map knows are worth stripping. A `.css` file is
# excluded rather than handled: unknown extensions fall through to the shell
# marker, and a stylesheet's id selectors start with the character that marker
# blanks — over-stripping, on a NEGATIVE arm, is the silent direction. `.jsx`
# stays in and is deliberately UNDER-stripped for the same asymmetry: a missed
# comment can only make this arm shout, never make it sleep.
server_runtime_files() { # $1 = src dir; echoes the number of files whose CODE matches
  local d="$1" f hits=0
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    if has_code "$f" -E -e 'createRequestHandler|"use server"'; then hits=$((hits+1)); fi
  done < <(find "$d" -type f \( -name '*.ts' -o -name '*.tsx' -o -name '*.js' \
                             -o -name '*.mjs' -o -name '*.jsx' \) 2>/dev/null | sort)
  echo "$hits"
}

for m in $RR_MANIFESTS; do
  d=$(dirname "$m"); rel=${d#"$REPO"/}

  # RAW ON PURPOSE, as above: a manifest is JSON.
  grep -q '"@react-router/' "$m" \
    && bad "$rel now depends on a @react-router/* server package — RE-ASSESS the waiver in scripts/npm-audit-gate.mjs, it assumes there is no server runtime" \
    || ok "$rel has no @react-router/* server runtime package"

  if compgen -G "$d/react-router.config.*" >/dev/null 2>&1; then
    bad "$rel has a react-router.config.* — framework mode is on, so RSC is reachable; RE-ASSESS the waiver"
  else
    ok "$rel has no react-router.config.* (framework mode absent)"
  fi

  if [ "$(server_runtime_files "$d/src")" -gt 0 ]; then
    bad "$rel now has a request handler or a server action — the waiver's core premise is gone; RE-ASSESS it"
  else
    ok "$rel has no createRequestHandler and no server actions"
  fi
done

echo
echo "── B: a site is installed at the scheme it can actually serve ──"

# The install runs in a task beside auto-SSL, which may still fail. Pinning the
# literal here is the point: this is the line that decided a brand-new site was
# unreachable on both schemes. Measured at s338: commenting that one line out
# left this arm AND its negative twin below green together, so both halves of
# this section went quiet at once.
if has_code "$SITES" -F -e '"url": format!("http://{cms_domain}")'; then
  ok "the CMS installer is handed the plain-HTTP URL"
else
  bad "the CMS installer no longer installs at the plain-HTTP URL"
fi

# NEGATIVE arm. Controlled in §I.
if has_code "$SITES" -F -e '"url": format!("https'; then
  bad "an unconditional secure install URL is back in sites.rs"
else
  ok "no unconditional secure install URL remains"
fi

if has_code "$SITES" -F -e 'promote-https'; then
  ok "the panel promotes the canonical URL once the certificate lands"
else
  bad "nothing promotes the canonical URL after a late certificate"
fi

echo
echo "── C: the promotion only ever touches the URL DockPanel itself set ──"

# Extracted so it can be tested at all — the surrounding function shells out to
# wp-cli. A settable canonical URL is a site-takeover primitive, so the guard
# matters more than the rewrite.
#
# All three arms here were measured comment-satisfiable at s338, and the shape
# was the ordinary one: rename or retire the symbol, leave a note saying what it
# used to be, and the pin never notices.
if has_code "$WP" -F -e 'fn https_promotion_target'; then
  ok "the promotion decision is a separate, testable function"
else
  bad "the promotion decision is not extracted"
fi

if has_code "$WP" -F -e 'eq_ignore_ascii_case'; then
  ok "…and compares the stored URL against this vhost's own plain-HTTP form"
else
  bad "the promotion no longer pins the comparison to this vhost's own domain"
fi

if has_code "$AGENT_SSL" -F -e 'promote_site_url_to_https(domain)'; then
  ok "every path that enables SSL runs the promotion (single choke point)"
else
  bad "enabling SSL no longer promotes the canonical URL"
fi

echo
echo "── D: a certificate that stops renewing is announced, not just logged ──"

if has_code "$NOTIF" -F -e 'fn fire_alert_deduped'; then
  ok "there is a deduped alert path"
else
  bad "no deduped alert path — an alert from a 120s loop is a flood"
fi

# The bail-outs. These run BEFORE any attempt is made, which is exactly the
# case F6 named: issuance is rescued by the fallback contact, renewal is not
# even tried, and sixty days later the certificate expires on a live server.
#
# EXACT-COUNT arms, and over raw source they were the worst pins in the file:
# commenting a call site out CONSERVES the count, so three live call sites and
# two live ones plus a note about the third are the same number. Counted from
# code, the count moves.
check "auto-healer alerts when a renewal cannot be attempted" \
  "$(code_count "$HEALER" -F -e 'ssl_renewal_blocked(')" "3"
# Still four, and the count is load-bearing in a new way since v2.139.0. A fifth
# outcome exists — a renewal deliberately NOT attempted, because the installed
# certificate was issued by someone other than Let's Encrypt and renewing would
# have REPLACED it — but it announces through `ssl_renewal_declined_alert`, not
# this helper. That split is deliberate: this helper says "SSL renewal failed"
# and pages at `critical`, and neither is true of a refusal the panel made on
# purpose. If this count reaches five, check whether a declined renewal has been
# wired into the failure wording again.
check "the scanner alerts on every renewal outcome it can see" \
  "$(code_count "$SCAN" -F -e 'ssl_renewal_alert(')" "4"
check "and a DECLINED renewal announces without claiming a failure" \
  "$(code_count "$SCAN" -F -e 'ssl_renewal_declined_alert(')" "2"

# Both loops touch the same certificate; neither may alert unconditionally.
if has_code "$HEALER" -F -e 'fire_alert_deduped' && has_code "$SCAN" -F -e 'fire_alert_deduped'; then
  ok "both loops dedupe, so one stuck certificate is one alert"
else
  bad "a loop still alerts unconditionally"
fi

if has_code "$SCAN" -F -e 'fire_alert(' && has_code "$SCAN" -F -e 'ssl_renewal'; then
  ok "the scanner's renewal path reaches the alert system at all"
else
  bad "the scanner's renewal failure is still log-only"
fi


# ── E: the installer must not read "I could not ask" as "it failed" ──────────
#
# Driven out of a real fresh-box install (s258): a dbus hiccup made
# `systemctl is-active` exit non-zero with an empty answer, and the installer
# aborted at step 11/15 blaming a service that was running. Stubbed here because
# a transient bus failure cannot be summoned on demand — but the three answers it
# has to tell apart can.
echo
echo "── E: the installer's unit readiness check ──"

if has_code "$SETUP" -F -e 'wait_for_unit()'; then
  ok "there is a readiness helper"
else
  bad "the installer is back to a single is-active call"
fi
# NEGATIVE arm, exact-count-zero. Reading it raw made it noisy: a note describing
# the one-liner this helper replaced turned it red on a correct installer.
# Controlled in §I.
check "no bare is-active decides a service's fate" \
  "$(code_count "$SETUP" -F -e 'if systemctl is-active --quiet dockpanel')" "0"

# Extract the helper and run it against a stubbed systemctl. STRIP FIRST, then
# slice: code_lines blanks rather than deletes, so the range boundaries land on
# exactly the lines they did before, and a helper whose body was commented out
# arrives here as an empty function instead of a working one.
code_lines "$SETUP" | sed -n '/^wait_for_unit() {/,/^}/p' > "$TMP/helper.sh"
mkstub() { mkdir -p "$TMP/bin"; printf '#!/usr/bin/env bash\n%s\n' "$1" > "$TMP/bin/systemctl"; chmod +x "$TMP/bin/systemctl"; }
runhelper() { ( PATH="$TMP/bin:$PATH"; . "$TMP/helper.sh"; wait_for_unit some-unit "${1:-3}" >/dev/null 2>&1; echo $?; ); }

mkstub 'echo active'
check "an active unit is accepted"                       "$(runhelper)" "0"

# THE REGRESSION: dbus down — non-zero exit, nothing on stdout, unit is fine.
mkstub 'echo "Failed to retrieve unit state: Message recipient disconnected from message bus without replying" >&2; exit 1'
check "a bus that cannot answer is not a failed service"  "$(runhelper 2)" "1"
mkstub 'if [ -f "$0.seen" ]; then echo active; else touch "$0.seen"; echo "Failed to retrieve unit state" >&2; exit 1; fi'
rm -f "$TMP/bin/systemctl.seen"
check "…and one bad answer followed by a good one succeeds" "$(runhelper 5)" "0"

# A unit that is still coming up must be waited for, not condemned.
mkstub 'if [ -f "$0.seen" ]; then echo active; else touch "$0.seen"; echo activating; exit 3; fi'
rm -f "$TMP/bin/systemctl.seen"
check "an activating unit is waited for"                 "$(runhelper 5)" "0"

# And a genuinely dead one still fails — the check must keep its teeth.
mkstub 'echo failed; exit 3'
check "a failed unit fails immediately"                  "$(runhelper 3)" "1"
mkstub 'echo inactive; exit 3'
check "a unit that never starts still fails"             "$(runhelper 2)" "1"

echo
echo "── F: a renewal records the certificate it installed (s306) ──"

# §D above pins that a renewal FAILURE is announced. It says nothing about a
# renewal SUCCESS, and that is where the panel's view of a certificate went
# stale: the scanner's auto-fix renewed correctly and discarded the `expiry` the
# agent returned, so the site's stored expiry stayed at the value written when the
# certificate was first issued. The dashboard countdown ran to zero and the
# warning ladder walked down to the EXPIRED sentinel on a certificate that had
# renewed perfectly — with no way back, because the resolve branch fires only
# when the remaining days RISE.
#
# These arms were the only ones in the file already stripping comments, because
# the code under test explains this defect in its own prose and names the very
# columns being asserted. They now use the shared stripper instead of a local
# one, so there is a single implementation to get right (and §G to keep it right).
#
# THE SUBJECTS ARE DERIVED. Every backend file that asks an agent to issue or
# renew a certificate FOR A ROW IN `sites` must store what came back. A
# hardcoded list is the arm that cannot see the next one — and the path that
# carried this defect is one a list written from memory would have missed,
# because it is a security scanner rather than an SSL route (lesson #182).
#
# Both halves of the predicate are load-bearing, and each was added because the
# looser version named something real that is not in this class:
#
#   * the AGENT call. Matching the bare path also matches the panel's own route
#     table (`mod.rs` registers the renew endpoint), which issues nothing.
#   * a reference to the `sites` table — the certificate must be one the panel
#     TRACKS. `mail.rs` provisions for a mail host and works entirely in
#     `mail_domains`; it holds no `sites` row, so there is no expiry column for
#     it to write and demanding one would be a false red for ever.
#
# Named here rather than filtered silently, so the next session extends the
# class deliberately instead of rediscovering the exclusions.
#
# BOTH halves are counted from CODE. Discovery through a raw `grep -r` is a
# membership decision made on prose: a file could join the class on the strength
# of a commented-out call, or — worse, because it is the quiet direction — be
# excluded from it because its only `sites` reference had been commented out.
#
# ⚠ s348: `mail.rs`'s exclusion is now EXPLICIT, and it had to become explicit.
# The paragraph above always intended it out of the class, but the mechanism was
# incidental — mail.rs simply happened to contain no `sites` reference. GitHub
# #106 gave it one: `create_domain` reads `sites` to name the account that a new
# mail domain hands its mailboxes to. The membership test then pulled mail.rs in
# and this suite went red on correct code, for a cert it has nowhere to record.
# An exclusion that depends on a file NOT mentioning a table is an exclusion that
# any unrelated feature can revoke. Named, and its premise re-asserted below.
RENEW_CALLERS=()
RENEW_SKIPPED=0
RENEW_EXCLUDED=0
while IFS= read -r f; do
  [ -n "$f" ] || continue
  has_code "$f" -E -e 'format!\("/ssl/(provision/|\{)' || continue
  case "$(basename "$f")" in
    mail.rs)
      RENEW_EXCLUDED=$((RENEW_EXCLUDED + 1))
      echo "  · mail.rs excluded by name: it provisions for a mail host and has no sites row to record an expiry on"
      continue
      ;;
  esac
  if has_code "$f" -E -e 'FROM sites|UPDATE sites|INTO sites'; then
    RENEW_CALLERS+=("$f")
  else
    RENEW_SKIPPED=$((RENEW_SKIPPED + 1))
  fi
done < <(find "$REPO/panel/backend/src" -name '*.rs' 2>/dev/null | sort)

# The exclusion asserts its own premise, so it cannot outlive its reason. If
# `mail_domains` ever grows a place to put an expiry, mail.rs stops being a file
# with nowhere to write one and belongs back in the class above.
if [ "$RENEW_EXCLUDED" -eq 0 ]; then
  bad "mail.rs no longer calls an agent SSL route — remove its exclusion above rather than leaving a carve-out for a file that left the class"
elif grep -rqE 'mail_domains[^;]*ssl_expiry|ssl_expiry[^;]*mail_domains' "$REPO/panel/backend/migrations/" 2>/dev/null; then
  bad "mail_domains now carries an expiry column — mail.rs has somewhere to record its certificate, so its exclusion from the renewal class is stale"
else
  ok "mail.rs is excluded for a reason that still holds: no expiry column exists for a mail domain"
fi
[ "$RENEW_SKIPPED" -gt 0 ] && \
  echo "  · $RENEW_SKIPPED agent SSL caller(s) hold no sites row and are out of this class"

if [ "${#RENEW_CALLERS[@]}" -lt 3 ]; then
  bad "enumerated only ${#RENEW_CALLERS[@]} backend callers of an agent SSL issue/renew route — implausible, so the arms below would pass having examined nothing"
else
  ok "enumerated ${#RENEW_CALLERS[@]} backend callers of an agent SSL issue/renew route"
  for f in "${RENEW_CALLERS[@]}"; do
    if [ "$(prod_count "$f" -E -e 'ssl_expiry[ ]*=[ ]*\$')" -gt 0 ]; then
      ok "$(basename "$f") stores the expiry it was handed"
    else
      bad "$(basename "$f") renews a certificate and never records it — the panel keeps describing the retired one, down to a false EXPIRED it cannot resolve"
    fi
  done
fi

# The ARI trap: on a RENEWAL the two bookkeeping columns must move with the
# expiry. A surviving renewal-window value is one computed for the certificate
# that was just replaced. (Initial issuance in sites.rs has nothing to clear, so
# this is asserted on the renewal paths that carry the pattern, not on every
# writer.)
for f in "$SCAN" "$HEALER"; do
  if [ "$(prod_count "$f" -F -e 'ssl_renewal_at = NULL')" -gt 0 ] \
  && [ "$(prod_count "$f" -F -e 'ssl_renewal_checked_at = NULL')" -gt 0 ]; then
    ok "$(basename "$f") clears both ARI columns when it records a renewal"
  else
    bad "$(basename "$f") records a renewal without clearing the ARI window computed for the retired certificate"
  fi
done

# And the host-blind re-read that already wrote across hosts. `domain` is unique
# only per server, so a post-renewal lookup keyed on it can return another
# host's row — which this path then pushes as a vhost through THIS host's agent.
#
# NEGATIVE arm. Controlled in §I.
if [ "$(prod_count "$SCAN" -F -e 'FROM sites WHERE domain')" -eq 0 ]; then
  ok "the scanner's post-renewal re-read is not keyed on a domain"
else
  bad "the scanner re-reads the site by domain after renewing — on a fleet that can rebuild another host's vhost through this host's agent"
fi

echo
echo "── G: the strippers strip comments, and nothing else (s338) ──"

# Everything above now depends on code_lines. A stripper that under-strips
# restores the whole defect silently; one that OVER-strips turns the five
# negative arms green on broken code. So it gets its own fixtures, and they are
# hermetic — no repository file is involved, because a fixture that shares a
# subject with the thing it validates cannot tell the two apart.
cat > "$FIXDIR/f.sh" <<'FIX'
#!/usr/bin/env bash
# TOKENWORD lives here as a full-line comment
run_the TOKENWORD --now
echo "a literal hash # inside a string keeps TOKENSTR"
FIX

cat > "$FIXDIR/f.rs" <<'FIX'
// TOKENWORD in a line comment
/*
 * TOKENWORD in a block comment
 */
let live = call(TOKENWORD);
let trail = other(); // TOKENWORD trailing
let url = "https://example.invalid/TOKENURL";
let glob = "COPY /app/target/release/*";
let after_glob = keep(TOKENAFTER);
FIX

printf '{ "name": "x", "note": "TOKENWORD" }\n' > "$FIXDIR/f.json"

if [ "$(code_count "$FIXDIR/f.sh" -F -e 'TOKENWORD')" -eq 1 ]; then
  ok "shell: a full-line '#' comment carrying the token is blanked (1 code hit, not 2)"
else
  bad "shell: expected exactly 1 code hit for TOKENWORD, got $(code_count "$FIXDIR/f.sh" -F -e 'TOKENWORD')"
fi

if [ "$(code_count "$FIXDIR/f.sh" -F -e 'TOKENSTR')" -eq 1 ]; then
  ok "shell: a '#' inside a string literal is left alone (no trailing-comment stripping)"
else
  bad "shell: the stripper ate a '#' inside a string — a negative arm would now pass on broken code"
fi

if [ "$(code_count "$FIXDIR/f.rs" -F -e 'TOKENWORD')" -eq 1 ]; then
  ok "rust: '//' line, '/* */' block and trailing '//' all blanked (1 code hit of 4)"
else
  bad "rust: expected exactly 1 code hit for TOKENWORD, got $(code_count "$FIXDIR/f.rs" -F -e 'TOKENWORD')"
fi

if [ "$(code_count "$FIXDIR/f.rs" -F -e 'TOKENURL')" -eq 1 ]; then
  ok "rust: '//' inside a quoted string survives — a URL is not a comment"
else
  bad "rust: the stripper cut a line at the '//' of a URL inside a string literal"
fi

# s294: a `/*` inside a string literal opened a block comment that ran to the
# next `*/` and deleted 485 lines of git_build.rs. The guard is that a block is
# recognised only where one is WRITTEN — opening at line start.
if [ "$(code_count "$FIXDIR/f.rs" -F -e 'TOKENAFTER')" -eq 1 ]; then
  ok "rust: a '/*' inside a string literal does not open a block (the s294 guard holds)"
else
  bad "rust: a string containing '/*' swallowed the code after it — the s294 stripper defect is back"
fi

if [ "$(code_count "$FIXDIR/f.json" -F -e 'TOKENWORD')" -eq 1 ]; then
  ok "json: nothing is stripped, because JSON has no comment syntax"
else
  bad "json: the stripper altered a JSON file, which has no comments to remove"
fi

# §E slices a shell helper by line range and §J compares line numbers. Deleting
# rather than blanking would renumber every subject and change what they measure.
SUBJECTS=("$GATE" "$HOOK" "$SITES" "$WP" "$AGENT_SSL" "$NOTIF" "$HEALER" "$SCAN" "$SETUP" "$BE_SSL")
LINEDRIFT=""
for f in "${SUBJECTS[@]}"; do
  a=$(wc -l < "$f"); b=$(code_lines "$f" | wc -l)
  [ "$a" != "$b" ] && LINEDRIFT="$LINEDRIFT $(basename "$f")($a≠$b)"
done
if [ -z "$LINEDRIFT" ]; then
  ok "blanking preserves the line count of all ${#SUBJECTS[@]} subjects, so line numbers stay true"
else
  bad "code_lines changed the line count of:$LINEDRIFT — §E's helper slice is now cutting the wrong lines"
fi

# The most direct over-strip control available: if the stripper removed part of a
# command, the result stops being a valid script.
PARSEFAIL=""
for f in "$SETUP" "$HOOK"; do
  code_lines "$f" | bash -n 2>/dev/null || PARSEFAIL="$PARSEFAIL $(basename "$f")"
done
if [ -z "$PARSEFAIL" ]; then
  ok "every shell subject still parses after blanking, so no command was cut in half"
else
  bad "these no longer parse after blanking, so the stripper removed code:$PARSEFAIL"
fi

echo
echo "── H: every pinned pattern is live CODE, and a comment cannot satisfy it (s338) ──"

# This is the s338 mutation, encoded. A round of mutation testing done by hand
# dies with the session that did it; the arms below re-run it on every push.
#
# For each subject: assert every pattern this suite pins is present in CODE (a
# pattern that has quietly become prose-only would otherwise turn red later and
# look like a test bug rather than the defect it is), then turn each matching
# code line into a comment in a throwaway copy and assert the count falls to
# zero. If it does not, some arm above is still reading the file raw.
PINNED=(
  "$GATE|-F|stale "
  "$GATE|-F|const ALLOWLIST = ["
  "$HOOK|-F|npm-audit-gate.mjs"
  "$SITES|-F|\"url\": format!(\"http://{cms_domain}\")"
  "$SITES|-F|promote-https"
  "$SITES|-E|ssl_expiry[ ]*=[ ]*\\\$"
  "$WP|-F|fn https_promotion_target"
  "$WP|-F|eq_ignore_ascii_case"
  "$AGENT_SSL|-F|promote_site_url_to_https(domain)"
  "$NOTIF|-F|fn fire_alert_deduped"
  "$HEALER|-F|ssl_renewal_blocked("
  "$HEALER|-F|fire_alert_deduped"
  "$HEALER|-F|ssl_renewal_at = NULL"
  "$HEALER|-F|ssl_renewal_checked_at = NULL"
  "$HEALER|-E|ssl_expiry[ ]*=[ ]*\\\$"
  "$SCAN|-F|ssl_renewal_alert("
  "$SCAN|-F|fire_alert_deduped"
  "$SCAN|-F|fire_alert("
  "$SCAN|-F|ssl_renewal"
  "$SCAN|-F|ssl_renewal_at = NULL"
  "$SCAN|-F|ssl_renewal_checked_at = NULL"
  "$SCAN|-E|ssl_expiry[ ]*=[ ]*\\\$"
  "$SETUP|-F|wait_for_unit()"
  "$BE_SSL|-E|ssl_expiry[ ]*=[ ]*\\\$"
)

# Turn every CODE line matching the pattern into a comment, leaving the text.
# That is the defect this section is about: the mechanism removed, the
# explanation left behind.
#
# The copy KEEPS the original's extension, and that is load-bearing rather than
# tidy: comment_marker_for dispatches on the extension, so a scratch file called
# `mutated` is read as shell no matter what it holds. Two subjects here share a
# basename (the panel's SSL route and the agent's SSL service), so the scratch
# name is derived from the whole path with the extension left on the end.
#
# The hit list is identified by FILENAME rather than by `NR == FNR`. The idiom
# they usually stand in for each other, and it breaks on exactly the input this
# section produces when a pin has gone prose-only: an EMPTY hit list. With no
# records in the first file, the first record of the SECOND file also satisfies
# NR == FNR, so the whole subject is consumed as hit numbers and the copy comes
# out empty — which then reads as "the stripper is not reaching this subject"
# when the truth is "this pattern has no code lines left to comment out".
comment_out_matches() {
  local src="$1" dst="$2" mode="$3" pat="$4" marker
  marker=$(comment_marker_for "$src")
  code_lines "$src" | grep -n "$mode" -e "$pat" 2>/dev/null | cut -d: -f1 > "$FIXDIR/hits" || true
  awk -v marker="$marker" -v hitsfile="$FIXDIR/hits" \
      'FILENAME == hitsfile { hit[$1]=1; next } { if (FNR in hit) print marker " " $0; else print }' \
      "$FIXDIR/hits" "$src" > "$dst"
}

PIN_SUBJECTS=$(printf '%s\n' "${PINNED[@]}" | cut -d'|' -f1 | sort -u)
for subj in $PIN_SUBJECTS; do
  dead="" survived=""
  for entry in "${PINNED[@]}"; do
    f=${entry%%|*}; rest=${entry#*|}; mode=${rest%%|*}; pat=${rest#*|}
    [ "$f" = "$subj" ] || continue
    [ "$(code_count "$f" "$mode" -e "$pat")" -gt 0 ] || dead="$dead [$pat]"
    mutated="$FIXDIR/m_$(printf '%s' "${f#"$REPO"/}" | tr '/' '_')"
    comment_out_matches "$f" "$mutated" "$mode" "$pat"
    # Assert the plant LANDED before reading the result: an empty copy would
    # score zero and read exactly like a stripper that works.
    if [ ! -s "$mutated" ]; then
      survived="$survived [$pat→copy-empty]"
      continue
    fi
    left=$(code_lines "$mutated" | grep -c "$mode" -e "$pat" 2>/dev/null || true)
    [ "$left" -eq 0 ] || survived="$survived [$pat→$left]"
  done

  rel=${subj#"$REPO"/}
  if [ -z "$dead" ]; then
    ok "$rel: every pattern this suite pins is present in CODE, not only in prose"
  else
    bad "$rel: pinned pattern(s) present only as prose —$dead — the arm reading it is passing on a comment"
  fi

  if [ -z "$survived" ]; then
    ok "$rel: commenting out the matching code lines drives every pinned pattern to zero"
  else
    bad "$rel: pattern(s) still counted after their code lines were commented out —$survived — the stripper is not reaching this subject"
  fi
done

echo
echo "── I: each negative arm still fires when the real defect is in code (s338) ──"

# ONE PLANTED REAL DEFECT PER NEGATIVE ARM, and this is the section that matters
# most. Stripping makes a check see LESS. For a positive arm that is loudly red;
# for a NEGATIVE arm (match ⇒ bad) it is silently green on broken code, and a
# green run looks identical either way. §H cannot catch it — commenting a line
# out is exactly what a negative arm wants to survive. So the defect goes back in
# as CODE, in a copy, and the grep is required to fire.
#
# Each plant is written into a copy that keeps the subject's extension, because
# the marker is chosen from the filename.

cp "$HOOK" "$FIXDIR/pre-push"
printf '\nNPM_AUDIT_ALLOWLIST="GHSA-0000-0000-0000"\n' >> "$FIXDIR/pre-push"
if has_code "$FIXDIR/pre-push" -F -e 'NPM_AUDIT_ALLOWLIST'; then
  ok "§A's second-allowlist arm still fires when the override is really in code"
else
  bad "§A's second-allowlist arm no longer fires on a real local allowlist — the stripper is eating code"
fi

cp "$REPO/panel/frontend/package.json" "$FIXDIR/manifest.json"
printf '  "@react-router/node": "^7.18.2",\n' >> "$FIXDIR/manifest.json"
if grep -q '"@react-router/' "$FIXDIR/manifest.json"; then
  ok "§A's server-package arm still fires when the dependency is really declared"
else
  bad "§A's server-package arm no longer fires on a real @react-router/* dependency"
fi

TSX_VICTIM=$(find "$REPO/panel/frontend/src" -name '*.tsx' 2>/dev/null | sort | head -1)
cp "$TSX_VICTIM" "$FIXDIR/probe.tsx"
printf '\nexport const handler = createRequestHandler({ build });\n' >> "$FIXDIR/probe.tsx"
if has_code "$FIXDIR/probe.tsx" -E -e 'createRequestHandler|"use server"'; then
  ok "§A's server-runtime arm still fires when a request handler is really in code"
else
  bad "§A's server-runtime arm no longer fires on a real request handler — the stripper is eating code"
fi

cp "$SITES" "$FIXDIR/x_sites.rs"
printf '\n        "url": format!("https://{cms_domain}"),\n' >> "$FIXDIR/x_sites.rs"
if has_code "$FIXDIR/x_sites.rs" -F -e '"url": format!("https'; then
  ok "§B's secure-install-URL arm still fires when the URL is really in code"
else
  bad "§B's secure-install-URL arm no longer fires on a real https install URL — the stripper is eating code"
fi

cp "$SETUP" "$FIXDIR/x_setup.sh"
printf '\nif systemctl is-active --quiet dockpanel; then :; fi\n' >> "$FIXDIR/x_setup.sh"
if [ "$(code_count "$FIXDIR/x_setup.sh" -F -e 'if systemctl is-active --quiet dockpanel')" -gt 0 ]; then
  ok "§E's bare-is-active arm still fires when the one-liner is really in code"
else
  bad "§E's bare-is-active arm no longer fires on a real bare is-active — the stripper is eating code"
fi

cp "$SCAN" "$FIXDIR/x_scanner.rs"
# Insert BEFORE any `#[cfg(test)]` boundary rather than blindly appending at
# literal EOF: `prod_count` blanks everything from that marker onward (see the
# K7 comment above), so a plain `>>` would land the planted defect inside the
# blanked region for any subject whose own test module sits at the true end of
# the file — which security_scanner.rs does since the s413 stack-SSL work.
SCAN_TEST_LINE=$(grep -n '^#\[cfg(test)\]' "$FIXDIR/x_scanner.rs" | head -1 | cut -d: -f1)
if [ -n "$SCAN_TEST_LINE" ]; then
  {
    head -n "$((SCAN_TEST_LINE - 1))" "$FIXDIR/x_scanner.rs"
    printf '    let row = sqlx::query("SELECT id FROM sites WHERE domain = ?").fetch_one(pool).await?;\n'
    tail -n "+$SCAN_TEST_LINE" "$FIXDIR/x_scanner.rs"
  } > "$FIXDIR/x_scanner.rs.new"
  mv "$FIXDIR/x_scanner.rs.new" "$FIXDIR/x_scanner.rs"
else
  printf '\n    let row = sqlx::query("SELECT id FROM sites WHERE domain = ?").fetch_one(pool).await?;\n' >> "$FIXDIR/x_scanner.rs"
fi
if [ "$(prod_count "$FIXDIR/x_scanner.rs" -F -e 'FROM sites WHERE domain')" -gt 0 ]; then
  ok "§F's host-blind-re-read arm still fires when the query is really in code"
else
  bad "§F's host-blind-re-read arm no longer fires on a real domain-keyed re-read — the stripper is eating code"
fi

echo
echo "── J: no arm in this suite may read a subject raw again (s338) ──"

# The structural guard. §H proves today's arms are comment-blind; this one stops
# a NEW arm from being written the old way, which is exactly how this file came
# to have eighteen raw arms sitting above a stripper it already contained.
SELF="$REPO/tests/ssl-correctness-pin-e2e.sh"

# Assembled at runtime from a variable, so this line does not contain the shape
# it searches for and the scan cannot match itself. The JSON subjects are
# deliberately absent from the list: a manifest and a lockfile are prose by
# design and are meant to be read raw.
SUBJ_VARS='GATE|HOOK|SITES|WP|AGENT_SSL|NOTIF|HEALER|SCAN|SETUP|BE_SSL|REPO'
RAW_ARMS=$(code_lines "$SELF" | grep -nE "grep[^|;]*\"\\\$($SUBJ_VARS)[\"/]" || true)
if [ -z "$RAW_ARMS" ]; then
  ok "no arm greps a subject file directly; every read goes through code_lines"
else
  bad "these lines read a subject raw instead of through code_lines: $(printf '%s' "$RAW_ARMS" | cut -d: -f1 | tr '\n' ' ')"
fi

# s336 #364: a check placed below an early exit inherits that exit's blind spot.
# The stripper this file used to carry had the same shape — defined two thirds of
# the way down, used by nothing above it. Assert the definition precedes the
# first section rather than trusting it.
DEF_LINE=$(code_lineno "$SELF" -E -e '^code_lines\(\) \{')
SEC1_LINE=$(code_lineno "$SELF" -F -e '── A: the npm audit gate waives')
if [ -n "$DEF_LINE" ] && [ -n "$SEC1_LINE" ] && [ "$DEF_LINE" -lt "$SEC1_LINE" ]; then
  ok "code_lines is defined (line $DEF_LINE) above the first section (line $SEC1_LINE)"
else
  bad "code_lines is no longer defined above section A — the arms above its definition are reading raw source again (def=${DEF_LINE:-none} §A=${SEC1_LINE:-none})"
fi

echo
echo "── K: a mail host's certificate never destroys the one already there ──"

# The mail auto-SSL leg writes /etc/dockpanel/ssl/{domain}/fullchain.pem through
# the agent, and that file is not the mail host's private property: a DNS-01
# WILDCARD is stored under the zone apex, which is exactly the name a mail domain
# is normally created at. Every arm here is scoped with `prod_lines`, because this
# file carries unit tests that spell each of these symbols — counting raw source
# would let the TEST DATA satisfy the pin (the trap that has cost this repo a
# green suite over a restored defect more than once).
MAILF="$REPO/panel/backend/src/routes/mail.rs"

if [ ! -f "$MAILF" ]; then
  bad "K0 mail.rs is missing — every arm below would pass having examined nothing"
else
  # Extract the guarded issuer's own body. Everything below reads THIS, not the
  # file, so an arm cannot be satisfied by a sibling function 1500 lines away.
  PROV_BODY=$(prod_lines "$MAILF" \
    | awk '/^async fn provision_mail_host_cert\(/ {inb=1} inb {print} inb && /^\}$/ {exit}')
  PROV_N=$(grep -c . <<< "$PROV_BODY")

  # H0 — the extractor's own control. An anchor that stops matching yields an
  # EMPTY body, under which every `hasnt`-shaped arm below passes green. Floor it
  # and make the break say so in its own words.
  if [ "$PROV_N" -ge 20 ]; then
    ok "K0 provision_mail_host_cert extracted — $PROV_N lines"
  else
    bad "K0 provision_mail_host_cert extracted — only $PROV_N lines (the extractor broke; the arms below examined nothing)"
  fi

  # H1 — ONE door. The raw agent provision call must exist exactly once in the
  # file, i.e. only inside the guarded helper. Two auto-DNS providers reached this
  # through byte-identical blocks before v2.156.0, and a guard applied to one leg
  # and not the other is this project's most-repeated regression shape.
  RAW_PROV=$(prod_count "$MAILF" -cE 'format!\("/ssl/provision/')
  if [ "$RAW_PROV" -eq 1 ]; then
    ok "K1 the agent provision call appears once in mail.rs — both providers share the guarded path"
  else
    bad "K1 the agent provision call appears once in mail.rs — found $RAW_PROV, so a leg issues unguarded"
  fi

  # H2 — and both legs actually reach it. One definition plus two call sites.
  # Counted on the bare name so rustfmt wrapping the argument list cannot make a
  # correct tree read as a broken one.
  PROV_REFS=$(prod_count "$MAILF" -c 'provision_mail_host_cert')
  if [ "$PROV_REFS" -eq 3 ]; then
    ok "K2 provision_mail_host_cert is defined once and called from both auto-DNS legs"
  else
    bad "K2 provision_mail_host_cert is defined once and called from both auto-DNS legs — $PROV_REFS references (expected 3)"
  fi

  # H3 — the verdict is consulted BEFORE the call, not after it and not at all.
  # Position, because a guard whose result is computed and then ignored reads
  # identically to one that is absent.
  V_AT=$(grep -n 'mail_cert_verdict' <<< "$PROV_BODY" | head -1 | cut -d: -f1)
  P_AT=$(grep -n '/ssl/provision/' <<< "$PROV_BODY" | head -1 | cut -d: -f1)
  if [ -n "$V_AT" ] && [ -n "$P_AT" ] && [ "$V_AT" -lt "$P_AT" ]; then
    ok "K3 the verdict is taken before the certificate is ordered (verdict line $V_AT, order line $P_AT)"
  else
    bad "K3 the verdict is taken before the certificate is ordered — verdict=${V_AT:-absent} order=${P_AT:-absent}"
  fi

  # H4 — the guard returns rather than falling through. A verdict that refuses and
  # then orders anyway is the shape a mutation of the branch produces, and it is
  # invisible to a presence-shaped arm.
  # ⚠ Written first as `grep -A 3` and it was RED against correct code: rustfmt
  # wraps the verdict's argument list, so the `return;` sits six lines below the
  # match, not three. A fixed -A window is not a branch (#172) — this asks the
  # only question that matters and cannot be fooled by formatting: the refusal
  # returns BEFORE the order is placed.
  R_AT=$(grep -n 'MailCertVerdict::Refuse' <<< "$PROV_BODY" | head -1 | cut -d: -f1)
  RET_AT=$(grep -nE '^\s+return;' <<< "$PROV_BODY" | head -1 | cut -d: -f1)
  if [ -n "$R_AT" ] && [ -n "$RET_AT" ] && [ -n "$P_AT" ] \
     && [ "$R_AT" -lt "$RET_AT" ] && [ "$RET_AT" -lt "$P_AT" ]; then
    ok "K4 a refusal returns before the certificate is ordered (refuse $R_AT, return $RET_AT, order $P_AT)"
  else
    bad "K4 a refusal returns before the certificate is ordered — refuse=${R_AT:-absent} return=${RET_AT:-absent} order=${P_AT:-absent}"
  fi

  # H5 — BOTH inputs survive. The columns catch the LE wildcard the issuer probe
  # is blind to (foreign_cert_issuer collapses ours-and-unknown to None); the
  # issuer catches the uploaded certificate the columns cannot see. Deleting
  # either half leaves a guard that still looks like a guard.
  VERDICT_BODY=$(prod_lines "$MAILF" \
    | awk '/^pub fn mail_cert_verdict\(/ {inb=1} inb {print} inb && /^\}$/ {exit}')
  VERDICT_N=$(grep -c . <<< "$VERDICT_BODY")
  if [ "$VERDICT_N" -lt 8 ]; then
    bad "K5-control mail_cert_verdict extracted — only $VERDICT_N lines (the extractor broke)"
  else
    ok "K5-control mail_cert_verdict extracted — $VERDICT_N lines"
    # ⚠ FIRST CUT OF THIS ARM WAS WEAK AND A MUTATION PROVED IT. It grepped the
    # three INPUT NAMES, and `foreign_issuer` is also the parameter's name — so
    # deleting the whole issuer check left the signature behind and the arm passed
    # over a guard with half its cases gone. #766's family, in an arm written to
    # catch exactly that. What is asserted now is that each input reaches a
    # DECISION: both blocker variants must be returned from this body, which no
    # signature can satisfy.
    K5_MISS=""
    grep -qE 'ssl_wildcard'   <<< "$VERDICT_BODY" || K5_MISS="$K5_MISS ssl_wildcard"
    grep -qE 'dns-01'         <<< "$VERDICT_BODY" || K5_MISS="$K5_MISS dns-01"
    grep -qE 'foreign_issuer' <<< "$VERDICT_BODY" || K5_MISS="$K5_MISS foreign_issuer"
    grep -qE 'MailCertBlocker::Wildcard' <<< "$VERDICT_BODY" || K5_MISS="$K5_MISS refuses-wildcard"
    grep -qE 'MailCertBlocker::Foreign'  <<< "$VERDICT_BODY" || K5_MISS="$K5_MISS refuses-foreign"
    if [ -z "$K5_MISS" ]; then
      ok "K5 the verdict consults the wildcard columns AND the issuer — neither half alone covers the other's case"
    else
      bad "K5 the verdict consults the wildcard columns AND the issuer — missing:$K5_MISS"
    fi
  fi

  # H6 — a refusal is ANNOUNCED. Silence here is indistinguishable from the ACME
  # failure the Err arm reports, so the operator could not tell a refusal that
  # saved their wildcard from a network error they should retry.
  if grep -qE 'announce_mail_cert_conflict' <<< "$PROV_BODY"; then
    ok "K6 a refusal is announced to the operator, not skipped silently"
  else
    bad "K6 a refusal is announced to the operator — the refuse branch tells nobody"
  fi

  # H7 — severed-pair. A dedup key nothing raises decides nothing, and a key
  # raised under a name nothing defines is an inline literal by another route.
  # ⚠ The definition side is read from the KEY MODULE'S OWN RANGE, not from the
  # whole file. `prod_lines` blanks from the first `#[cfg(test)]` to EOF, and
  # notifications.rs carried one at line 136 of 1463 — so a whole-file `prod_count`
  # here answered 0 for a constant that is plainly defined at 1107, and this arm
  # was RED against correct code. The module was moved to the end of that file in
  # the same commit; scoping to the range as well means the arm survives it coming
  # back. Same defect as routes/ssl.rs at s388 (#669).
  DEF_MHC=$(sed -n '/^pub mod ssl_renewal_key {/,/^}/p' "$NOTIF" | grep -c 'MAIL_HOST_CONFLICT')
  USE_MHC=$(prod_count "$MAILF" -c 'MAIL_HOST_CONFLICT')
  if [ "$DEF_MHC" -ge 1 ] && [ "$USE_MHC" -ge 1 ]; then
    ok "K7 the mail-conflict dedup key is both defined and raised (def=$DEF_MHC use=$USE_MHC)"
  else
    bad "K7 the mail-conflict dedup key is both defined and raised — def=$DEF_MHC use=$USE_MHC"
  fi

  # H8 — the no-`sites`-row leg is announced too, not just the site leg. Until
  # s411 `announce_mail_cert_conflict` took a non-optional site id and owner, so
  # a Compose stack or Docker app domain (which can never own a `sites` row —
  # `domain_claim::find_occupant` guarantees it) hit only a `tracing::warn!`
  # with no alert behind it: the exact severed-pair shape K6/K7 already guard
  # against for the site-owning leg, just on the other branch of the same `if`.
  # Two calls in one function body is the only shape that proves both branches
  # reach it — K6 alone is satisfied by either one.
  ANNOUNCE_N=$(grep -c 'announce_mail_cert_conflict' <<< "$PROV_BODY")
  if [ "$ANNOUNCE_N" -eq 2 ]; then
    ok "K8 both the site leg and the no-site leg announce a refusal ($ANNOUNCE_N call sites)"
  else
    bad "K8 both the site leg and the no-site leg announce a refusal — found $ANNOUNCE_N call sites (expected 2)"
  fi

  # H9 — the signature the fix actually depends on. K8 would stay green even if
  # `site_id` were forced back to a non-optional `Uuid` and the no-site branch
  # invented an id to satisfy it — a shape that compiles, calls the function
  # twice, and reintroduces exactly what H8's comment describes. Pin the type.
  ANNOUNCE_SIG=$(prod_lines "$MAILF" \
    | awk '/^async fn announce_mail_cert_conflict\(/ {inb=1} inb {print} inb && /^\) \{$/ {exit}')
  if grep -qE 'site_id: Option<uuid::Uuid>' <<< "$ANNOUNCE_SIG"; then
    ok "K9 announce_mail_cert_conflict's site id is Option — the no-site leg cannot be served by an invented id"
  else
    bad "K9 announce_mail_cert_conflict's site id is no longer Option — the no-site leg may be back to inventing one, or gone"
  fi
fi

echo
echo "──────────────────────────────────────────"
printf 'PASS: %d   FAIL: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
