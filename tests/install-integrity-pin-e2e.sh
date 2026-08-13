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
# THE THIRD SHAPE, and the reason this file was rewritten at s338:
#
#   S3  A PIN SATISFIED BY PROSE. Every structural arm below greps a script, and
#       a grep over raw source cannot tell a live line from a disabled one. Four
#       of these arms were measured surviving a mutation that removed the guard
#       and left its own name behind in the comment explaining the removal — one
#       of them survived a TRAILING comment on an otherwise defective line, the
#       weakest prose there is. The fix is the stripper set copied verbatim from
#       tests/sandbox-paths-pin-e2e.sh and placed ABOVE every call site, plus a
#       named predicate per pin so section 7 can re-run the REAL predicate
#       against a deliberately broken copy on every single run.
#
#       The stripper for shell is FULL-LINE ONLY, deliberately, and one pin pays
#       for that: a trailing comment on a live line still reaches the grep. That
#       pin is therefore written as a conjunction — the construct we want must
#       be present in code AND the construct we replaced it with must be absent
#       from code — so the disabling edit trips it on the absence side, where no
#       comment can help.
#
#       Every subject here is executable shell. There is no markdown or JSON
#       subject, so no arm is deliberately left raw for prose-reading; the two
#       places that stay raw are section 5's hook copies, which are EXECUTED and
#       must therefore be the real program, comments and all.
#
# Sections 4 and 5 EXECUTE the guards rather than reading them, because a
# structural arm cannot tell a guard that works from a guard that merely looks
# right — and §4's subject is a function whose whole defect was in its exit
# statuses. §4 stubs readelf, so it is hermetic: no real binaries, no network,
# and it runs the same on any box and in any container. §5 builds throwaway git
# repositories under mktemp and never touches this one. §7 only ever mutates
# private copies under mktemp.
#
# Polarity: every arm in §1-§6 was run against v2.93.0 and is RED there.

set -uo pipefail
cd "$(dirname "$0")/.."
REPO=$PWD

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

# ── the comment strippers, ABOVE every arm that needs them (s337) ────────────
#
# A check placed below the thing it must run before inherits that position as a
# defect even when the check itself is perfect — the s336 lesson, and the reason
# these functions are the first thing in the file rather than the middle.
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

# BLANK the comment, do not delete it. Arms below compare line numbers obtained
# from this stream against each other and against the raw file; a stripper that
# removes lines renumbers the file and would silently change what they measure.
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
    # convention in this tree is that explanation goes on its own line, and §7
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

DEMO=scripts/deploy-demo.sh
IAGENT=scripts/install-agent.sh
PRECOMMIT=scripts/hooks/pre-commit
PREPUSH=scripts/hooks/pre-push
SUBJECTS="scripts/setup.sh scripts/update.sh scripts/agent-self-update.sh $DEMO $IAGENT $PRECOMMIT $PREPUSH"

for f in $SUBJECTS; do
  [ -f "$f" ] || { echo "missing file: $f"; exit 1; }
done

# ── the pinned predicates, named once and called twice ───────────────────────
#
# Each structural pin below is a named function over a FILE PATH, so §7 can run
# the identical predicate against a deliberately broken copy. A §7 that restated
# the predicate in its own words would drift from the pin it claims to test, and
# the first thing to drift would be the stripping.

# Diagnostics only. An offending line can carry a multibyte dash, and a byte-wise
# truncation splits it into an invalid sequence that crashes whatever reads this
# output. Reduce to ASCII first, then cut.
snippet() { printf '%s' "$1" | head -1 | tr -cd '\11\40-\176' | cut -c1-70; }

verifies_its_download() { has_code "$1" 'checksums\.txt' && has_code "$1" 'sha256sum'; }

live_bin_writes() { # -o targets directly under the live bin dir whose basename is not a dotfile
  code_lines "$1" | grep -nE -- '-o[[:space:]]+"?/usr/local/bin/[^.]' || true
}

assets_declaration() { code_lines "$1" | grep -n '^ASSETS=(' || true; }
assets_count()       { code_lines "$1" | sed -n 's/^ASSETS=(\(.*\))/\1/p' | tr ' ' '\n' | grep -c 'dockpanel-'; }
first_asset()        { code_lines "$1" | sed -n 's/^ASSETS=(\([^ )]*\).*/\1/p' | head -1; }

