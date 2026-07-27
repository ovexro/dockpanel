#!/usr/bin/env bash
# Regression pins for s263 — the documentation-truth pass.
#
# DockPanel publishes the same facts on three surfaces: the .md files GitHub
# renders, docs.dockpanel.dev (mdBook, built from docs/), and the dockpanel.dev
# marketing site (website/client). Every number on them was maintained by hand
# in a dozen places at once.
#
# It drifted, exactly as you would expect. s263 found the Docker-app template
# count published as 152 on all three surfaces while the catalogue in source
# held 153 — one template added, ten claim sites not updated, nobody able to
# notice because checking meant counting a Rust array by hand.
#
# So the rule this suite enforces is: a number on a public surface must have a
# derivation from source, and that derivation runs in CI. The suite LOCATES the
# claims by pattern rather than by hard-coded line number, so adding a new claim
# site puts it under the pin automatically instead of creating a blind spot —
# the failure mode that produced the drift in the first place.
#
# Pure source/text analysis: no box, no network, no build.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

# The surfaces a reader can actually reach. website/client/src is the marketing
# SPA's source; it is built and served separately, so it drifts independently.
MD_SURFACES=(README.md FEATURES.md COMPARISON.md SECURITY.md CONTRIBUTING.md)
while IFS= read -r f; do MD_SURFACES+=("$f"); done < <(find docs -name '*.md' -not -path 'docs/book/*')
WEB_SURFACES=()
while IFS= read -r f; do WEB_SURFACES+=("$f"); done < <(find website/client/src -name '*.tsx' -o -name '*.ts' 2>/dev/null)

echo "── 1. Docker app template count is derived from the catalogue, not typed ──"

# The catalogue is a single static slice; each entry opens with `id:`. Counting
# `AppTemplateDef` would be one too many — the slice's own type annotation
# names it as well.
CATALOGUE=panel/agent/src/services/docker_apps.rs
N_TEMPLATES=$(awk '
  /static TEMPLATES/ { inside = 1; next }
  inside && /^\];/   { exit }
  inside && /^        id: "/ { n++ }
  END { print n + 0 }
' "$CATALOGUE")

if [ "$N_TEMPLATES" -gt 100 ]; then
  ok "catalogue parsed: $N_TEMPLATES templates in $CATALOGUE"
else
  bad "catalogue parse returned $N_TEMPLATES — the parser has drifted from the source shape, not the docs"
  echo; printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"; exit 1
fi

# Locate template-count claims by what they say, on every surface at once.
CLAIM_RE='[0-9]{2,4}([[:space:]]+(one-click|Docker|app))*[[:space:]]+[Tt]emplates'
drift=0
for f in "${MD_SURFACES[@]}" "${WEB_SURFACES[@]}"; do
  [ -f "$f" ] || continue
  while IFS= read -r hit; do
    num=$(grep -oE '^[0-9]+' <<<"$hit")
    [ -z "$num" ] && continue
    if [ "$num" != "$N_TEMPLATES" ]; then
      bad "$f claims '$hit' but the catalogue holds $N_TEMPLATES"
      drift=1
    fi
  done < <(grep -ohE "$CLAIM_RE" "$f" 2>/dev/null)
done

# The marketing site used to carry the count a second time as a bare stat-tile
# value, which the prose pattern above could not see, so it had its own arm here.
# That tile was removed in s271 and the arm then matched nothing on every run —
# a check that cannot fail, sitting in the suite looking like coverage. It is
# gone; §6 covers the site instead, by way of the register.
[ "$drift" -eq 0 ] && ok "every template-count claim on all three surfaces reads $N_TEMPLATES"

echo
echo "── 2. FEATURES.md's version stamp tracks the shipped version ──"

VERSION=$(grep -m1 '^version = ' panel/backend/Cargo.toml | sed -e 's/.*"\(.*\)".*/\1/')
STAMP=$(grep -m1 -oE '\*\*Version\*\*: v?[0-9]+\.[0-9]+\.[0-9]+' FEATURES.md | grep -oE '[0-9]+\.[0-9]+\.[0-9]+')

if [ -z "$STAMP" ]; then
  bad "FEATURES.md has no '**Version**: vX.Y.Z' stamp to check"
elif [ "$STAMP" = "$VERSION" ]; then
  ok "FEATURES.md stamped v$STAMP, matching panel/backend/Cargo.toml"
else
  bad "FEATURES.md stamped v$STAMP but the shipped version is v$VERSION — the manifest is describing an older product"
