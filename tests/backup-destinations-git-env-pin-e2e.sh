#!/usr/bin/env bash
#
# Regression pins for s288 — issues #93 and #94, two reports of the same shape:
# a feature whose halves were each present and were not connected to each other.
#
#   #94  The panel sent the container environment under the key "env_vars".
#        The agent's DeployRequest declared that field as `env` with a bare
#        #[serde(default)], so the unrecognised key was dropped and the
#        recognised one defaulted to empty. Every git deploy — Dockerfile and
#        Nixpacks, first deploy and redeploy, rollback and PR preview — started
#        its container with no environment, and every layer reported success.
#        A fifth call site, auto-rollback, sent no environment under either
#        spelling AND no memory_mb/cpu_percent, so a crashed container came back
#        both unconfigured and unbounded.
#
#   #93  The S3/SFTP destination form was written in Settings, then switched off
#        behind `{false && (` by the commit that moved the Destinations tab to
#        Backup Manager — which carried across the list and the Test button and
#        left create, update and delete with no caller anywhere in the panel.
#        The empty state went on telling operators to "add one via Settings".
#        Underneath, two of the three places that hand a destination to the
#        agent posted the stored CIPHERTEXT as the credential, while the third —
#        Test Connection — decrypted, so a destination verified green and then
#        failed every real upload. And the per-site schedule's destination check
#        inner-joined on a column that is NULL on every destination the panel
#        creates, so it answered 403 for all of them.
#
# ⚠ EVERY CHECK BELOW STRIPS COMMENTS FIRST. The code these pins inspect
# explains these defects in its own comments, naming the very strings under
# test — "env_vars", `{false && (`, "via Settings". A raw-source grep would
# match the prose that documents the trap and pass while the code re-broke.
# See feedback_source_pin_prose_trap and lesson #110f.
#
# Pure source analysis: no box, no network, no build.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0
FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

GD_RS=panel/backend/src/routes/git_deploys.rs
AGENT_GB=panel/agent/src/routes/git_build.rs
BD_RS=panel/backend/src/routes/backup_destinations.rs
BSCHED_RS=panel/backend/src/services/backup_scheduler.rs
BPOL_RS=panel/backend/src/services/backup_policy_executor.rs
BSCHEDR_RS=panel/backend/src/routes/backup_schedules.rs
ORCH_TSX=panel/frontend/src/pages/BackupOrchestrator.tsx
SETTINGS_TSX=panel/frontend/src/pages/Settings.tsx
BACKUPS_TSX=panel/frontend/src/pages/Backups.tsx

for f in "$GD_RS" "$AGENT_GB" "$BD_RS" "$BSCHED_RS" "$BPOL_RS" "$BSCHEDR_RS" \
         "$ORCH_TSX" "$SETTINGS_TSX" "$BACKUPS_TSX"; do
  [ -f "$f" ] || { echo "missing file: $f"; exit 1; }
done

# Strip whole-line `//` comments FIRST, then block comments — that order matters
# and the reverse is a live hazard. The sibling suites do it the other way, and a
# doc comment containing `/*` (a path glob such as the agent's backups route with
# a wildcard, which this file's own subject matter has) reads as a block-comment
# opener: the scanner then swallows every remaining line, and each later check
# inspects an empty stream and PASSES for having found nothing wrong. That is the
# failure direction a pin must never have (lesson #100). Stripping line comments
# first means a glob inside one is gone before anything looks for an opener.
#
# `//` is only stripped at the start of a (trimmed) line: stripping it mid-line
# would blank the inside of every string containing a URL.
#
# `#[cfg(test)]` modules are dropped too. Several arms below COUNT call sites,
# and a unit test that exercises the very function being counted would inflate
# the total and fail an arm whose subject is correct. That is not hypothetical:
# it cost the previous release's suite two false failures (#94c).
code_of() {
  awk '{ t=$0; sub(/^[ \t]+/,"",t); if (t !~ /^\/\//) print }' "$1" \
    | sed 's:/\*.*\*/::g' \
    | awk '
        /\/\*/ { inblk=1 }
        !inblk { print }
        /\*\// { inblk=0 }
      ' \
    | awk '/^#\[cfg\(test\)\]/ { intest=1 } !intest { print }'
}

