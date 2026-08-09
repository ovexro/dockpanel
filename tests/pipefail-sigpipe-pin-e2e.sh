#!/usr/bin/env bash
# Regression pins for s335 — a successful match must never be read as a failure.
#
#   S1  A PIPELINE INTO A BOOLEAN GREP REPORTS A SUCCESSFUL MATCH AS A FAILURE.
#       Under `set -o pipefail`, a boolean grep exits at its FIRST match. The
#       producer upstream is usually still writing, so it dies of SIGPIPE with
#       status 141, and pipefail propagates that as the status of the whole
#       pipeline. The match succeeded; the shell was told it failed.
#
#       This is NOT merely flaky. It is a race only while the producer's output
#       fits the 64 KiB pipe buffer — below that the producer finishes before the
#       consumer exits and the bug cannot fire. ABOVE it the producer is
#       guaranteed to still be writing, and the false negative becomes
#       DETERMINISTIC. Measured on this tree at s335: 400 of 400 runs reported NO
#       MATCH on a 2.2 MB subject in which every single line matched. On a small
#       subject the same code measured 2 in 400 — which is why it first surfaced
#       as an unreproducible red rather than a broken arm (s333 recorded exactly
#       that and could not explain it).
#
#       The consequence is direction-dependent and the dangerous direction is
#       quiet: a PRESENCE arm turns falsely RED, which announces itself, but an
#       ABSENCE arm — "fail if this bad thing is present" — turns falsely GREEN,
#       and a green arm is what success looks like.
#
#       The fix is to count instead of to quit: a counting grep must read to EOF
#       to produce a count, so the producer is never signalled, and its exit
#       status still answers the same boolean question — zero when nothing
#       matched, zero-exit when something did. Redirect the count away and the
#       call site is unchanged. A here-string works too, by removing the pipeline.
#
#   S2  THE SWEEP THAT FIXED IT COULD NOT STAY FIXED ON ITS OWN. s334 converted
#       two suites and left the rest, and nothing prevented the next author from
#       reintroducing the shape — the rule existed in three comments and in
#       memory, and was enforced nowhere. §A is that enforcement.
#
#       §A deliberately does NOT restrict itself to files that set pipefail, even
#       though only those can fire the bug today. Scoping it that way was the
#       first draft and it was wrong twice over. It exempted a pin suite whose
#       producers are whole 90 KB scripts and which is one `set -o pipefail` away
#       from silently failing six assertions. And it exempted the two git hooks
#       that `core.hooksPath` makes live on every push — the SECRET SCANNERS,
#       whose producer is the entire diff being pushed and whose polarity is the
#       quiet one: a false negative there does not fail a build, it waves a
#       credential through. A rule that depends on a shell option somebody may
#       add later is a rule that reports "clean" right up until it matters.
#
#   S3  THE SAME HAZARD BELONGS TO EVERY EARLY-EXITING CONSUMER, not to one flag.
#       A pipeline into a line-limited head, or into a match-limited grep, dies
#       identically — measured at s335, 200 runs of 200. It is DELIBERATELY NOT
#       PINNED HERE, and the reason is worth writing down rather than rediscovering.
#
#       A draft of this suite did pin it, and indicted seven files. Checked one by
#       one, all seven were wrong to indict: one was a phantom (the `set -e` it
#       matched lives inside a `bash -c '...'` STRING, not in the suite's own
#       shell); one is protected by a trailing `|| true`; and the remaining five
#       have producers that cannot reach the 64 KiB pipe buffer, so the race has
#       nothing to race. The only site with any teeth is a health check whose
#       pipeline would abort with 141 instead of its own `exit 1` — the same
#       verdict, a different number, and loud either way.
#
#       So the class is REAL, REPRODUCIBLE, and currently harmless. Pinning it
#       would mean shipping a detector that reports six false alarms on its first
#       run, and a detector that cries wolf is one somebody switches off. The
#       measurement is recorded; the enforcement is not, on purpose. If a future
#       change makes one of those producers large, this comment is the trail.
#
# POLARITY, MEASURED AT BOTH TAGS: A1 is RED at v2.92.0 — 105 sites across 28
# files — and green here. The first census, scoped to files that set pipefail,
# found 85 across 23; widening the rule as S2 describes added 20 more, two of
# them in files neither the census nor an independent review had named. A0, A0b, A2, A2b, A3, A3b, A4 and A5 are CONTROLS and are
# green at BOTH tags on purpose — each proves the detector beside it is not
# vacuous, by feeding it a subject planted in a temp directory whose verdict is
# known in advance. A2b and A5 exist because a unit test caught the detector
# getting each of them wrong: an earlier draft saw only the plainest spelling of
# the defect, and the draft that fixed that indicted a correct line whose search
# pattern merely contained the flag.
#
# ⚠ THIS SUITE GREPS THE RAW SOURCE OF OTHER SUITES, so its own text is a hazard
# to itself: any line here that spelled the offending shape would be found by its
# own detector. Two defences, both required. First, the census EXCLUDES this file
# by name, and A0 asserts the exclusion removes EXACTLY ONE file, so it cannot
# quietly grow into a blanket. Second, the pattern is assembled at runtime from
# fragments, so the literal shape never appears in this file at all — including
# in this comment.
#
# ⚠ THE DETECTOR DROPS WHOLE-LINE COMMENTS BEFORE MATCHING. Six suites carry
# comments that spell the offending shape deliberately, to warn about it. A
# detector that counted those would be counting OCCURRENCES rather than THINGS,
# and would report offenders that do not exist. It deliberately does NOT truncate
# at a mid-line comment character — see code_lines below for why that direction
# is worse. Known limit: shell embedded in a heredoc or a `bash -c '...'` string
# is read as ordinary code, which is the right default here but is exactly what
# made the dropped §B indict a file that was never at risk.
#
# Pure source analysis: no box, no network, no build.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

