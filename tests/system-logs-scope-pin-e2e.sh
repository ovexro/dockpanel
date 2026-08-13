#!/usr/bin/env bash
# Regression pins for s334 — the page an operator opens to find out what went
# wrong must not be drowned by a metric, must not answer a typo with the whole
# table, and must not render a database failure as silence.
#
#   S1  A METRIC WAS BEING FILED AS A LOG AND IT WAS 99.98% OF THE TABLE.
#       `backup_policy_executor` inserts one row per 60-second tick recording
#       total backup bytes, and `backup_orchestrator` reads them back as a 30-day
#       series — so the rows have a consumer and must keep being written. But at
#       the listing's 50-row window they buried everything: measured live on this
#       panel, 43,272 metric rows against 10 genuine ones, one more every minute,
#       forever, with a 30-day retention holding the steady state there. Reaching
#       a day back by hand cost 29 pages of storage samples.
#
#   S2  ...AND THE DROPDOWN BUILT TO FILTER BY SOURCE DID NOT OFFER IT. The
#       frontend hardcoded seven sources; ten are written. The three it omitted
#       included the one that is almost the whole table, and there is no exclude
#       operator — so the operator could neither select the noise nor suppress it.
#
#   S3  AN UNRECOGNISED FILTER RETURNED THE UNFILTERED LIST. A level or range the
#       API did not recognise pushed no condition and bound nothing, so it neither
#       errored nor returned empty: it returned everything, which reads as "your
#       filter matched a great many rows". A typo silently widened the answer.
#
#   S4  A DATABASE FAILURE LOOKED LIKE AN EMPTY LOG. Both endpoints ended in
#       `.unwrap_or_default()`, so "we could not read the logs" and "there are no
#       logs" rendered as the same screen — the silence-as-safety class the canary
#       reader was corrected for one release earlier.
#
#   S5  A WARNING LEVEL NOTHING COULD COUNT. Two writers spelled the level short
#       while every reader spells it long, so those rows were absent from the
#       Warnings tile, unreachable through the level filter, and rendered with the
#       neutral badge used for unknown levels. One of the two files already
#       spelled it the long way at its other two call sites.
#
# POLARITY, WRITTEN DOWN BEFORE THE RUN (#335): S1, S3, S4, S5, S6, S7, S8, S9 are
# RED at v2.91.0 and green here. S2, S10 and S11 are CONTROLS and are green at BOTH
# tags on purpose.
#
# ⚠ S1 IS A NARROWING — it makes the endpoint return LESS than before, and a
# narrowing that overshoots fails in the reassuring direction, because a shorter
# list looks exactly like a healthy one. S2 is its paired control: the metric
# source must STILL be reachable when explicitly named. Both were mutation-tested
# by planting the defect in a subject that is still a member.
#
# Pure source analysis: no box, no network, no build.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }
# Boolean grep that CONSUMES ALL INPUT.
#
# `producer | grep -q PAT` is a race under `set -o pipefail`: grep -q exits at
# the first match, the producer dies of SIGPIPE, and the pipeline reports 141 —
# so a SUCCESSFUL match is read as a failure. Measured here at ~2 in 400 runs on
# a 2,250-line subject, which is why it surfaced as an unreproducible red rather
# than a broken arm. `grep -c` reads to EOF, and its exit status is still 0 on a
# match, so this is the same boolean with no early exit.
qgrep() { grep -c "$@" >/dev/null; }
BE=panel/backend/src
FE=panel/frontend/src

for d in "$BE" "$FE" panel/backend/migrations; do
  [ -d "$d" ] || { echo "missing dir: $d — refusing to report a clean sweep"; exit 1; }
done

