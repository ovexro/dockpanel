#!/usr/bin/env bash
# The scheduled check — the mechanism for the rot CI structurally cannot see.
#
# DockPanel's documentation-truth register (memory: the four rot classes) says
# the same thing four times: different facts rot for different reasons, so one
# mechanism cannot cover them.
#
#   Class A drifts when CODE changes         → tests/docs-claims-pin-e2e.sh, on every commit.
#   Class B drifts with TIME, not commits    → THIS FILE. CI never runs, because nothing changed.
#   Class C is deploy freshness              → THIS FILE. Source correct is not served correct.
#   Class D is the missing denominator       → the coverage ledger; not this file.
#
# Class B was the one class with no mechanism at all, for nine sessions. It is
# the class that let the OG card advertise a 10 MB binary for thirty-four
# releases, /security state a version thirty-seven releases old, and the panel's
# memory footprint sit at ~19 MB on three surfaces against a real reading of
# ~49 MB. Every one of those was found by a person reading the page. That is not
# a mechanism, that is luck with good intentions.
#
# What makes this file different from every other suite in tests/: it needs the
# NETWORK and it needs to run when nothing has happened. It is therefore driven
# by .github/workflows/live-surfaces.yml on a cron, from a GitHub runner —
# deliberately not from the box that serves the sites, because a check that runs
# on the origin cannot see the CDN in front of it, and s268 lost half a day to
# exactly that: committed and built agreed while the edge served the old file.
#
# Usage:
#   bash tests/live-surfaces-check.sh            # everything
#   SKIP_BUILD=1 bash tests/live-surfaces-check.sh   # skip the class-C rebuild
#
# Exit 0 = every live surface agrees with this commit. Non-zero = something out
# there has drifted from something in here.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()   { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }
note() { printf '    \033[0;90m%s\033[0m\n' "$1"; }