fi

echo
echo "── 3. docs/testing.md is checked against the suites it describes ──"

TESTING=docs/testing.md
if [ ! -f "$TESTING" ]; then
  bad "docs/testing.md is missing — the published evidence page is the point of this suite"
else
  t_stamp=$(grep -m1 -oE 'Reflects v[0-9]+\.[0-9]+\.[0-9]+' "$TESTING" | grep -oE '[0-9]+\.[0-9]+\.[0-9]+')
  if [ "$t_stamp" = "$VERSION" ]; then
    ok "docs/testing.md stamped v$t_stamp, matching the shipped version"
  else
    bad "docs/testing.md stamped v${t_stamp:-none} but the shipped version is v$VERSION"
  fi

  # Every suite named in the page's assertion table must exist AND still report
  # the number the page publishes. This is the row most likely to rot: it
  # changes whenever anyone adds a pin.
  rows=0
  sum=0
  while IFS='|' read -r _ name count _; do
    suite=$(tr -d ' `' <<<"$name")
    want=$(tr -d ' ' <<<"$count")
    [[ "$suite" =~ \.sh$ ]] || continue
    [[ "$want" =~ ^[0-9]+$ ]] || continue
    rows=$((rows+1))
    sum=$((sum+want))

    if [ ! -f "tests/$suite" ]; then
      bad "docs/testing.md names tests/$suite, which does not exist"
      continue
    fi

    # Each suite prints its own tally in its own format — "passed: 25",
    # "PASS 52", "══ 17 passed, 0 failed ══". Match the number ADJACENT to the
    # word on either side rather than assuming a separator, since assuming one
    # is what made this check silently report "nothing" for the one suite whose
    # format differed.
    got=$(bash "tests/$suite" 2>/dev/null | grep -iE '(pass(ed)?[^a-z]*[0-9]+|[0-9]+[^a-z]*pass(ed)?)' | tail -1 \
          | grep -oiE '(pass(ed)?[^0-9a-z]*([0-9]+)|([0-9]+)[^0-9a-z]*pass(ed)?)' | head -1 | grep -oE '[0-9]+')
    if [ "$got" = "$want" ]; then
      ok "$suite reports $got assertions, as published"
    else
      bad "docs/testing.md publishes $want assertions for $suite; it actually reports ${got:-nothing}"
    fi
  done < <(grep -E '^\| *`[a-z0-9-]+\.sh` *\|' "$TESTING")

  if [ "$rows" -eq 0 ]; then
    bad "no assertion-table rows parsed out of docs/testing.md — the table shape changed and this check went blind"
  else
    ok "assertion table parsed: $rows suites checked against live output"
  fi

  # The sentence introducing the table publishes its own totals ("Ten suites,
  # 260 assertions"). Every row was verified above and that sentence was not, so
  # it drifted: it read "Seven suites, 195 assertions" over an eight-row table
  # summing to 228 — two published numbers, both wrong, in front of a table that
  # was right. A number nothing derives is a number that rots.
  prose=$(grep -oE '[A-Za-z]+ suites, \*\*[0-9]+ assertions\*\*' "$TESTING" | head -1)
  if [ -z "$prose" ]; then
    bad "no 'N suites, M assertions' summary found above the table — if it was removed, drop this check deliberately"
  else
    p_assert=$(printf '%s' "$prose" | grep -oE '[0-9]+')
    p_suites=$(printf '%s' "$prose" | sed -E 's/ suites.*//')
    words="one two three four five six seven eight nine ten eleven twelve"
    n=0; want_word=""
    for w in $words; do n=$((n+1)); [ "$n" -eq "$rows" ] && want_word="$w"; done
    if [ "$p_assert" != "$sum" ]; then
      bad "docs/testing.md's summary says $p_assert assertions; its own table sums to $sum"
    else
      ok "the summary's assertion total ($p_assert) matches the table it introduces"
    fi
    if [ -n "$want_word" ] && [ "$(printf '%s' "$p_suites" | tr 'A-Z' 'a-z')" != "$want_word" ]; then
      bad "docs/testing.md's summary says '$p_suites suites'; the table has $rows rows ($want_word)"
    else
      ok "the summary's suite count matches the $rows rows in the table"
    fi
  fi
fi

echo
echo "── 4. Claims the behavioural drills disproved must not reappear ──"

