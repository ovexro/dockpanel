#!/usr/bin/env bash
#
# DockPanel — pins for the two update-subsystem defects closed at s282.
#
# 1. RESTORE (s231): a failure in a post-commit stage must not restart the
#    still-installed NEWER dockpanel-api on top of a database that has just been
#    reverted. It used to: `database-verify` and `record-rollback` both run after
#    the restore transaction commits and before the binaries are swapped back, so
#    a `fail` in either tripped the exit trap, the trap restarted the api
#    unconditionally, and that api migrated the reverted database forward again —
#    quietly undoing the rollback it was reporting as FAILED.
#
# 2. UPDATE (F1): `update.sh` re-execs itself into a transient unit with
#    `exec systemd-run` and no `--wait`, so the child the orchestrator waits on
#    exits 0 within milliseconds while the real update runs for minutes. Nothing
#    may promote that zero into "the update worked", and a run that fails before
#    the api is stopped has to leave a verdict behind, because no restart will
#    ever come to carry one.
#
# Part 1 DRIVES restore-snapshot.sh end to end against a scratch tree with
# stubbed systemctl/docker/curl — the guard decides whether to leave a panel
# down, and a guard that has never been executed is a comment. Part 2 is source
# analysis over CODE ONLY: it strips comments from both subjects and reads the
# Rust one as production code, because a claim about behaviour that a paragraph
# of prose can satisfy is not a claim (s338). Its own arms are mutation-tested
# on every run — commented out for the positive ones, and the real defect
# planted for each negative one, which is the direction that fails quietly.
# Neither part needs a box, a network or a build.
#
# Usage:   bash tests/update-rollback-pin-e2e.sh
#          KEEP=1 bash tests/update-rollback-pin-e2e.sh   # keep the scratch tree
#
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Overridable so the suite can be re-run against an older tree to prove it
# actually asserts something. A green run is indistinguishable from a suite that
# checks nothing; the evidence is the failure count against the code the fix
# replaced (lesson #109c).
RESTORE_SH="${RESTORE_SH:-$REPO_ROOT/scripts/restore-snapshot.sh}"
UPDATE_SH="${UPDATE_SH:-$REPO_ROOT/scripts/update.sh}"
ORCHESTRATOR="${ORCHESTRATOR:-$REPO_ROOT/panel/backend/src/services/panel_update.rs}"

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }
chk() { if [ "$1" = "0" ]; then ok "$2"; else bad "$2"; fi; }

contains()     { case "$2" in *"$1"*) return 0;; *) return 1;; esac; }
assert_has()   { if contains "$2" "$3"; then ok "$1"; else bad "$1 — expected to find: $2"; fi; }
assert_lacks() { if contains "$2" "$3"; then bad "$1 — must NOT contain: $2"; else ok "$1"; fi; }

# ── the comment strippers, ABOVE every arm that needs them (s337) ────────────
#
# A check placed below the thing it must run before inherits that position as a
# defect even when the check itself is perfect — the s336 lesson, and the reason
# these functions sit here rather than next to the source arms in Part 2.
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

# BLANK the comment, do not delete it. An ordering arm below compares line
# numbers obtained from this stream against each other; a stripper that removes
# lines renumbers the file and would silently change what that arm measures.
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
    # convention in this tree is that explanation goes on its own line, and §2c
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

# ── one addition to the set, for a second kind of blindness ──────────────────
# The orchestrator's unit-test module quotes, in its own assertions, the very
# filename the arms below look for. An arm that reads the whole file is
# therefore satisfied BY THE TESTS even when the production constant has been
# retargeted at something else entirely — measured against this suite at s338,
# with nothing commented out at all. So the Rust subject is read as production
# code: comments stripped as above, then everything from the test attribute to
# EOF blanked. Height is preserved here too, for the same reason.
prod_lines() {
  code_lines "$1" | awk '
    /^[[:space:]]*#\[cfg\(test\)\]/ { intest = 1 }
    intest                          { print ""; next }
                                    { print }'
}
prod_count() { local f="$1"; shift; prod_lines "$f" | grep -c "$@" 2>/dev/null || true; }
has_prod()   { local f="$1"; shift; [ "$(prod_count "$f" "$@")" -gt 0 ]; }

