#!/usr/bin/env bash
# Regression pins for s340 — THE SHIP PATH ITSELF.
#
# v2.95.1 published 33 assets carrying a reverted SFTP transport: every SFTP
# upload and Test Connection broken, on every destination, by a test mutation
# left in the tree. Three checks passed on it, all in the reassuring direction
# (lesson #383). Two of the mechanisms behind that are structural, not personal,
# and this suite pins them shut.
#
#   S1  THE PINS DID NOT GATE THE SHIP. `ci.yml` triggers only on
#       `push: branches: [main]` and `pull_request`, so IT DOES NOT RUN ON A TAG.
#       `release.yml` triggers on the tag and its job graph was
#       changelog -> build -> frontend -> release -> smoke-test, with no
#       dependency on CI at all. This repo has no rulesets and no required status
#       checks (`gh api repos/:owner/:repo/rulesets` returned `[]`), so nothing
#       anywhere made a red pin stop a release. v2.95.1's Release job started
#       FIVE SECONDS after its CI run, on a different trigger, racing it with no
#       authority to lose. The pins were the instrument; nothing made them a gate.
#       Fixed by extracting the sweep to a reusable workflow that BOTH callers
#       invoke, and making `build` need it.
#
#   S2  THE TEST THAT WOULD HAVE CAUGHT IT COULD NOT FAIL. The end-to-end lab
#       test drives the real `test_sftp`/`upload_sftp` against a chrooted sshd,
#       and would have caught the reverted transport instantly. It returned early
#       when `DOCKPANEL_SFTP_LAB` was unset — so a run that executed NOTHING
#       printed the same `ok` as a run that proved the transport, and the summary
#       counted it as a pass. A test that cannot run must not be able to report
#       success. Now `#[ignore]`d (a counted, visible state in the summary's
#       `N ignored` column) and `expect`ing the variable, so running it without a
#       lab PANICS instead of passing vacuously.
#
# POLARITY, WRITTEN DOWN BEFORE THE RUN (#335 — a false green hides inside a
# batch of reds): every arm here is RED at v2.95.2 and green at HEAD. There are
# no controls that pass at both tags; this suite pins changes made this session,
# so `git stash` is its mutation test.
#
# ⚠ THIS SUITE PRACTISES WHAT IT PINS (#383). Its Rust subject is rustfmt-managed,
# so every arm over it UN-WRAPS the body before matching and uses a BOUNDED gap
# (` *`), never a bare `.*`. Flattening alone is NOT enough: rustfmt writes
# `run_sftp(\n    "ssh",`, which flattens to `run_sftp( "ssh"` WITH A SPACE, so a
# pattern spelled `run_sftp\("` still misses it. Both halves or neither.
#
# Pure source analysis: no box, no network, no build.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

# Boolean grep that CONSUMES ALL INPUT. `producer | grep -q PAT` is a race under
# `set -o pipefail`: grep -q exits at the first match, the producer dies of
# SIGPIPE, and a SUCCESSFUL match is reported as failure.
hasE() { grep -cE -- "$2" <<< "$1" >/dev/null; }

CI=.github/workflows/ci.yml
REL=.github/workflows/release.yml
PINS=.github/workflows/regression-pins.yml
RB=panel/agent/src/services/remote_backup.rs

for f in "$CI" "$REL" "$RB"; do
  [ -f "$f" ] || { echo "missing subject $f"; exit 1; }
done

