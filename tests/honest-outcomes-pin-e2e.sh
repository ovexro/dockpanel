#!/usr/bin/env bash
# Regression pins for the s367 ship — the panel stops reporting outcomes it cannot support.
#
# Five defects with one shape: the product told the operator something that was not
# true, and in four of the five the lie was the reassuring direction.
#
#   * Deleting an account that had ever scheduled a maintenance window raised a
#     foreign-key violation inside the delete transaction and surfaced as an
#     incident id. The account could never be removed through the panel again, and
#     nothing in the product could clear the blocking row — the only handler that
#     deletes one is scoped to its own owner.
#   * Clearing a Slack, Discord or PagerDuty destination was impossible. The client
#     substituted a null for an empty box and the handler COALESCEs a null onto the
#     stored value, so the old destination survived and the save answered green.
#   * "All containers are up to date" was rendered whenever the update count was
#     zero — including a run in which every registry check had failed, because a
#     check that never got an answer scores the same as an image that is current.
#   * The update door emitted the pull step as running and never resolved it on
#     failure, so a live spinner sat under a red failure header for ever.
#   * The deploy door filed every failure under the step that pulls the image —
#     the defect the update door was repaired for one release earlier.
#
# And one that is not a lie but is the same ship: five container teardowns removed
# a container without its anonymous volumes, orphaning them permanently.
#
# §A and §E are CLASS arms: they enumerate their own subjects from the tree rather
# than naming them, so the next table and the next teardown are covered without an
# edit here. Both fail on an implausibly small subject rather than reporting green.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

PASS=0; FAIL=0
ok()  { printf '\033[32m✓\033[0m %s\n' "$1"; PASS=$((PASS+1)); }
bad() { printf '\033[31m✗\033[0m %s — %s\n' "$1" "$2"; FAIL=$((FAIL+1)); }
eq()  { [ "$2" = "$3" ] && ok "$1" || bad "$1" "expected '$3', got '$2'"; }

# ugrep's --ignore-files shim honours .gitignore, so every count below uses the
# real binary explicitly (dockpanel-ops-p2, s357).
G=/usr/bin/grep

API=panel/backend/src/routes/docker_apps.rs
APPS=panel/frontend/src/pages/Apps.tsx
SETTINGS=panel/frontend/src/pages/Settings.tsx
ALERTS=panel/backend/src/routes/alerts.rs
NOTIFY=panel/backend/src/services/notifications.rs
MIGDIR=panel/backend/migrations

for f in "$API" "$APPS" "$SETTINGS" "$ALERTS" "$NOTIFY"; do
  [ -f "$f" ] || { bad "SETUP" "$f missing"; exit 1; }
done
[ -d "$MIGDIR" ] || { bad "SETUP" "$MIGDIR missing"; exit 1; }