stray_single_asset_lines() { # an ACTION line that spells one asset instead of iterating
  code_lines "$1" | grep -nE '(readelf|^file |[^a-z]file |install -m|curl)' \
                  | grep -E 'dockpanel-(agent|api|cli)-linux-' || true
}

step_iterates() { [ "$(code_count "$1" -F "$2")" -gt 0 ]; }
guard_source()  { code_lines "$1" | sed -n '/^require_static_musl() {/,/^}/p'; }

# The new-branch arm's patch source, both halves. `git diff <bare sha>` compares
# that commit with the WORKING TREE, so a clean tree makes it empty; the absence
# half is what a trailing comment on a live line cannot rescue.
worktree_diff_on_bare_sha()  { code_lines "$1" | grep -nE 'git diff[^;)]*\$LOCAL_SHA' || true; }
newbranch_asks_for_commits() { has_code "$1" 'git log -p' && [ -z "$(worktree_diff_on_bare_sha "$1")" ]; }

unresolvable_range_fails_closed() {
  has_code "$1" -E 'RC=\$\?' && has_code "$1" 'refusing to push commits that were never scanned'
}

echo "── 1. every path that installs a published binary verifies it first ──"
# Four scripts consume a published release; install-agent.sh is the fifth path
# that puts our binary on a machine. Three of the five always verified. The
# other two are why this section exists.
for f in scripts/setup.sh scripts/update.sh scripts/agent-self-update.sh "$DEMO" "$IAGENT"; do
  if verifies_its_download "$f"; then
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
#
# NEGATIVE ARM: a match is the defect. Stripping makes it see less, which is the
# silent direction, so §7 plants the real write as CODE and requires it to fire.
LIVE_WRITES=0
for f in scripts/setup.sh scripts/update.sh scripts/agent-self-update.sh "$DEMO" "$IAGENT"; do
  HITS=$(live_bin_writes "$f")
  if [ -n "$HITS" ]; then
    bad "$f downloads onto a live executable path: $(snippet "$HITS")"
    LIVE_WRITES=$((LIVE_WRITES+1))
  fi
done
[ "$LIVE_WRITES" -eq 0 ] && ok "no install path curls onto a live /usr/local/bin target"

echo "── 3. deploy-demo.sh checks every asset it installs, not a named one ──"
ASSETS_DECL=$(assets_declaration "$DEMO")
if [ -n "$ASSETS_DECL" ]; then
  ok "deploy-demo.sh names its assets once, in an ASSETS array"
  N=$(assets_count "$DEMO")
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
# NEGATIVE ARM — §7 plants the real single-asset action line and requires a fire.
STRAY=$(stray_single_asset_lines "$DEMO")
if [ -z "$STRAY" ]; then
  ok "no readelf/file/install/curl line names a single asset — they all iterate the list"
else
  bad "an action line still names one asset literally: $(snippet "$STRAY")"
fi

for what in 'download' 'checksum' 'linkage' 'swap'; do
  case "$what" in
    # Repinned s354: the download moved behind dp_fetch when the retry loop
    # reached this script. The property is unchanged — the step must still
    # iterate the shared list — so the arm follows the code rather than freezing
    # the old call text. S9 below is its pair: it asserts the helper this now
    # names actually exists here and that nothing fetches around it.
    download) NEED='dp_fetch "$BASE/$a" "$TMP/$a"' ;;
    checksum) NEED='sha256sum "$TMP/$a"' ;;
    linkage)  NEED='require_static_musl "$TMP/$a"' ;;
    swap)     NEED='install -m 0755 "$TMP/$a"' ;;
  esac
  if step_iterates "$DEMO" "$NEED"; then
    ok "the $what step iterates the shared asset list"
  else
    bad "the $what step does not iterate the shared asset list — it can fall behind the others"
  fi
done

