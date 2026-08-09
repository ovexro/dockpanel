#!/usr/bin/env bash
# Regression pins for s336 — the guards that decide WHAT WE INSTALL and WHAT WE
# PUSH, and the two ways they were quietly not guarding.
#
# THE SHAPE. Both defects this suite pins are the same shape, and it is a shape
# worth naming because it hides so well:
#
#   S1  A GUARD APPLIED TO ONE OF SEVERAL. deploy-demo.sh fetched three binaries
#       and ran the static-linkage check on one of them — and the one it skipped
#       was the AGENT, the exact binary that put a stale glibc build on the demo
#       at s271, which the comment above the guard cites as the reason the guard
#       exists. Three lines of a loop above it, three installs below it, one
#       filename in the middle.
#
#   S2  A CHECK WHOSE OWN FAILURE IS THE REASSURING ANSWER. That same guard read
#       ANY readelf failure as proof of static linkage. The mechanism is worth
#       carrying: the check ended in a grep, and under `pipefail` a pipeline
#       keeps the RIGHTMOST non-zero status, so readelf's 127 was overwritten by
#       grep's no-match 1. Measured at f80de33: a genuinely dynamic binary with
#       a broken or absent readelf printed "confirmed: no DT_NEEDED (static)",
#       as did a non-ELF file and a zero-byte download.
#
# The same two shapes were live in the credential gate. scripts/hooks/pre-commit
# narrowed the staged list to eleven source extensions and exited 0 with a GREEN
# "No scannable files staged" when the list came back empty — with the
# .env/.pem/.key filename check, which never needed that list, sitting BELOW the
# exit. scripts/hooks/pre-push had the identical inversion plus two more: its
# new-branch arm ran `git diff <sha>`, which compares that commit with the
# WORKING TREE and is empty on a clean tree, and it discarded git's stderr so an
# unresolvable range looked exactly like a clean push. Reproduced before the fix
# with the control that makes it undeniable: a commit of one .pem holding a
# github token plus an nginx .conf holding the real origin address COMMITTED AND
# PUSHED GREEN, while the same .pem with any .rs file riding along was blocked.
#
# Sections 4 and 5 EXECUTE the guards rather than reading them, because a
# structural arm cannot tell a guard that works from a guard that merely looks
# right — and §4's subject is a function whose whole defect was in its exit
# statuses. §4 stubs readelf, so it is hermetic: no real binaries, no network,
# and it runs the same on any box and in any container. §5 builds throwaway git
# repositories under mktemp and never touches this one.
#
# Polarity: every arm below was run against v2.93.0 and is RED there.

set -uo pipefail
cd "$(dirname "$0")/.."
REPO=$PWD

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

DEMO=scripts/deploy-demo.sh
IAGENT=scripts/install-agent.sh
PRECOMMIT=scripts/hooks/pre-commit
PREPUSH=scripts/hooks/pre-push

for f in "$DEMO" "$IAGENT" "$PRECOMMIT" "$PREPUSH" scripts/setup.sh scripts/update.sh scripts/agent-self-update.sh; do
  [ -f "$f" ] || { echo "missing file: $f"; exit 1; }
done

echo "── 1. every path that installs a published binary verifies it first ──"
# Four scripts consume a published release; install-agent.sh is the fifth path
# that puts our binary on a machine. Three of the five always verified. The
# other two are why this section exists.
for f in scripts/setup.sh scripts/update.sh scripts/agent-self-update.sh "$DEMO" "$IAGENT"; do
  if grep -q 'checksums\.txt' "$f" && grep -q 'sha256sum' "$f"; then
    ok "$f verifies its download against the release checksums.txt"
  else
    bad "$f installs a downloaded binary without comparing it to the release checksums.txt"
  fi
done

echo "── 2. no downloaded asset is written straight onto a live executable path ──"
# setup.sh states the contract in its own comment: the live executable path must
# never hold unverified bytes. A quarantine name (a leading dot) is the shape
# that satisfies it. Writing to a live path also loses to ETXTBSY on a box where
# the binary is already running, which is how install-agent.sh came to report a
# busy file as a network failure.
LIVE_WRITES=0
for f in scripts/setup.sh scripts/update.sh scripts/agent-self-update.sh "$DEMO" "$IAGENT"; do
  # -o targets directly under /usr/local/bin whose basename is not a dotfile
  HITS=$(grep -nE -- '-o[[:space:]]+"?/usr/local/bin/[^.]' "$f" || true)
  if [ -n "$HITS" ]; then
    bad "$f downloads onto a live executable path: $(printf '%s' "$HITS" | head -1 | cut -c1-70)"
    LIVE_WRITES=$((LIVE_WRITES+1))
  fi