# One top-level job block from a workflow file, bounded by the next job's own
# two-space key — not a fixed -A window, which is not a declaration (#172).
yamljob() { awk -v j="$2" '
  $0 ~ "^  " j ":[[:space:]]*$" { f=1; print; next }
  f && /^  [A-Za-z_][A-Za-z0-9_-]*:/ { exit }
  f { print }' "$1"; }

# Un-wrap: rejoin exactly the lines a formatter split, with no separator, so a
# wrapped call reads as the single-line spelling it came from. A line ending in
# `;` never joins, so independent statements stay apart.
unwrap() { awk '
  { L[++n] = $0 }
  END { i = 1
    while (i <= n) { cur = L[i]
      while (i < n) {
        t = cur; sub(/[ \t]+$/, "", t)
        nx = L[i+1]; sub(/^[ \t]+/, "", nx)
        j = 0
        if (t ~ /[,([{]$/) j = 1
        if (t ~ /(&&|\|\||->|=>|\+|=|\.)$/) j = 1
        if (nx ~ /^([.?)\]}])/) j = 1
        if (t ~ /;$/) j = 0
        if (j == 0) break
        cur = t nx; i++
      }
      print cur; i++ } }' <<< "$1"; }

# The dependency list of a job, as one flat string, in EITHER YAML spelling:
#   needs: [changelog, regression-pins]
#   needs:
#     - changelog
# A pin that understood only the inline form would go falsely RED the day someone
# reformatted the block — and a false red on a RELEASE-GATING arm blocks shipping,
# so this reads the structure rather than one spelling of it. Comments are dropped
# first: an arm must not be satisfied by the prose that narrates it (#149).
needs_of() {
  sed 's/#.*//' <<< "$1" | awk '
    /^[[:space:]]*needs:/ {
      rest = $0; sub(/^[[:space:]]*needs:[[:space:]]*/, "", rest)
      if (rest != "") { print rest; next }
      inlist = 1; next
    }
    inlist && /^[[:space:]]*-[[:space:]]*/ { sub(/^[[:space:]]*-[[:space:]]*/, "", $0); print; next }
    inlist { inlist = 0 }
  '
}

# A Rust fn body plus the attribute lines directly above it, bounded on the fn's
# OWN closing brace (s323: a window ending at the successor's position swallows
# the next declaration).
fn_with_attrs() {
  awk -v want="$2" '
    /^[[:space:]]*#\[/ { if (!collecting) { abuf = abuf $0 "\n" }; next }
    {
      if (!f && index($0, want)) {
        printf "%s", abuf; print; f = 1; collecting = 1
        depth = gsub(/\{/, "{") - gsub(/\}/, "}")
        if (depth <= 0 && /\{/) { exit }
        next
      }
      if (f) {
        print
        depth += gsub(/\{/, "{") - gsub(/\}/, "}")
        if (depth <= 0) exit
        next
      }
      abuf = ""
    }' <<< "$1"
}

echo
echo "§A a red pin must be able to stop a release (S1)"

# A1. The sweep exists in ONE place, callable.
if [ -f "$PINS" ] && hasE "$(cat "$PINS")" '^[[:space:]]*workflow_call:'; then
  ok "A1 the pin sweep is a reusable workflow both pipelines can call"
else
  bad "A1 $PINS is missing or is not workflow_call — there is nothing for release to depend on"
fi

# A2. Release declares the job...
REL_PINS=$(yamljob "$REL" regression-pins)
if hasE "$REL_PINS" 'uses:[[:space:]]*\./\.github/workflows/regression-pins\.yml'; then
  ok "A2 release.yml calls the shared pin sweep"
else
  bad "A2 release.yml does not call regression-pins.yml — a tag never runs the pins (ci.yml does not fire on tags)"
fi

# A3. ...and BUILD depends on it. This is the arm that makes it a gate rather
# than a job that fails alongside a release that publishes anyway.
REL_BUILD=$(yamljob "$REL" build)
if hasE "$(needs_of "$REL_BUILD")" '(^|[^a-z-])regression-pins([^a-z-]|$)'; then
  ok "A3 release build needs the pin sweep, so a red pin skips build → release → publish"
else
  bad "A3 release build does NOT need regression-pins — the pins run beside the release instead of gating it"
fi

# A4. ...and the publish step still hangs off build, or A3 gates nothing.
REL_REL=$(yamljob "$REL" release)
if hasE "$(needs_of "$REL_REL")" '(^|[^a-z-])build([^a-z-]|$)'; then
  ok "A4 the publishing job still descends from build, so A3 reaches it"
else
  bad "A4 the release job no longer needs build — A3 is now pinning a path that does not reach publication"
fi

# A5. Both callers use the SAME definition. This repo has already been bitten by
# two copies of one list disagreeing: the pre-push hook carried its own npm-audit
# allowlist and waived an advisory CI failed on for six straight releases.
CI_PINS=$(yamljob "$CI" regression-pins)
if hasE "$CI_PINS" 'uses:[[:space:]]*\./\.github/workflows/regression-pins\.yml'; then
  ok "A5 ci.yml calls the same shared definition — one list, both callers"
else
  bad "A5 ci.yml no longer calls the shared sweep — the two pipelines can now disagree about what 'the pins' means"
fi

# A6. And the sweep is not ALSO inlined somewhere, which would let one copy drift
# green while the other rots.
INLINE=$(grep -lE 'for s in tests/\*-pin-e2e\.sh' .github/workflows/*.yml 2>/dev/null | grep -v 'regression-pins\.yml' || true)
if [ -z "$INLINE" ]; then
  ok "A6 the suite-sweeping loop is defined once, in the reusable workflow"
else
  bad "A6 the sweep is also inlined in: $INLINE — two copies of the enumeration can disagree"
fi

echo
echo "§B the lab test cannot report success without running (S2)"

LAB=$(fn_with_attrs "$(cat "$RB")" 'fn lab_upload_and_test_survive_forcecommand')
if [ -z "$LAB" ]; then
  bad "B0 the lab test could not be extracted — every §B arm below would pass vacuously, which is this suite's own subject"
else
  ok "B0 the lab test was extracted ($(wc -l <<< "$LAB") lines)"

  LABU=$(unwrap "$LAB")

  # B1. Ignored by default => the summary reports it in the `N ignored` column
  # instead of silently counting a no-op as a pass.
  if hasE "$LABU" '#\[ignore'; then
    ok "B1 the lab test is #[ignore]d, so a run that skips it says so in the summary"
  else
    bad "B1 the lab test is not #[ignore]d — an unconfigured run is indistinguishable from a passing one"
  fi

  # B2. The early return is the actual defect. Named as the ABSENCE it is.
  # The `,? *` is not decoration. rustfmt adds a TRAILING COMMA when it wraps an
  # argument list, so the un-wrapped text reads `std::env::var("DOCKPANEL_SFTP_LAB",)`
  # and a pattern demanding `" *\)` misses it. Writing this suite ABOUT #383 I still
  # shipped #383 into it, and only mutation-testing the arm in the wrapped spelling
  # caught it. Un-wrapping is half the fix; tolerating what the formatter LEAVES
  # BEHIND is the other half.
  if hasE "$LABU" 'let Ok\( *[a-z_]+ *\) *= *std::env::var\( *"DOCKPANEL_SFTP_LAB" *,? *\) *else'; then
    bad "B2 the lab test still returns early when DOCKPANEL_SFTP_LAB is unset — it can pass having executed nothing"
  else
    ok "B2 the lab test does not return early on a missing lab"
  fi

  # B3. And say positively what it DOES do, so a third behaviour that is neither
  # cannot satisfy B2 by being absent (the v2.95.2 lesson: name the transport).
  if hasE "$LABU" 'std::env::var\( *"DOCKPANEL_SFTP_LAB" *\) *\. *expect'; then
    ok "B3 it expects the variable, so running it without a lab panics rather than passing"
  else
    bad "B3 the lab test does not expect() DOCKPANEL_SFTP_LAB — B2 alone would be satisfied by deleting the read entirely"
  fi
fi

echo
echo "── S3 the release cannot CLAIM a signature it did not attach ──"

# v2.99.0 published 19 assets where v2.98.0 published 33. Signing stopped after
# the 4th artefact: cosign's ambient identity detection missed and it fell back
# to INTERACTIVE device flow, printing "Enter the verification code ... in your
# browser" on a headless runner. The step is deliberately continue-on-error so a
# Sigstore outage cannot cost the publish — that stays — but the release BODY
# went on asserting that the .sig/.pem pairs were attached, over 7 that were not.
SRC_REL=$(cat "$REL")

# Keyed on the INVOCATION, not the bare flag name. The first draft of this arm
# grepped the bare name and stayed green after the flag was deleted from the
# cosign call — because the comment explaining the flag still spelled it, three
# lines above. A pin reads raw source, so prose in the subject satisfies it.
if hasE "$SRC_REL" '\-\-identity-token "\$TOK"'; then
  ok "signing supplies an explicit identity token, so it cannot prompt for a browser"
else
  bad "cosign may fall back to interactive device flow and silently drop every later asset"
fi

# A partial run must not leave a half-written pair behind: it would satisfy a
# coverage count while verifying against nothing.
if hasE "$SRC_REL" 'rm -f "\$\{f\}\.sig"'; then
  ok "a failed signature removes its own partial pair"
else
  bad "a half-written .sig can survive and be counted as coverage"
fi

# The claim is reconciled against what was produced, and it happens BEFORE the
# release is created — after it, the body is already published.
if hasE "$SRC_REL" 'Reconcile the signing claim'; then
  ok "the signing claim is reconciled against what was actually signed"
else
  bad "nothing compares the published signing claim to the assets attached"
fi

RECON_LINE=$(grep -n 'Reconcile the signing claim' "$REL" | head -1 | cut -d: -f1)
CREATE_LINE=$(grep -n 'Create GitHub Release' "$REL" | head -1 | cut -d: -f1)
if [ -n "$RECON_LINE" ] && [ -n "$CREATE_LINE" ] && [ "$RECON_LINE" -lt "$CREATE_LINE" ]; then
  ok "it runs BEFORE the release is created, while the body can still be corrected"
else
  bad "the reconcile runs after publish, where correcting the body is too late"
fi

# And the publish must still survive a total outage — the property that made the
# step non-fatal in the first place. Pinned so a future 'fix' cannot trade it away.
if hasE "$SRC_REL" 'continue-on-error: true'; then
  ok "signing stays non-fatal, so an outage ships unsigned rather than losing the publish"
else
  bad "signing became fatal — a Sigstore outage now costs the release entirely"
fi

echo
echo "── S4 every test runs somewhere, or is exempt by name ──"

# S1 was "the pins did not gate the ship". This is its quieter sibling: a test
# that runs NOWHERE gates nothing either — and unlike a red pin, it makes no
# noise while failing to.
#
# tier2 is the proof. Fourteen assertions covering TOFU cert-pin capture, MITM
# rejection, the malformed-fingerprint gate and the v2.7.18 rustls
# CryptoProvider panic guard — excluded from the sweep for a perfectly good
# reason (a live suite wearing a `-pin-e2e.sh` name) and then executed nowhere
# at all, on any machine but its author's, for its entire life. Nothing counted
# it as missing, because the only census in the tree walks the pin glob and it
# had been carved out of that too. An exclusion not paired with a home is a
# deletion nobody wrote down.
#
# So this census is over EVERY test file, and a file earns its place three ways
# only: the shared sweep runs it, a workflow names it, or it is written out
# below with the reason it cannot run in CI.

# The suites that genuinely need a provisioned box: seven take `<host> [port]`
# and drive a remote install, two dial a live panel on 127.0.0.1:3080. None can
# run on a bare runner and none is a regression pin.
EXEMPT="backup-orchestrator-e2e.sh chain-report-e2e.sh deep-e2e.sh e2e.sh
        full-e2e.sh incident-management-e2e.sh secrets-manager-e2e.sh
        security-enhancements-e2e.sh webhook-gateway-e2e.sh"

ALL_TESTS=$(find tests -maxdepth 1 -name '*.sh' -printf '%f\n' | sort)
N_ALL=$(grep -c . <<< "$ALL_TESTS")

# The enumeration first (#143): a glob that matched nothing would let every arm
# below pass having examined zero files, in the reassuring direction.
if [ "$N_ALL" -ge 50 ]; then
  ok "S4 the census enumerated $N_ALL test files"
else
  bad "S4 only $N_ALL test files found — the enumeration is wrong, so the arms below prove nothing"
fi

# The sweep's exclusion list is READ FROM the workflow, never restated here. A
# second copy of that list is exactly what §A exists to prevent, and this arm
# would be the first place it drifted.
SWEEP_EXCL=$(grep -oE '[a-z0-9|.-]+\) continue ;;' "$PINS" | sed 's/) continue ;;//' | tr '|' '\n' | sort -u)
SWEPT=$(comm -23 \
  <(find tests -maxdepth 1 -name '*-pin-e2e.sh' -printf '%f\n' | sort) \
  <(printf '%s\n' "$SWEEP_EXCL"))
NAMED=$(grep -rhoE 'tests/[a-z0-9._-]+\.sh' .github/workflows/ 2>/dev/null | sed 's|tests/||' | sort -u)
RUNS=$(printf '%s\n%s\n' "$SWEPT" "$NAMED" | grep . | sort -u)

COVERED=$(printf '%s\n%s\n' "$RUNS" "$(tr -s ' \n' '\n' <<< "$EXEMPT" | grep .)" | sort -u)
UNCOVERED=$(comm -23 <(printf '%s\n' "$ALL_TESTS") <(printf '%s\n' "$COVERED") | tr '\n' ' ')

if [ -z "${UNCOVERED// /}" ]; then
  ok "S4 every test file runs in CI or is exempt by name"
else
  bad "S4 these tests run nowhere and are not exempt: $UNCOVERED — give each a job, or name it exempt with its reason"
fi

# An exemption for a file that no longer exists is a fudge factor with a name
# on it. The list has to decay when the tree does.
STALE=""
for e in $EXEMPT; do
  [ -f "tests/$e" ] || STALE="$STALE $e"
done
if [ -z "${STALE// /}" ]; then
  ok "S4 every exempt suite still exists on disk"
else
  bad "S4 exempt but absent:$STALE — drop the exemption instead of leaving it to cover some future file"
fi

# And an exemption for something that DOES run is the same rot pointing the
# other way: it would go on silently covering the file the day its job was
# deleted, which is the failure this section is about.
#
# Exact-LINE matching, not substring: `e2e.sh` is a substring of `deep-e2e.sh`,
# `full-e2e.sh` and five more, so a `*"$e"*` glob would report every one of them
# as already-covered and quietly empty this arm.
REDUNDANT=""
for e in $EXEMPT; do
  case $'\n'"$RUNS"$'\n' in
    *$'\n'"$e"$'\n'*) REDUNDANT="$REDUNDANT $e" ;;
  esac
done
if [ -z "${REDUNDANT// /}" ]; then
  ok "S4 no suite is both exempt and already covered — the list cannot mask a deleted job"
else
  bad "S4 exempt but already running:$REDUNDANT — drop the exemption, the job is what covers it"
fi

echo
echo "── ship gate pins: PASS $PASS / FAIL $FAIL ──"
[ "$FAIL" -eq 0 ] || exit 1