# The source arms in Part 2 are written through these four, so that no arm can
# quietly go back to reading raw text: the file is named, never `cat`ed.
pin_code()    { local l="$1" f="$2"; shift 2
                if has_code "$f" "$@"; then ok "$l"; else bad "$l — expected to find in CODE: $*"; fi; }
pin_no_code() { local l="$1" f="$2"; shift 2
                if has_code "$f" "$@"; then bad "$l — must NOT appear in CODE: $*"; else ok "$l"; fi; }
pin_prod()    { local l="$1" f="$2"; shift 2
                if has_prod "$f" "$@"; then ok "$l"; else bad "$l — expected to find in PRODUCTION code: $*"; fi; }
pin_no_prod() { local l="$1" f="$2"; shift 2
                if has_prod "$f" "$@"; then bad "$l — must NOT appear in PRODUCTION code: $*"; else ok "$l"; fi; }

SCRATCH="$(mktemp -d /tmp/dp-rollback-pin.XXXXXX)"
cleanup() { [ -n "${KEEP:-}" ] || rm -rf "$SCRATCH"; }
trap cleanup EXIT

# ── Stub environment ─────────────────────────────────────────────────────────
# systemctl records what it was asked to do — that log IS the assertion for
# "was the api restarted". docker fakes postgres. curl fakes /api/health.
STUBS="$SCRATCH/stubs"
mkdir -p "$STUBS"

cat > "$STUBS/systemctl" <<'EOF'
#!/usr/bin/env bash
echo "systemctl $*" >> "$STUB_ACTIONS"
exit 0
EOF

cat > "$STUBS/systemd-cat" <<'EOF'
#!/usr/bin/env bash
cat > /dev/null
EOF

cat > "$STUBS/curl" <<'EOF'
#!/usr/bin/env bash
echo '{"status":"ok"}'
exit 0
EOF

# The docker stub only has to answer the four things the restore asks postgres.
# Each scenario steers it with STUB_* variables.
cat > "$STUBS/docker" <<'EOF'
#!/usr/bin/env bash
echo "docker $*" >> "$STUB_ACTIONS"
args="$*"
case "$args" in
    *pg_isready*)
        exit 0 ;;
    *pg_dump*)
        # The pre-rollback capture.
        echo "-- pre-rollback dump"
        exit 0 ;;
    *"INSERT INTO panel_snapshots"*)
        exit "${STUB_RECORD_RC:-0}" ;;
    *"information_schema.tables"*)
        echo "${STUB_VERIFY_COUNT:-3}"
        exit 0 ;;
    *psql*)
        # The restore itself: consume the stream it is fed.
        cat > /dev/null
        exit "${STUB_PSQL_RC:-0}" ;;
esac
exit 0
EOF