echo "── 4. the linkage guard, EXECUTED against a stubbed readelf ──"
# Extract the function from the script and run it. A guard that cannot be
# extracted is a FAIL, not an error: at v2.93.0 there is no function, only an
# inline branch, and that is exactly the state this arm exists to report.
# Stripping runs BEFORE the slice, so the range still opens and closes on the
# same lines it always did — code_lines blanks, it does not renumber.
GUARD_SRC=$(guard_source "$DEMO")
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
# scan themselves. Read one back out of the CODE and un-bracket it to build the
# fixtures — never spell an infrastructure address in this file. The hook COPIES
# below are deliberately RAW: they are executed, so they must be the real
# program, not a stripped rendering of it.
BLOCKED_IP=$(code_lines "$PRECOMMIT" | grep -oE '95\[\.\][0-9]+\[\.\][0-9]+\[\.\][0-9]+' | head -1 | tr -d '[]')
BLOCKED_TOKEN="$(code_lines "$PRECOMMIT" | grep -oE 'github_pat\[_\]' | head -1 | tr -d '[]')11EXAMPLE"
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
  BLOCKED_HOST=$(code_lines "$PRECOMMIT" | grep -oE '[a-z]+\[\.\]dockpanel\[\.\]dev' | head -1 | tr -d '[]')
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
# This is the one pin that has to read source: the behaviour it pins is an
# ABSENCE, and the absence is of a construct rather than of an output. It is a
# CONJUNCTION for a measured reason — the disabling edit that beat it left the
# wanted symbol behind in a TRAILING comment, which the shell stripper does not
# remove, so the arm has to also refuse the construct that replaced it.
if newbranch_asks_for_commits "$PREPUSH"; then
  ok "the new-branch arm asks git for the commits being pushed"
else
  bad "the new-branch arm does not ask git for the commits being pushed — either the log-of-patches call is gone from CODE, or a diff against a bare local sha is back, and that compares the commit with the working tree and is empty on a clean one"
fi
if unresolvable_range_fails_closed "$PREPUSH"; then
  ok "a range git cannot resolve fails closed instead of reading as a clean push"
else
  bad "the pushed range's exit status is still discarded — an unresolvable range is indistinguishable from a clean push"
fi

echo "── 7. the pins are mutation-tested against broken copies, on every run ──"
# A stripped pin is a pin that sees LESS, and both ways of getting it wrong are
# invisible from a green run: a stripper that misses prose leaves the original
# defect in place, and a stripper that eats code turns a NEGATIVE arm silently
# green on broken code. Neither shows up by reading. So every structural pin
# above is re-run here against a copy that has been deliberately broken, and the
# copy always keeps the subject's BASENAME — the stripper picks its language
# from the filename, and a copy called something else is read as the wrong one.
M_TMP=$(mktemp -d)

scratch() { # scratch <repo-relative subject> → private copy, SAME BASENAME
  local d; d=$(mktemp -d "$M_TMP/plant.XXXXXX")
  cp "$REPO/$1" "$d/$(basename "$1")" && printf '%s' "$d/$(basename "$1")"
}
comment_out_matching() { # <file> <ere> — prefix every matching line with the file's own marker
  local f="$1" re="$2" m; m=$(comment_marker_for "$f")
  awk -v re="$re" -v m="$m" '{ if ($0 ~ re) print m " " $0; else print }' "$f" > "$f.mut" && mv "$f.mut" "$f"
}
append_code()  { printf '%s\n' "$2" >> "$1"; }
append_prose() { local m; m=$(comment_marker_for "$1"); printf '%s %s\n' "$m" "$2" >> "$1"; }
mutation_landed() { # <repo original> <mutated copy> — height held, content moved
  [ "$(wc -l < "$1")" -eq "$(wc -l < "$2")" ] && ! cmp -s "$1" "$2"
}

mutation_arm() { # <label> <repo-relative subject> <ere> <predicate>
  local label="$1" subject="$2" re="$3" pred="$4" copy
  copy=$(scratch "$subject")
  if ! "$pred" "$copy"; then
    bad "$label — the pin was already unsatisfied on an untouched copy, so the mutation proves nothing"
  elif ! { comment_out_matching "$copy" "$re"; mutation_landed "$REPO/$subject" "$copy"; }; then
    bad "$label — the mutation did not land: nothing matched, or the file changed height"
  elif "$pred" "$copy"; then
    bad "$label — the pin is still satisfied with that code commented out: prose is holding it up"
  else
    ok "$label"
  fi
}

p_verifies()      { verifies_its_download "$1"; }
p_assets_decl()   { [ -n "$(assets_declaration "$1")" ]; }
p_linkage_step()  { step_iterates "$1" 'require_static_musl "$TMP/$a"'; }
p_guard_extract() { [ -n "$(guard_source "$1")" ]; }
p_newbranch()     { newbranch_asks_for_commits "$1"; }
p_failclosed()    { unresolvable_range_fails_closed "$1"; }

mutation_arm "mutation: §1 notices install-agent.sh's hash compare being commented out" \
  "$IAGENT" 'sha256sum' p_verifies