# docs/guides/backups.md claimed for months that site backups contained the
# database while `create_backup` was tar-over-the-webroot and nothing else
# (s259). The claim is TRUE now, since v2.34.0 — this pin exists so that if the
# database half is ever reverted, the documentation cannot go back to lying
# about it silently.
if grep -q 'site_database_specs' panel/backend/src/routes/backups.rs 2>/dev/null; then
  ok "site backups still collect database specs — the backups guide's claim remains true"
else
  bad "site backups no longer collect database specs, but docs/guides/backups.md still promises databases are included"
fi

# The mail sandbox fix (s262): OpenDKIM's config was RELOCATED into a permitted
# path. "Fixing" it by widening ReadWritePaths would pass every functional test
# while removing the reason the bug was survivable.
UNIT=$(find . -name 'dockpanel-agent.service' -not -path './panel/*/target/*' | head -1)
if [ -n "$UNIT" ] && grep -qE '^ReadWritePaths=.*/etc/opendkim\.conf' "$UNIT"; then
  bad "the agent unit was widened to /etc/opendkim.conf instead of relocating the config (see docs/testing.md)"
else
  ok "agent sandbox still not widened for opendkim — as docs/testing.md tells readers"
fi

echo
echo "── 5. The published support matrix cannot exceed the tested matrix ──"

# README's masthead names six distro families. Until 2026-07-26 the smoke matrix
# tested two of them, so four families were advertised on the strength of
# nothing — the same "green check measuring the honest half" shape the drills
# keep finding in the product, this time in our own marketing copy. The claim
# and the evidence are now bound together: name a family in the README and CI
# must exec the release binaries on it.
SMOKE=.github/workflows/smoke-test.yml

# The support claim is NOT in one place. It is in README.md, docs/getting-started.md
# and TWICE in the marketing SPA (the FAQ answer and the strip under the install
# box) — four independently-edited sites, which is precisely the shape that let
# the template count drift to 152-in-ten-places. So locate claims by what they
# SAY: any line naming three or more distro families IS a support claim,
# wherever it lives. A new claim site joins the pin by existing.
#
# Prose that merely mentions one or two distros (docs/testing.md describing the
# apt matrix, cli-reference.md's sample output) stays below the threshold.
#
# Threshold: four families, OR three families plus a word that makes the line a
# promise rather than a mention. Three-alone is not enough — CONTRIBUTING.md's
# build-tools line names Ubuntu/Debian and Fedora purely to give two package
# lists, and reading that as a support claim would put a distro under the pin
# that nobody promised. The marketing strip (`Ubuntu · Debian · CentOS · Rocky ·
# Amazon Linux`) has no promise-word at all, which is why the count arm exists.
DISTRO_RE='Ubuntu|Debian|CentOS|Rocky|Alma[Ll]inux|Fedora|Amazon Linux'
PROMISE_RE='[Ss]upports?|[Rr]uns on|\*\*OS\*\*|Requirements'

# Normalise a distro name to a family slug, from either side of the comparison.
family_of() {
  case "$(tr '[:upper:]' '[:lower:]' <<<"$1")" in
    ubuntu*)              echo ubuntu ;;
    debian*)              echo debian ;;
    *centos*)             echo centos ;;
    rocky*)               echo rocky ;;
    alma*)                echo alma ;;
    fedora*)              echo fedora ;;
    amazon*|amzn*)        echo amazon ;;
    *)                    echo UNKNOWN ;;
  esac
}

if [ ! -f "$SMOKE" ]; then
  bad "$SMOKE is missing — nothing verifies the support claim"