# An arm that measures an empty subject prints green for every absence below, so
# each subject is asserted before it is measured (lesson #143).
for pair in "$API:2000" "$APPS:1500" "$SETTINGS:2500"; do
  f=${pair%:*}; min=${pair##*:}
  n=$($G -c '' "$f")
  [ "$n" -gt "$min" ] && ok "A0 subject extracted — $f is $n lines" \
    || bad "A0 subject extracted" "$f is only $n lines — arms over it examined nothing"
done

echo "── A. Every owner link to an account declares what happens when it is deleted ─"

# A1 is the durable arm and the reason this suite exists. It does NOT name the two
# tables that were wrong; it enumerates every foreign key to the accounts table
# across the whole migration tree and asks each one whether it says what a delete
# should do. A table added next year is covered without touching this file.
#
# The awkward part is that a declaration made without a delete action can be
# repaired by a later ALTER — that is exactly what the release before this one did
# for one column, and what this release does for two more. So a flat "no bare
# declaration anywhere" arm would go RED on a correct tree. The arm therefore
# models last-writer-wins: a bare declaration is a violation only if nothing later
# repairs that same table and column.
FK_REPORT=$(python3 - "$MIGDIR" <<'PY'
import os, re, sys
d = sys.argv[1]
# Strip SQL comments first: this repo's own pins have been satisfied by prose
# describing the hazard they forbid, so comments must never reach the matcher.
def strip(s):
    s = re.sub(r'/\*.*?\*/', ' ', s, flags=re.S)
    return re.sub(r'--[^\n]*', ' ', s)

bare, repaired, total = set(), set(), 0
for fn in sorted(os.listdir(d)):
    if not fn.endswith('.sql'):
        continue
    src = strip(open(os.path.join(d, fn)).read())

    # Form 1 — inline on a column definition: `<col> UUID ... REFERENCES <parent>(id) ...`
    # The statement ends at the next comma or the closing paren of CREATE TABLE.
    #
    # The parent table is a WILDCARD, and that widening is the point. This arm was
    # written naming `users`, so the release that added it could not see the two
    # remaining bare links in the schema — they pointed at `servers`, and an account
    # that could not be deleted got a guard while a server that could not be deleted
    # did not. A census that names one parent measures one parent.
    #
    # Every foreign-key column in the live catalogue is UUID (112 of 112, checked
    # against pg_constraint rather than assumed), so restricting form 1 to UUID
    # columns costs no coverage today; if that ever stops being true this arm
    # under-counts, which is what the control below exists to catch.
    for m in re.finditer(r'(\w+)\s+UUID[^,\)]*?REFERENCES\s+(\w+)\s*\(\s*id\s*\)([^,\)]*)', src, re.I):
        total += 1
        col, tail = m.group(1).lower(), m.group(3)
        tbl = None
        head = src[:m.start()]
        t = re.findall(r'CREATE\s+TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?(\w+)', head, re.I)
        if t:
            tbl = t[-1].lower()
        if not re.search(r'ON\s+DELETE', tail, re.I):
            bare.add((tbl, col))
        else:
            repaired.add((tbl, col))

    # Form 2 — an ALTER that adds the constraint explicitly.
    for m in re.finditer(
            r'ALTER\s+TABLE\s+(\w+)\s+ADD\s+CONSTRAINT\s+\w+\s+FOREIGN\s+KEY\s*\(\s*(\w+)\s*\)\s*'
            r'REFERENCES\s+(\w+)\s*\(\s*id\s*\)([^;]*)', src, re.I):
        total += 1
        tbl, col, tail = m.group(1).lower(), m.group(2).lower(), m.group(4)
        if re.search(r'ON\s+DELETE', tail, re.I):
            repaired.add((tbl, col))
        else:
            bare.add((tbl, col))

violations = sorted(f"{t}.{c}" for (t, c) in bare if (t, c) not in repaired)
print(f"TOTAL={total}")
print(f"REPAIRED={len(repaired)}")
print(f"VIOLATIONS={len(violations)}")
for v in violations:
    print(f"  {v}")
PY
)
FK_TOTAL=$(printf '%s' "$FK_REPORT"   | $G -oE '^TOTAL=[0-9]+'      | cut -d= -f2)
FK_REPAIR=$(printf '%s' "$FK_REPORT"  | $G -oE '^REPAIRED=[0-9]+'   | cut -d= -f2)
FK_VIOL=$(printf '%s' "$FK_REPORT"    | $G -oE '^VIOLATIONS=[0-9]+' | cut -d= -f2)

# POSITIVE CONTROL, and it comes first: an empty or failed scan reports zero
# violations, which is indistinguishable from a healthy tree. A parse that finds
# implausibly few account links has not measured anything (lessons #480, #532).
if [ -n "${FK_TOTAL:-}" ] && [ "$FK_TOTAL" -ge 90 ]; then
  ok "A1-control the migration scan found $FK_TOTAL foreign keys, $FK_REPAIR carrying a delete action"
else
  bad "A1-control the migration scan found $FK_TOTAL foreign keys, $FK_REPAIR carrying a delete action" \
      "fewer than 90 — the scan is broken, so A1 below measured nothing"
fi

if [ "${FK_VIOL:-1}" = "0" ]; then
  ok "A1 every foreign key declares a delete action"
else
  bad "A1 every foreign key declares a delete action" \
      "$FK_VIOL never repaired: $(printf '%s' "$FK_REPORT" | $G -E '^  ' | tr -d ' ' | tr '\n' ' ')"
fi

# A2: the specific table that made an account undeletable. Named as well as
# enumerated, because A1 would also go green if somebody deleted the table.
#
# Counts the STATEMENTS, not the files that contain one. The first draft of this
# arm asked whether any file mentioned the table and survived its own mutation
# test: the repair is a drop and an add, so removing either left the other
# matching and the arm reported green on a half-applied repair.
eq "A2 the maintenance-window owner link is dropped and re-added" \
   "$($G -c '^ALTER TABLE maintenance_windows$' "$MIGDIR/20260816000000_user_fk_delete_actions.sql")" "2"

# A3: and the repair is the one that removes the row rather than one that cannot
# apply — both columns are NOT NULL, so the alternative would fail at migration time.
eq "A3 the repair removes the dependent rows" \
   "$($G -c 'FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE' \
      "$MIGDIR/20260816000000_user_fk_delete_actions.sql")" "1"

echo "── B. A cleared notification destination is actually cleared ─────────"

# B1: ABSENCE arm. The three destination fields must reach the wire as the operator
# left them. The null substitution is what the handler COALESCEs away.
DEST_BLOCK=$(/usr/bin/sed -n '/await api.put("\/alert-rules"/,/muted_types/p' "$SETTINGS")
eq "B1 no destination field is nulled on its way to the handler" \
   "$(printf '%s' "$DEST_BLOCK" | $G -cE 'notify_(slack_url|discord_url|pagerduty_key): [A-Za-z]+ \|\| null')" "0"

# POSITIVE CONTROL for B1: the block really is the save payload and really was read.
eq "B1-control the save payload block carries all three destinations" \
   "$(printf '%s' "$DEST_BLOCK" | $G -cE 'notify_(slack_url|discord_url|pagerduty_key):')" "3"

# B2: the clear is only safe because both ends treat an empty string as "off".
# If either end changes, the fix above becomes a delivery failure instead.
# Counted rather than predicted: the service carries two sender paths and every
# destination in both is guarded, which is 7 URL guards and 2 key guards. Scoped to
# the whole sender service deliberately — it is the region the claim is about, and
# a guard removed from either path is what would make the clear above unsafe.
eq "B2 every URL destination is guarded on non-empty before delivery" \
   "$($G -c 'if !url.is_empty() {' "$NOTIFY")" "7"
eq "B2b every key destination is guarded on non-empty before delivery" \
   "$($G -c 'if !key.is_empty() {' "$NOTIFY")" "2"
eq "B3 the URL validators skip an empty destination rather than rejecting it" \
   "$($G -c 'if !url.is_empty() {' "$ALERTS")" "3"

echo "── C. The update-check summary cannot claim success over a failed check ─"

# C1: ABSENCE arm — the success sentence must no longer be reachable from the
# update count alone. Keyed on the ternary that produced it.
eq "C1 the summary does not decide success on the update count alone" \
   "$($G -c 'type: result.updates_available > 0 ? "warning" : "success"' "$APPS")" "0"

# POSITIVE CONTROL for C1: the sentence itself is still in the file, so the zero
# above measures the decision rather than a deleted feature.
eq "C1-control the up-to-date sentence still exists for the case that earns it" \
   "$($G -c 'All containers are up to date' "$APPS")" "1"

# C2: the count of failed checks is derived from the payload the badges already use.
eq "C2 the summary counts the checks that failed" \
   "$($G -c 'result.updates.filter(u => u.check_error).length' "$APPS")" "1"

# C3: and a run with failures cannot be typed as a success.
eq "C3 a run carrying a failed check is not typed success" \
   "$($G -c 'type: n > 0 || unchecked > 0 ? "warning" : "success"' "$APPS")" "1"

echo "── D. Every phase the doors announce reaches a terminal state ─────────"

# D1: the update door resolves the pull step on the failure path. ABSENCE arms
# elsewhere forbid reporting it as a pull failure, so this asserts the presence of
# the honest terminal state instead.
eq "D1 the update failure path resolves the pull step" \
   "$($G -c 'emit("pull", "Pulling latest image", "pending", None)' "$API")" "1"

# D2: the deploy door stops filing every failure under the step that pulls.
eq "D2 the deploy failure is not reported as a pull failure" \
   "$($G -c 'emit("pull", "Pulling Docker image", "error"' "$API")" "0"

# POSITIVE CONTROL for D2: the deploy door still announces and completes that step
# on the path that earns it, so the zero above is not a deleted feature.
eq "D2-control the deploy success path still emits the pull step" \
   "$($G -c 'emit("pull", "Pulling Docker image", "done", None)' "$API")" "1"

# D3: and it emits something in its place — an error with no step at all would
# satisfy D2 while leaving the operator with nothing.
eq "D3 the deploy failure names a step that asserts no phase" \
   "$($G -c 'emit("deploy", "Deploying app", "error"' "$API")" "1"

# D4: the terminal state must be one the log can actually render as finished.
# `in_progress` is the only status that draws a spinner, so a step left in it is
# the defect; the union below is what the client accepts.
eq "D4 the client models a non-running terminal state for a step" \
   "$($G -c '"pending" | "in_progress" | "done" | "error"' panel/frontend/src/components/ProvisionLog.tsx)" "1"

echo "── E. Every container teardown says what happens to its volumes ───────"

# E1: CLASS arm. Enumerates every teardown in the agent and requires each to state
# its volume disposition explicitly. Three probe/cleanup sites inherited the
# derived default, which orphaned an anonymous volume on every call for ever.
# Keyed on the option struct rather than on a list of files, so a teardown added to
# a new service is covered without an edit here.
VOL_REPORT=$(python3 - <<'PY'
import glob, re
total = 0
missing = []
for f in glob.glob('panel/agent/src/**/*.rs', recursive=True):
    lines = open(f).read().split('\n')
    for i, l in enumerate(lines):
        if 'RemoveContainerOptions {' not in l:
            continue
        total += 1
        win = []
        for j in range(i + 1, min(i + 14, len(lines))):
            win.append(lines[j])
            if lines[j].strip().startswith('}'):
                break
        # A comment mentioning the field must not satisfy the arm.
        body = '\n'.join(x for x in win if not x.strip().startswith('//'))
        if not re.search(r'^\s*v:\s*(true|false)\s*,', body, re.M):
            missing.append(f"{f}:{i+1}")
print(f"TOTAL={total}")
print(f"MISSING={len(missing)}")
for m in missing:
    print(f"  {m}")
PY
)
VOL_TOTAL=$(printf '%s' "$VOL_REPORT"   | $G -oE '^TOTAL=[0-9]+'   | cut -d= -f2)
VOL_MISS=$(printf '%s' "$VOL_REPORT"    | $G -oE '^MISSING=[0-9]+' | cut -d= -f2)

# POSITIVE CONTROL first, same reasoning as A1-control.
if [ -n "${VOL_TOTAL:-}" ] && [ "$VOL_TOTAL" -ge 20 ]; then
  ok "E1-control the teardown scan found $VOL_TOTAL container removals"
else
  bad "E1-control the teardown scan found $VOL_TOTAL container removals" \
      "fewer than 20 — the scan is broken, so E1 below measured nothing"
fi

if [ "${VOL_MISS:-1}" = "0" ]; then
  ok "E1 every container teardown states its volume disposition"
else
  bad "E1 every container teardown states its volume disposition" \
      "$VOL_MISS inherit the derived default: $(printf '%s' "$VOL_REPORT" | $G -E '^  ' | tr -d ' ' | tr '\n' ' ')"
fi

# E2: the two probes are the sites that leaked on every call, and they are the ones
# whose containers are created with no host config at all. Named because E1 would
# also go green if the probes were deleted.
eq "E2 the image probes take their anonymous volumes with them" \
   "$(/usr/bin/sed -n '/async fn seed_volume_from_image/,/^}/p' panel/agent/src/services/docker_apps.rs | $G -c 'v: true,')" "1"
eq "E3 the file-read probe takes its anonymous volumes with them" \
   "$(/usr/bin/sed -n '/async fn read_file_from_image/,/^}/p' panel/agent/src/services/docker_apps.rs | $G -c 'v: true,')" "1"

# ── §F the migration wizard reports what it actually stored (s369) ──────────
#
# `INSERT INTO databases` in the import loop omitted `site_id`, which is NOT NULL
# with no default — so the statement could never succeed — and discarded its own
# result, so the loop carried on and announced "Imported" over a row that was
# never written. The operator was left with a running database container the
# panel could not list, reach or delete, holding a password that existed nowhere
# else. `ON CONFLICT DO NOTHING` arbitrates unique conflicts and does not soften
# a not-null violation.
#
# A CLASS arm: it enumerates every INSERT INTO databases in the tree rather than
# naming the one that was wrong, and fails on an implausibly small subject.
DB_INSERTS=$($G -rn "INSERT INTO databases" panel/backend/src --include=*.rs | wc -l)
if [ "$DB_INSERTS" -ge 3 ]; then
  ok "F0 discovered $DB_INSERTS INSERT INTO databases sites (>= 3 known to exist)"
else
  bad "F0 only $DB_INSERTS INSERT INTO databases sites" "the scan broke; F1 is vacuous"
fi
eq "F1 every INSERT INTO databases names the NOT NULL site_id" \
   "$($G -rn "INSERT INTO databases" panel/backend/src --include=*.rs | $G -cv "site_id")" "0"
eq "F2 the import's insert is checked rather than discarded" \
   "$(/usr/bin/sed -n '/Insert the DB record/,/^            let landed/p' panel/backend/src/routes/migration.rs | $G -c 'fetch_optional')" "1"
eq "F3 a database that cannot be recorded has its container taken back down" \
   "$(/usr/bin/sed -n '/if !landed {/,/^            }/p' panel/backend/src/routes/migration.rs | $G -c 'agent.delete')" "1"
# The site a database belongs to is resolved BEFORE the container is created, or
# the failure it prevents has already happened by the time it is detected.
RESOLVE_LINE=$($G -n "Which site does this database belong to" panel/backend/src/routes/migration.rs | cut -d: -f1)
CREATE_LINE=$($G -n 'agent.post("/databases"' panel/backend/src/routes/migration.rs | cut -d: -f1)
if [ -n "$RESOLVE_LINE" ] && [ -n "$CREATE_LINE" ] && [ "$RESOLVE_LINE" -lt "$CREATE_LINE" ]; then
  ok "F4 the site is resolved before the container is created"
else
  bad "F4 the site resolution moved after the container creation" "resolve=$RESOLVE_LINE create=$CREATE_LINE"
fi

# The wizard must not offer a location the agent structurally cannot read. The
# unit runs `PrivateTmp=yes`, so an archive copied to the host's /tmp — the
# obvious place, and the one this guard used to name — is invisible to it, and
# the very next line answered "File not found" for a file plainly on disk.
# Driven at s369: the identical archive fails from /tmp and analyzes from
# /var/backups/.
eq "F5 the migration path guard no longer offers /tmp" \
   "$($G -c 'starts_with("/tmp/")' panel/agent/src/routes/migration.rs)" "0"
eq "F5b …and it is still guarded to /var/backups (control: the check exists)" \
   "$($G -c 'starts_with("/var/backups/")' panel/agent/src/routes/migration.rs)" "2"
eq "F5c the agent unit really does have a private /tmp, which is why" \
   "$($G -c '^PrivateTmp=yes' panel/agent/dockpanel-agent.service 2>/dev/null || $G -rc 'PrivateTmp=yes' panel/agent/src/ scripts/ 2>/dev/null | awk -F: '{s+=$2} END{print (s>0)?1:0}')" "1"
# "Container running" is not "database usable": mariadb:11 refuses the app user
# while it initialises, so a fixed sleep produced `Access denied` — a
# credentials message for a timing problem.
eq "F6 the engine wait is a poll, not a fixed sleep" \
   "$(/usr/bin/sed -n '/Waiting for {db_name} engine/,/Import SQL dump via agent/p' panel/backend/src/routes/migration.rs | $G -c 'ENGINE_READY_TIMEOUT_SECS')" "2"
eq "F6b the probe goes through the engine, not just docker" \
   "$(/usr/bin/sed -n '/Waiting for {db_name} engine/,/Import SQL dump via agent/p' panel/backend/src/routes/migration.rs | $G -c '/databases/query')" "1"
# …and it has to carry a real statement and real credentials, or the agent
# rejects the body, every poll fails, and the wait degrades back to a timeout
# that reports nothing. An arm that only checks the ENDPOINT survives a broken
# payload (found by mutation, s369).
eq "F6c the probe carries a statement the engine can answer" \
   "$(/usr/bin/sed -n '/Waiting for {db_name} engine/,/Import SQL dump via agent/p' panel/backend/src/routes/migration.rs | $G -cE '\"sql\": \"SELECT 1\"')" "1"
eq "F6d …and the credentials the container was created with" \
   "$(/usr/bin/sed -n '/Waiting for {db_name} engine/,/Import SQL dump via agent/p' panel/backend/src/routes/migration.rs | $G -cE '\"(container|engine|database|user|password)\":')" "5"

# ── §G Update does not also START an app the operator stopped (s369) ────────
#
# All three recreate doors started the new container unconditionally, so
# pressing Update on something deliberately stopped brought it back up. The
# fourth start is inside `blue_green_update`, which is the branch taken for any
# app with a domain — the normal case — so guarding only the three visible
# stop/start sites would have missed the majority of installs.
#
# CLASS arm: it enumerates every start_container in the recreate file rather
# than naming the four that were wrong.
ADS=panel/agent/src/services/docker_apps.rs
STARTS=$($G -c "\.start_container(" "$ADS")
if [ "$STARTS" -ge 5 ]; then
  ok "G0 discovered $STARTS start_container sites (>= 5 known to exist)"
else
  bad "G0 only $STARTS start_container sites" "the scan broke; G1 is vacuous"
fi
eq "G1 the pre-update state is captured on all three recreate doors" \
   "$($G -c 'let was_running = matches!(' "$ADS")" "3"
eq "G2 every recreate door's start is guarded by it" \
   "$($G -c '^    if was_running {' "$ADS")" "3"
eq "G3 blue-green is not taken for an app that was already stopped" \
   "$($G -c '} else if !was_running {' "$ADS")" "1"
# A paused container reports Running in bollard, so the guard must read `status`.
eq "G4 the guard reads status, not the running flag" \
   "$($G -c 'ContainerStateStatusEnum::RUNNING' "$ADS")" "3"

# ── §H a refusal has to come from a layer that can answer 4xx (s369, v2.122.1) ──
#
# The agent's custom_nginx validator runs inside the template RENDER, and
# `agent_error` hands an agent sentence through only on 4xx — a render failure is
# a 500, so it reaches the operator as `Operation failed. Reference: <uuid>`.
# Driven on a fresh box: the new duplicate-directive refusal came back 502 with a
# reference, exactly like the nginx error it replaced. The reason now also lives
# in the BACKEND validator, which answers 400 and carries its own text.
#
# Both layers must refuse. The backend one is what the operator reads; the agent
# one is what holds if anything ever writes a vhost without going through it.
eq "H1 the backend validator refuses the directives the template already sets" \
   "$(/usr/bin/sed -n '/fn is_safe_nginx_config/,/^}/p' panel/backend/src/routes/mod.rs | $G -cE '\"client_max_body_size\" =>')" "1"
eq "H2 …and gzip's pair with it" \
   "$(/usr/bin/sed -n '/fn is_safe_nginx_config/,/^}/p' panel/backend/src/routes/mod.rs | $G -cE '\"gzip\" \| \"gzip_min_length\" =>')" "1"
eq "H3 the agent keeps its own copy (defence in depth)" \
   "$($G -c 'ALREADY_SET_BY_THE_TEMPLATE' panel/agent/src/services/nginx.rs)" "2"
eq "H4 the caller answers 400 from the backend check, not 502 from the agent" \
   "$(/usr/bin/sed -n '/pub async fn update_limits/,/^}/p' panel/backend/src/routes/sites.rs | $G -c 'is_safe_nginx_config')" "1"
# Control: the three that legitimately repeat must NOT be in the refusal list, or
# the fix takes away working configuration.
eq "H5 add_header/gzip_types/expires are not refused" \
   "$(/usr/bin/sed -n '/fn is_safe_nginx_config/,/^}/p' panel/backend/src/routes/mod.rs | $G -cE '\"(add_header|gzip_types|expires)\" =>')" "0"

echo
echo "PASS $PASS  FAIL $FAIL"
[ "$FAIL" -eq 0 ] || exit 1
