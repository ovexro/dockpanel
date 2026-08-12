#!/usr/bin/env bash
# db-schema-browser-pin-e2e.sh — s350
#
# Pins the database Schema Browser: the columns pane, the indexes pane, and the
# panel↔agent contract underneath both.
#
# WHY THIS SUITE EXISTS. `29d959b` ("Security hardening: fix 117 vulnerabilities
# across 45 files", 2026-03-23, v2.0.6) rewrote the column-listing statements to
# use bound placeholders and sent the values to the agent under a "params" key.
# The agent has never had a field to receive them: its request struct carries
# `sql` and nothing else, so serde dropped the key and the placeholder reached
# `mariadb -e` / `psql -c` unbound. Every column listing failed from that commit
# until s350 — and nobody reported it, because the caller discarded the rejection
# in a bare `catch {}` and the pane renders empty either way.
#
# Coverage before this file: `grep -rln 'table_schema\|/indexes/\|schema-overview'
# tests/` returned ZERO across all 60 suites (control: `grep -rln databases
# tests/` returns 10). The fix would have shipped unpinned.
#
# THE SHAPE OF THE REGRESSION THIS GUARDS. Not deletion — REINTRODUCTION of a
# send-side key with no receive-side field, and re-severing the two panes. So:
#
#   §A  THE CONTRACT, cross-tree and the arm worth the most: every key the
#       backend puts in a /databases/query body must be a field the AGENT
#       declares. This is the invariant `29d959b` broke; an arm that merely
#       greps for the literal "params" would pass the day someone invents a
#       differently-named orphan key.
#   §B  the STATEMENTS interpolate and carry no placeholder — a bound `?` is
#       unservable on this endpoint, so its presence is the defect itself.
#   §C  ONE validator, and it is the strict one. Two copies with two charsets is
#       what let the interpolating sibling and the placeholder sibling drift.
#   §D  the FRONTEND settles the two calls independently and surfaces failure.
#       A shared all-or-nothing await makes a working pane collateral damage.
#
# Pure source analysis: no box, no network, no build.

set -u
cd "$(dirname "$0")/.."
REPO=$(pwd)