else
  # Every claim site, and the families each one names.
  CLAIMED=""
  sites=0
  for f in "${MD_SURFACES[@]}" "${WEB_SURFACES[@]}"; do
    [ -f "$f" ] || continue
    while IFS= read -r line; do
      fams=$(grep -oE "$DISTRO_RE" <<<"$line" | while read -r n; do family_of "$n"; done | sort -u)
      n_fams=$(wc -w <<<"$fams")
      if [ "$n_fams" -lt 4 ]; then
        [ "$n_fams" -ge 3 ] && grep -qE "$PROMISE_RE" <<<"$line" || continue
      fi
      sites=$((sites+1))
      CLAIMED="$CLAIMED $fams"
    done < <(grep -hE "$DISTRO_RE" "$f" 2>/dev/null)
  done
  CLAIMED=$(tr ' ' '\n' <<<"$CLAIMED" | grep -v '^$' | sort -u)

  if [ "$sites" -eq 0 ]; then
    bad "no support-claim line found on any surface — the claim moved and this check went blind"
  else
    ok "support claim located on $sites site(s) across the three surfaces"
  fi

  # Tested images, from the matrix. Stops at the first non-item line so a later
  # `container:` key cannot be swept in.
  IMAGES=$(awk '
    /^ *distro:/      { inside=1; next }
    inside && /^ *#/  { next }
    inside && /^ *- / { sub(/^ *- /,""); print; next }
    inside            { exit }
  ' "$SMOKE")

  if [ -z "$CLAIMED" ] || [ -z "$IMAGES" ]; then
    bad "parsed $(wc -w <<<"$CLAIMED") claimed families and $(wc -w <<<"$IMAGES") matrix images — one side of the comparison went blind"
  else
    TESTED=$(while read -r i; do [ -n "$i" ] && family_of "$i"; done <<<"$IMAGES" | sort -u)

    # An image nobody can classify would silently count for nothing.
    if grep -qx UNKNOWN <<<"$TESTED"; then
      bad "$SMOKE lists an image this check cannot map to a distro family — extend family_of()"
    fi

    missing=""
    for fam in $CLAIMED; do
      grep -qx "$fam" <<<"$TESTED" || missing="$missing $fam"
    done

    if [ -n "$missing" ]; then
      bad "a published surface claims support for:${missing} — no image in $SMOKE tests $([ "$(wc -w <<<"$missing")" -eq 1 ] && echo it || echo them)"
    else
      ok "all $(wc -w <<<"$CLAIMED") claimed distro families are exec-tested by $SMOKE"
    fi
  fi

  # The same surfaces claim ARM64. That half IS covered — by a separate job,
  # since arm64 needs QEMU binfmt on the host rather than a container image — so
  # the matrix check above structurally cannot see it.
  if grep -qi 'ARM64' README.md; then
    if grep -qE '^ *smoke-arm64:' "$SMOKE"; then
      ok "ARM64 is claimed and $SMOKE has the smoke-arm64 job that proves it"
    else
      bad "ARM64 support is claimed but $SMOKE has no smoke-arm64 job"
    fi
  fi
fi

echo
echo "── 6. Every published figure is quoted from the measurement register ──"

# FEATURES.md §"Verified Metrics" is the register: the one place a number about
# DockPanel is written down.
#
# s271 audited every figure on the marketing page by hand and corrected six of
# them. s272 found three still wrong in the same file — two of them entries of
# the same FAQ list, three lines apart, giving different answers for the same
# measurement — and found that the GitHub and docs surfaces, which that audit
# never opened, were still publishing the panel's own memory footprint at
# ~19 MB against a real reading of ~49 MB. Five register rows were wrong, one by
# a factor of three.
#
# Auditing a page of numbers does not converge. Writing each number down once
# does. So this section enforces two properties: the register agrees with the
# source it claims to describe, and no surface states a figure the register does
# not.

REGISTER=FEATURES.md
MEAS=website/client/src/measurements.ts

declare -A REG REG_SRC
reg_rows=0
while IFS='|' read -r _ metric value source _; do
  metric=$(sed -e 's/^ *//' -e 's/ *$//' -e 's/\*\*//g' <<<"$metric")
  value=$(sed -e 's/^ *//' -e 's/ *$//' <<<"$value")
  source=$(tr -d ' ' <<<"$source")
  [ "$metric" = "Metric" ] && continue
  num=$(grep -oE '[0-9]+(\.[0-9]+)?' <<<"$value" | head -1)
  [ -z "$metric" ] || [ -z "$num" ] && continue
  REG["$metric"]="$num"
  REG_SRC["$metric"]="$source"
  reg_rows=$((reg_rows+1))
done < <(awk '/^## Verified Metrics/ {inside=1; next} inside && /^## / {exit} inside && /^\|/ {print}' "$REGISTER")

if [ "$reg_rows" -lt 10 ]; then
  bad "parsed only $reg_rows rows out of $REGISTER's register — the table shape changed and this whole section went blind"
else
  ok "measurement register parsed: $reg_rows metrics from $REGISTER"
fi

# --- 6a. Every metric the register calls "derived" must match its derivation ---
#
# The derivation lives here, beside the assertion, so that reading the check
# tells you what the number means. A register row marked `derived` with no case
# below is a FAILURE rather than a skip: otherwise adding a metric would quietly
# create the blind spot this suite exists to prevent.

derive() {
  case "$1" in
    "App templates")
      echo "$N_TEMPLATES" ;;
    "HTTP routes")
      # Routes registered on the two axum routers. The published figure used to
      # be "776 API endpoints (496 backend + 280 agent)" — a decomposition that
      # summed correctly and matched source on neither side.
      echo $(( $(grep -rhoE '\.route\("[^"]+"' panel/backend/src --include='*.rs' 2>/dev/null | wc -l) \
              + $(grep -rhoE '\.route\("[^"]+"' panel/agent/src   --include='*.rs' 2>/dev/null | wc -l) )) ;;
    "Regression-pin assertions")
      # Re-summed from docs/testing.md's own table, independently of §3.
      awk -F'|' '/^\| *`[a-z0-9-]+\.sh` *\|/ { gsub(/ /,"",$3); s+=$3 } END { print s+0 }' docs/testing.md ;;
    "Frontend pages")
      find panel/frontend/src/pages -maxdepth 1 -name '*.tsx' 2>/dev/null | wc -l ;;
    "DB migrations")
      find panel/backend/migrations -maxdepth 1 -name '*.sql' 2>/dev/null | wc -l ;;
    "Supervised background services")
      grep -cE '^ *spawn_supervised\("' panel/backend/src/main.rs 2>/dev/null ;;
    *)
      return 1 ;;
  esac
}