mutation_arm "mutation: §3 notices deploy-demo.sh's per-asset linkage call being commented out" \
  "$DEMO" 'require_static_musl "' p_linkage_step
mutation_arm "mutation: §3 notices the shared asset declaration being commented out" \
  "$DEMO" '^ASSETS=' p_assets_decl
mutation_arm "mutation: §4 notices the linkage guard's definition being commented out" \
  "$DEMO" '^require_static_musl' p_guard_extract
mutation_arm "mutation: §6 notices the new-branch patch call being commented out" \
  "$PREPUSH" 'git log -p' p_newbranch
mutation_arm "mutation: §6 notices the fail-closed branch being commented out" \
  "$PREPUSH" 'refusing to push commits that were never scanned' p_failclosed

# The mechanism itself, stated once: after that same mutation the wanted symbol
# is STILL THERE in the raw bytes and gone only from the code stream. This is the
# whole s338 defect in one assertion — every arm above used to read the raw bytes.
MECH=$(scratch "$IAGENT")
comment_out_matching "$MECH" 'sha256sum'
if [ "$(grep -c 'sha256sum' "$MECH")" -gt 0 ] && [ "$(code_count "$MECH" 'sha256sum')" -eq 0 ]; then
  ok "the disabling edit leaves the searched symbol in the file's raw bytes and removes it only from the code stream"
else
  bad "the raw/code split does not hold on a commented-out subject — the stripper is either doing nothing or deleting the line"
fi

# ── negative controls. Three arms above are match-⇒-bad, and for those the
# dangerous direction is over-stripping: the check goes quiet and the run stays
# green over live broken code. Each one gets the REAL defect written as CODE and
# must FIRE, plus the same text written as prose, which must NOT.
BIN_DIR=/usr/local/bin
DEFECT_WRITE="curl -fsSL -o $BIN_DIR/dockpanel-api \"\$URL\""
NEG=$(scratch scripts/setup.sh); append_code "$NEG" "$DEFECT_WRITE"
[ -n "$(live_bin_writes "$NEG")" ] \
  && ok "negative control: a real download onto the live executable path IS reported" \
  || bad "negative control: a real download onto the live executable path was NOT reported — §2 has gone blind and every install path reads clean"
NEG=$(scratch scripts/setup.sh); append_prose "$NEG" "$DEFECT_WRITE"
[ -z "$(live_bin_writes "$NEG")" ] \
  && ok "negative control: the same line written as prose is NOT reported" \
  || bad "negative control: §2 fires on a comment — a false alarm that trains the next reader to ignore it"

A1=$(first_asset "$DEMO")
if [ -z "$A1" ]; then
  bad "negative control: could not read an asset name out of the shared declaration, so the single-asset controls cannot be built"
  bad "negative control: (skipping the prose half for the same reason)"
else
  DEFECT_STRAY="file \"\$TMP/$A1\" | cut -c1-100"
  NEG=$(scratch "$DEMO"); append_code "$NEG" "$DEFECT_STRAY"
  [ -n "$(stray_single_asset_lines "$NEG")" ] \
    && ok "negative control: a real action line naming one asset IS reported" \
    || bad "negative control: a real action line naming one asset was NOT reported — §3 has gone blind and the s271 shape can come back"
  NEG=$(scratch "$DEMO"); append_prose "$NEG" "$DEFECT_STRAY"
  [ -z "$(stray_single_asset_lines "$NEG")" ] \
    && ok "negative control: the same action line written as prose is NOT reported" \
    || bad "negative control: §3 fires on a comment — a false alarm that trains the next reader to ignore it"
fi

DEFECT_DIFF='PATCH=$(git diff --no-color "$LOCAL_SHA" 2>/dev/null); RC=$?'
NEG=$(scratch "$PREPUSH"); append_code "$NEG" "$DEFECT_DIFF"
if [ -n "$(worktree_diff_on_bare_sha "$NEG")" ] && ! p_newbranch "$NEG"; then
  ok "negative control: a real diff against a bare local sha IS reported"
else
  bad "negative control: a real diff against a bare local sha was NOT reported — §6 has gone blind and a new branch is pushed unscanned"
fi
NEG=$(scratch "$PREPUSH"); append_prose "$NEG" "$DEFECT_DIFF"
if [ -z "$(worktree_diff_on_bare_sha "$NEG")" ] && p_newbranch "$NEG"; then
  ok "negative control: the same diff written as prose is NOT reported"