done
[ "$LIVE_WRITES" -eq 0 ] && ok "no install path curls onto a live /usr/local/bin target"

echo "── 3. deploy-demo.sh checks every asset it installs, not a named one ──"
ASSETS_DECL=$(grep -n '^ASSETS=(' "$DEMO" || true)
if [ -n "$ASSETS_DECL" ]; then
  ok "deploy-demo.sh names its assets once, in an ASSETS array"
  N=$(sed -n 's/^ASSETS=(\(.*\))/\1/p' "$DEMO" | tr ' ' '\n' | grep -c 'dockpanel-')
  if [ "$N" -eq 3 ]; then
    ok "the ASSETS array holds all three published binaries"
  else
    bad "the ASSETS array holds $N binaries, expected 3 — an asset was added or dropped without the checks following"
  fi
else
  bad "deploy-demo.sh has no ASSETS array — each check names its own subject and can fall behind the download loop"
fi

# The load-bearing arm: no ACTION line may spell an asset name. The declaration
# and the destination map may; readelf, file, install and curl may not.
STRAY=$(grep -nE '(readelf|^file |[^a-z]file |install -m|curl)' "$DEMO" \
        | grep -E 'dockpanel-(agent|api|cli)-linux-' || true)
if [ -z "$STRAY" ]; then
  ok "no readelf/file/install/curl line names a single asset — they all iterate the list"
else
  bad "an action line still names one asset literally: $(printf '%s' "$STRAY" | head -1 | cut -c1-80)"
fi

for what in 'download' 'checksum' 'linkage' 'swap'; do
  case "$what" in
    download) NEED='curl -fsSL -o "$TMP/$a"' ;;
    checksum) NEED='sha256sum "$TMP/$a"' ;;
    linkage)  NEED='require_static_musl "$TMP/$a"' ;;
    swap)     NEED='install -m 0755 "$TMP/$a"' ;;
  esac
  if grep -qF "$NEED" "$DEMO"; then
    ok "the $what step iterates the shared asset list"
  else
    bad "the $what step does not iterate the shared asset list — it can fall behind the others"
  fi
done

echo "── 4. the linkage guard, EXECUTED against a stubbed readelf ──"
# Extract the function from the script and run it. A guard that cannot be
# extracted is a FAIL, not an error: at v2.93.0 there is no function, only an
# inline branch, and that is exactly the state this arm exists to report.
GUARD_SRC=$(sed -n '/^require_static_musl() {/,/^}/p' "$DEMO")
if [ -z "$GUARD_SRC" ]; then
  bad "deploy-demo.sh has no require_static_musl function to exercise — the linkage check is inline and unverifiable"
  bad "  (skipping the five behavioural cases: dynamic, static, readelf-error, readelf-absent, zero-byte)"
else
  ok "the linkage guard is a function, so it can be exercised rather than read"
  G_TMP=$(mktemp -d)
  printf '%s\n' "$GUARD_SRC" > "$G_TMP/guard.sh"
  printf 'pretend-elf\n' > "$G_TMP/subject"
  : > "$G_TMP/empty"
  mkdir -p "$G_TMP/stub"

  drive() { # drive <stub-body> <file> ; echoes PASS or REFUSE
    printf '#!/bin/sh\n%s\n' "$1" > "$G_TMP/stub/readelf"
    chmod +x "$G_TMP/stub/readelf"
    if PATH="$G_TMP/stub:$PATH" bash -c ". '$G_TMP/guard.sh'; require_static_musl '$2'" >/dev/null 2>&1; then
      echo PASS
    else
      echo REFUSE
    fi
  }
  # The guard exits rather than returning, so each case runs in its own bash.
  case_check() { # case_check <label> <expect> <stub> <file>
    local got; got=$(drive "$3" "$4")
    if [ "$got" = "$2" ]; then ok "$1 → $got"; else bad "$1 → got $got, expected $2"; fi
  }
  case_check "a dynamically linked binary is refused" REFUSE \
    'echo " 0x0001 (NEEDED) Shared library: [libc.so.6]"; exit 0' "$G_TMP/subject"
  case_check "a static musl binary is accepted" PASS \
    'echo "There is no dynamic section in this file."; exit 0' "$G_TMP/subject"
  case_check "readelf failing on a non-ELF file is refused, not believed" REFUSE \
    'echo "readelf: Error: Failed to read file header" >&2; exit 1' "$G_TMP/subject"
  case_check "readelf being absent is refused, not believed" REFUSE \
    'echo "readelf: command not found" >&2; exit 127' "$G_TMP/subject"
  case_check "a zero-byte download is refused before readelf is consulted" REFUSE \
    'echo "There is no dynamic section in this file."; exit 0' "$G_TMP/empty"
  rm -rf "$G_TMP"