# Strip comments before asking a question about CODE. Load-bearing here: this
# session's fix explains the short/long level spelling IN A COMMENT beside the
# writer, so an arm grepping the raw source would match the explanation and
# report the defect it was written to catch (#the source-pin prose trap).
code() {
  [ -f "$1" ] || return 0
  perl -0777 -pe '
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
    s{^\s*///.*$}{}gm;
  ' "$1"
}

SL="$BE/routes/system_logs.rs"
UI="$FE/pages/SystemLogs.tsx"
[ -f "$SL" ] || { echo "missing $SL — refusing to report a clean sweep"; exit 1; }
[ -f "$UI" ] || { echo "missing $UI — refusing to report a clean sweep"; exit 1; }

echo ""
echo "── system logs scope pins ──"

# ── S1. The default listing must exclude the metric source. ──────────────────
# Keyed on the exclusion being APPLIED — pushed into the condition list that
# builds the WHERE clause — not on the inequality merely EXISTING in the file.
#
# ⚠ This arm's first draft grepped for the inequality anywhere in the source. It
# survived a mutation that deleted the only call site, because the helper that
# BUILDS the string still existed and still contained it. An arm that proves a
# guard is DEFINED tells you nothing about whether anything invokes it; the
# mutation harness is what exposed that, not review.
if code "$SL" | qgrep -E 'conditions\.push\(hide_metric\(\)\)'; then
  ok "S1 the default listing applies the metric exclusion, so genuine entries are not buried"
else
  bad "S1 nothing applies the metric exclusion — one row a minute still buries the log"
fi

# ── S2. CONTROL, green at both tags. The metric source must remain REACHABLE. ─
# The paired control for S1's narrowing: whatever else changes, a source the
# caller names explicitly must still be filtered on, or the rows S1 hides become
# unreachable — the same defect pointed the other way.
#
# ⚠ This arm originally tried to extract the whole `match source` block with a
# non-greedy window and judge both halves inside it. The window closed on the FIRST
# brace at any indent, which is the one ending the `Some(_)` arm, so the block it
# judged never contained the `None` arm and it reported red on correct code. A
# window bounded on "the next closing brace" is not a block (#172). Each half is
# now its own single-line assertion, bounded by nothing.
if code "$SL" | qgrep -E 'source = \$'; then
  ok "S2 CONTROL an explicitly named source is still filtered on, so the metric rows stay reachable"
else
  bad "S2 CONTROL no source filter exists at all — the dropdown cannot reach any source"
fi

# ── S2b. The exclusion must be CONFINED to the unnamed-source branch. ────────
# Not a control: red at the previous tag, where no exclusion existed at all.
if code "$SL" | qgrep -E 'None *=> *conditions\.push\(hide_metric\(\)\)'; then
  ok "S2b the exclusion applies only when no source was named, so naming the metric still returns it"
else
  bad "S2b the exclusion is not confined to the unnamed-source branch — it may hide the rows unconditionally"
fi

# ── S3. An unrecognised level must be refused, not ignored. ──────────────────
if code "$SL" | qgrep -E 'Unknown level'; then
  ok "S3 an unrecognised level is refused rather than silently returning the unfiltered list"
else
  bad "S3 an unrecognised level still falls through to no condition, returning everything"
fi

# ── S4. An unrecognised range must be refused, not ignored. ──────────────────
if code "$SL" | qgrep -E 'Unknown range'; then
  ok "S4 an unrecognised range is refused rather than silently dropping the time filter"
else
  bad "S4 an unrecognised range still drops the time filter, returning every row ever written"
fi

# ── S5/S6. Neither endpoint may render a database failure as an empty result. ─
if [ "$(code "$SL" | grep -c 'unwrap_or_default')" -eq 0 ]; then
  ok "S5 a failed log read is an error, not an empty list that reads as 'nothing went wrong'"
else
  bad "S5 an endpoint still ends in unwrap_or_default — a database failure renders as silence"
fi

if [ "$(code "$SL" | grep -cE 'map_err\(\|e\| internal_error')" -ge 2 ]; then
  ok "S6 both the list and the count endpoints propagate a database failure"
else
  bad "S6 fewer than two endpoints propagate — one of them still swallows its error"
fi

# ── S7. The tiles must count the same population the default listing shows. ──
COUNT_FN=$(code "$SL" | perl -0777 -ne 'print $1 if /(pub async fn count.*)/s')
if [ -z "$COUNT_FN" ]; then
  bad "S7 could not extract the count endpoint — the arm has no subject, not a passing result"
elif printf '%s' "$COUNT_FN" | qgrep -E 'source <> |\{hide\}'; then
  ok "S7 the count endpoint applies the same exclusion, so the Info tile is not one metric a minute"
else
  bad "S7 the count endpoint counts the metric rows the listing hides — the tiles disagree with the list"
fi

# ── S8. No writer may emit a level the readers cannot count. ─────────────────
# Comments stripped first — this session's fix names the short spelling in prose
# beside both writers, and an arm reading raw source would match that prose.
SHORT=0
while IFS= read -r f; do
  n=$(code "$f" | perl -0777 -ne 'my $c = () = /log_event\s*\(\s*[^,]+,\s*"warn"/gs; print $c')
  SHORT=$((SHORT + ${n:-0}))
done < <(grep -rl 'log_event' "$BE" --include=*.rs)
if [ "$SHORT" -eq 0 ]; then
  ok "S8 every writer spells the warning level the way the readers filter and count it"
else
  bad "S8 $SHORT writer(s) still emit a warning level no reader can count or filter"
fi

# ── S9. The dropdown must offer every source the backend writes. ─────────────
# DERIVED from the writers, not from a list typed here — a hardcoded expectation
# is the same defect this arm exists to catch, one level up. The enumeration is
# asserted before it is used: an empty subject list would let an absence arm pass
# having examined nothing (#143).
mapfile -t WRITTEN < <(
  grep -rl --include=*.rs -e 'log_event' -e 'INSERT INTO system_logs' "$BE" |
  while IFS= read -r f; do
    code "$f" | perl -0777 -ne '
      while (/log_event\s*\(\s*[^,]+,\s*"[a-z_]+"\s*,\s*"([a-z_]+)"/gs) { print "$1\n" }
      while (/INSERT INTO system_logs.*?VALUES\s*\(\s*\x27[a-z]+\x27\s*,\s*\x27([a-z_]+)\x27/gs) { print "$1\n" }
    '
  done | sort -u
)
if [ "${#WRITTEN[@]}" -lt 8 ]; then
  bad "S9 only ${#WRITTEN[@]} written sources extracted — the enumeration is implausible, refusing to judge"
else
  OFFERED=$(perl -0777 -ne 'print $1 if /const SOURCES = \[(.*?)\]/s' "$UI")
  MISSING=""
  for s in "${WRITTEN[@]}"; do
    printf '%s' "$OFFERED" | qgrep "\"$s\"" || MISSING="$MISSING $s"
  done
  if [ -z "$MISSING" ]; then
    ok "S9 the source dropdown offers all ${#WRITTEN[@]} sources the backend writes"
  else
    bad "S9 the source dropdown omits written source(s):$MISSING"
  fi

  # ── S12. The excluded source must be one that is actually WRITTEN. ─────────
  # Without this, the exclusion constant could drift to a name nothing emits: S1
  # would still find its inequality and pass, while the listing excluded nothing
  # and the metric kept burying the log. The arm that checks a filter EXISTS
  # cannot tell you the filter is pointed at anything.
  METRIC=$(code "$SL" | perl -0777 -ne 'print $1 if /METRIC_SOURCE: &str = "([a-z_]+)"/')
  if [ -z "$METRIC" ]; then
    bad "S12 could not read the excluded source constant — the arm has no subject"
  elif printf '%s\n' "${WRITTEN[@]}" | qgrep -x "$METRIC"; then
    ok "S12 the excluded source ($METRIC) is one the backend actually writes"
  else
    bad "S12 the excluded source ($METRIC) is written by nobody — the exclusion hides nothing"
  fi
fi

# ── S10. CONTROL, green at both tags. The endpoints stay admin-only. ─────────
# ⚠ THIS ARM WAS BLIND UNTIL s354, and blind in the reassuring direction. It
# counted the gate's name over the WHOLE FILE against a threshold of two — but
# `code()` strips comments and NOT the import line, so a correct tree counts
# THREE. Deleting either handler's gate left two, and an arm labelled CONTROL
# stayed green while the endpoint that returns every tenant's rows answered
# anyone holding a token. It could only fire when BOTH gates went at once, which
# is the one case nobody was worried about — it missed the exact single-gate
# deletion it names itself as controlling for.
#
# Two rules it now obeys. Assert each handler SEPARATELY, because a file-wide
# count is satisfied by whichever occurrence you did not break (#408). And
# assert the ENUMERATION before trusting it, because an arm that discovers its
# own subjects reports a confident green over an empty list (#143).
SL_CODE=$(code "$SL")

# Body from a handler's signature to its own column-0 closing brace. A fixed
# `grep -A n` window is not a declaration and bleeds into the next function,
# which is how an arm confidently answers a question about code it never read.
fnbody() {
  awk -v want="$1" '
    $0 ~ ("^pub async fn " want "\\(") { inside = 1 }
    inside                            { print }
    inside && /^\}/                   { exit }
  '
}

HANDLERS=$(printf '%s\n' "$SL_CODE" | perl -ne 'print "$1\n" if /^pub async fn (\w+)\(/')
HANDLER_N=$(printf '%s\n' "$HANDLERS" | grep -c '[a-z]')
if [ "$HANDLER_N" -lt 2 ]; then
  bad "S10 enumerated $HANDLER_N handlers in system_logs.rs, expected at least 2 — the arm has no subject"
else
  ok "S10 enumerated $HANDLER_N request handlers in system_logs.rs"
  for fn in $HANDLERS; do
    BODY=$(printf '%s\n' "$SL_CODE" | fnbody "$fn")
    if [ "$(printf '%s\n' "$BODY" | grep -c '[^[:space:]]')" -lt 3 ]; then
      bad "S10 could not extract the body of $fn() — the arm has no subject"
    elif printf '%s\n' "$BODY" | qgrep -E 'require_admin\(&claims\.role\)'; then
      ok "S10 CONTROL $fn() is still admin-only"
    else
      bad "S10 CONTROL $fn() lost its admin gate — that endpoint answers any authenticated caller"
    fi
  done
fi

# ── S11. CONTROL, green at both tags. The metric must KEEP being written. ────
# The consumer is a 30-day storage series; silencing the writer to clean up the
# log would break a chart, which is why the fix hides the rows instead.
if qgrep -E "INSERT INTO system_logs" "$BE/services/backup_policy_executor.rs" \
   && qgrep -E 'FROM system_logs' "$BE/routes/backup_orchestrator.rs"; then
  ok "S11 CONTROL the storage metric is still written and still read as a series"
else
  bad "S11 CONTROL the storage metric writer or its reader is gone — the chart lost its data"
fi

echo ""
echo "── system logs scope pins: PASS $PASS / FAIL $FAIL ──"
[ "$FAIL" -eq 0 ] || exit 1