SELF="tests/$(basename "$0")"

# Assembled from fragments on purpose — see the header. The literal shape must
# not appear anywhere in this file, or the suite indicts itself.
Q='q'
C='c'
# The option cluster is matched as a whole token, after any number of other
# option tokens. Two earlier drafts of this line were wrong and a unit test
# found both: one anchored the flag to the END of the cluster, so every bundled
# spelling escaped and, with it, every second grep in a chain; the other allowed
# arbitrary text before the flag, so a quoted PATTERN containing the flag was
# indicted. Requiring option-shaped tokens only is what separates the two.
BOOLGREP="\\|[[:space:]]*grep([[:space:]]+-{1,2}[a-zA-Z-]+)*[[:space:]]+(-[a-zA-Z]*${Q}[a-zA-Z]*|--${Q}uiet|--silent)([[:space:]]|\$)"

# Drop WHOLE-LINE comments and blanks; everything else is code for our purposes.
#
# Deliberately NOT "delete from the first # to end of line": a search pattern may
# legitimately contain one — `producer | grep -q '^#!'` is a real line in this
# tree — and truncating there would erase the very shape being looked for. That
# is a false negative, which is the direction that fails quietly. Leaving trailing
# comments in risks the opposite, an indicted comment, which fails loudly and is
# fixed by rewording. A3b pins the choice.
code_lines() { grep -vE '^[[:space:]]*(#|$)' "$1" 2>/dev/null; }

# Reports how many subjects set pipefail — context for the reader, NOT a filter.
pipefail_count() {
  local n=0 f
  while IFS= read -r f; do [ -n "$f" ] || continue
    grep -c 'set[[:space:]][^;]*o[[:space:]]*pipefail' "$f" >/dev/null 2>&1 && n=$((n+1))
  done <<< "$1"
  printf '%s' "$n"
}

echo "── §A no pipeline into a boolean grep, in any shell we ship or run ──"

# The census is derived from the TREE, not from a list somebody remembered to
# update: a new script joins it by existing. An arm that enumerates its own
# subjects must assert the enumeration first, or it can pass having examined
# nothing.
SUBJECTS=""
while IFS= read -r f; do
  [ -n "$f" ] || continue
  [ "$f" = "$SELF" ] && continue
  SUBJECTS="${SUBJECTS}${f}"$'\n'
done < <(find tests scripts .github -type f \( -name '*.sh' -o -name '*.yml' \) 2>/dev/null | sort)

SUBJ_N=$(grep -c . <<< "$SUBJECTS" || true)
ALL_N=$(find tests scripts .github -type f \( -name '*.sh' -o -name '*.yml' \) 2>/dev/null | grep -c . || true)

PF_N=$(pipefail_count "$SUBJECTS")
if [ "${SUBJ_N:-0}" -ge 60 ]; then
  ok "A0 census enumerated $SUBJ_N shell/workflow files ($PF_N of them set pipefail), derived from the tree rather than a hardcoded list"
else
  bad "A0 census found only ${SUBJ_N:-0} files — the enumeration is broken and §A is measuring nothing"
fi

# The self-exclusion must remove exactly one file. A blanket exclusion would let
# this suite pass by not looking.
if [ -f "$SELF" ]; then
  SELF_EXCLUDED=$(find tests scripts .github -type f \( -name '*.sh' -o -name '*.yml' \) 2>/dev/null | grep -cx "$SELF" || true)
  if [ "${SELF_EXCLUDED:-0}" -eq 1 ]; then
    ok "A0b the self-exclusion removes exactly one file ($SELF), not a class"
  else
    bad "A0b the self-exclusion matched ${SELF_EXCLUDED:-0} files — it must remove exactly one"
  fi
else
  bad "A0b cannot locate own source at $SELF — the self-exclusion is unverifiable"