chmod +x "$STUBS"/*

# ── Build a snapshot tarball ─────────────────────────────────────────────────
# `with_api=no` produces the snapshot-carries-no-api case, which reaches the
# hazard window on the SUCCESS path with nothing having failed at all.
make_snapshot() {
    local dir="$1" with_api="${2:-yes}"
    rm -rf "$dir"; mkdir -p "$dir/src/binaries" "$dir/src/db" "$dir/src/etc"

    printf 'OLD-API\n'   > "$dir/src/binaries/dockpanel-api"
    printf 'OLD-AGENT\n' > "$dir/src/binaries/dockpanel-agent"
    printf 'OLD-CLI\n'   > "$dir/src/binaries/dockpanel"
    [ "$with_api" = "yes" ] || rm -f "$dir/src/binaries/dockpanel-api"
    printf 'OLD-ENV\n' > "$dir/src/etc/api.env"

    {
        echo "CREATE TABLE a (id int);"
        echo "CREATE TABLE b (id int);"
        echo "CREATE TABLE c (id int);"
        echo "-- PostgreSQL database dump complete"
    } > "$dir/src/db/dump.sql"
    gzip -c "$dir/src/db/dump.sql" > "$dir/src/db/dump.sql.gz"
    rm -f "$dir/src/db/dump.sql"

    cat > "$dir/src/metadata.json" <<'JSON'
{"from_version":"2.47.3","trigger":"pre-update:2.48.0","created_at":"2026-07-28T10:00:00Z"}
JSON

    tar -C "$dir/src" -czf "$dir/snap.tar.gz" .
}

# Run the restore against a throwaway tree.
#
# Sets the globals RC / RES_JSON / ACTIONS / BIN_DIR_OUT rather than echoing the
# exit code: `run_restore …` runs the function in a SUBSHELL, so every
# variable it set vanished and each caller read an unbound one. Assigning
# globals directly is the whole reason this is not a command substitution.
run_restore() {
    local name="$1"; shift
    local with_api="${1:-yes}"; shift || true

    local base="$SCRATCH/$name"
    make_snapshot "$base" "$with_api"
    mkdir -p "$base/state" "$base/bin" "$base/etc"

    # What is installed right now: the NEWER build the operator is rolling away
    # from. If it is still here at the end, the rollback did not happen.
    printf 'NEW-API\n'   > "$base/bin/dockpanel-api"
    printf 'NEW-AGENT\n' > "$base/bin/dockpanel-agent"
    printf 'NEW-CLI\n'   > "$base/bin/dockpanel"
    printf 'NEW-ENV\n'   > "$base/etc/api.env"

    ACTIONS="$base/actions.log"; : > "$ACTIONS"
    RES_JSON="$base/state/last-restore.json"
    BIN_DIR_OUT="$base/bin"

    local sha; sha="$(sha256sum "$base/snap.tar.gz" | awk '{print $1}')"

    # `env`, not a bare assignment prefix. The shell decides which words are
    # assignments SYNTACTICALLY, before expansion — so a `"$@"` that expands to
    # `STUB_RECORD_RC=1` is not an assignment, it becomes the COMMAND NAME, and
    # every scenario that passed an override silently ran nothing and exited 127.
    # `env` takes its VAR=VALUE arguments after expansion, which is the point.
    env PATH="$STUBS:$PATH" \
        STUB_ACTIONS="$ACTIONS" \
        DOCKPANEL_RESTORE_DETACHED=1 \
        DOCKPANEL_SNAPSHOT_ID="0000-$name" \
        DOCKPANEL_SNAPSHOT_TARBALL="$base/snap.tar.gz" \
        DOCKPANEL_SNAPSHOT_SHA256="$sha" \
        DOCKPANEL_STATE_DIR="$base/state" \
        DOCKPANEL_BIN_DIR="${BIN_DIR_OVERRIDE:-$base/bin}" \
        DOCKPANEL_ETC_DIR="$base/etc" \
        DOCKPANEL_HEALTH_URL="http://127.0.0.1:1/api/health" \
        "$@" \
        bash "$RESTORE_SH" > "$base/out.log" 2>&1
    RC=$?
}

echo
echo "══ Part 1 · the restore, driven end to end ══════════════════════════════"
echo
echo "S1 · a clean restore still restores, and still brings the panel back"
run_restore happy yes
chk "$([ "$RC" = "0" ] && echo 0 || echo 1)" "exits 0"
J="$(cat "$RES_JSON" 2>/dev/null)"; A="$(cat "$ACTIONS" 2>/dev/null)"
assert_has "verdict is ok=true"                  '"ok":true'              "$J"
assert_has "stage is complete"                   '"stage":"complete"'     "$J"
assert_has "dockpanel-api was started"           'systemctl start dockpanel-api' "$A"
chk "$([ "$(cat "$BIN_DIR_OUT/dockpanel-api")" = "OLD-API" ] && echo 0 || echo 1)" \
    "the api binary was actually reverted to the snapshot's"

echo
echo "S2 · the bookkeeping INSERT fails — the box must still end CONSISTENT"
run_restore record_fails yes STUB_RECORD_RC=1
chk "$([ "$RC" = "0" ] && echo 0 || echo 1)" "exits 0 — the restore itself succeeded"
J="$(cat "$RES_JSON")"; A="$(cat "$ACTIONS")"
assert_has "verdict is ok=false — there is something to read"  '"ok":false'      "$J"
assert_has "the detail names the bookkeeping failure"   'could not be recorded'  "$J"
assert_has "dockpanel-api WAS started (binaries match the database)" \
           'systemctl start dockpanel-api' "$A"
chk "$([ "$(cat "$BIN_DIR_OUT/dockpanel-api")" = "OLD-API" ] && echo 0 || echo 1)" \
    "the rollback completed rather than aborting mid-way"

echo
echo "S3 · the post-restore table count comes back short, and unreadable"
run_restore verify_short yes STUB_VERIFY_COUNT=1
chk "$([ "$RC" = "0" ] && echo 0 || echo 1)" "exits 0"
J="$(cat "$RES_JSON")"; A="$(cat "$ACTIONS")"
assert_has "verdict is ok=false"                        '"ok":false'             "$J"
assert_has "the detail names the shortfall"             'expected >= 3'          "$J"
assert_has "dockpanel-api WAS started"     'systemctl start dockpanel-api'       "$A"

run_restore verify_junk yes STUB_VERIFY_COUNT=notanumber
chk "$([ "$RC" = "0" ] && echo 0 || echo 1)" "a NON-NUMERIC count does not abort the script either"
J="$(cat "$RES_JSON")"
assert_has "the unreadable count is reported, not raised" 'could not read a table count' "$J"

echo
echo "S4 · THE HAZARD · database reverted, api binary NOT — panel stays down"
run_restore no_api_in_snapshot no
chk "$([ "$RC" = "1" ] && echo 0 || echo 1)" "exits non-zero — this one needs an operator"
J="$(cat "$RES_JSON")"; A="$(cat "$ACTIONS")"
assert_has "verdict is ok=false"                         '"ok":false'            "$J"
assert_has "the detail says the panel was left down ON PURPOSE" 'LEFT STOPPED'   "$J"
assert_has "the detail points at the pre-rollback dump"  'pre-rollback'          "$J"
assert_lacks "dockpanel-api was NOT started"   'systemctl start dockpanel-api'   "$A"
assert_has "the agent WAS started — the box stays reachable" \
           'systemctl start dockpanel-agent' "$A"

echo
echo "S5 · THE HAZARD · a hard failure in the binaries stage, same answer"
# The binary swap is made to fail AFTER the database has committed, so the script
# dies inside the window with nothing wrong with the database at all. This is the
# s231 shape exactly: the old exit trap restarted the api from here.
#
# The failure is injected by pointing BIN_DIR at a directory that does not exist,
# NOT by removing write permission — this suite runs as root on a box where the
# panel is installed, and root ignores the permission bits, so a chmod-based
# injection quietly produces a HAPPY PATH and the scenario asserts nothing.
BIN_DIR_OVERRIDE="$SCRATCH/binaries_fail/nonexistent-bin-dir" \
    run_restore binaries_fail yes
unset BIN_DIR_OVERRIDE
J="$(cat "$RES_JSON" 2>/dev/null)"; A="$(cat "$ACTIONS")"
chk "$([ "$RC" != "0" ] && echo 0 || echo 1)" "exits non-zero"
assert_has  "the binary swap is what failed"  'could not stage'                  "$J"
assert_has  "the detail says the panel was left down ON PURPOSE" 'LEFT STOPPED'  "$J"
assert_lacks "dockpanel-api was NOT started"  'systemctl start dockpanel-api'    "$A"

echo
echo "S6 · a failure BEFORE the commit must still bring the panel back"
# The guard has to be conditional. If it refused unconditionally it would turn
# every harmless early failure into a dark box — the s228 shape it must not
# reintroduce.
run_restore psql_fails yes STUB_PSQL_RC=3
chk "$([ "$RC" != "0" ] && echo 0 || echo 1)" "exits non-zero"
J="$(cat "$RES_JSON")"; A="$(cat "$ACTIONS")"
assert_has  "the detail says nothing changed"   'nothing changed'                "$J"
assert_has  "dockpanel-api WAS restarted — nothing was lost, so nothing is held" \
            'systemctl start dockpanel-api' "$A"
assert_lacks "and it is NOT reported as left down" 'LEFT STOPPED'                "$J"

echo
echo "══ Part 2 · the update path, by source ══════════════════════════════════"
echo
# Every behavioural arm below reads CODE. This part used to `cat` both files and
# grep the text, which let a line of DOCUMENTATION satisfy a claim about
# behaviour — measured against this suite at s338: five separate mutations that
# disabled the update path outright left all six arms green, because the
# disabling edits' own comments still spelled the strings the arms looked for,
# and in one case a pre-existing paragraph did it with nothing planted at all.
#
# Each pattern is held in a variable so the mutation section (§2d) can re-use the
# arm's EXACT text. A mutation test that re-types the pattern proves the copy.
# Bracket classes rather than backslash escapes throughout: the same strings are
# handed to both grep -E and awk, and awk eats backslashes in a -v assignment.
#
# None of these patterns spells a name declared in the subjects. They match the
# SHAPE of the declaration and lift the identifier out of the source when one is
# needed, so that this file can never become the thing that satisfies a pin.
PAT_VERDICT_PATH='^[[:space:]]*[A-Za-z_][A-Za-z0-9_]*="[^"]*last-panel-update[.]json"'
PAT_EXIT_TRAP='^[[:space:]]*trap[[:space:]]+[A-Za-z_][A-Za-z0-9_]*[[:space:]]+EXIT'
PAT_DETACHED='^[[:space:]]*exec[[:space:]]+systemd-run'
PAT_RESULT_CONST='^[[:space:]]*(pub[[:space:]]+)?(const|static)[[:space:]]+[A-Z][A-Z0-9_]*[^=]*=[[:space:]]*"[^"]*last-panel-update[.]json"'
PAT_RESULT_READ='read_to_string[(][A-Z][A-Z0-9_]*[)]'
PAT_PROMOTED='verdict[[:space:]]*=[[:space:]]*Some[(][(][[:space:]]*true'
PAT_VERDICT_DECL='^[[:space:]]*let[[:space:]]+mut[[:space:]]+[a-z_]+[[:space:]]*=[[:space:]]*None;'

O="$(cat "$ORCHESTRATOR")"   # raw, and used by exactly one arm — see §2b

echo "§2a · update.sh, read as code"
# The assignment has to be an assignment: the filename must sit inside the
# QUOTED VALUE of a statement that starts the line. The census retargeted this
# variable at /dev/null and left the old path in a trailing comment on the same
# line; the arm passed, because a substring search cannot tell a value from an
# apology.
pin_code "update.sh writes a verdict file"  "$UPDATE_SH" -E "$PAT_VERDICT_PATH"
pin_code "it writes one on every exit path" "$UPDATE_SH" -E "$PAT_EXIT_TRAP"

# The trap is only worth anything if the handler it names already exists when
# the trap is registered. Both line numbers come from the stripped stream, which
# BLANKS rather than deletes — the numbers are the file's real ones. The handler
# name is lifted out of the source rather than written here.
HANDLER="$(code_lines "$UPDATE_SH" | grep -oE "$PAT_EXIT_TRAP" | awk 'NR==1{print $2}')"
TRAP_LN="$(code_lineno "$UPDATE_SH" -E "$PAT_EXIT_TRAP")"
DEF_LN=""
[ -n "$HANDLER" ] && DEF_LN="$(code_lineno "$UPDATE_SH" -E "^[[:space:]]*${HANDLER}[(][)]")"
if [ -n "$HANDLER" ] && [ -n "$TRAP_LN" ] && [ -n "$DEF_LN" ] && [ "$DEF_LN" -lt "$TRAP_LN" ]; then
    ok "the trap names a handler that is already defined above it"
else
    bad "the trap names a handler that is already defined above it — handler='${HANDLER:-none}' defined=${DEF_LN:-none} trapped=${TRAP_LN:-none}"
fi

# The whole point: systemd-run's exit status describes the handoff, never the
# work. The agent side learned this at s232 (lesson #49); the local path is the
# sibling call site that never got it.
#
# The GATE reads code too, and that is not a detail. It used to read raw text, so
# when the census BLANKED the entire re-exec block — no comment planted anywhere
# — a paragraph further down still mentioned the invocation, the gate held, and
# both arms below printed green over a subject that no longer existed. An
# absence is only meaningful while the thing it is an absence FROM is present.
if has_code "$UPDATE_SH" -E "$PAT_DETACHED"; then
    pin_no_code "update.sh does not --wait on its own detached unit" "$UPDATE_SH" -F -e '--wait'
    # --pipe implies --wait AND wires the unit's stdout to the caller, so when
    # the api is stopped the updater's next write takes SIGPIPE mid-swap.
    pin_no_code "update.sh does not use --pipe either"               "$UPDATE_SH" -F -e '--pipe'
else
    bad "update.sh no longer re-execs into a transient unit"
fi

echo
echo "§2b · the orchestrator, read as production code"
# Read as PRODUCTION code: the file's own test module spells this filename twice
# in its assertions, and those two lines alone kept this arm green while the
# constant pointed somewhere else entirely.
pin_prod "the orchestrator reads that file" "$ORCHESTRATOR" -E "$PAT_RESULT_CONST"

# A constant that nothing reads is a comment with a type. The name is taken from
# whatever the declaration above actually declares, so the arm cannot drift from
# the constant it is about.
const_use_count() {
    local f="$1" name
    name="$(prod_lines "$f" | grep -E "$PAT_RESULT_CONST" \
            | sed -E 's/^[[:space:]]*(pub[[:space:]]+)?(const|static)[[:space:]]+([A-Z][A-Z0-9_]*).*/\3/' \
            | awk 'NR==1{print}')"
    if [ -z "$name" ]; then echo 0; return; fi
    prod_count "$f" -F -e "$name"
}
if [ "$(const_use_count "$ORCHESTRATOR")" -ge 2 ]; then
    ok "the path constant is read, not merely declared"
