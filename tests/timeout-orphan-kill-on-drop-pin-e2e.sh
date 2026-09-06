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
# The exact same class was already fixed, correctly, in THREE other files
# (security_scanner.rs, database.rs, migration.rs, and git_build.rs's OWN
# docker-build call three lines below its unpatched git fetch) — this suite
# originally pinned the six sites the p189 fixes missed:
#   git_build.rs  : git fetch (clone_or_pull's pull branch)
#   git_build.rs  : git clone (clone_or_pull's fresh-clone branch)
#   deploy.rs     : git fetch (its OWN, independent clone_or_pull — a second
#                   implementation of the same idea, not a call into git_build.rs)
#   deploy.rs     : git clone (same function, fresh-clone branch)
#   backups.rs    : tar czf under a 300s timeout
#   remote_backup.rs : curl to S3 under a caller-supplied timeout (s3_curl,
#                   the one runner shared by test_s3/list_s3/delete_s3/upload)
#
# s451 CORRECTION + EXTENSION: this suite's own header, above, claimed
# database_backup.rs was among the "already fixed" files. That claim was
# FALSE — a s451 dockpanel-fanout finder/skeptic pair (rotation target
# database_backup.rs, per feedback_dockpanel_audit_scope) found only
# dump_mongo (1 of 6 privileged subprocess functions) actually had
# kill_on_drop(true); dump_mysql/dump_postgres/restore_mysql/
# restore_postgres/restore_mongo did not — meaning this pin suite provided
# ZERO real coverage for those five functions despite its own text asserting
# they were covered. Now fixed (9 call sites: dump_mysql and dump_postgres
# each spawn 2 children — docker+gzip; restore_mysql and restore_postgres
# each spawn 2 — gunzip+docker; restore_mongo spawns 1 — docker) and pinned
# in section F below, function-scoped rather than whole-file-anchored: the
# gzip/gunzip Command builders are byte-identical between the mysql and
# postgres functions, so a whole-file anchor on that text would alias across
# both sites (one fixed, one regressed, and the check would still pass) —
# the exact "pin greps raw source, matches comments/duplicate-code too" trap
# this project's own memory already names.
#
# The same critic pass found an UNRELATED file with the identical bug,
# off the original menu: panel/agent/src/routes/mail.rs's mailbox_backup/
# mailbox_restore (tar czf/xzf under a 300s timeout) — invisible to every
# earlier "grep services/ for kill_on_drop" sweep because mail.rs inlines
# its subprocess logic directly in the route handler, not a service module.
# Fixed and pinned in section G below.
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
DATABASE_BACKUP_SVC="$REPO/panel/agent/src/services/database_backup.rs"
MAIL_ROUTE="$REPO/panel/agent/src/routes/mail.rs"
WORDPRESS_SVC="$REPO/panel/agent/src/services/wordpress.rs"
WORDPRESS_ROUTE="$REPO/panel/agent/src/routes/wordpress.rs"
BACKUPS_ROUTE="$REPO/panel/agent/src/routes/backups.rs"
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