else
  bad "negative control: §6 fires on a comment — a false alarm that trains the next reader to ignore it"
fi

# ── the stripper's own contract, because everything above rests on it ──
HEIGHT_BAD=""
for f in $SUBJECTS; do
  SNAP="$M_TMP/snap"
  code_lines "$f" > "$SNAP"
  if [ "$(wc -l < "$SNAP")" -ne "$(wc -l < "$f")" ]; then HEIGHT_BAD="$f (height)"; break; fi
  # every stripped line is either blank or byte-identical to the raw line at the
  # same index: the stripper may empty a line, never move or rewrite one.
  awk 'NR==FNR { s[FNR]=$0; next } { if (s[FNR] != "" && s[FNR] != $0) exit 1 }' "$SNAP" "$f" \
    || { HEIGHT_BAD="$f (content)"; break; }
done
[ -z "$HEIGHT_BAD" ] \
  && ok "the stripper blanks and never deletes: every subject keeps its height and every surviving line keeps its index" \
  || bad "the stripper renumbers or rewrites $HEIGHT_BAD — every line number and every slice boundary above is now measuring something else"

FIX="$M_TMP/fixture.sh"
printf '%s\n' 'X=$(sed "s/#//" f)' '   # a whole-line comment' 'Y=${VAR#pre}' > "$FIX"
FIXOUT=$(code_lines "$FIX")
if [ "$(printf '%s\n' "$FIXOUT" | sed -n 1p)" = 'X=$(sed "s/#//" f)' ] \
   && [ -z "$(printf '%s\n' "$FIXOUT" | sed -n 2p)" ] \
   && [ "$(printf '%s\n' "$FIXOUT" | sed -n 3p)" = 'Y=${VAR#pre}' ]; then
  ok "the shell stripper blanks a whole-line comment and leaves a hash inside live code alone"
else
  bad "the shell stripper is truncating live code at a hash — parameter expansions and quoted literals are being eaten, which is the direction that turns a negative arm silently green"
fi

MARKER_BAD=""
for f in $SUBJECTS; do
  C=$(scratch "$f")
  [ "$(comment_marker_for "$C")" = "$(comment_marker_for "$f")" ] || { MARKER_BAD="$f"; break; }
done
[ -z "$MARKER_BAD" ] \
  && ok "a scratch copy is read in the same language as its subject — the basename, and so the extension, is preserved" \
  || bad "a scratch copy of $MARKER_BAD is read in a different language than the subject, so every mutation above was applied with the wrong comment marker"

rm -rf "$M_TMP"

# ── S9. THE SAME OPERATION IN EVERY PROGRAM THAT PERFORMS IT ────────────────
# This suite's founding shape (S1) is a guard applied to one of several. s354
# found the same shape in the fix for it: s353 gave the INSTALLER a shell retry
# loop for the one failure `curl --retry` does not cover — a connection dropped
# mid-transfer, exit 56, which had aborted three real installs against the
# release CDN — and gave it to the installer only. Three other programs download
# the same assets from the same host and were all still bare, and one bare call
# was left eleven lines below the new function inside setup.sh itself.
#
# Two arms, because either alone is satisfiable the wrong way: the function must
# EXIST in each consumer, and no bare curl may fetch a release asset behind its
# back. dp_fetch's own curl names `"$url"`, never the release base, so a line
# holding both `curl` and a release base variable is by construction a download
# that bypassed it.
RELEASE_CONSUMERS="scripts/setup.sh scripts/update.sh scripts/agent-self-update.sh scripts/deploy-demo.sh"
for consumer in $RELEASE_CONSUMERS; do
  if [ ! -f "$consumer" ]; then
    bad "S9 $consumer is missing — refusing to report a clean sweep"
    continue
  fi
  if grep -qE '^dp_fetch\(\) \{' "$consumer"; then
    ok "S9 $consumer fetches release assets through a retrying dp_fetch"
  else
    bad "S9 $consumer has no dp_fetch — its release downloads cannot survive a mid-transfer drop (exit 56)"
  fi
  BARE=$(code_lines "$consumer" | grep -E 'curl' | grep -cE '\$\{?BASE(_URL)?\}?/' || true)
  if [ "$BARE" -eq 0 ]; then
    ok "S9 $consumer has no bare curl fetching a release asset"
  else
    bad "S9 $consumer has $BARE bare curl download(s) of a release asset — they bypass dp_fetch's retry"
  fi
done

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