else
    bad "the path constant is read, not merely declared — it appears on fewer than two production lines"
fi

# THE behavioural claim, and it is a NEGATIVE one: nothing may write a success
# verdict from the handoff. The census proved the point by promoting a zero exit
# straight into a recorded success and dead-coding the watch loop; the arm that
# used to carry this label was satisfied by a paragraph of prose two dozen lines
# above the code, so it never moved. §2e plants that exact defect and requires
# this pattern to fire on it.
pin_no_prod "the orchestrator says a zero exit is a handoff, not a success" \
            "$ORCHESTRATOR" -E "$PAT_PROMOTED"

# DELIBERATELY RAW, and the only arm here that is. Its subject IS the prose: the
# explanation of why a zero exit proves nothing has to stay beside the code that
# depends on it, and stripping comments for this one would be the defect rather
# than the fix. It makes no claim about behaviour — the arm above does that.
assert_has "the reason a zero exit proves nothing is still written down beside it" \
           'never "the work succeeded"' "$O"

echo
echo "§2c · the stripper itself, over and under"
# The strippers are now load-bearing for eight arms, so they get their own
# subjects. Over-stripping is the quiet failure on the negative arms above:
# every `--wait` in update.sh lives in a comment, so a stripper that ate one
# character too many would leave those two arms green forever.
PROBES="$SCRATCH/probes"; mkdir -p "$PROBES"
cat > "$PROBES/probe.sh" <<'PROBE'
#!/usr/bin/env bash
# this whole line is explanation and must be blanked
tag="colour #ffcc00"
suffix="${tag#colour }"
argc=$#
PROBE
cat > "$PROBES/probe.rs" <<'PROBE'
// this whole line is explanation and must be blanked
/* and so is this one,
   which runs on */