derived_checked=0
for metric in "${!REG[@]}"; do
  [ "${REG_SRC[$metric]}" = "derived" ] || continue
  if ! got=$(derive "$metric"); then
    bad "the register marks '$metric' as derived, but this suite has no derivation for it — add one, or the row is published on nothing"
    continue
  fi
  got=$(tr -d ' \n' <<<"$got")
  derived_checked=$((derived_checked+1))
  if [ "$got" = "${REG[$metric]}" ]; then
    ok "$metric: register says ${REG[$metric]}, source derives $got"
  else
    bad "$metric: register says ${REG[$metric]}, source derives $got"
  fi
done

if [ "$derived_checked" -eq 0 ]; then
  bad "no derived metric was checked — either the register lost its Source column or every row stopped being derivable"
else
  ok "$derived_checked derived metrics re-computed from source"
fi

# --- 6b. The marketing site quotes the register rather than restating it ---
#
# website/client/src/measurements.ts is the SPA's single copy of these figures;
# every page imports it, so the intra-page contradiction s272 found is no longer
# expressible. What remains checkable is that the SPA's copy still agrees with
# the register — the one seam left between them.

# Read a value out of one nested block, so that `api` under `binary` and `api`
# under `ram` cannot satisfy each other's assertion.
meas_value() {
  awk -v blk="$1" -v key="$2" '
    $0 ~ "^  " blk ": \\{" { inside = 1; next }
    inside && /^  \},/     { exit }
    inside && $0 ~ "^ +" key ": " {
      line = $0
      sub(/^[^:]+: */, "", line)
      sub(/,.*$/, "", line)
      print line
      exit
    }
  ' "$MEAS"
}

check_spa() {
  local metric="$1" blk="$2" key="$3"
  local want="${REG[$metric]:-}"
  if [ -z "$want" ]; then
    bad "the register has no '$metric' row — the site check for $blk.$key lost its anchor"
    return
  fi
  local got; got=$(meas_value "$blk" "$key")
  if [ -z "$got" ]; then
    bad "$MEAS has no $blk.$key — the site's copy of '$metric' moved and this check went blind"
  elif [ "$got" = "$want" ]; then
    ok "$MEAS quotes $metric as $want"
  else
    bad "$MEAS says $blk.$key = $got; the register says '$metric' is $want"
  fi
}

if [ ! -f "$MEAS" ]; then
  bad "$MEAS is missing — the marketing site has gone back to writing its own numbers"
else
  check_spa "API binary"                               binary api
  check_spa "Agent binary"                             binary agent
  check_spa "CLI binary"                               binary cli
  check_spa "Panel binaries, all three"                binary total
  check_spa "API RAM (RSS)"                            ram    api
  check_spa "Agent RAM (RSS)"                          ram    agent
  check_spa "Panel services RAM (agent + API)"         ram    services
  check_spa "Full-stack RAM (with bundled PostgreSQL)" ram    fullStack
fi

