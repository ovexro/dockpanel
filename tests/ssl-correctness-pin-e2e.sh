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
#
# No running panel needed.
#   run: bash tests/ssl-correctness-pin-e2e.sh
set -u

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GATE="$REPO/scripts/npm-audit-gate.mjs"
SITES="$REPO/panel/backend/src/routes/sites.rs"
WP="$REPO/panel/agent/src/services/wordpress.rs"
AGENT_SSL="$REPO/panel/agent/src/services/ssl.rs"
PASS=0; FAIL=0
ok()   { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (expected [$3], got [$2])"; fi; }

TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT

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
grep -q 'stale ' "$GATE" \
  && ok "the gate still reports an allowlist entry that matches nothing" \
  || bad "the stale-entry report was removed along with the entry it was written for"
if [ "$(node -e "const s=require('fs').readFileSync('$GATE','utf8');const m=s.match(/const ALLOWLIST = \\[([\\s\\S]*?)\\n\\];/);process.stdout.write(String((m[1].match(/^\\s*\\{/gm)||[]).length))")" = "0" ]; then
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
HOOK="$REPO/scripts/hooks/pre-push"
grep -q 'npm-audit-gate.mjs' "$HOOK" \
  && ok "the pre-push hook uses the same gate CI does" \
  || bad "the pre-push hook audits npm its own way again"
grep -q 'NPM_AUDIT_ALLOWLIST' "$HOOK" \
  && bad "a second npm allowlist is back in the pre-push hook" \
  || ok "there is exactly one reviewed-advisory allowlist"

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

for m in $RR_MANIFESTS; do
  d=$(dirname "$m"); rel=${d#"$REPO"/}

  grep -q '"@react-router/' "$m" \
    && bad "$rel now depends on a @react-router/* server package — RE-ASSESS the waiver in scripts/npm-audit-gate.mjs, it assumes there is no server runtime" \
    || ok "$rel has no @react-router/* server runtime package"

  if compgen -G "$d/react-router.config.*" >/dev/null 2>&1; then
    bad "$rel has a react-router.config.* — framework mode is on, so RSC is reachable; RE-ASSESS the waiver"
  else
    ok "$rel has no react-router.config.* (framework mode absent)"
  fi

  if grep -rqE 'createRequestHandler|"use server"' "$d/src" 2>/dev/null; then
    bad "$rel now has a request handler or a server action — the waiver's core premise is gone; RE-ASSESS it"
  else
    ok "$rel has no createRequestHandler and no server actions"
  fi
done

echo
echo "── B: a site is installed at the scheme it can actually serve ──"

# The install runs in a task beside auto-SSL, which may still fail. Pinning the
# literal here is the point: this is the line that decided a brand-new site was
# unreachable on both schemes.
grep -q '"url": format!("http://{cms_domain}")' "$SITES" \
  && ok "the CMS installer is handed the plain-HTTP URL" \
  || bad "the CMS installer no longer installs at the plain-HTTP URL"

grep -q '"url": format!("https' "$SITES" \
  && bad "an unconditional secure install URL is back in sites.rs" \
  || ok "no unconditional secure install URL remains"

grep -q 'promote-https' "$SITES" \
  && ok "the panel promotes the canonical URL once the certificate lands" \
  || bad "nothing promotes the canonical URL after a late certificate"

echo
echo "── C: the promotion only ever touches the URL DockPanel itself set ──"

# Extracted so it can be tested at all — the surrounding function shells out to
# wp-cli. A settable canonical URL is a site-takeover primitive, so the guard
# matters more than the rewrite.
grep -q 'fn https_promotion_target' "$WP" \
  && ok "the promotion decision is a separate, testable function" \
  || bad "the promotion decision is not extracted"

grep -q 'eq_ignore_ascii_case' "$WP" \
  && ok "…and compares the stored URL against this vhost's own plain-HTTP form" \
  || bad "the promotion no longer pins the comparison to this vhost's own domain"

grep -q 'promote_site_url_to_https(domain)' "$AGENT_SSL" \
  && ok "every path that enables SSL runs the promotion (single choke point)" \
  || bad "enabling SSL no longer promotes the canonical URL"

echo
echo "── D: a certificate that stops renewing is announced, not just logged ──"

HEALER="$REPO/panel/backend/src/services/auto_healer.rs"
SCAN="$REPO/panel/backend/src/services/security_scanner.rs"
NOTIF="$REPO/panel/backend/src/services/notifications.rs"

grep -q 'fn fire_alert_deduped' "$NOTIF" \
  && ok "there is a deduped alert path" \
  || bad "no deduped alert path — an alert from a 120s loop is a flood"

# The bail-outs. These run BEFORE any attempt is made, which is exactly the
# case F6 named: issuance is rescued by the fallback contact, renewal is not
# even tried, and sixty days later the certificate expires on a live server.
check "auto-healer alerts when a renewal cannot be attempted" \
  "$(grep -c 'ssl_renewal_blocked(' "$HEALER")" "3"
check "the scanner alerts on every renewal outcome it can see" \
  "$(grep -c 'ssl_renewal_alert(' "$SCAN")" "4"

# Both loops touch the same certificate; neither may alert unconditionally.
grep -q 'fire_alert_deduped' "$HEALER" && grep -q 'fire_alert_deduped' "$SCAN" \
  && ok "both loops dedupe, so one stuck certificate is one alert" \
  || bad "a loop still alerts unconditionally"

grep -q 'fire_alert(' "$SCAN" && grep -q 'ssl_renewal' "$SCAN" \
  && ok "the scanner's renewal path reaches the alert system at all" \
  || bad "the scanner's renewal failure is still log-only"


# ── E: the installer must not read "I could not ask" as "it failed" ──────────
#
# Driven out of a real fresh-box install (s258): a dbus hiccup made
# `systemctl is-active` exit non-zero with an empty answer, and the installer
# aborted at step 11/15 blaming a service that was running. Stubbed here because
# a transient bus failure cannot be summoned on demand — but the three answers it
# has to tell apart can.
echo
echo "── E: the installer's unit readiness check ──"

SETUP="$REPO/scripts/setup.sh"
grep -q 'wait_for_unit()' "$SETUP" \
  && ok "there is a readiness helper" \
  || bad "the installer is back to a single is-active call"
check "no bare is-active decides a service's fate" \
  "$(grep -c 'if systemctl is-active --quiet dockpanel' "$SETUP")" "0"

# Extract the helper and run it against a stubbed systemctl.
sed -n '/^wait_for_unit() {/,/^}/p' "$SETUP" > "$TMP/helper.sh"
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
# agent returned, so `sites.ssl_expiry` stayed at the value written when the
# certificate was first issued. The dashboard countdown ran to zero and the
# warning ladder walked down to the EXPIRED sentinel on a certificate that had
# renewed perfectly — with no way back, because the resolve branch fires only
# when the remaining days RISE.
#
# These arms strip comments first. The code under test explains this defect in
# its own comments, naming the very columns being asserted, and a raw grep would
# match the prose and stay green while the write-back was removed.
# (feedback_source_pin_prose_trap; the stripper is the FIXED one — a `/*` inside
# a string literal must not open a block comment, lesson #136.)
sslcode() {
  awk '{ t=$0; sub(/^[ \t]+/,"",t); if (t !~ /^\/\//) print }' "$1" \
    | awk '
        /^[ \t]*\/\*/ { inblk=1 }
        !inblk { print }
        /\*\/[ \t]*$/ { inblk=0 }
      ' \
    | awk '/^#\[cfg\(test\)\]/ { intest=1 } !intest { print }'
}
sslcount() { sslcode "$1" | grep -cE -- "$2" || true; }

# THE SUBJECTS ARE DERIVED. Every backend file that asks an agent to issue or
# renew a certificate FOR A ROW IN `sites` must store what came back. A
# hardcoded list is the arm that cannot see the next one — and the path that
# carried this defect is one a list written from memory would have missed,
# because it is a security scanner rather than an SSL route (lesson #182).
#
# Both halves of the predicate are load-bearing, and each was added because the
# looser version named something real that is not in this class:
#
#   * `format!("/ssl/…` — the AGENT call. Matching the bare path also matches
#     the panel's own route table (`mod.rs` registers `/api/ssl/{id}/renew`),
#     which issues nothing.
#   * a reference to the `sites` table — the certificate must be one the panel
#     TRACKS. `mail.rs` provisions for a mail host and works entirely in
#     `mail_domains`; it holds no `sites` row, so there is no expiry column for
#     it to write and demanding one would be a false red for ever.
#
# Named here rather than filtered silently, so the next session extends the
# class deliberately instead of rediscovering the exclusions.
RENEW_CALLERS=()
RENEW_SKIPPED=0
while IFS= read -r f; do
  [ -n "$f" ] || continue
  if grep -qE 'FROM sites|UPDATE sites|INTO sites' "$f"; then
    RENEW_CALLERS+=("$f")
  else
    RENEW_SKIPPED=$((RENEW_SKIPPED + 1))
  fi
done < <(grep -rlE 'format!\("/ssl/(provision/|\{)' \
           "$REPO/panel/backend/src" --include=*.rs 2>/dev/null | sort)
[ "$RENEW_SKIPPED" -gt 0 ] && \
  echo "  · $RENEW_SKIPPED agent SSL caller(s) hold no sites row and are out of this class"

if [ "${#RENEW_CALLERS[@]}" -lt 3 ]; then
  bad "enumerated only ${#RENEW_CALLERS[@]} backend callers of an agent SSL issue/renew route — implausible, so the arms below would pass having examined nothing"
else
  ok "enumerated ${#RENEW_CALLERS[@]} backend callers of an agent SSL issue/renew route"
  for f in "${RENEW_CALLERS[@]}"; do
    if [ "$(sslcount "$f" 'ssl_expiry[ ]*=[ ]*\$')" -gt 0 ]; then
      ok "$(basename "$f") stores the expiry it was handed"
    else
      bad "$(basename "$f") renews a certificate and never records it — the panel keeps describing the retired one, down to a false EXPIRED it cannot resolve"
    fi
  done
fi

# The ARI trap: on a RENEWAL the two bookkeeping columns must move with the
# expiry. A surviving `ssl_renewal_at` is a window computed for the certificate
# that was just replaced. (Initial issuance in sites.rs has nothing to clear, so
# this is asserted on the renewal paths that carry the pattern, not on every
# writer.)
for f in "$REPO/panel/backend/src/services/security_scanner.rs" \
         "$REPO/panel/backend/src/services/auto_healer.rs"; do
  if [ "$(sslcount "$f" 'ssl_renewal_at = NULL')" -gt 0 ] \
  && [ "$(sslcount "$f" 'ssl_renewal_checked_at = NULL')" -gt 0 ]; then
    ok "$(basename "$f") clears both ARI columns when it records a renewal"
  else
    bad "$(basename "$f") records a renewal without clearing the ARI window computed for the retired certificate"
  fi
done

# And the host-blind re-read that already wrote across hosts. `domain` is unique
# only per server, so a post-renewal lookup keyed on it can return another
# host's row — which this path then pushes as a vhost through THIS host's agent.
if [ "$(sslcount "$REPO/panel/backend/src/services/security_scanner.rs" 'FROM sites WHERE domain')" -eq 0 ]; then
  ok "the scanner's post-renewal re-read is not keyed on a domain"
else
  bad "the scanner re-reads the site by domain after renewing — on a fleet that can rebuild another host's vhost through this host's agent"
fi

echo
echo "──────────────────────────────────────────"
printf 'PASS: %d   FAIL: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