let endpoint = "https://example.invalid/api";
let kept = 1; // this tail is explanation and must be blanked
PROBE

if [ "$(code_lines "$PROBES/probe.sh" | wc -l)" = "$(wc -l < "$PROBES/probe.sh")" ] \
   && [ "$(code_count "$PROBES/probe.sh" -F -e 'must be blanked')" = "0" ]; then
    ok "shell: a whole-line comment is blanked, and the file keeps its height"
else
    bad "shell: a whole-line comment is blanked, and the file keeps its height"
fi
if [ "$(code_count "$PROBES/probe.sh" -F -e '#ffcc00')" = "1" ] \
   && [ "$(code_count "$PROBES/probe.sh" -F -e '${tag#colour }')" = "1" ] \
   && [ "$(code_count "$PROBES/probe.sh" -F -e 'argc=$#')" = "1" ]; then
    ok "shell: a # inside a string, a parameter expansion and \$# all survive"
else
    bad "shell: a # inside a string, a parameter expansion and \$# all survive"
fi
if [ "$(code_lines "$PROBES/probe.rs" | wc -l)" = "$(wc -l < "$PROBES/probe.rs")" ] \
   && [ "$(code_count "$PROBES/probe.rs" -F -e 'must be blanked')" = "0" ] \
   && [ "$(code_count "$PROBES/probe.rs" -F -e 'which runs on')" = "0" ]; then
    ok "rust: line comments, a block comment and a trailing tail are all blanked"
