#!/usr/bin/env bash
# Regression pins for the s364 fix — pressing Update on an app whose image no
# longer resolves refuses BEFORE the container is touched, and the panel stops
# telling the operator the app updated when it did not.
#
# The four-way decision itself is covered by mutation-proved Rust unit tests in
# panel/agent/src/services/docker_apps.rs (pull_verdict). This suite pins what a
# unit test cannot reach: that the decision is actually consulted, that it is
# consulted before anything is stopped, that the failed step is no longer filed
# under a step the update may never have reached, and that the field recording a
# failed version check has a reader as well as a writer.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

PASS=0; FAIL=0
ok()   { printf '\033[32m✓\033[0m %s\n' "$1"; PASS=$((PASS+1)); }
bad()  { printf '\033[31m✗\033[0m %s — %s\n' "$1" "$2"; FAIL=$((FAIL+1)); }
eq()   { [ "$2" = "$3" ] && ok "$1" || bad "$1" "expected '$3', got '$2'"; }

# ugrep's --ignore-files shim honours .gitignore, so every count below uses the
# real binary explicitly (dockpanel-ops-p2, s357).
G=/usr/bin/grep

# ⚠ THREE files in this repo are named docker_apps.rs. Each is spelled out in
# full, every time — a basename here is what destroyed two files at s363.
AGENT=panel/agent/src/services/docker_apps.rs
API=panel/backend/src/routes/docker_apps.rs
APPS=panel/frontend/src/pages/Apps.tsx

for f in "$AGENT" "$API" "$APPS"; do
  [ -f "$f" ] || { bad "SETUP" "$f missing"; exit 1; }
done

# The body of a top-level fn, from its declaration to the closing brace in
# column 0. Bounded on the brace rather than a fixed -A window, which bleeds
# into the next function and reads its tokens as this one's (lesson #172).
fnbody() { sed -n "/^\(pub \)\?\(async \)\?fn $2(/,/^}/p" "$1"; }

UPDATE_BODY=$(fnbody "$AGENT" update_app)
# An extractor that reads nothing prints green for every absence arm below, so
# the subject is asserted before it is measured (lesson #143).
UB_LINES=$(printf '%s' "$UPDATE_BODY" | wc -l)
[ "$UB_LINES" -gt 100 ] && ok "A0 update_app body extracted ($UB_LINES lines)" \
  || bad "A0 update_app body extracted" "only $UB_LINES lines — §A examined nothing"

echo "── A. The refusal happens, and happens before anything is stopped ─────"

# A1: severed-pair guard. A decision helper nothing calls is the
# GPU_RECOMMENDED_TEMPLATES shape, which sat unnoticed for 19 releases.
eq "A1 update_app consults the pull verdict" \
   "$(printf '%s' "$UPDATE_BODY" | $G -c 'pull_verdict(')" "1"

# A2: THE load-bearing arm. A refusal that fires after the container is stopped
# is not a refusal, it is the take-down this fix exists to avoid. Compare the
# positions inside the extracted body, so a later edit that moves the guard
# below the stop turns this red.
V_AT=$(printf '%s\n' "$UPDATE_BODY" | $G -n 'pull_verdict(' | head -1 | cut -d: -f1)
S_AT=$(printf '%s\n' "$UPDATE_BODY" | $G -n 'stop_container(' | head -1 | cut -d: -f1)
if [ -n "$V_AT" ] && [ -n "$S_AT" ] && [ "$V_AT" -lt "$S_AT" ]; then
  ok "A2 the verdict is reached before the container is stopped (line $V_AT < $S_AT)"
else
  bad "A2 the verdict is reached before the container is stopped" "verdict at '${V_AT:-none}', stop at '${S_AT:-none}'"
fi

# A3: the pull error is carried to the decision rather than logged and dropped.
eq "A3 update_app captures the pull error" \
   "$(printf '%s' "$UPDATE_BODY" | $G -c 'pull_error = Some(')" "1"

# A4: ABSENCE arm — the discarded-warning shape is gone from this door.
eq "A4 update_app no longer swallows the pull error" \
   "$(printf '%s' "$UPDATE_BODY" | $G -c 'Image pull warning')" "0"
# POSITIVE CONTROL for A4: the same pattern must still match the other place
# that shape lives, or A4's zero proves only that the pattern never runs.
CTL=$($G -c 'Image pull warning' "$AGENT")
[ "$CTL" -gt 0 ] && ok "A4-control the discarded-warning pattern still matches $CTL site(s) elsewhere" \
  || bad "A4-control the discarded-warning pattern" "matched nothing — A4's zero is vacuous"

