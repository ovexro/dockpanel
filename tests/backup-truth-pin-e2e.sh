#!/usr/bin/env bash
# backup-truth-pin-e2e.sh — s292 / v2.50.0
#
# Pins the backup defects fixed this release. Pure source analysis: no box, no
# network, no build.
#
# The arms are written against the CAPABILITY a regression must use, not against
# the spelling the code happens to have today (lesson #122: the previous
# provision-log suite passed 30/30 while five reintroductions walked past it).
#
# NO PIPES INTO `grep -q`. Under `set -o pipefail`, grep -q closes the pipe on
# its first match, the upstream process dies of SIGPIPE (141), and pipefail
# reports the PIPELINE as failed — so an arm goes red on correct code, and does
# it non-deterministically. backup-lands-pin-e2e.sh's first draft shipped with
# exactly this bug. Every arm here feeds grep a here-string instead.
#
# Comment handling: every arm reads COMMENT-STRIPPED source (line AND block), so
# a comment can neither satisfy nor break an arm — unlike
# site-backup-databases-pin-e2e.sh, which strips nothing.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

ORCH=panel/backend/src/routes/backup_orchestrator.rs
EXEC=panel/backend/src/services/backup_policy_executor.rs
SCHED=panel/backend/src/services/backup_scheduler.rs
DEST=panel/backend/src/routes/backup_destinations.rs
BACKUPS=panel/backend/src/routes/backups.rs
PREREQ=panel/backend/src/services/prerequisites/backups.rs
UI=panel/frontend/src/pages/BackupOrchestrator.tsx
GUIDE=docs/guides/backup-orchestrator.md

for f in "$ORCH" "$EXEC" "$SCHED" "$DEST" "$BACKUPS" "$PREREQ" "$UI" "$GUIDE"; do
  [ -f "$f" ] || bad "MISSING SUBJECT FILE: $f"
done

# Block comments first: stripping line comments first would let a `//` holding a
# `/*` eat the rest of the file (feedback_source_pin_prose_trap, variant 4).
code() { perl -0777 -pe 's{/\*.*?\*/}{}gs; s{^\s*//.*$}{}gm' "$1"; }

# An arm whose subject could not be extracted must SKIP, not print a confident
# green next to a red about the same subject (lesson #122b).
subj() { local t; t=$(code "$1"); [ -n "$t" ] || return 1; printf '%s' "$t"; }

# has/hasnt/count all take the already-extracted source as $1.
has()   { grep -qE -- "$2" <<< "$1"; }
count() { grep -cE -- "$2" <<< "$1" || true; }

echo "== §1  the SLA queries derive a site's server instead of inventing a column =="
if S=$(subj "$ORCH"); then
  # `backups` has no server_id column and never has. A union branch that labels
  # itself 'site' AND selects a server must therefore reach it through `sites`.
  # Checked as a two-line window rather than a literal, so a differently-spelled
  # reintroduction fails too.
  SITE_SRV=$(count "$S" "server_id, '(site)'::text")
  WINDOW=$(grep -A1 -E "server_id, '(site)'::text" <<< "$S" || true)
  SITE_SRV_JOINED=$(count "$WINDOW" "FROM backups b JOIN sites s")
  if [ "$SITE_SRV" -ge 2 ] && [ "$SITE_SRV" = "$SITE_SRV_JOINED" ]; then
    ok "every site branch that selects a server joins sites ($SITE_SRV/$SITE_SRV_JOINED)"
  else
    bad "a site branch selects a server without joining sites ($SITE_SRV branches, $SITE_SRV_JOINED joined)"
  fi

  # The pre-fix literal, kept as a cheap second opinion on the window arm.
  if has "$S" "SELECT id, created_at, server_id, 'site'"; then
    bad "the original defect literal is back in an SLA CTE"
  else
    ok "no SLA CTE selects server_id straight off backups"
  fi

  # A failed measurement that renders as an empty state is the defect this
  # release removes; the flag is what lets the UI tell them apart.
  if has "$S" "sla_unavailable"; then
    ok "health reports whether the SLA could be measured at all"
  else
    bad "sla_unavailable is gone — a failed SLA query is indistinguishable from no backups again"
  fi
else
  echo "  - skipped §1 (subject unreadable)"
fi
if S=$(subj "$UI"); then
  if has "$S" "sla_unavailable"; then
    ok "the card distinguishes 'could not measure' from 'nothing to measure'"
  else
    bad "the card renders a failed measurement as its empty state again"
  fi
else
  echo "  - skipped §1 UI (subject unreadable)"
fi