else
    bad "rust: line comments, a block comment and a trailing tail are all blanked"
fi
if [ "$(code_count "$PROBES/probe.rs" -F -e 'https://example.invalid/api')" = "1" ] \
   && [ "$(code_count "$PROBES/probe.rs" -F -e 'let kept = 1;')" = "1" ]; then
    ok "rust: a // inside a string literal is code, and the line before a tail survives"
else
    bad "rust: a // inside a string literal is code, and the line before a tail survives"
fi
if [ "$(comment_marker_for x.rs)" = '//' ] && [ "$(comment_marker_for x.sh)" = '#' ] \
   && [ -z "$(comment_marker_for x.json)" ]; then
    ok "the marker comes from the extension, so a copy must keep the subject's"
else
    bad "the marker comes from the extension, so a copy must keep the subject's"
fi

echo
echo "§2d · every pinned pattern, commented out on a copy"
# An arm that has never been made to fail is a decoration. Each pattern above is
# commented out in a COPY of its subject and its own predicate is run again: it
# has to come back blind. The copy KEEPS THE SUBJECT'S EXTENSION, because the
# stripper picks its language from the filename — a copy called `mutated` is read
# as shell whatever it holds, and a Rust subject would keep every `//` line.
MUTANTS="$SCRATCH/mutants"; mkdir -p "$MUTANTS"