fi

OFFENDERS=""
while IFS= read -r f; do
  [ -n "$f" ] || continue
  hits=$(code_lines "$f" | grep -cE "$BOOLGREP" || true)
  [ "${hits:-0}" -gt 0 ] && OFFENDERS="${OFFENDERS}${f}:${hits}"$'\n'
done <<< "$SUBJECTS"

OFF_N=$(grep -c . <<< "$OFFENDERS" || true)
if [ "${OFF_N:-0}" -eq 0 ]; then
  ok "A1 nothing we ship or run pipes into a boolean grep (a counting grep reads to EOF, so the producer is never signalled)"
else
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    bad "A1 ${line%%:*} pipes into a boolean grep at ${line##*:} site(s) — one set -o pipefail away from reading a successful match as a failure"
  done <<< "$OFFENDERS"
fi

# ── controls: the detector must fire, and must not fire on the two things that
# look like the defect and are not.
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT

{ echo 'set -uo pipefail'; echo 'cat /etc/hostname | grep -'"$Q"' foo'; } > "$TMP/positive.sh"
if [ "$(code_lines "$TMP/positive.sh" | grep -cE "$BOOLGREP" || true)" -gt 0 ]; then
  ok "A2 CONTROL the detector flags a planted pipeline into a boolean grep, so A1 is not vacuous"
else
  bad "A2 CONTROL the detector did NOT flag a planted offender — A1 proves nothing"
fi

# The bundled spelling is its own control, because the first draft of the
# detector missed EVERY bundled flag — and a detector that silently sees only the
# plainest spelling of a defect reports zero offenders in a tree full of them.
{ echo 'set -uo pipefail'; echo 'grep -A6 x f | grep -'"$Q"'xF bar'; } > "$TMP/bundled.sh"
if [ "$(code_lines "$TMP/bundled.sh" | grep -cE "$BOOLGREP" || true)" -gt 0 ]; then
  ok "A2b CONTROL the detector flags a BUNDLED option cluster on a second grep in a chain, not only the plainest spelling"
else
  bad "A2b CONTROL a bundled cluster escaped the detector — A1 sees only the most obvious spelling of the defect"
fi

{ echo 'set -uo pipefail'; echo '# never write: producer | grep -'"$Q"' PATTERN'; } > "$TMP/comment.sh"
if [ "$(code_lines "$TMP/comment.sh" | grep -cE "$BOOLGREP" || true)" -eq 0 ]; then
  ok "A3 CONTROL a comment that spells the shape is not counted as a call site"
else
  bad "A3 CONTROL a comment spelling the shape was counted — the detector counts occurrences, not things"
fi

{ echo 'set -uo pipefail'; printf '%s\n' 'head -1 f | grep -'"$Q"" '^#!'" ; } > "$TMP/hashpat.sh"
if [ "$(code_lines "$TMP/hashpat.sh" | grep -cE "$BOOLGREP" || true)" -gt 0 ]; then
  ok "A3b CONTROL an offender whose SEARCH PATTERN contains a comment character is still flagged, not truncated away"
else
  bad "A3b CONTROL a pattern containing a comment character hid the offender — the stripper is deleting code"
fi

{ echo 'set -uo pipefail'; echo 'grep -'"$Q"' foo /etc/hostname'; } > "$TMP/nopipe.sh"
if [ "$(code_lines "$TMP/nopipe.sh" | grep -cE "$BOOLGREP" || true)" -eq 0 ]; then
  ok "A4 CONTROL a boolean grep reading a FILE is not flagged — with no pipeline there is no producer to signal"
else
  bad "A4 CONTROL a boolean grep with no pipeline was flagged — the detector is indicting safe code"
fi

# The opposite failure of A2b: a draft that allowed arbitrary text between the
# command and the flag indicted a counting grep whose SEARCH PATTERN happened to
# contain the flag. A detector that cries wolf on correct code gets switched off.
{ echo 'set -uo pipefail'; echo 'cat f | grep -'"$C"' "foo -'"$Q"' bar" >/dev/null'; } > "$TMP/quoted.sh"
if [ "$(code_lines "$TMP/quoted.sh" | grep -cE "$BOOLGREP" || true)" -eq 0 ]; then
  ok "A5 CONTROL a counting grep whose PATTERN contains the flag is not indicted — options are matched as tokens, not as text"
else
  bad "A5 CONTROL a correct counting grep was indicted because its search pattern spelled the flag"
fi

echo
if [ "$FAIL" -eq 0 ]; then
  printf '\033[0;32mPASS %d  FAIL %d\033[0m\n' "$PASS" "$FAIL"
else
  printf '\033[0;31mPASS %d  FAIL %d\033[0m\n' "$PASS" "$FAIL"
fi
[ "$FAIL" -eq 0 ]