fi

echo "── 5. the credential gate, EXECUTED on throwaway repositories ──"
# The patterns live in the hooks in a self-non-matching form, so the hooks can
# scan themselves. Read one back out and un-bracket it to build the fixtures —
# never spell an infrastructure address in this file.
BLOCKED_IP=$(grep -oE '95\[\.\][0-9]+\[\.\][0-9]+\[\.\][0-9]+' "$PRECOMMIT" | head -1 | tr -d '[]')
BLOCKED_TOKEN="$(grep -oE 'github_pat\[_\]' "$PRECOMMIT" | head -1 | tr -d '[]')11EXAMPLE"
if [ -z "$BLOCKED_IP" ]; then
  bad "could not read a blocked address out of $PRECOMMIT in bracket form — the self-non-matching idiom is gone, so the hooks can no longer scan themselves"
else
  ok "the hooks' patterns are in the self-non-matching bracket form"

  H_TMP=$(mktemp -d)
  git init -q "$H_TMP/r" 2>/dev/null
  (
    cd "$H_TMP/r" || exit 1
    git config user.email pin@test; git config user.name pin
    mkdir -p .h && cp "$REPO/$PRECOMMIT" "$REPO/$PREPUSH" .h/
    echo seed > README.md; git add -A; git commit -qm seed --no-verify
  ) >/dev/null 2>&1

  hook_says_blocked() { # hook_says_blocked <precommit|prepush>
    local out
    if [ "$1" = precommit ]; then
      out=$(cd "$H_TMP/r" && bash .h/pre-commit 2>&1)
    else
      out=$(cd "$H_TMP/r" && printf 'refs/heads/main %s refs/heads/main %s\n' \
              "$(git -C "$H_TMP/r" rev-parse HEAD)" \
              "$(git -C "$H_TMP/r" rev-parse HEAD~1 2>/dev/null || echo 0000000000000000000000000000000000000000)" \
            | bash .h/pre-push origin file:///dev/null 2>&1)
    fi
    printf '%s' "$out" | grep -cE 'BLOCKED: (Sensitive file|Real server IP|Real infrastructure IP|Password or token)' >/dev/null && echo BLOCK || echo PASS
  }
  stage() { ( cd "$H_TMP/r" && rm -f leak.* && printf '%s\n' "$2" > "$1" && git add -A ) >/dev/null 2>&1; }

  stage leak.pem "$BLOCKED_TOKEN"
  [ "$(hook_says_blocked precommit)" = BLOCK ] \
    && ok "pre-commit blocks a .pem that arrives with no source file beside it" \
    || bad "pre-commit lets a lone .pem through — the filename check is below the early exit again"

  stage leak.conf "proxy_pass http://$BLOCKED_IP;"
  [ "$(hook_says_blocked precommit)" = BLOCK ] \
    && ok "pre-commit blocks an nginx .conf carrying a real origin address" \
    || bad "pre-commit does not scan .conf files — the extension allowlist is back"

  # MUST-NOT-FIRE. Without this the section above is satisfied by a hook that
  # blocks everything, which is not a working gate, it is a broken one.
  ( cd "$H_TMP/r" && rm -f leak.* && cp "$REPO/$PRECOMMIT" ./pc.copy && cp "$REPO/$PREPUSH" ./pp.copy && git add -A ) >/dev/null 2>&1
  [ "$(hook_says_blocked precommit)" = PASS ] \
    && ok "pre-commit does NOT block the hooks' own text (they can still be edited)" \
    || bad "pre-commit blocks its own source — the patterns match themselves and no hook change can be committed"

  ( cd "$H_TMP/r" && rm -f pc.copy pp.copy && printf 'fn main(){}\n' > ok.rs && git add -A ) >/dev/null 2>&1
  [ "$(hook_says_blocked precommit)" = PASS ] \
    && ok "pre-commit does NOT block an ordinary clean source file" \
    || bad "pre-commit blocks a clean .rs — the gate is crying wolf and will be switched off"

  # A commit that REMOVES a secret must never be blocked — otherwise the one
  # change this hook refuses to allow is the change that cleans the repository.
  # MODIFY rather than delete: --diff-filter=ACMR already drops whole-file
  # deletions, so deleting the file would prove nothing about the line scan.
  ( cd "$H_TMP/r" && rm -f leak.* ok.rs \
      && printf 'keep me\nx=%s\n' "$BLOCKED_TOKEN" > carrier.txt \
      && git add -A && git commit -qm carrier --no-verify \
      && printf 'keep me\n' > carrier.txt && git add -A ) >/dev/null 2>&1
  [ "$(hook_says_blocked precommit)" = PASS ] \
    && ok "pre-commit does NOT block a commit that REMOVES a line holding a secret" \
    || bad "pre-commit blocks the removal of a secret — the scan is reading deleted lines"

  # The hostname arm is deliberately NON-BLOCKING — these names appear in docs
  # and examples legitimately. But the scan used to close with an unconditional
  # "Clean — no infrastructure leaks detected.", so a run that had just printed a
  # hostname hit still signed off as clean, and the green line is the one an
  # operator reads. Derive the hostname from the hook's own bracket form; never
  # spell an internal address in this file.
  BLOCKED_HOST=$(grep -oE '[a-z]+\[\.\]dockpanel\[\.\]dev' "$PRECOMMIT" | head -1 | tr -d '[]')
  hook_summary() { ( cd "$H_TMP/r" && bash .h/pre-commit 2>&1 ) | sed 's/\x1b\[[0-9;]*m//g'; }

  if [ -z "$BLOCKED_HOST" ]; then
    bad "could not read a hostname pattern out of $PRECOMMIT in bracket form"
  else
    stage leak.conf "server_name $BLOCKED_HOST;"
    S=$(hook_summary)
    if printf '%s' "$S" | grep -cE 'WARNING: Internal hostname' >/dev/null \
       && ! printf '%s' "$S" | grep -cE 'Clean — no infrastructure leaks' >/dev/null; then
      ok "pre-commit warns on an internal hostname WITHOUT then calling the scan clean"
    else
      bad "pre-commit's summary contradicts its own warning — it printed a hostname hit and still signed off as clean"
    fi
  fi

  # MUST-NOT-FIRE for the arm above: a genuinely clean scan must still say so, or
  # the check is satisfied by a hook that never claims cleanliness at all.
  ( cd "$H_TMP/r" && rm -f leak.* carrier.txt && printf 'fn main(){}\n' > fine.rs && git add -A ) >/dev/null 2>&1
  if printf '%s' "$(hook_summary)" | grep -cE 'Clean — no infrastructure leaks' >/dev/null; then
    ok "pre-commit still reports a genuinely clean scan as clean"
  else
    bad "pre-commit no longer says anything positive on a clean scan — the verdict has been lost, not corrected"
  fi

  ( cd "$H_TMP/r" && rm -f ok.rs fine.rs && printf '%s\n' "$BLOCKED_TOKEN" > leak.pem \
      && git add -A && git commit -qm "lone pem" --no-verify ) >/dev/null 2>&1
  [ "$(hook_says_blocked prepush)" = BLOCK ] \
    && ok "pre-push blocks a commit whose only file is a .pem" \
    || bad "pre-push lets a lone .pem through — the pathspec disarms the filename check again"

  rm -rf "$H_TMP"
fi

echo "── 6. the new-branch arm asks for commits, not for a working-tree diff ──"
# `git diff <sha>` compares that commit with the working tree, so on a clean
# tree the new-branch arm scanned nothing while its comment said it scanned
# everything. This is the one arm that has to read source: the behaviour it
# pins is an ABSENCE, and the absence is of a construct rather than of an output.
if grep -q 'git log -p' "$PREPUSH"; then
  ok "the new-branch arm asks git for the commits being pushed"
else
  bad "the new-branch arm does not use git log -p — a bare git diff <sha> is empty on a clean tree and scans nothing"
fi
if grep -qE 'RC=\$\?' "$PREPUSH" && grep -q 'refusing to push commits that were never scanned' "$PREPUSH"; then
  ok "a range git cannot resolve fails closed instead of reading as a clean push"
else
  bad "the pushed range's exit status is still discarded — an unresolvable range is indistinguishable from a clean push"
fi

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