mutate_out() {   # subject, ERE, tag  →  path of a copy with matching lines commented
    local src="$1" pat="$2" tag="$3" marker out
    out="$MUTANTS/$tag-$(basename "$src")"
    marker="$(comment_marker_for "$src")"; [ -n "$marker" ] || marker='#'
    awk -v pat="$pat" -v m="$marker" '$0 ~ pat { print m " " $0; next } { print }' "$src" > "$out"
    printf '%s\n' "$out"
}

# Did the plant LAND? Same height, and every changed line is the original with a
# marker in front of it — nothing added, nothing deleted, nothing else touched.
# A mutation arm that goes green over a plant which never landed is the blinded
# arm all over again, one level up.
plant_landed() {   # marker, subject, mutant  →  lines commented, or -1
    awk -v m="$1" '
        NR == FNR { a[FNR] = $0; n = FNR; next }
        { if (FNR > n) { broke = 1 }
          else if ($0 != a[FNR]) { if ($0 == m " " a[FNR]) c++; else broke = 1 } }
        END { if (FNR != n || broke) print -1; else print c + 0 }' "$2" "$3"
}

mutation_blinds() {   # label, subject, ERE, tag, predicate
    local label="$1" src="$2" pat="$3" tag="$4" pred="$5" mut marker n
    mut="$(mutate_out "$src" "$pat" "$tag")"
    marker="$(comment_marker_for "$src")"; [ -n "$marker" ] || marker='#'
    n="$(plant_landed "$marker" "$src" "$mut")"
    if [ "${n:--1}" -lt 1 ]; then
        bad "$label — the mutation never landed (changed=$n), so a green arm proves nothing"
    elif "$pred" "$mut" -E "$pat"; then
        bad "$label — the arm still finds it with the line commented out"
    else
        ok "$label"
    fi
}

mutation_blinds "commenting out the verdict-path assignment blinds its arm" \
                "$UPDATE_SH" "$PAT_VERDICT_PATH" vpath has_code
mutation_blinds "commenting out the exit trap blinds its arm" \
                "$UPDATE_SH" "$PAT_EXIT_TRAP" trap has_code
mutation_blinds "commenting out the detached re-exec trips the gate" \
                "$UPDATE_SH" "$PAT_DETACHED" detach has_code