echo "== §2  a site that has NEVER been backed up is representable =="
if S=$(subj "$ORCH"); then
  # The HAVING clause deliberately admits sites with no backup at all, and
  # ORDER BY NULLS FIRST puts them on row 1 — so a non-Option row type fails the
  # whole fetch, not just that row.
  if has "$S" "last_backup: Option<chrono::DateTime"; then
    ok "StaleBackup.last_backup is Option"
  else
    bad "StaleBackup.last_backup is not Option — a never-backed-up site cannot decode"
  fi
  if has "$S" "days_since: Option<i64>"; then
    ok "StaleBackup.days_since is Option"
  else
    bad "StaleBackup.days_since is not Option"
  fi
  if has "$S" "Vec<\(String, Option<chrono::DateTime"; then
    ok "the stale-site query decodes into an Option"
  else
    bad "the stale-site query row type no longer admits NULL"
  fi
fi
if S=$(subj "$UI"); then
  if has "$S" "never backed up"; then
    ok "the UI names the never-backed-up case instead of printing an arithmetic artefact"
  else
    bad "the UI lost the never-backed-up rendering"
  fi
fi

echo "== §3  no backup is reported as a success the panel cannot list =="
# Dropping a Result requires spelling `let _ =` in front of the statement.
for f in "$EXEC" "$SCHED" "$BACKUPS"; do
  if S=$(subj "$f"); then
    W=$(grep -A1 -E 'let _ = sqlx::query\(' <<< "$S" || true)
    N=$(count "$W" '"INSERT INTO (backups|database_backups|volume_backups)')
    if [ "$N" = "0" ]; then
      ok "$(basename "$f"): no INSERT into a backup table discards its Result"
    else
      bad "$(basename "$f"): $N backup INSERT(s) still discard their Result"
    fi
  fi
done
if S=$(subj "$EXEC"); then
  # `failures` is what downgrades the status and lights the incident + alert.
  N=$(count "$S" "could not be recorded")
  if [ "$N" -ge 3 ]; then
    ok "all three policy kinds report an unrecorded backup ($N)"
  else
    bad "expected 3 unrecorded-backup reports in the executor, found $N"
  fi
fi
if S=$(subj "$SCHED"); then
  if has "$S" "could not be recorded"; then
    ok "the scheduler fails the run when the row does not land"
  else
    bad "the scheduler no longer fails the run when the row does not land"
  fi
fi
if S=$(subj "$BACKUPS"); then
  if has "$S" "Backup not recorded"; then
    ok "the manual path's live log reports an unrecorded backup"
  else
    bad "the manual path emits a green complete over an unrecorded backup again"
  fi
  # That branch must NOT return early: the tail of the spawned task removes the
  # provisioning log from the shared map, so returning leaks it and its owner.
  #
  # Guarded on a non-empty window. Without the guard this arm passes vacuously
  # whenever the branch is absent — printing a confident green beside the red
  # about the very same subject (lesson #122b).
  W=$(grep -A3 "Backup not recorded" <<< "$S" || true)
  if [ -z "$W" ]; then
    bad "cannot check the unrecorded-backup branch: it is not there"
  elif [ "$(count "$W" '^\s*return;')" = "0" ]; then
    ok "the unrecorded-backup branch falls through to the log cleanup"
  else
    bad "the unrecorded-backup branch returns early and leaks the provisioning log"
  fi
fi

echo "== §4  a stored destination credential is never destroyed nor echoed =="
if S=$(subj "$DEST"); then
  if has "$S" "fn reject_empty_secrets"; then
    ok "an empty secret is refused rather than written over the stored one"
  else
    bad "reject_empty_secrets is gone — an empty secret can destroy the credential again"
  fi
  M=$(count "$S" "mask_config_secrets\(&mut dest\)")
  if [ "$M" -ge 2 ]; then
    ok "create and update both mask before returning ($M)"
  else
    bad "expected >=2 masked returning paths, found $M"
  fi
  # test_connection must NOT mask — it feeds the agent the real credential.
  # Only meaningful once a masker exists; asserting its absence in a tree that
  # has no masker at all is a green about nothing.
  W=$(grep -A25 "pub async fn test_connection" <<< "$S" || true)
  if [ "$M" -lt 2 ] || [ -z "$W" ]; then
    bad "cannot check test_connection: no masker to look for"
  elif [ "$(count "$W" "mask_config_secrets")" = "0" ]; then
    ok "test_connection is deliberately unmasked"
  else
    bad "test_connection masks its row — the agent would authenticate with the sentinel"
  fi
  if has "$S" "fn carry_unmentioned_keys"; then
    ok "a stored key the form does not send survives an edit"
  else
    bad "carry_unmentioned_keys is gone — an unrelated edit erases key_path again"
  fi