# --- 6c. Every surface that publishes a figure still carries the current one ---
#
# The markdown surfaces cannot import anything, so the seam there is a presence
# check: a file listed below must contain the register's value for that metric.
# Change the register and forget a surface, and that surface no longer contains
# the new number — which is exactly the drift, and it fails here.
#
# The map is deliberately explicit rather than a scan. A scan would have to read
# English to tell "the panel idles at 49 MB" from "cPanel uses 800 MB", and a
# regex that tries and quietly matches nothing is the failure mode this suite
# keeps finding elsewhere.

declare -A SURFACES=(
  ["Panel services RAM (agent + API)"]="README.md COMPARISON.md docs/getting-started.md"
  ["Full-stack RAM (with bundled PostgreSQL)"]="COMPARISON.md docs/getting-started.md"
  ["Panel binaries, all three"]="README.md COMPARISON.md"
  ["App templates"]="README.md"
  ["HTTP routes"]="README.md"
  ["Regression-pin assertions"]="README.md docs/testing.md"
  ["Supervised background services"]="README.md"
)

surface_checks=0
for metric in "${!SURFACES[@]}"; do
  want="${REG[$metric]:-}"
  if [ -z "$want" ]; then
    bad "the register has no '$metric' row, but surfaces are mapped to it — the map and the register disagree"
    continue
  fi
  for f in ${SURFACES[$metric]}; do
    if [ ! -f "$f" ]; then
      bad "$f is mapped as publishing '$metric' but does not exist"
      continue
    fi
    surface_checks=$((surface_checks+1))
    if grep -qE "(^|[^0-9.])${want}([^0-9.]|\$)" "$f"; then
      ok "$f carries the register's $metric ($want)"
    else
      bad "$f publishes '$metric' but does not contain the register's value $want — it is quoting an older measurement"
    fi
  done
done

if [ "$surface_checks" -eq 0 ]; then
  bad "no surface was checked against the register — the map is empty and this arm proves nothing"
else
  ok "$surface_checks surface/metric pairs checked against the register"
fi

# --- 6d. The superseded figures must not come back ---
#
# 6c is a presence check, and mutation testing showed exactly where that is not
# enough: README states the panel's memory footprint in THREE places, so reverting
# one of them to the old ~19MB left the other two carrying the current value and
# the file still "contained" it. The arm passed while the masthead — the most-read
# line in the project — was wrong again.
#
# So the corrected figures get the same treatment §4 gives a disproved claim: an
# assertion that the old one cannot reappear. These are matched as full published
# strings rather than bare digits, because 11 and 41 occur innocently everywhere
# and a pin that fires on a coincidence gets disabled, which is worse than no pin.

declare -A SUPERSEDED=(
  ["~19MB"]="the panel's memory footprint, published for four months against a real reading of ~49MB"
  ["~19 MB"]="the panel's memory footprint, published for four months against a real reading of ~49MB"
  ["~85MB"]="full-stack memory; the real figure with PostgreSQL is ~109MB"
  ["~85 MB"]="full-stack memory; the real figure with PostgreSQL is ~109MB"
  ["~41MB"]="binaries on disk; the published release totals ~45MB"
  ["~41 MB"]="binaries on disk; the published release totals ~45MB"
  ["776 API endpoints"]="a count whose own decomposition matched source on neither side; it is 809 routes"
  ["454 E2E tests"]="a total nothing derived; the derived figure is 302 regression assertions"
  ["11 background services"]="there are 15 supervised background services"
)

STALE_SURFACES=(README.md COMPARISON.md FEATURES.md docs/getting-started.md
                docs/testing.md website/client/src/measurements.ts
                website/client/src/pages/Landing.tsx)

stale_hits=0
stale_checked=0
for phrase in "${!SUPERSEDED[@]}"; do
  for f in "${STALE_SURFACES[@]}"; do
    [ -f "$f" ] || continue
    stale_checked=$((stale_checked+1))
    # FEATURES.md explains the correction in prose, so it is allowed to name the
    # old figure inside a line that also says what replaced it.
    if grep -nF -- "$phrase" "$f" 2>/dev/null | grep -qv 'against a real reading\|had been published at'; then
      bad "$f has gone back to publishing '$phrase' — $(printf '%s' "${SUPERSEDED[$phrase]}")"
      stale_hits=$((stale_hits+1))
    fi
  done
done

if [ "$stale_checked" -eq 0 ]; then
  bad "no surface was scanned for superseded figures — this arm proves nothing"
elif [ "$stale_hits" -eq 0 ]; then
  ok "none of the ${#SUPERSEDED[@]} superseded figures has reappeared on any of the ${#STALE_SURFACES[@]} surfaces"
fi

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