# A5: both refusals must tell the operator the app survived. An actionable
# refusal that does not say what state it left behind sends them to a shell.
eq "A5 both refusals state the app is still running" \
   "$(printf '%s' "$UPDATE_BODY" | $G -c 'The app is still running')" "2"

# A6: ABSENCE arm, scoped to the guard's own match block. The panel's premise
# is that you do not need a shell, and there is no terminal on this screen, so
# a refusal that hands out a command is not actionable.
#
# ⚠ Scoped deliberately, and the reason is worth keeping: `update_app` still
# contains ONE such instruction, in the migrate-abort path further down. That is
# a real and separate defect (the take-down on abort, recorded at s363 and not
# fixed here). Pinning the whole body at 0 would fail on code this ship never
# claimed to touch; pinning it at 1 would go falsely red the day somebody fixes
# it. The arm measures the refusals this ship actually added.
GUARD=$(printf '%s\n' "$UPDATE_BODY" | sed -n '/^    match pull_verdict(/,/^    }$/p')
GUARD_LINES=$(printf '%s' "$GUARD" | wc -l)
[ "$GUARD_LINES" -gt 20 ] && ok "A6-subject the verdict block extracted ($GUARD_LINES lines)" \
  || bad "A6-subject the verdict block extracted" "only $GUARD_LINES lines — A6 examined nothing"
eq "A6 neither refusal tells the operator to run a shell command" \
   "$(printf '%s' "$GUARD" | $G -c 'docker start')" "0"
# POSITIVE CONTROL for A6: the pattern still matches the doors that DO print it,
# so the zero above is a property of this block, not of a pattern that never
# matches.
CTL6=$($G -c 'docker start' "$AGENT")
[ "$CTL6" -gt 0 ] && ok "A6-control the shell-instruction pattern matches $CTL6 site(s) elsewhere" \
  || bad "A6-control the shell-instruction pattern" "matched nothing — A6's zero is vacuous"

echo "── B. The panel stops asserting a step it may never have reached ──────"

# B1: ABSENCE arm — the failure arm no longer files every outcome under the
# step that pulls the image. An operator whose migration aborted was sent to
# look at the registry.
eq "B1 the update failure is not reported as a pull failure" \
   "$($G -c 'emit("pull", "Pulling latest image", "error"' "$API")" "0"
# POSITIVE CONTROL for B1: the success path still emits that exact step, so the
# zero above measures the error arm rather than a renamed helper.
eq "B1-control the success path still emits the pull step" \
   "$($G -c 'emit("pull", "Pulling latest image", "done"' "$API")" "1"

# B2: and it emits something in its place — an error with no step at all would
# also satisfy B1.
eq "B2 the failure arm emits a step that does not name a phase" \
   "$($G -c 'emit("update", "Updating app", "error"' "$API")" "1"

echo "── C. A failed version check reaches the operator ────────────────────"

# C1: severed-pair guard, the shape this whole ship is about. The agent has
# always written this field and the client has always stored it; nothing
# rendered it, so an app whose tag had been deleted looked identical to one
# that was up to date.
# ⚠ Word-bounded on purpose. The first cut of this arm used a plain substring
# and a mutation renaming the field to `check_errorX` left it GREEN — the arm
# could not fail, because the mutated spelling still contains the pattern. Found
# by the battery, which is the only thing that could have found it.
eq "C1 the agent still writes the failed-check reason" \
   "$($G -cw 'check_error' "$AGENT")" "4"

# C2: the client renders it rather than only holding it in state. Keyed on the
# rendered label, which cannot be satisfied by the state plumbing.
eq "C2 the client renders a badge for a failed check" \
   "$($G -c 'Check failed' "$APPS")" "1"

# C3: and the reason itself reaches the operator, not just the fact.
eq "C3 the badge carries the agent's own reason" \
   "$($G -c 'Could not check for a newer image' "$APPS")" "1"

echo
if [ "$FAIL" -eq 0 ]; then
  printf '\033[32mPASS %d\033[0m / FAIL %d\n' "$PASS" "$FAIL"
else
  printf 'PASS %d / \033[31mFAIL %d\033[0m\n' "$PASS" "$FAIL"
fi
[ "$FAIL" -eq 0 ]
