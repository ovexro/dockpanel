#!/usr/bin/env bash
# Regression pins for the s362 deploy-approval repair (v2.115.0).
#
# The two-person rule for protected deployments shipped COMPLETE on the backend —
# list / approve / reject, each carefully scoped so a second tenant's admin cannot
# sign off on hardware they do not run — and no screen in the SPA ever called any
# of them. The operator could turn the control on and then had no way to finish a
# deploy except two hand-written curls against ids nothing displayed.
#
# Nothing here needs a running panel or a database: every arm is a shape
# assertion over the source that the reachability depends on. Section D's SQL
# behaviour was proved separately by executing the migration against a real
# Postgres inside a rolled-back transaction (planted 3 pending rows for one
# deployment, watched it collapse to the oldest, watched the new index refuse a
# second) — that half cannot run here, and this file does not pretend it does.
#
#   run: bash tests/deploy-approval-reachable-pin-e2e.sh
set -u

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
UI="$REPO/panel/frontend/src/pages/GitDeploys.tsx"
API="$REPO/panel/backend/src/routes/git_deploys.rs"
ROUTES="$REPO/panel/backend/src/routes/mod.rs"
MIG="$REPO/panel/backend/migrations/20260815000000_deploy_approvals_one_pending.sql"
PASS=0; FAIL=0
ok()   { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (expected [$3], got [$2])"; fi; }

TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT

for f in "$UI" "$API" "$ROUTES" "$MIG"; do
  [ -f "$f" ] || { printf '  \033[0;31m✗\033[0m missing subject: %s\n' "$f"; exit 1; }
done

# The `startDeploy` body, bounded on its own closing brace rather than a fixed
# -A window: a comment inserted above the branch must not push the subject out of
# the window and report a false green (that trap has fired in this repo before).
awk '/^  const startDeploy = async/{f=1} f{print} f&&/^  };$/{exit}' "$UI" > "$TMP/startDeploy"
check "the shared deploy call site was extracted (a fixed window would lie)" \
      "$([ -s "$TMP/startDeploy" ] && echo yes || echo no)" "yes"

# Same discipline on the backend: `deploy()` contains BOTH the approval INSERT and
# an `if filed` that also appears in the response body, so a file-wide count of
# either would be measuring the wrong occurrences.
awk '/^pub async fn deploy\(/{f=1} f{print} f&&/^}$/{exit}' "$API" > "$TMP/deploy_fn"
check "the deploy() body was extracted" \
      "$([ -s "$TMP/deploy_fn" ] && echo yes || echo no)" "yes"
awk '/^        if filed \{$/{f=1} f{print} f&&/^        \}$/{exit}' "$TMP/deploy_fn" > "$TMP/filed_block"
check "the new-request gate block was extracted" \
      "$([ -s "$TMP/filed_block" ] && echo yes || echo no)" "yes"

echo "── A: the approval endpoints are REACHABLE from the panel ──"
# A hardened handler nobody can call is not a control. A1 is the positive control
# for A2: it proves the backend half exists, so a red A2 means the SPA, not a typo.
# Anchored on the leading whitespace AND the handler, not just on the path: an
# unanchored count is satisfied by a commented-out registration, and this arm is
# the positive control the next one leans on.
check "all three approval routes are registered" \
      "$(grep -cE '^ +\.route\("/api/deploy-approvals[^\"]*", (get|post)\(git_deploys::' "$ROUTES")" "3"
check "the panel LISTS pending approvals" \
      "$(grep -c 'api.get<DeployApproval\[\]>("/deploy-approvals")' "$UI")" "1"
check "the panel POSTs approve/reject" \
      "$(grep -c '/deploy-approvals/\${approvalId}/\${action}' "$UI")" "1"
# Calling an endpoint is not the same as an operator having a control that calls
# it: these two are the buttons.
check "an Approve button exists"  "$(grep -c 'type: "approve-request"' "$UI")" "1"
# Two controls resolve through reject: a second admin's Reject, and the
# requester's own Withdraw. Both must exist.
check "Reject and Withdraw both exist" "$(grep -c 'type: "reject-request"' "$UI")" "2"
check "Approve is wired to the resolver" "$(grep -c 'resolveApproval(id, "approve")' "$UI")" "1"
check "Reject is wired to the resolver"  "$(grep -c 'resolveApproval(id, "reject")' "$UI")" "1"
check "a request you filed yourself offers no Approve button" \
      "$(grep -c 'a.requested_by === user.id' "$UI")" "1"
# The declaration alone is not the guard — inverting the ternary keeps the string
# byte-identical while showing Approve on exactly the rows the server refuses.
# Pin the SHAPE, so an inversion is a text change the arm can see.
check "  the own-branch is the one that hides Approve" "$(grep -c '{own ? (' "$UI")" "1"
check "  the guard is not inverted"                    "$(grep -c '{!own' "$UI")" "0"
# ...and the requester is not trapped: withdrawal is the only exit on a
# single-admin install, where nobody else can ever approve.
check "your own request can be withdrawn" \
      "$(grep -c 'Withdraw your own deploy request' "$UI")" "1"
# Bounded on the mount effect's OWN terminator. Unbounded, this greened on any
# loadApprovals() anywhere below — including the three that only run AFTER an
# action, which is precisely the regression it exists to catch.
check "approvals load with the PAGE, not with a selected row" \
      "$(awk '/^  useEffect\(\(\) => \{$/{f=1} f&&/^  \}, \[\]\);$/{exit} f&&/loadApprovals\(\);/{print "hit"; exit}' "$UI")" "hit"

echo
echo "── B: the 202 that carries no deploy_id must not be read as success ──"
# The protected branch answers 202 with a reason and no id. The client read only
# the id, so the button spun and the server's own sentence was discarded.
check "the deploy response type admits an absent id" \
      "$(grep -c 'deploy_id?: string' "$TMP/startDeploy")" "1"
check "the no-id branch clears the spinner" \
      "$(grep -c 'setDeploying(null);' "$TMP/startDeploy")" "2"
check "the no-id branch surfaces the SERVER's sentence" \
      "$(grep -c 'result.message' "$TMP/startDeploy")" "1"
check "the no-id branch refreshes the approvals list" \
      "$(grep -c 'loadApprovals();' "$TMP/startDeploy")" "1"
# Both Deploy buttons must go through it — the second call site is the one that
# fires when the client's list is stale because protection was just switched on.
check "both deploy call sites route through the shared branch" \
      "$(grep -c 'await startDeploy(id);' "$UI")" "2"

echo
echo "── C: the copy describes the control that actually exists ──"
check "the checkbox names the second administrator" \
      "$(grep -c "Require another admin's approval before deploy" "$UI")" "1"
check "the help text names the approval queue" \
      "$(grep -c 'must approve it under' "$UI")" "1"
# The three paths that ignore deploy_protected are named rather than hidden:
# a control whose scope is undisclosed reads as broader than it is.
check "the copy discloses the paths the gate does NOT cover" \
      "$(grep -c 'not covered' "$UI")" "1"
# ABSENCE arm + its positive control. Without the control, a malformed pattern
# and a genuinely absent string are the same green.
printf 'Shows a confirmation prompt before deploying\n' > "$TMP/oldcopy.fixture"
check "  [control] the pattern still matches copy of the old shape" \
      "$(grep -c 'confirmation prompt' "$TMP/oldcopy.fixture")" "1"
check "no copy still calls this a confirmation prompt" \
      "$(grep -c 'confirmation prompt' "$UI")" "0"

echo
echo "── D: one pending request per deployment, not one per click ──"
check "the approval INSERT cannot write a duplicate" \
      "$(grep -c 'ON CONFLICT DO NOTHING' "$TMP/deploy_fn")" "1"
check "a repeat click does not re-notify every admin" \
      "$(grep -c 'notify_panel' "$TMP/filed_block")" "1"
check "the requester is told which case they are in" \
      "$(grep -c 'already has a request waiting' "$API")" "1"
check "the migration creates the constraint" \
      "$(grep -c 'CREATE UNIQUE INDEX IF NOT EXISTS idx_deploy_approvals_one_pending' "$MIG")" "1"
# PARTIAL, or the first request ever filed would make that deployment
# permanently un-requestable — the resolved rows are the audit trail and stay.
check "the constraint is scoped to PENDING rows" \
      "$(grep -c "ON deploy_approvals (deploy_id) WHERE status = 'pending'" "$MIG")" "1"
# Order matters: a bare CREATE UNIQUE INDEX against a table that already holds
# duplicates FAILS, and a failed migration stops the panel from starting.
DEL_LINE=$(grep -n '^DELETE FROM deploy_approvals a' "$MIG" | cut -d: -f1)
IDX_LINE=$(grep -n '^CREATE UNIQUE INDEX' "$MIG" | cut -d: -f1)
check "duplicates are collapsed BEFORE the index is created" \
      "$([ -n "$DEL_LINE" ] && [ -n "$IDX_LINE" ] && [ "$DEL_LINE" -lt "$IDX_LINE" ] && echo yes || echo no)" "yes"
# It must keep the OLDEST — that is the request an approver may already have been
# shown, and the newer rows are the accidental ones.
check "the collapse keeps the oldest request" \
      "$(grep -c '(b.created_at, b.id) < (a.created_at, a.id)' "$MIG")" "1"

echo
echo "── E: the payload contract, pinned at BOTH ends ──"
# `api.get<DeployApproval[]>` is a bare cast: `request<T>` validates nothing, so
# TypeScript never sees a server-side rename. The map builds its keys by hand and
# already renames one on the way out (`g.name` → `deploy_name`), so the two ends
# agree by convention and nothing checks it. A rename shows up as an empty row in
# front of an administrator about to authorise a production deploy — or, for
# `requested_by`, as `undefined === user.id`, which puts the Approve button back
# on your own request.
awk '/^pub async fn list_approvals\(/{f=1} f{print} f&&/^}$/{exit}' "$API" > "$TMP/list_fn"
grep -oE '^ +"[a-z_]+": ' "$TMP/list_fn" | tr -d '": ' | sort > "$TMP/srv.keys"
awk '/^interface DeployApproval \{$/{f=1;next} f&&/^}$/{exit} f{print}' "$UI" \
  | sed -E 's/^ *([a-z_]+)\??:.*/\1/' | sort > "$TMP/cli.keys"
# Both sides non-empty FIRST — otherwise two empty files diff clean and this whole
# section is a green that examined nothing.
check "the server's response map was extracted" "$(wc -l < "$TMP/srv.keys" | tr -d ' ')" "9"
check "the client's interface was extracted"    "$(wc -l < "$TMP/cli.keys" | tr -d ' ')" "9"
check "every key the panel reads is a key the server sends" \
      "$(diff -q "$TMP/srv.keys" "$TMP/cli.keys" >/dev/null && echo match || echo drift)" "match"

echo
echo "── F: a request whose flag was switched off must not stay approvable ──"
# Before the panel could list them, a pending row left behind by a cleared flag
# was inert. It is not inert now, so three places have to agree: the list filters
# on it, approve re-reads it, and turning the flag off resolves what is waiting.
check "the list only offers requests that are still protected" \
      "$(grep -c "AND g.deploy_protected AND EXISTS" "$API")" "1"
check "approve re-reads the flag rather than trusting the row" \
      "$(grep -c 'if !config.deploy_protected {' "$API")" "1"
check "clearing the flag resolves what was waiting on it" \
      "$(grep -c "SET status = 'cancelled', resolved_at = NOW()" "$API")" "1"
# FIVE doors replace the running container — deploy, webhook, trigger_deploy_task,
# approve and rollback. This read FOUR until s364: approve was fixed at s362, and
# rollback was still writing the status unconditionally and merely warning on
# error, so a rollback could start on top of a running build and each would swap
# the container out from under the other.
check "all five container-replacing doors take the atomic build lock" \
      "$(grep -c "status IS DISTINCT FROM 'building'" "$API")" "5"
check "the approver's name surviving their deletion is the FK's job" \
      "$(grep -c 'ON DELETE SET NULL' "$MIG")" "1"

echo
echo "── G: disarming the requirement leaves a trace that cannot be edited ──"
# The flag was the one edit on this handler that removes a control while leaving
# no record: the row simply changed. Without this the feature is a preference,
# because nothing could afterwards say when it stopped applying or who did it.
# ⚠ The closing quote is part of the pattern. Without it a mutation renaming the
# event to `git_deploy.protection_changedX` left this arm green — the mutated
# spelling still contains the shorter one. Same defect as C1 in the s364 suite,
# in both cases found by the mutation battery rather than by review.
check "clearing or setting the flag is written to the audit log" \
      "$(grep -c '"git_deploy.protection_changed"' "$API")" "1"
# It goes to the immutable table specifically. The activity feed is editable
# state; the point of the record is that the account disarming its own guard
# cannot then remove the evidence.
check "the record goes to the immutable log, not the activity feed" \
      "$(grep -c 'security_hardening::audit_log' "$API")" "1"
# A COALESCE write cannot tell "set to true" from "left true", so the previous
# value has to be read for the comparison to mean anything. Without this the
# log would fire on every unrelated field edit.
check "the previous value is fetched so an unchanged flag logs nothing" \
      "$(grep -c 'deploy_protected != was_protected' "$API")" "1"
# v2.168.0: this query grew d.server_id/d.ssl_email/d.tls_mode/c.alias (the
# TLS-mode resolution `update()` now needs) and a LEFT JOIN, so the literal
# text changed — the control now asserts what it always meant: deploy_protected
# is still fetched alongside the pre-existing current values, in this one query.
check "  [control] the flag is still selected alongside the other current values" \
      "$(grep -c 'SELECT d\.domain, d\.deploy_cron, d\.deploy_protected,' "$API")" "1"

echo
echo "══ $PASS passed, $FAIL failed ══"
[ "$FAIL" -eq 0 ]