fi

echo "== §5  a policy can be edited, and an omitted field is not a reset =="
if S=$(subj "$ORCH"); then
  if has "$S" "struct UpdatePolicyRequest"; then
    ok "update has its own request type"
  else
    bad "update shares CreatePolicyRequest again — absent and default collapse"
  fi
  # The three columns the UPDATE used to assign unconditionally.
  for col in server_id destination_id retention_count; do
    if has "$S" "req\.$col\.unwrap_or\(cur_"; then
      ok "omitted keeps its current value: $col"
    else
      bad "reset-on-omission is back for $col"
    fi
  done
  # Explicit null must still clear a destination, or local-only is unreachable.
  if has "$S" "de_double_option"; then
    ok "explicit null is distinguishable from omitted"
  else
    bad "explicit null and omitted are the same value again"
  fi
fi
if S=$(subj "$UI"); then
  if has "$S" 'api\.put\(.\/backup-orchestrator\/policies\/'; then
    ok "the policy edit form calls PUT"
  else
    bad "nothing in the browser calls the policy PUT — it is create-once again"
  fi
  if has "$S" "policies/protect-all"; then
    ok "Protect Everything has a caller"
  else
    bad "Protect Everything is dead again while the guide still says to click it"
  fi
fi
if S=$(subj "$PREREQ"); then
  # The advice must point at the screen that can carry it out.
  W=$(grep -A4 '"Choose a destination"' <<< "$S" || true)
  if [ "$(count "$W" "tab=policies")" != "0" ]; then
    ok "'Choose a destination' links to where a policy is edited"
  else
    bad "'Choose a destination' points away from the only control that can do it"
  fi
fi

echo "== §6  Encrypt promises only what the agent can deliver =="
if S=$(subj "$EXEC"); then
  if has "$S" "encryption applies to database dumps only"; then
    ok "the executor says so when a policy encrypts and also covers sites/volumes"
  else
    bad "the executor is silent about unencrypted site/volume archives again"
  fi
fi
if S=$(subj "$UI"); then
  if has "$S" "Encrypt DB dumps"; then
    ok "the checkbox names its scope"
  else
    bad "the checkbox promises unqualified encryption again"
  fi
fi
# The guide is prose, so it is read raw on purpose.
if grep -q "database dumps only" "$GUIDE"; then
  ok "the guide scopes encryption to database dumps"
else
  bad "the guide promises unqualified AES-256 again"
fi
# If a future change gives site/volume archives a real encryption path, THIS is
# the arm that should go red first, so the copy above gets revisited with it.
ENC_FILES=$(grep -rl "encrypt_file" panel/agent/src/ 2>/dev/null | wc -l)
if [ "$ENC_FILES" -le 2 ]; then
  ok "agent-side encryption is still database-only ($ENC_FILES file(s))"
else
  bad "agent encryption spread to $ENC_FILES files — re-check the Encrypt copy"
fi

echo "== §7  remote retention that is not enforced is reported =="
for f in "$EXEC" "$SCHED"; do
  if S=$(subj "$f"); then
    # Keyed on DESTRUCTURING, not on one discard spelling. Forbidding
    # `let _ = agent.post("/backups/prune"` was evaded in testing by
    # `let _unused = ….ok(); if false {`, which kept the message-handling code
    # in the file — and therefore kept the sibling arm green — while it could
    # never run. To read the response you have to open a block on it; no form of
    # discard does.
    W=$(grep -A12 '"/backups/prune"' <<< "$S" || true)
    if [ -z "$W" ]; then
      bad "$(basename "$f"): no prune call found at all"
    elif [ "$(count "$W" '\.await \{')" = "0" ]; then
      bad "$(basename "$f"): the prune response is not destructured — it cannot be read"
    elif [ "$(count "$W" 'let _|\.ok\(\)|if false')" != "0" ]; then
      bad "$(basename "$f"): the prune response is discarded or its handling is dead"
    else
      ok "$(basename "$f"): the prune response is destructured and read"
    fi
    # The message must reach a durable operator surface, not just a journal.
    MW=$(grep -A12 "emote retention was not enforced" <<< "$S" || true)
    if [ -z "$MW" ]; then
      bad "$(basename "$f"): nothing records that retention was a no-op"
    elif [ "$(count "$MW" 'log_event')" = "0" ] && [ "$(count "$W" 'log_event')" = "0" ]; then
      bad "$(basename "$f"): the unenforced-retention message never reaches system_log"
    else
      ok "$(basename "$f"): an unenforceable retention reaches system_log"
    fi
  fi
done

echo
echo "backup-truth: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
