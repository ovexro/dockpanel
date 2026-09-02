#!/usr/bin/env bash
# Regression pins for the "timeout doesn't actually stop the thing"
# kill_on_drop gap found in the s450 pkg.rs fan-out's completeness-critic
# stage (project_dockpanel_tech_debt, ledger p189 — dockpanel-fanout run
# wf_2ad3e1e8-ab5).
#
# safe_command() (panel/agent/src/safe_cmd.rs:36) is a bare
# tokio::process::Command — it sets no kill_on_drop. Six call sites across
# four files wrapped its .output() in tokio::time::timeout(...) without ever
# calling .kill_on_drop(true) on the Command first: when the timeout fires
# and the future is dropped, tokio's documented default leaves the child
# process running, orphaned, reparented to PID 1 — NOT bounded by the timeout
# that looked like it would stop it. Reproduced live during the audit: an
# orphaned `git fetch`/`git clone` leaves `.git/index.lock`, and the very
# next deploy/clone attempt on the same site — including an operator's
# natural immediate retry — fails instantly with a generic, non-actionable
# "Another git process seems to be running" error instead of succeeding or
# reporting the true cause.
#
# The exact same class was already fixed, correctly, in FIVE other files
# (security_scanner.rs, database.rs, database_backup.rs, migration.rs, and
# git_build.rs's OWN docker-build call three lines below its unpatched git
# fetch) — this suite pins the six sites the earlier fixes missed:
#   git_build.rs  : git fetch (clone_or_pull's pull branch)
#   git_build.rs  : git clone (clone_or_pull's fresh-clone branch)
#   deploy.rs     : git fetch (its OWN, independent clone_or_pull — a second
#                   implementation of the same idea, not a call into git_build.rs)
#   deploy.rs     : git clone (same function, fresh-clone branch)
#   backups.rs    : tar czf under a 300s timeout
#   remote_backup.rs : curl to S3 under a caller-supplied timeout (s3_curl,
#                   the one runner shared by test_s3/list_s3/delete_s3/upload)
#
# pkg.rs/safe_cmd.rs's OWN systemd-run-backed timeout (a harder, separate
# problem — killing the LOCAL waiter does not stop the REMOTE transient unit,
# proven live during the same audit) is explicitly OUT of scope here — see
# project_dockpanel_tech_debt ledger p189 for why that one is a sized carry,
# not a same-session fix.
#
# These are SOURCE pins and need no running panel.
#   run: bash tests/timeout-orphan-kill-on-drop-pin-e2e.sh
set -u

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GIT_BUILD_SVC="$REPO/panel/agent/src/services/git_build.rs"
DEPLOY_SVC="$REPO/panel/agent/src/services/deploy.rs"
BACKUPS_SVC="$REPO/panel/agent/src/services/backups.rs"
REMOTE_BACKUP_SVC="$REPO/panel/agent/src/services/remote_backup.rs"
SAFE_CMD="$REPO/panel/agent/src/safe_cmd.rs"

PASS=0
FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

# Position-scoped: kill_on_drop(true) must appear within the ~15 lines after
# the command-building anchor, not merely SOMEWHERE in the file — a raw
# whole-file count (what the pre-existing docker-build pin in
# git-deploy-pre-build-rce-pin-e2e.sh uses) would stay green even if every one
# of THESE sites regressed, as long as the one unrelated docker-build site
# elsewhere in the same file kept its annotation.
windowHasKillOnDrop() {
  local file="$1" anchor="$2" label="$3"
  if [ ! -f "$file" ]; then
    bad "$label ($file does not exist)"
    return
  fi
  # Counting grep, not -q: a boolean grep piped from a producer quits at its
  # first match and can die of SIGPIPE mid-write, which under `set -o
  # pipefail` reads a successful match as a failure (s335, see codeNot() in
  # git-deploy-pre-build-rce-pin-e2e.sh for the same fix). Redirect the count
  # away instead — the call site's pass/fail meaning is unchanged.
  if grep -A15 -F -- "$anchor" "$file" 2>/dev/null | grep -c 'kill_on_drop(true)' >/dev/null; then
    ok "$label"
  else
    bad "$label"
  fi
}

echo "== A. safe_command() itself still sets no kill_on_drop (the premise every site below depends on) =="
if grep -A10 'pub fn safe_command(binary: &str)' "$SAFE_CMD" 2>/dev/null | grep -c 'kill_on_drop' >/dev/null; then
  bad "safe_command() now sets kill_on_drop itself — every per-call-site annotation below would be redundant, not wrong, but this suite's framing needs updating"
else
  ok "safe_command() still returns a bare Command with no kill_on_drop — per-call-site annotation is still the live mechanism"
fi

echo
echo "== B. git_build.rs clone_or_pull: both network legs are annotated =="
windowHasKillOnDrop "$GIT_BUILD_SVC" '"-C", &repo_dir, "fetch", "origin", branch' \
  "git_build.rs's git fetch (pull branch) carries kill_on_drop(true)"
windowHasKillOnDrop "$GIT_BUILD_SVC" '"clone", "--branch", branch, "--single-branch", "--depth", "50",' \
  "git_build.rs's git clone (fresh-clone branch) carries kill_on_drop(true)"

echo
echo "== C. deploy.rs's OWN, independent clone_or_pull: both network legs are annotated =="
windowHasKillOnDrop "$DEPLOY_SVC" '"-C", &site_dir, "fetch", "origin", branch' \
  "deploy.rs's git fetch (pull branch) carries kill_on_drop(true)"
windowHasKillOnDrop "$DEPLOY_SVC" '"clone", "--branch", branch, "--single-branch", "--depth", "50", repo_url, &staging' \
  "deploy.rs's git clone (fresh-clone branch) carries kill_on_drop(true)"

echo
echo "== D. backups.rs: the staged tar under its 300s timeout is annotated =="
windowHasKillOnDrop "$BACKUPS_SVC" 'safe_command("tar").args(&tar_args)' \
  "backups.rs's tar czf carries kill_on_drop(true)"

echo
echo "== E. remote_backup.rs: s3_curl (the one runner behind test_s3/list_s3/delete_s3/upload) is annotated =="
windowHasKillOnDrop "$REMOTE_BACKUP_SVC" 'safe_command("curl").args(&full)' \
  "remote_backup.rs's s3_curl carries kill_on_drop(true)"

echo
echo "== F. reachability: both git legs sit behind live, unauthenticated-beyond-the-flat-token HTTP endpoints =="
# Not a security claim on their own (this suite is a reliability/operational-
# honesty pin, matching how the finding itself was framed) — just confirming
# the routes that reach the fixed functions still exist, so this suite does
# not silently stop meaning anything if a route gets renamed out from under it.
if grep -q '"/git/clone"\|clone_or_pull' "$REPO/panel/agent/src/routes/git_build.rs" 2>/dev/null; then
  ok "git_build.rs's route table still reaches clone_or_pull"
else
  bad "git_build.rs's route table no longer visibly reaches clone_or_pull — re-scope this pin"
fi
if grep -q 'clone_or_pull' "$REPO/panel/agent/src/routes/deploy.rs" 2>/dev/null; then
  ok "deploy.rs's route table still reaches its own clone_or_pull"
else
  bad "deploy.rs's route table no longer visibly reaches clone_or_pull — re-scope this pin"
fi

echo
printf 'passed %d, failed %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