# Guard the guard: if stripping ever eats most of a file again, say so loudly
# rather than let the checks below pass on an empty stream.
assert_readable() {
  local raw stripped
  raw=$(wc -l < "$1")
  stripped=$(code_of "$1" | grep -c . || true)
  if [ "$raw" -gt 40 ] && [ "$stripped" -lt $((raw / 4)) ]; then
    bad "comment stripping consumed $1 ($stripped of $raw lines survive) — every check against it would pass vacuously"
    return 1
  fi
  return 0
}

# grep -c, never grep -q: under pipefail a `grep -q` match SIGPIPEs its producer
# and the pipeline reports FAILURE for a SUCCESSFUL match (#103d).
has()    { [ "$(code_of "$1" | grep -cF -- "$2")" -gt 0 ]; }
hasre()  { [ "$(code_of "$1" | grep -cE -- "$2")" -gt 0 ]; }
countre(){ code_of "$1" | grep -cE -- "$2" || true; }
countf() { code_of "$1" | grep -cF -- "$2" || true; }

echo "§0 the source these pins read survives comment stripping"
STRIP_OK=0
for f in "$GD_RS" "$AGENT_GB" "$BD_RS" "$BSCHED_RS" "$BPOL_RS" "$BSCHEDR_RS" \
         "$ORCH_TSX" "$SETTINGS_TSX" "$BACKUPS_TSX"; do
  assert_readable "$f" && STRIP_OK=$((STRIP_OK+1))
done
if [ "$STRIP_OK" -eq 9 ]; then
  ok "all 9 inspected files survive stripping"
fi

echo
echo "§1 the panel spells the environment the way the agent reads it (#94)"

# DISCOVERY, not a restated constant: the accepted spellings are parsed out of
# the agent's struct, and the sent spelling out of the panel's one builder. A
# rename on either side has to survive being compared with the other.
AGENT_FIELD=$(code_of "$AGENT_GB" \
  | awk '/struct DeployRequest/,/^}/' \
  | grep -A1 -E '#\[serde\(default(, alias)?' \
  | grep -oE '^[ \t]*[a-z_]+: HashMap<String, String>' \
  | grep -oE '[a-z_]+' | head -1)
AGENT_ALIASES=$(code_of "$AGENT_GB" \
  | awk '/struct DeployRequest/,/^}/' \
  | grep -oE 'alias = "[a-z_]+"' | grep -oE '"[a-z_]+"' | tr -d '"')
AGENT_KEYS=$(printf '%s\n%s\n' "$AGENT_FIELD" "$AGENT_ALIASES" | grep -c . || true)

if [ -n "$AGENT_FIELD" ] && [ "$AGENT_KEYS" -ge 2 ]; then
  ok "agent DeployRequest env field discovered: '$AGENT_FIELD' (+$((AGENT_KEYS-1)) alias) "
else
  bad "could not discover the agent's env field and at least one alias — a broken pattern must not read as a clean sweep (#100)"
fi

PANEL_KEY=$(code_of "$GD_RS" \
  | awk '/fn build_deploy_body/,/^}/' \
  | grep -oE '"(env|env_vars)":' | grep -oE 'env(_vars)?' | head -1)

if [ -n "$PANEL_KEY" ]; then
  ok "panel build_deploy_body sends the environment as '$PANEL_KEY'"
else
  bad "build_deploy_body sends no env key at all — the deploy body has lost the environment again"
fi