SITE=${SITE:-https://dockpanel.dev}
DOCS=${DOCS:-https://docs.dockpanel.dev}
REPO=${REPO:-ovexro/dockpanel}
CURL=(curl -fsS --max-time 30 --retry 2 --retry-delay 3)

# Two arms below call the GitHub API. Unauthenticated it allows 60 requests an
# hour PER IP, and GitHub's hosted runners share addresses — so an unauthenticated
# scheduled check is one busy hour away from reporting "could not read the latest
# release" and looking like a product failure. Building this suite exhausted that
# budget locally in a single mutation sweep.
#
# In Actions the workflow passes GITHUB_TOKEN; locally `gh` supplies one if it is
# logged in. The arms fail loudly when the API is unreachable rather than skipping,
# so a token problem cannot be mistaken for a clean run.
GH_TOKEN_VALUE="${GITHUB_TOKEN:-${GH_TOKEN:-}}"
if [ -z "$GH_TOKEN_VALUE" ] && command -v gh >/dev/null 2>&1; then
  GH_TOKEN_VALUE=$(gh auth token 2>/dev/null || true)
fi
GH_API=("${CURL[@]}" -H 'Accept: application/vnd.github+json')
if [ -n "$GH_TOKEN_VALUE" ]; then
  GH_API+=(-H "Authorization: Bearer $GH_TOKEN_VALUE")
fi

# A measurement nobody has retaken is a claim. 120 days is long enough that a
# quiet quarter does not nag, short enough that a figure cannot go two releases
# without somebody putting a number next to a running process.
MEASUREMENT_BUDGET_DAYS=120

# Days of certificate life below which "it works today" stops being reassuring.
CERT_MIN_DAYS=14

TODAY_EPOCH=$(date -u +%s)

echo "── 1. Class B — the advertised install one-liner still serves an installer ──"

# The whole product is behind this URL. README and the marketing site both tell
# a stranger to pipe it into bash. Nothing in CI touches it: the file is static,
# the nginx vhost is not in this repository, and the SPA is configured to serve
# index.html for unknown paths — so the failure mode is not a 404, it is a 200
# that returns a React page, which `curl | bash` would cheerfully execute.
INSTALL_URL="$SITE/install.sh"
INSTALL_TMP=$(mktemp)

# Downloaded to a FILE, not captured in a variable. `$(curl ...)` strips trailing
# newlines, so hashing the capture reported a mismatch against a file that was in
# fact byte-identical — this check's first run accused the live site of serving an
# installer we did not ship, and the difference was one absent '\n' introduced by
# the check itself. Comparing bytes means handling bytes.
if "${CURL[@]}" -o "$INSTALL_TMP" "$INSTALL_URL" 2>/dev/null; then
  ok "$INSTALL_URL answers"
  if head -1 "$INSTALL_TMP" | grep -q '^#!'; then
    ok "it begins with a shebang"
  else
    bad "$INSTALL_URL returned 200 but the body does not start with a shebang — the SPA fallback is answering, and the advertised one-liner pipes a web page into bash"
    note "first line: $(head -1 "$INSTALL_TMP" | cut -c1-80)"
  fi
  if head -c 2000 "$INSTALL_TMP" | grep -qiE '<!doctype html|<html'; then
    bad "$INSTALL_URL is serving HTML"
  else
    ok "it is not HTML"
  fi
  # It must be OUR installer, not merely a shell script.
  if grep -q 'dockpanel' "$INSTALL_TMP"; then
    ok "it mentions dockpanel"
  else
    bad "$INSTALL_URL serves a shell script that never mentions dockpanel"
  fi
  # And it must be the one in this repository. The published copy is the file
  # nginx serves out of website/client/dist/, which is built from public/.
  if [ -f website/client/public/install.sh ]; then
    live_sum=$(sha256sum "$INSTALL_TMP" | cut -d' ' -f1)
    repo_sum=$(sha256sum website/client/public/install.sh | cut -d' ' -f1)
    if [ "$live_sum" = "$repo_sum" ]; then
      ok "the served installer is byte-identical to website/client/public/install.sh"
    else
      bad "the served installer differs from website/client/public/install.sh — the site is publishing an installer this commit does not contain"
      note "served $live_sum vs repo $repo_sum"
    fi
  else
    bad "website/client/public/install.sh is missing — the advertised one-liner has no source in this repo"
  fi
else
  bad "$INSTALL_URL does not answer — the install command on the README and the front page is dead"
fi
rm -f "$INSTALL_TMP"

echo
echo "── 2. Class B — both sites answer, on certificates with life left ──"

for url in "$SITE" "$DOCS"; do
  host=${url#https://}
  code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 "$url" 2>/dev/null || echo 000)
  if [ "$code" = "200" ]; then
    ok "$url → 200"
  else
    bad "$url → $code"
  fi

  # Expiry, not merely validity: a cert that is fine today and gone on Sunday is
  # the class-B failure exactly. curl already refused the connection above if the
  # chain were invalid, so this arm is about time.
  end=$(echo | openssl s_client -servername "$host" -connect "$host:443" 2>/dev/null \
        | openssl x509 -noout -enddate 2>/dev/null | cut -d= -f2)
  if [ -z "$end" ]; then
    bad "could not read a certificate for $host"
  else
    end_epoch=$(date -u -d "$end" +%s 2>/dev/null || echo 0)
    if [ "$end_epoch" -eq 0 ]; then
      bad "could not parse the certificate expiry for $host ($end)"
    else
      days=$(( (end_epoch - TODAY_EPOCH) / 86400 ))
      if [ "$days" -ge "$CERT_MIN_DAYS" ]; then
        ok "$host certificate has $days days left"
      else
        bad "$host certificate expires in $days days (below the $CERT_MIN_DAYS-day floor) — renewal is not happening"
      fi
    fi
  fi
done

echo
echo "── 3. Class B — claims that expire have not expired ──"

# FEATURES.md carries two tables this arm reads: the measurement register's
# `measured` rows (figures only a real box can produce) and "Claims That Expire"
# (facts about the world, which nothing in this repo can ever derive).
#
# Neither can be verified automatically — that is the point. What CAN be
# enforced automatically is that somebody looked recently.

expiry_rows=0

check_age() {   # label, YYYY-MM-DD, budget-days
  local label="$1" when="$2" budget="$3"
  local when_epoch age
  when_epoch=$(date -u -d "$when" +%s 2>/dev/null || echo 0)
  if [ "$when_epoch" -eq 0 ]; then
    bad "$label has an unparseable verification date '$when'"
    return
  fi
  expiry_rows=$((expiry_rows+1))
  age=$(( (TODAY_EPOCH - when_epoch) / 86400 ))
  if [ "$age" -le "$budget" ]; then
    ok "$label — verified $age days ago (budget $budget)"
  else
    bad "$label — last verified $age days ago, past its $budget-day budget. Nobody has checked this since $when; it is not known to be wrong, it is unknown."
  fi
}

# The register's measured rows.
while IFS='|' read -r _ metric value source verified _; do
  metric=$(sed -e 's/^ *//' -e 's/ *$//' <<<"$metric")
  source=$(tr -d ' ' <<<"$source")
  verified=$(tr -d ' ' <<<"$verified")
  [ "$source" = "measured" ] || continue
  check_age "register: $metric" "$verified" "$MEASUREMENT_BUDGET_DAYS"
done < <(awk '/^## Verified Metrics/ {inside=1; next} inside && /^## / {exit} inside && /^\|/ {print}' FEATURES.md)

# The claims-that-expire table.
while IFS='|' read -r _ claim _ when budget _; do
  claim=$(sed -e 's/^ *//' -e 's/ *$//' <<<"$claim")
  when=$(tr -d ' ' <<<"$when")
  budget=$(tr -d ' ' <<<"$budget")
  [[ "$when" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]] || continue
  [[ "$budget" =~ ^[0-9]+$ ]] || continue
  check_age "$claim" "$when" "$budget"
done < <(awk '/^## Claims That Expire/ {inside=1; next} inside && /^## / {exit} inside && /^\|/ {print}' FEATURES.md)

if [ "$expiry_rows" -eq 0 ]; then
  bad "no expiring claim was checked — both tables in FEATURES.md parsed to nothing and this entire arm proves nothing"
else
  ok "$expiry_rows expiring claims checked"
fi

echo
echo "── 4. Class B — the react-router waiver's own re-check condition ──"

# scripts/npm-audit-gate.mjs waives GHSA-qwww-vcr4-c8h2 with a written reason:
# the advisory needs RSC mode with server actions, both frontends import only
# BrowserRouter and ship as static assets, and npm's only offered "fix" is a
# DOWNGRADE out of the 7.x line. The waiver ends with "re-check when react-router
# ships a fix inside ^7" — and for five ships nothing checked that, which is
# textbook class B: the waiver was correct when written and no commit here will
# ever tell us it stopped being.
ADVISORY=GHSA-qwww-vcr4-c8h2
if adv=$("${GH_API[@]}" "https://api.github.com/advisories/$ADVISORY" 2>/dev/null); then
  patched=$(grep -o '"first_patched_version":[^,}]*' <<<"$adv" | head -1 | grep -oE '"[0-9][^"]*"' | tr -d '"')
  if [ -z "$patched" ]; then
    ok "$ADVISORY still has no patched version — the waiver's condition has not been met"
  elif [[ "$patched" == 7.* ]]; then
    bad "$ADVISORY now has a fix inside ^7 ($patched) — the waiver in scripts/npm-audit-gate.mjs said to re-check when this happened. Bump and drop the waiver."
  else
    ok "$ADVISORY's first patched version is $patched, outside ^7 — the waiver still stands"
    note "the waiver is conditioned on a fix INSIDE ^7; a major-version fix is a migration, not a bump"
  fi
else
  bad "could not reach the GitHub advisory API for $ADVISORY — the waiver went unchecked this run"
fi

echo
echo "── 5. Class B — the published release still matches the register ──"

# Binary sizes are the one register row that is neither derivable from source
# (they come out of a musl release build, not a dev build) nor a human reading.
# They come from the published artefacts, which is a network fact, which is why
# it lives here rather than in the pin suite.
if ! command -v jq >/dev/null 2>&1; then
  # Loudly, rather than falling back to a regex. The first version of this arm
  # scraped the JSON with `"name":"…",[^}]*` and matched nothing, because every
  # asset embeds an `uploader` object whose closing brace ends the match before
  # `size` is reached — so it reported that the release shipped no binaries at
  # all. A parser that cannot parse must say so, not guess.
  bad "jq is not installed — the release-asset check cannot parse the API response"
elif rel=$("${GH_API[@]}" "https://api.github.com/repos/$REPO/releases/latest" 2>/dev/null); then
  tag=$(jq -r '.tag_name // empty' <<<"$rel")
  if [ -z "$tag" ]; then
    bad "the releases API returned no tag_name for $REPO"
  else
    ok "latest published release is $tag"
  fi

  asset_mb() {  # asset name → size in MiB, rounded to one decimal
    jq -r --arg n "$1" '.assets[] | select(.name == $n) | .size' <<<"$rel" \
      | head -1 | awk 'NF { printf "%.1f", $1/1048576 }'
  }

  # register metric → published asset
  check_asset() {  # metric, asset-name
    local metric="$1" asset="$2"
    local want got
    want=$(awk -F'|' -v m="$metric" '
      $0 ~ /^\|/ { gsub(/^ +| +$/,"",$2); if ($2 == m) { gsub(/[^0-9.]/,"",$3); print $3; exit } }
    ' FEATURES.md)
    if [ -z "$want" ]; then
      bad "the register has no '$metric' row — the release check lost its anchor"
      return
    fi
    got=$(asset_mb "$asset")
    if [ -z "$got" ]; then
      bad "$tag publishes no asset named $asset — the register describes binaries this release does not ship"
      return
    fi
    # Register values are whole MB for the two big binaries; compare with a 1 MB
    # tolerance so a normal release does not fail on rounding, and a real drift
    # (the register said 10 MB while the binary was 22) still does.
    if awk -v a="$got" -v b="$want" 'BEGIN { exit !(a - b < 1.0 && b - a < 1.0) }'; then
      ok "$asset is ${got} MB; the register says $metric = $want"
    else
      bad "$asset is ${got} MB but the register publishes $metric = $want — every surface quoting that row is wrong by $(awk -v a="$got" -v b="$want" 'BEGIN{printf "%.1f", (a>b?a-b:b-a)}') MB"
    fi
  }

  check_asset "API binary"   dockpanel-api-linux-amd64
  check_asset "Agent binary" dockpanel-agent-linux-amd64
  check_asset "CLI binary"   dockpanel-cli-linux-amd64
else
  bad "could not read the latest release of $REPO"
fi

echo
echo "── 6. Class C — committed == built == served ──"

# The marketing site is built by hand and served straight off the origin, behind
# Cloudflare. There are therefore THREE things that can disagree, and s263 and
# s268 each found a different pair of them agreeing while the third did not:
# s263 had source fixed and the live bundle stale; s268 had committed and built
# agreeing while the CDN edge served the previous image. Any two matching proves
# nothing, so this compares all three, by hash, fetched from outside.
if [ "${SKIP_BUILD:-0}" = "1" ]; then
  note "SKIP_BUILD=1 — class C not checked this run"
else
  if [ ! -d website/client ]; then
    bad "website/client is missing"
  else
    build_log=$(mktemp)
    # Built into a TEMPORARY directory, never into website/client/dist.
    #
    # On the origin box nginx serves that dist/ directly, so `npm run build` IS
    # the deploy. The first version of this arm ran it, and therefore overwrote
    # the live site with the working tree and then compared the live site to the
    # working tree — it reported "committed == built == served" a second after
    # making that true, and would have reported it however stale the site was.
    # Worse, running the check published uncommitted edits to production.
    #
    # A check that repairs what it measures cannot measure it.
    OUT=$(mktemp -d)
    if (cd website/client \
        && npm ci --silent \
        && npx tsc --noEmit \
        && npx vite build --outDir "$OUT" --emptyOutDir --logLevel error) >"$build_log" 2>&1; then
      ok "the SPA builds from committed source"

      built=$(grep -oE '/assets/index-[A-Za-z0-9_-]+\.js' "$OUT/index.html" | head -1)
      served_html=$("${CURL[@]}" "$SITE/" 2>/dev/null || true)
      served=$(grep -oE '/assets/index-[A-Za-z0-9_-]+\.js' <<<"$served_html" | head -1)

      if [ -z "$built" ]; then
        bad "no hashed entry bundle in the freshly built index.html — the build output shape changed and this check went blind"
      elif [ -z "$served" ]; then
        bad "no hashed entry bundle in the HTML $SITE actually serves — this check cannot see what is live"
      elif [ "$built" = "$served" ]; then
        ok "committed source builds to $built, which is what $SITE serves"

        # The filename matching is not enough on its own: Vite hashes content, so
        # an identical name should mean identical bytes — but the edge is a cache,
        # and a cache is exactly the thing that can hand back something else under
        # the right name. Compare the bytes.
        fetched=$(mktemp)
        if "${CURL[@]}" -o "$fetched" "$SITE$served" 2>/dev/null; then
          a=$(sha256sum "$OUT${served}" | cut -d' ' -f1)
          b=$(sha256sum "$fetched" | cut -d' ' -f1)
          if [ "$a" = "$b" ]; then
            ok "and the served bytes hash identically to the built ones"
          else
            bad "the served bundle has the built bundle's NAME but different bytes — a stale object is cached under a current filename"
          fi
        else
          bad "could not fetch $SITE$served"
        fi
        rm -f "$fetched"
      else
        bad "dockpanel.dev is serving a stale build — committed source builds to $built, the live page loads $served. Run npm run build in website/client and purge the Cloudflare cache."
      fi
    else
      bad "the SPA does not build from committed source"
      note "$(tail -5 "$build_log")"
    fi
    rm -rf "$build_log" "$OUT"
  fi
fi

echo
echo "── 7. Class C — the docs site serves this version's evidence page ──"

# docs/testing.md carries a "Reflects vX.Y.Z" stamp that docs-claims-pin-e2e.sh
# checks IN SOURCE. That pin cannot tell you whether mdbook was ever re-run —
# and the docs site has no deploy step at all, so "correct in source" and "live"
# are entirely independent facts.
VERSION=$(grep -m1 '^version = ' panel/backend/Cargo.toml | sed -e 's/.*"\(.*\)".*/\1/')
if page=$("${CURL[@]}" "$DOCS/testing.html" 2>/dev/null); then
  live_stamp=$(grep -oE 'Reflects v[0-9]+\.[0-9]+\.[0-9]+' <<<"$page" | head -1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+')
  if [ -z "$live_stamp" ]; then
    bad "$DOCS/testing.html carries no 'Reflects vX.Y.Z' stamp — the published page lost the marker this check reads"
  elif [ "$live_stamp" = "$VERSION" ]; then
    ok "$DOCS/testing.html says Reflects v$live_stamp, matching Cargo.toml"
  else
    bad "$DOCS/testing.html says Reflects v$live_stamp but this commit is v$VERSION — mdbook has not been re-run since that stamp changed"
  fi
else
  bad "$DOCS/testing.html does not answer"
fi

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