# Function-scoped, chain-bounded variant: isolates ONE function's body (by sed range
# between two fn-signature markers), then within it slices from the anchor line to
# THAT chain's own next `.spawn()` call (not a fixed line count). Needed where the
# Command-builder text is byte-identical across sibling functions (dump_mysql vs
# dump_postgres's gzip_child, restore_mysql vs restore_postgres's gunzip_child) — a
# whole-file anchor there would alias across both sites and stay green even if one
# regressed. A first attempt at this scoped so on the FUNCTION but still checked with
# a fixed -A15 window, which mutation-testing caught as still-wrong: dump_postgres's
# docker_child and gzip_child chains sit only ~10 lines apart, so a 15-line window
# from docker's anchor also swallowed gzip's kill_on_drop and stayed green even with
# docker's own annotation deleted. Bounding each check to its own chain's `.spawn()`
# — the natural, code-structural end of exactly one Command builder — removes the
# window-size guess entirely.
scopedChainHasKillOnDrop() {
  local file="$1" start="$2" end="$3" anchor="$4" label="$5"
  if [ ! -f "$file" ]; then
    bad "$label ($file does not exist)"
    return
  fi
  local body chain
  body="$(sed -n "/$start/,/$end/p" "$file" 2>/dev/null)"
  if [ -z "$body" ]; then
    bad "$label (could not isolate the function body — re-scope this pin)"
    return
  fi
  chain="$(printf '%s\n' "$body" | awk -v anchor="$anchor" '
    index($0, anchor) { infound=1 }
    infound { print; if (index($0, ".spawn()")) exit }
  ')"
  if [ -z "$chain" ]; then
    bad "$label (anchor or its .spawn() not found — re-scope this pin)"
    return
  fi
  if printf '%s\n' "$chain" | grep -c 'kill_on_drop(true)' >/dev/null; then
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
echo "== F. database_backup.rs: all 9 privileged child-process builders are annotated (dump_mongo's own kill_on_drop, the pre-existing 10th site, is the reference pattern and is not re-checked here) =="
scopedChainHasKillOnDrop "$DATABASE_BACKUP_SVC" 'pub async fn dump_mysql(' 'pub async fn dump_postgres(' \
  '"mariadb-dump",' "dump_mysql's docker exec (mariadb-dump) carries kill_on_drop(true)"
scopedChainHasKillOnDrop "$DATABASE_BACKUP_SVC" 'pub async fn dump_mysql(' 'pub async fn dump_postgres(' \
  'let mut gzip_child = safe_command("gzip")' "dump_mysql's gzip carries kill_on_drop(true)"
scopedChainHasKillOnDrop "$DATABASE_BACKUP_SVC" 'pub async fn dump_postgres(' 'pub async fn dump_mongo(' \
  '"--no-owner", "--no-acl", "--clean", "--if-exists",' "dump_postgres's docker exec (pg_dump) carries kill_on_drop(true)"
scopedChainHasKillOnDrop "$DATABASE_BACKUP_SVC" 'pub async fn dump_postgres(' 'pub async fn dump_mongo(' \
  'let mut gzip_child = safe_command("gzip")' "dump_postgres's gzip carries kill_on_drop(true)"
scopedChainHasKillOnDrop "$DATABASE_BACKUP_SVC" 'pub async fn restore_mysql(' 'pub async fn restore_postgres(' \
  'let mut gunzip_child = safe_command("gunzip")' "restore_mysql's gunzip carries kill_on_drop(true)"
scopedChainHasKillOnDrop "$DATABASE_BACKUP_SVC" 'pub async fn restore_mysql(' 'pub async fn restore_postgres(' \
  '"mariadb", "-u", user, db_name,' "restore_mysql's docker exec (mariadb restore) carries kill_on_drop(true)"
scopedChainHasKillOnDrop "$DATABASE_BACKUP_SVC" 'pub async fn restore_postgres(' 'pub async fn restore_mongo(' \
  'let mut gunzip_child = safe_command("gunzip")' "restore_postgres's gunzip carries kill_on_drop(true)"
scopedChainHasKillOnDrop "$DATABASE_BACKUP_SVC" 'pub async fn restore_postgres(' 'pub async fn restore_mongo(' \
  '"psql", "-v", "ON_ERROR_STOP=1", "--single-transaction", "-U", user, "-d", db_name,' \
  "restore_postgres's docker exec (psql restore) carries kill_on_drop(true)"
scopedChainHasKillOnDrop "$DATABASE_BACKUP_SVC" 'pub async fn restore_mongo(' 'pub fn list_db_backups(' \
  '"mongorestore", "--db", db_name, "--archive", "--gzip", "--drop",' \
  "restore_mongo's docker exec (mongorestore) carries kill_on_drop(true)"

echo
echo "== G. mail.rs: the mailbox backup/restore tar children are annotated (found off-menu by the s451 completeness critic — inlined in a route handler, invisible to every services/-scoped grep for this bug class) =="
windowHasKillOnDrop "$MAIL_ROUTE" '"czf", &backup_file,' \
  "mail.rs's mailbox_backup tar czf carries kill_on_drop(true)"
windowHasKillOnDrop "$MAIL_ROUTE" '"xzf", backup_file,' \
  "mail.rs's mailbox_restore tar xzf carries kill_on_drop(true)"

echo
echo "== H. wordpress.rs: the crate's single wp-cli choke point + its own 5 pre-existing subprocess sites are all annotated (s452 dockpanel-fanout run wf_72e4e04f-373, rotation target wp_vulnerability.rs) =="
# wp_at_root's own doc comment already says every wp-cli invocation in this
# crate goes through it — which means it was ALSO the single highest-leverage
# gap: no timeout at all (so no kill_on_drop could matter until one was added
# here) protecting info/plugins/themes/update/plugin_action/theme_action AND
# every wp_vulnerability.rs entry point (scan_site/check_security/
# apply_hardening, via set_wp_constant) in one place.
windowHasKillOnDrop "$WORDPRESS_SVC" 'cmd.stdout(Stdio::piped())' \
  "wordpress.rs's wp_at_root (the crate's single wp-cli choke point) carries kill_on_drop(true)"
# install()'s two raw wp-cli calls predate wp_at_root and bypass it entirely —
# a second hand-rolled implementation, the exact shape wp_at_root's own doc
# comment warns went wrong once already, just for THIS bug class instead.
windowHasKillOnDrop "$WORDPRESS_SVC" '&format!("--dbname={db_name}")' \
  "wordpress.rs's install() wp config create carries kill_on_drop(true)"
windowHasKillOnDrop "$WORDPRESS_SVC" '&format!("--admin_password={admin_pass}")' \
  "wordpress.rs's install() wp core install carries kill_on_drop(true)"
# These three already had a tokio::time::timeout wrap (120s/120s/15s) before
# this session — the exact "timeout doesn't actually stop the thing" shape:
# the future gets dropped on expiry, but without kill_on_drop the child
# (a tar extracting into a live site's document root, or a sudo'd wp-cli
# eval) keeps running orphaned rather than being bounded by the timeout that
# looked like it would stop it.
windowHasKillOnDrop "$WORDPRESS_SVC" '"czf", &snapshot_path, "-C", "/var/www"' \
  "wordpress.rs's create_update_snapshot tar czf carries kill_on_drop(true)"
windowHasKillOnDrop "$WORDPRESS_SVC" '"xzf", snapshot_path, "-C", "/var/www"' \
  "wordpress.rs's rollback_from_snapshot tar xzf carries kill_on_drop(true)"
windowHasKillOnDrop "$WORDPRESS_SVC" '"-u", "www-data", WP_CLI, "eval"' \
  "wordpress.rs's health_check sudo wp-cli eval carries kill_on_drop(true)"

echo
echo "== J. backups.rs (services): the 4 sites the s452 fix to THIS SAME FILE (site D above) did not cover are annotated (s453 dockpanel-fanout run wf_a5d044b9-fa0, rotation target services/backups.rs + routes/backups.rs) =="
# extract_payload / restore_inner / list_backup_files / restore_single_file all
# wrap tar in tokio::time::timeout without kill_on_drop — the D-above fix only
# ever covered create_backup's OWN tar czf, not these four siblings in the
# same file.
windowHasKillOnDrop "$BACKUPS_SVC" '"xzf", archive,' \
  "backups.rs's extract_payload tar xzf carries kill_on_drop(true)"
windowHasKillOnDrop "$BACKUPS_SVC" '"--exclude", PAYLOAD_DIR,' \
  "backups.rs's restore_inner tar xzf (into the live webroot) carries kill_on_drop(true)"
windowHasKillOnDrop "$BACKUPS_SVC" '"tzf", filepath_str' \
  "backups.rs's list_backup_files tar tzf carries kill_on_drop(true)"
windowHasKillOnDrop "$BACKUPS_SVC" '"xzf", backup_str,' \
  "backups.rs's restore_single_file tar xzf (into the live webroot) carries kill_on_drop(true)"

echo
echo "== K. routes/backups.rs: the post-restore chown no longer hands the application .git (setup-critic finding, s453 — routes/backups.rs:340 was 1 of 8 .git-blind recursive site-root chowns tracked since p108/s377; the other 7 remain open, filed as their own future rotation target) =="
if grep -q 'fn chown_restored_tree' "$BACKUPS_ROUTE" 2>/dev/null \
  && { grep -A20 'fn chown_restored_tree' "$BACKUPS_ROUTE" 2>/dev/null | grep -c '\.git' >/dev/null; }; then
  ok "routes/backups.rs defines chown_restored_tree, and it names .git explicitly (skips it in the www-data chown, re-secures it to root)"
else
  bad "routes/backups.rs's chown_restored_tree is missing or no longer mentions .git — re-scope this pin"
fi
if grep -q 'chown_restored_tree(&format!("/var/www/{domain}"))' "$BACKUPS_ROUTE" 2>/dev/null; then
  ok "restore calls chown_restored_tree instead of a blanket chown -R"
else
  bad "restore no longer visibly calls chown_restored_tree — re-scope this pin"
fi

echo
echo "== I. reachability: git legs, the database-backup dump/restore entrypoints, the mailbox tar entrypoints, and the wordpress.rs subprocess sites all sit behind live, unauthenticated-beyond-the-flat-token HTTP endpoints =="
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
if grep -q 'dump_mysql\|dump_postgres\|dump_mongo\|restore_mysql\|restore_postgres\|restore_mongo' "$REPO/panel/agent/src/routes/database_backup.rs" 2>/dev/null; then
  ok "database_backup.rs's route table still reaches the dump/restore functions"
else
  bad "database_backup.rs's route table no longer visibly reaches the dump/restore functions — re-scope this pin"
fi
if grep -q '"/mail/backup"' "$MAIL_ROUTE" 2>/dev/null && grep -q '"/mail/restore"' "$MAIL_ROUTE" 2>/dev/null; then
  ok "mail.rs's route table still registers /mail/backup and /mail/restore"
else
  bad "mail.rs's route table no longer visibly registers /mail/backup or /mail/restore — re-scope this pin"
fi
if grep -q '"/wordpress/{domain}/install"' "$WORDPRESS_ROUTE" 2>/dev/null \
  && grep -q '"/wordpress/{domain}/update-with-rollback"' "$WORDPRESS_ROUTE" 2>/dev/null \
  && grep -q '"/wordpress/{domain}/harden"' "$WORDPRESS_ROUTE" 2>/dev/null; then
  ok "wordpress.rs's route table still registers install, update-with-rollback, and harden (the routes that reach wp_at_root, the two install() calls, and both snapshot/rollback tars)"
else
  bad "wordpress.rs's route table no longer visibly registers install, update-with-rollback, or harden — re-scope this pin"
fi
if grep -q '"/backups/{domain}/browse/{filename}"' "$BACKUPS_ROUTE" 2>/dev/null \
  && grep -q '"/backups/{domain}/restore-file/{filename}"' "$BACKUPS_ROUTE" 2>/dev/null; then
  ok "routes/backups.rs's route table still registers browse and restore-file (the routes that reach every site fixed in section J/K)"
else
  bad "routes/backups.rs's route table no longer visibly registers browse or restore-file — re-scope this pin"
fi

echo
printf 'passed %d, failed %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