# Both sides must be non-empty BEFORE they are compared. Without this guard the
# comparison passes when neither side was discovered: the accepted-spellings list
# is then two blank lines and `grep -x ""` matches the blank PANEL_KEY. Watched
# it do exactly that against the pre-fix tree, where it was the one arm of this
# section that stayed green while the defect it names was present.
if [ -z "$AGENT_FIELD" ] || [ -z "$PANEL_KEY" ]; then
  bad "cannot compare spellings: agent field='$AGENT_FIELD' panel key='$PANEL_KEY' — one side was not discovered, which is not a pass"
elif printf '%s\n%s\n' "$AGENT_FIELD" "$AGENT_ALIASES" | grep -qx -- "$PANEL_KEY"; then
  ok "'$PANEL_KEY' is a spelling the agent accepts"
else
  bad "the panel sends '$PANEL_KEY' and the agent accepts only: $(printf '%s %s' "$AGENT_FIELD" "$AGENT_ALIASES") — this is #94 exactly"
fi

echo
echo "§2 never both spellings in one body (#94's own fix would break every deploy)"

# serde rejects a payload carrying a field AND its alias as a duplicate field.
BOTH=$(code_of "$GD_RS" | awk '/fn build_deploy_body/,/^}/' | grep -cE '"env(_vars)?":' || true)
if [ "$BOTH" -eq 1 ]; then
  ok "build_deploy_body sets exactly one env key"
else
  bad "build_deploy_body sets $BOTH env keys — serde treats a field plus its alias as a duplicate field and rejects the whole request"
fi

echo
echo "§3 every deploy body comes from the one builder (#94's fifth call site)"

DEPLOY_CALLS=$(countf "$GD_RS" '"/git/deploy"')
BUILDER_CALLS=$(countre "$GD_RS" 'build_deploy_body\(DeployBody \{')

if [ "$DEPLOY_CALLS" -ge 5 ]; then
  ok "found $DEPLOY_CALLS callers of /git/deploy to account for"
else
  bad "found only $DEPLOY_CALLS /git/deploy callers — the discovery pattern is broken, not the code (#100)"
fi

if [ "$BUILDER_CALLS" -eq "$DEPLOY_CALLS" ]; then
  ok "all $DEPLOY_CALLS deploy bodies are built by build_deploy_body"
else
  bad "$DEPLOY_CALLS callers of /git/deploy but $BUILDER_CALLS uses of build_deploy_body — a body is being hand-rolled again, which is how auto-rollback lost env, memory_mb and cpu_percent"
fi

# The struct has no Default and no Option-defaulting, so every caller must name
# every field. If that stops being true, an omission goes quiet again.
if hasre "$GD_RS" 'struct DeployBody' && ! hasre "$GD_RS" 'impl Default for DeployBody'; then
  ok "DeployBody has no Default — a caller cannot omit a field silently"
else
  bad "DeployBody gained a Default; omitting env or memory_mb would compile again"
fi

echo
echo "§4 the environment reaching the agent is a map of strings (#94's aftermath)"

if hasre "$GD_RS" 'fn env_object'; then
  ok "env_vars is coerced before it is sent"
else
  bad "no env_object coercion — a non-string value in the env_vars JSONB now fails the WHOLE deploy at the agent's HashMap<String, String>"
fi

if hasre "$GD_RS" '"env": env_object\(' ; then
  ok "the deploy body's env comes from the coercion, not from the raw column"
else
  bad "build_deploy_body passes env_vars through without coercion"
fi

echo
echo "§5 the destination the agent is given has been decrypted (#93, the silent half)"

# Discovery: every site that posts a destination to the agent, versus the one
# helper that decrypts. These must be the same set.
DECRYPT_CALLERS=$(
  { countre "$BD_RS"    'agent_destination_payload\('
    countre "$BSCHED_RS" 'agent_destination_payload\('
    countre "$BPOL_RS"   'agent_destination_payload\('
  } | paste -sd+ | bc
)

if [ "$DECRYPT_CALLERS" -ge 3 ]; then
  ok "$DECRYPT_CALLERS call sites route through agent_destination_payload"
else
  bad "only $DECRYPT_CALLERS uses of agent_destination_payload — the scheduler and the policy executor must both go through it, or they post ciphertext as the credential while Test Connection reports success"