PASS=0
FAIL=0
ok()  { PASS=$((PASS + 1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL + 1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

BACK="$REPO/panel/backend/src/routes/databases.rs"
AGENT="$REPO/panel/agent/src/routes/database.rs"
PAGE="$REPO/panel/frontend/src/pages/Databases.tsx"

for f in "$BACK" "$AGENT" "$PAGE"; do
  [ -f "$f" ] || { echo "FATAL: missing subject $f"; exit 1; }
done

# Strip comments before measuring. Load-bearing here: the s350 fix is commented
# in all three subjects and those comments NAME the exact tokens the arms grep
# for — "params", the placeholders, `Promise.all`, `29d959b`. Grepping raw
# source would match the sentence explaining the check instead of the check
# ([[feedback_source_pin_prose_trap]]). The JSX form `{/* ... */}` is handled
# first and non-greedily, because a .tsx subject uses it inside markup.
code() {
  perl -0777 -pe '
    s{\{\s*/\*.*?\*/\s*\}}{}gs;
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*///.*$}{}gm;
    s{^\s*//.*$}{}gm;
  ' "$1"
}

has()         { grep -qE -- "$2" <<< "$1"; }
occurrences() { grep -oE -- "$2" <<< "$1" | wc -l; }

BACK_S=$(code "$BACK")
AGENT_S=$(code "$AGENT")
PAGE_S=$(code "$PAGE")

echo "db-schema-browser-pin-e2e"
echo

# ─────────────────────────────────────────────────────────────────────────────
echo "§0  the stripper actually stripped (a blinded stripper makes every arm below match its own explanation)"

# Each subject carries a comment naming a token an arm greps for. If the
# stripper is blinded, these three arms go red before any real arm can pass for
# the wrong reason.
if has "$BACK_S" '29d959b'; then
  bad "0.1 databases.rs: comments survived the strip (the commit sha is still present)"
else
  ok  "0.1 databases.rs: comments stripped"
fi

# Keyed on the commit sha, which appears ONLY in the explanatory comment. An
# earlier draft of this arm grepped `Promise.all`, which matched a real and
# unrelated `Promise.all(` elsewhere in the page — a self-check that fails on
# correct code is not a self-check.
if has "$PAGE_S" '29d959b'; then
  bad "0.2 Databases.tsx: comments survived the strip (the commit sha is still present)"
else
  ok  "0.2 Databases.tsx: comments stripped"
fi

# Positive control: stripping must not have eaten the code. A truncated subject
# makes every ABSENCE arm below pass having examined nothing (lesson #136).
B_FNS=$(occurrences "$BACK_S" '^pub async fn ')
P_HOOKS=$(occurrences "$PAGE_S" 'useState')
if [ "$B_FNS" -ge 10 ] && [ "$P_HOOKS" -ge 10 ]; then
  ok  "0.3 subjects survive stripping (databases.rs $B_FNS handlers, Databases.tsx $P_HOOKS useState)"
else
  bad "0.3 stripper ate the subject (databases.rs $B_FNS handlers, Databases.tsx $P_HOOKS useState)"
fi

# ─────────────────────────────────────────────────────────────────────────────
echo
echo "§A  the panel↔agent contract — every key sent is a field the agent declares"

# The agent's request struct for POST /databases/query.
AGENT_STRUCT=$(awk '/struct QueryDbRequest/{f=1} f{print} f&&/^}/{exit}' <<< "$AGENT_S")
AGENT_FIELDS=$(grep -oE '^[ \t]+[a-z_]+:' <<< "$AGENT_STRUCT" | tr -d ' \t:' | sort -u)

if [ -n "$AGENT_FIELDS" ]; then
  ok  "A1 agent QueryDbRequest parsed ($(wc -w <<< "$AGENT_FIELDS") fields: $(tr '\n' ' ' <<< "$AGENT_FIELDS"))"
else
  bad "A1 could not parse agent QueryDbRequest — every arm in §A would be vacuous"
fi

# Every json! body posted to /databases/query in the backend, and every key in it.
SENT_KEYS=$(awk '
  /serde_json::json!\(\{/ { buf=""; depth=1; collecting=1; next }
  collecting {
    buf = buf "\n" $0
    if (/\}\)/) {
      collecting=0
      if (buf ~ /"sql"/) print buf
    }
  }
' <<< "$BACK_S" | grep -oE '"[a-z_]+":' | tr -d '":' | sort -u)

if [ -n "$SENT_KEYS" ]; then
  ok  "A2 backend query bodies parsed ($(wc -w <<< "$SENT_KEYS") distinct keys: $(tr '\n' ' ' <<< "$SENT_KEYS"))"
else
  bad "A2 could not parse any /databases/query body — §A is vacuous"
fi

ORPHANS=""
for k in $SENT_KEYS; do
  grep -qx "$k" <<< "$AGENT_FIELDS" || ORPHANS="$ORPHANS $k"
done
if [ -z "$ORPHANS" ]; then
  ok  "A3 no orphan key — every key the backend sends has a field on the agent"
else
  bad "A3 ORPHAN KEY(S):$ORPHANS — sent by the backend, silently dropped by the agent (this is the 29d959b defect)"
fi

# The specific dead channel, kept as its own named arm so a regression reads
# clearly in the log rather than only as a generic orphan.
if has "$BACK_S" '"params"'; then
  bad "A4 the dead \"params\" key is back in databases.rs — the agent cannot bind it"
else
  ok  "A4 no \"params\" key in databases.rs"
fi

# ─────────────────────────────────────────────────────────────────────────────
echo
echo "§B  the statements interpolate, and carry no unservable placeholder"

SCHEMA_FN=$(awk '/pub async fn table_schema/{f=1} f{print} f&&/^}/{exit}' <<< "$BACK_S")
INDEX_FN=$(awk '/pub async fn table_indexes/{f=1} f{print} f&&/^}/{exit}' <<< "$BACK_S")

if [ -n "$SCHEMA_FN" ] && [ -n "$INDEX_FN" ]; then
  ok  "B1 both handlers extracted (table_schema $(wc -l <<< "$SCHEMA_FN") lines, table_indexes $(wc -l <<< "$INDEX_FN") lines)"
else
  bad "B1 could not extract handlers — §B is vacuous"
fi

if [ "$(occurrences "$SCHEMA_FN" "table_name = '\{table\}'")" -eq 2 ]; then
  ok  "B2 table_schema interpolates the table name in BOTH engine branches"
else
  bad "B2 table_schema does not interpolate in both branches (found $(occurrences "$SCHEMA_FN" "table_name = '\{table\}'"), want 2)"
fi

if has "$SCHEMA_FN" 'table_name = \?|table_name = \$1'; then
  bad "B3 table_schema carries a bound placeholder — /databases/query cannot bind it"
else
  ok  "B3 table_schema carries no bound placeholder"
fi

if has "$SCHEMA_FN" 'format!'; then
  ok  "B4 table_schema builds its statements with format! (same idiom as its working sibling)"
else
  bad "B4 table_schema no longer uses format!"
fi

# ─────────────────────────────────────────────────────────────────────────────
echo
echo "§C  one validator, and it is the strict one"

VALIDATOR=$(awk '/fn is_safe_table_name/{f=1} f{print} f&&/^}/{exit}' <<< "$BACK_S")

if [ -n "$VALIDATOR" ]; then
  ok  "C1 is_safe_table_name exists"
else
  bad "C1 is_safe_table_name is gone — §C is vacuous"
fi

if has "$VALIDATOR" "c == '-'"; then
  bad "C2 the validator admits '-' — it guards an interpolated statement; match the strict charset"
else
  ok  "C2 the validator admits no '-'"
fi

if has "$VALIDATOR" "is_ascii_alphanumeric\(\).*c == '_'"; then
  ok  "C3 the validator admits exactly [A-Za-z0-9_]"
else
  bad "C3 the validator's charset is not [A-Za-z0-9_]"
fi

# Both interpolating handlers must go through it, and no inline copy may remain.
CALLERS=$(occurrences "$BACK_S" '!is_safe_table_name\(&table\)')
if [ "$CALLERS" -eq 2 ]; then
  ok  "C4 both handlers call the shared validator"
else
  bad "C4 expected 2 callers of is_safe_table_name, found $CALLERS"
fi

# Outside the validator's own body — the validator is of course allowed to
# contain the one charset check. Anything else is a second copy.
BACK_MINUS_VALIDATOR=$(awk '
  /fn is_safe_table_name/ { skip=1 }
  skip && /^\}/           { skip=0; next }
  !skip                   { print }
' <<< "$BACK_S")
INLINE=$(occurrences "$BACK_MINUS_VALIDATOR" "table\.chars\(\)")
if [ "$INLINE" -eq 0 ]; then
  ok  "C5 no inline table-name charset check survives outside the shared validator"
else
  bad "C5 $INLINE inline table.chars() check(s) remain outside the validator — the two-copies drift is back"
fi

# ─────────────────────────────────────────────────────────────────────────────
echo
echo "§D  the frontend settles the two calls independently and surfaces failure"

LOADER=$(awk '/const loadTableDetails/{f=1} f{print} f&&/^  \};/{exit}' <<< "$PAGE_S")

if [ -n "$LOADER" ]; then
  ok  "D1 loadTableDetails extracted ($(wc -l <<< "$LOADER") lines)"
else
  bad "D1 could not extract loadTableDetails — §D is vacuous"
fi

if has "$LOADER" 'Promise\.allSettled'; then
  ok  "D2 the two requests are settled independently"
else
  bad "D2 loadTableDetails does not use Promise.allSettled — one pane can blank the other"
fi

if has "$LOADER" 'Promise\.all\('; then
  bad "D3 the all-or-nothing Promise.all is back"
else
  ok  "D3 no all-or-nothing Promise.all in loadTableDetails"
fi

if has "$LOADER" 'catch \{ *\}|catch \{ */\* *ignore'; then
  bad "D4 loadTableDetails swallows its rejection again — five months of a dead pane went unreported this way"
else
  ok  "D4 loadTableDetails does not swallow rejections"
fi

if has "$LOADER" 'setDetailError'; then
  ok  "D5 loadTableDetails records a failure reason"
else
  bad "D5 loadTableDetails records no failure reason"
fi

# The reason must reach the screen, not just state.
if has "$PAGE_S" '\{detailError && \('; then
  ok  "D6 the failure reason is rendered"
else
  bad "D6 detailError is set but never rendered — a reason nobody sees is a swallowed error with extra steps"
fi

# The indexes pane must not be gated behind the columns result.
if has "$PAGE_S" 'selectedTable && \(tableSchema \|\| tableIndexes \|\| detailError\)'; then
  ok  "D7 the detail pane renders when EITHER call succeeds, or when one failed"
else
  bad "D7 the detail pane is gated on tableSchema alone — a failed columns call hides the working indexes pane"
fi

# ─────────────────────────────────────────────────────────────────────────────
echo
printf 'PASS %d FAIL %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
