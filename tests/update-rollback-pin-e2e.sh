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
# analysis. Neither needs a box, a network or a build.
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
U="$(cat "$UPDATE_SH")"
O="$(cat "$ORCHESTRATOR")"

assert_has "update.sh writes a verdict file"   'last-panel-update.json'          "$U"
assert_has "it writes one on every exit path"  'trap _dockpanel_on_exit EXIT'    "$U"
assert_has "the orchestrator reads that file"  'last-panel-update.json'          "$O"

# The whole point: systemd-run's exit status describes the handoff, never the
# work. The agent side learned this at s232 (lesson #49); the local path is the
# sibling call site that never got it.
#
# These two are checked against EXECUTABLE lines only. update.sh documents at
# length why it uses neither flag, and a negative check that reads raw source
# fails the moment someone writes that documentation — the comment naming the
# trap trips the assertion that guards against it.
U_CODE="$(grep -v '^[[:space:]]*#' "$UPDATE_SH")"
if contains 'exec systemd-run' "$U"; then
    assert_lacks "update.sh does not --wait on its own detached unit" '--wait' "$U_CODE"
    # --pipe implies --wait AND wires the unit's stdout to the caller, so when
    # the api is stopped the updater's next write takes SIGPIPE mid-swap.
    assert_lacks "update.sh does not use --pipe either" '--pipe' "$U_CODE"
else
    bad "update.sh no longer re-execs into a transient unit"
fi

assert_has "the orchestrator says a zero exit is a handoff, not a success" \
           'never "the work succeeded"' "$O"

echo
echo "─────────────────────────────────────────────────────────────────────────"
printf 'passed %d · failed %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