fi

for f in "$BSCHED_RS" "$BPOL_RS"; do
  if hasre "$f" 'insert\("type"\.to_string\(\)'; then
    bad "$(basename "$f") folds \"type\" into a destination payload by hand — that is the hand-rolled path that skipped the decrypt"
  else
    ok "$(basename "$f") does not hand-roll a destination payload"
  fi
done

if hasre "$BD_RS" 'pub fn agent_destination_payload' && hasre "$BD_RS" 'decrypt_config_secrets\(config\)'; then
  ok "the shared helper is the thing that decrypts"
else
  bad "agent_destination_payload is missing or no longer decrypts"
fi

echo
echo "§6 a destination can be attached to a per-site schedule (#93, the 403)"

if hasre "$BSCHEDR_RS" 'JOIN backup_destinations|bd JOIN servers'; then
  bad "the destination check still INNER JOINs servers on bd.server_id — that column is NULL on every destination the panel creates, so this answers 403 for all of them"
else
  ok "the destination check no longer inner-joins on a column that is always NULL"
fi

if hasre "$BSCHEDR_RS" 'bd\.server_id IS NULL'; then
  ok "an unscoped (shared) destination is accepted"
else
  bad "the check does not accept a NULL server_id — destinations created through the API have no server"
fi

echo
echo "§7 the three mutating endpoints have a caller in the panel (#93)"

# Discovery: the routes as registered, then whether the frontend calls each.
for verb in post put delete; do
  case $verb in
    post)   pat='api\.post\("/backup-destinations"' ; label="create" ;;
    put)    pat='api\.put\(`/backup-destinations/' ; label="update" ;;
    delete) pat='api\.delete<[^>]*>\(`/backup-destinations/|api\.delete\(`/backup-destinations/' ; label="delete" ;;
  esac
  if hasre "$ORCH_TSX" "$pat"; then
    ok "Backup Manager calls $label"
  else
    bad "no caller for $label — the endpoint is registered and unreachable, which is #93"
  fi
done

if hasre "$ORCH_TSX" 'Add Destination'; then
  ok "the Destinations tab offers an Add control"
else
  bad "the Destinations tab has no Add control"
fi

echo
echo "§8 nothing points the operator at a screen that no longer has the control (#93)"

DEST_CALLS_IN_SETTINGS=$(countf "$SETTINGS_TSX" '/backup-destinations')
if [ "$DEST_CALLS_IN_SETTINGS" -eq 0 ]; then
  ok "Settings makes no backup-destination calls"
else
  bad "Settings still has $DEST_CALLS_IN_SETTINGS backup-destination call(s) — the split ownership that produced #93"
fi

if hasre "$SETTINGS_TSX" '\{false && \('; then
  bad "Settings has a {false && (...)} block — a feature switched off in place is a feature nobody can find and nobody deletes"
else
  ok "Settings has no disabled-in-place block"
fi

for f in "$ORCH_TSX" "$BACKUPS_TSX" "$SETTINGS_TSX"; do
  if hasre "$f" '(via|in) \*{0,2}Settings'; then
    bad "$(basename "$f") still sends the operator to Settings for a destination"
  else
    ok "$(basename "$f") does not send the operator to Settings for a destination"
  fi
done

echo
echo "§9 the deep link the guidance layer already used actually lands (#93)"

PREREQ_RS=panel/backend/src/services/prerequisites/backups.rs
if hasre "$PREREQ_RS" 'backup-orchestrator\?tab=destinations'; then
  ok "the prerequisite remediation links to the destinations tab"
else
  bad "the prerequisite no longer links to the destinations tab"
fi

if hasre "$ORCH_TSX" 'function tabFromSearch' && hasre "$ORCH_TSX" 'URLSearchParams'; then
  ok "the page reads ?tab= — the remediation link lands where it says"
else
  bad "BackupOrchestrator ignores ?tab=, so every link the panel already emits opens Overview instead"
fi

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