mutation_blinds "commenting out the path constant blinds its arm" \
                "$ORCHESTRATOR" "$PAT_RESULT_CONST" pconst has_prod

MUT_READ="$(mutate_out "$ORCHESTRATOR" "$PAT_RESULT_READ" pread)"
MUT_READ_N="$(plant_landed '//' "$ORCHESTRATOR" "$MUT_READ")"
if [ "${MUT_READ_N:--1}" -lt 1 ]; then
    bad "commenting out the read of the path constant blinds the read-count arm — the mutation never landed (changed=$MUT_READ_N)"
elif [ "$(const_use_count "$MUT_READ")" -ge 2 ]; then
    bad "commenting out the read of the path constant blinds the read-count arm — the count did not move"
else
    ok "commenting out the read of the path constant blinds the read-count arm"
fi

echo
echo "§2e · the negative arms, with the real defect planted"
# Stripping makes a check see LESS. On a POSITIVE arm that is loud — over-strip
# and it goes red at HEAD, in front of whoever ran it. On a NEGATIVE arm, where
# a match means BROKEN, it is silent: the arm goes green over exactly the code it
# exists to forbid, and nothing anywhere says so. So each negative arm gets the
# real defect written into a copy and has to FIRE on it.
plant_line_after() {   # subject, anchor ERE, defect line, tag  →  copy path
    local src="$1" anchor="$2" line="$3" tag="$4" out
    out="$MUTANTS/$tag-$(basename "$src")"
    # The defect text travels in the ENVIRONMENT, not in a -v assignment: awk
    # interprets escape sequences in -v, and the flag being planted here is a
    # shell continuation, so its trailing backslash was silently eaten and the
    # plant landed as a different line than the one this arm then looked for.
    # The landing check caught it — which is the entire reason it is here.
    DEFECT_LINE="$line" awk -v a="$anchor" '
        BEGIN { l = ENVIRON["DEFECT_LINE"] }
        { print }
        !done && $0 ~ a { print l; done = 1 }' "$src" > "$out"
    printf '%s\n' "$out"
}
landed_insert() {   # subject, copy, defect line
    [ "$(wc -l < "$2")" -eq "$(( $(wc -l < "$1") + 1 ))" ] \
      && [ "$(grep -cxF "$3" "$1")" -eq 0 ] \
      && [ "$(grep -cxF "$3" "$2")" -eq 1 ]
}

WAIT_DEFECT='            --wait \'
CTL="$(plant_line_after "$UPDATE_SH" "$PAT_DETACHED" "$WAIT_DEFECT" ctlwait)"
if landed_insert "$UPDATE_SH" "$CTL" "$WAIT_DEFECT" && has_code "$CTL" -F -e '--wait'; then
    ok "planting --wait into the detached unit makes that arm fire"
else
    bad "planting --wait into the detached unit makes that arm fire — it did NOT, the arm is blind"
fi

PIPE_DEFECT='            --pipe \'
CTL="$(plant_line_after "$UPDATE_SH" "$PAT_DETACHED" "$PIPE_DEFECT" ctlpipe)"
if landed_insert "$UPDATE_SH" "$CTL" "$PIPE_DEFECT" && has_code "$CTL" -F -e '--pipe'; then
    ok "planting --pipe into the detached unit makes that arm fire"
else
    bad "planting --pipe into the detached unit makes that arm fire — it did NOT, the arm is blind"
fi

# The census's own inversion, minus the branch surgery: a success verdict
# recorded from the handoff itself, written one line under the declaration it
# overwrites so it is in scope and unconditional.
PROMOTE_DEFECT='        verdict = Some((true, String::from("complete"), String::new()));'
CTL="$(plant_line_after "$ORCHESTRATOR" "$PAT_VERDICT_DECL" "$PROMOTE_DEFECT" ctlpromote)"
if landed_insert "$ORCHESTRATOR" "$CTL" "$PROMOTE_DEFECT" && has_prod "$CTL" -E "$PAT_PROMOTED"; then
    ok "planting a recorded success at the handoff makes that arm fire"
else
    bad "planting a recorded success at the handoff makes that arm fire — it did NOT, the arm is blind"
fi

echo
echo "─────────────────────────────────────────────────────────────────────────"
printf 'passed %d · failed %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
