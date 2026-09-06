#!/usr/bin/env bash
# Regression pins for the s456 dockpanel-fanout run over the '.git-blind
# recursive chown' bug class (workflow wf_76c47b26-e5f; project_dockpanel_tech_debt
# ledger p194's own item 14 — "Recommend its own dedicated dockpanel-fanout rep",
# carried since s377/project_dockpanel_tech_debt_p108).
#
#   G1  services/app_process.rs::create_app_service handed a git-deployed
#       Node/Python app's whole site directory to www-data, `.git` included —
#       the app can then write .git/config or .git/hooks/, which
#       deploy.rs::clone_or_pull runs as root on the next deploy.
#   G2  services/staging.rs::clone_files and G3 ::sync_files — same shape,
#       an rsync'd copy of a (possibly git-deployed) site handed over whole.
#   G4  routes/nginx.rs::clone_site — a second, independent implementation of
#       the same clone-a-site feature as staging.rs, same shape.
#   G5  services/migration.rs::import_site_files — same shape for a
#       node/proxy/python import; harmless no-op for php/static (dest nests
#       under `public/`, one level below where clone_or_pull's `.git` lives),
#       fixed uniformly rather than conditioned on runtime.
#   G6  services/cms.rs::chown_site — the shared helper behind 5 CMS
#       installers (Laravel/Drupal/Joomla/Symfony/CodeIgniter); prospective
#       (no caller has a `.git` present at call time today) but the directory
#       becomes www-data-owned afterward, so a future app compromise could
#       plant one.
#
# Completeness-critic finding, off the original topic menu, REALIZED not
# prospective (the mirror-image bug: missing chown, not over-broad chown):
#   G7  routes/backups.rs::restore — the tar-based restore path
#       (services/backups.rs::restore_inner, `--no-same-owner
#       --no-same-permissions`) left the whole restored site root:root,
#       unwritable by the www-data-running app, on DockPanel's flagship
#       documented "Restore a Backup" feature. The git-aware
#       chown_restored_tree helper existed in the same file but was wired
#       only to the separate opt-in Restic restore path.
#   G8  routes/backups.rs::restore_file — same gap, single-file restore path.
#
# Pure source analysis; no box, no network, no build.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

# Strip comments and the test module, so a pin can never be satisfied (or
# tripped) by prose describing the very thing it forbids.
#
# NOT the naive "sed-quit-at-first-#[cfg(test)]" shape other suites in this
# tree use — services/backups.rs has #[cfg(test)]-attributed function
# VARIANTS (test-safe path resolvers) at lines 23-29, ~650 lines before its
# real `#[cfg(test)] mod db_payload_tests` at line 930. That is exactly the
# class reference_dockpanel_ops_p7 documents (s447: "the trigger is ANY
# #[cfg(test)] token, not just a mod tests block") — quitting at the first
# token would blank restore_inner (line 680) and everything after it. Only
# exit at a #[cfg(test)] line immediately followed by `mod ` — a real test
# module, not a cfg-gated function/const variant.
code() {
  awk '
    held == 1 {
      if ($0 ~ /^mod /) { exit }
      print heldline
      held = 0
    }
    /^#\[cfg\(test\)\]$/ { held = 1; heldline = $0; next }
    { print }
  ' "$1" | grep -vE '^[[:space:]]*(///|//!|//|\*|/\*)'
}
has()   { [ -n "$(code "$1" | grep -F -- "$2")" ]; }
hasre() { [ -n "$(code "$1" | grep -E -- "$2")" ]; }

# One named function's body, comment-stripped. Never pipe this into `grep -q`
# under pipefail — see security-firewalld-ssh-include-pin-e2e.sh's header.
fnbody()    { code "$1" | awk "/$2/,/^}/"; }
bodyhas()   { [ -n "$(fnbody "$1" "$2" | grep -F -- "$3")" ]; }
bodyhasre() { [ -n "$(fnbody "$1" "$2" | grep -E -- "$3")" ]; }

APP_PROCESS=panel/agent/src/services/app_process.rs
STAGING=panel/agent/src/services/staging.rs
NGINX_ROUTE=panel/agent/src/routes/nginx.rs
MIGRATION=panel/agent/src/services/migration.rs
CMS=panel/agent/src/services/cms.rs
BACKUPS_ROUTE=panel/agent/src/routes/backups.rs
BACKUPS_SVC=panel/agent/src/services/backups.rs
DEPLOY=panel/agent/src/services/deploy.rs

for f in "$APP_PROCESS" "$STAGING" "$NGINX_ROUTE" "$MIGRATION" "$CMS" "$BACKUPS_ROUTE" "$BACKUPS_SVC" "$DEPLOY"; do
  [ -f "$f" ] || { echo "missing source file: $f"; exit 1; }
done

# Every UPHELD site's fix is the same shape: skip `.git` in the main chown
# loop, then re-secure `.git` to root:root + go-rwx. One helper per site
# keeps the six checks below from being six copies of the same four lines.
assert_split_chown() {
  local file="$1" fn="$2" label="$3"
  if bodyhasre "$file" "$fn" 'OsStr::new\(\"\.git\"\)'; then
    ok "$label: the chown loop skips .git"
  else
    bad "$label: no .git skip found in $fn — the split-chown fix may be gone"
  fi
  if bodyhasre "$file" "$fn" 'chown.*args.*root:root.*git_dir|chown.*"root:root".*&git_dir'; then
    ok "$label: .git is re-secured to root:root"
  else
    bad "$label: .git is no longer re-secured to root:root in $fn"
  fi
  if bodyhas "$file" "$fn" "go-rwx"; then
    ok "$label: .git's mode is locked down (go-rwx) after re-securing ownership"
  else
    bad "$label: .git is no longer chmod'd go-rwx in $fn"
  fi
}

echo "── 1. G1: app_process.rs::create_app_service — .git-aware chown ──"
assert_split_chown "$APP_PROCESS" "fn create_app_service" "create_app_service"
if bodyhas "$APP_PROCESS" "fn create_app_service" "www-data:www-data"; then
  ok "create_app_service still chowns the rest of the tree to www-data"
else
  bad "create_app_service no longer chowns to www-data at all"
fi

echo
echo "── 2. G2/G3: staging.rs::clone_files / sync_files — .git-aware chown ──"
assert_split_chown "$STAGING" "fn clone_files" "clone_files"
assert_split_chown "$STAGING" "fn sync_files" "sync_files"

echo
echo "── 3. G4: routes/nginx.rs::clone_site — .git-aware chown ──"
# s459: clone_site no longer reimplements this inline — it now delegates the
# whole copy+chown step to staging.rs::clone_files (consolidating the
# duplicate implementation this file's own header already flagged as "a
# second, independent implementation of the same clone-a-site feature", see
# pkg-rs-and-clone-dedup-hardening-pin-e2e.sh for the dedup's own pins). The
# .git-aware split-chown invariant itself is still fully covered — by G2
# above, against the function that now actually performs the work. What's
# left to pin here is that clone_site still gets there via real delegation,
# not a silent regression to some other, unaudited shape.
if bodyhas "$NGINX_ROUTE" "fn clone_site" "services::staging::clone_files("; then
  ok "clone_site: delegates to staging::clone_files (the .git-aware chown itself is pinned via G2 above, against the function that now performs it)"
else
  bad "clone_site: no longer delegates to staging::clone_files — either the .git-aware fix was removed or it moved to an unaudited third shape; re-verify by hand"
fi

echo
echo "── 4. G5: migration.rs::import_site_files — .git-aware chown, applied uniformly ──"
assert_split_chown "$MIGRATION" "fn import_site_files" "import_site_files"

echo
echo "── 5. G6: cms.rs::chown_site — one shared-helper fix covers all 5 installers ──"
assert_split_chown "$CMS" "fn chown_site" "chown_site"
for site in install_laravel install_drupal install_joomla install_symfony install_codeigniter; do
  if bodyhas "$CMS" "fn $site" "chown_site("; then
    ok "$site still routes ownership through the shared chown_site helper"
  else
    bad "$site no longer calls chown_site — it may have its own unguarded chown now"
  fi
done

echo
echo "── 6. REFUTED sites: confirmed no-ops must stay untouched (negative controls) ──"
# nginx.rs's two fastcgi-cache-dir chowns and its single-file .env chown are
# none of them recursive against a site tree — a .git guard here would be a
# tell that someone "fixed" something that was never broken. Assert the
# ORIGINAL one-line chown shape survives unmodified.
if hasre "$NGINX_ROUTE" 'safe_command\("chown"\)\s*$|safe_command_sync\("chown"\)'; then
  ok "nginx.rs still has plain (non-git-aware) chown calls for the cache-dir/.env sites"
else
  bad "nginx.rs's non-site-dir chown calls are gone entirely — re-verify site 4/6/7 weren't touched by mistake"
fi
if bodyhasre "$APP_PROCESS" "fn create_app_service" 'OsStr::new\(\"\.git\"\)'; then :; fi # already asserted above
# diagnostics.rs's create-root fix only ever runs against a directory it just
# created empty — it should have NO .git-skip logic (there is nothing to skip).
DIAG=panel/agent/src/services/diagnostics.rs
if [ -f "$DIAG" ]; then
  if bodyhasre "$DIAG" 'fn.*create.root|"create-root"' 'OsStr::new\(\"\.git\"\)'; then
    bad "diagnostics.rs's create-root fix grew a .git guard — it never needed one (always a fresh empty dir)"
  else
    ok "diagnostics.rs's create-root fix correctly has no .git guard (REFUTED site, left as-is)"
  fi
fi

echo
echo "── 7. G7/G8: routes/backups.rs — the plain restore paths now re-secure ownership ──"
if bodyhas "$BACKUPS_ROUTE" "^async fn restore\(" "chown_restored_tree("; then
  ok "restore() now calls chown_restored_tree after a successful file restore"
else
  bad "restore() no longer calls chown_restored_tree — restored sites will come back root-owned"
fi
if bodyhas "$BACKUPS_ROUTE" "async fn restore_file" "chown_restored_tree("; then
  ok "restore_file() now calls chown_restored_tree after a successful single-file restore"
else
  bad "restore_file() no longer calls chown_restored_tree"
fi
# The helper itself, and its original caller, must be untouched by this fix.
if bodyhasre "$BACKUPS_ROUTE" "fn chown_restored_tree" 'OsStr::new\(\"\.git\"\)'; then
  ok "chown_restored_tree itself is untouched — still .git-aware"
else
  bad "chown_restored_tree lost its own .git guard"
fi
# The underlying extraction flags that CAUSE the root-ownership are the
# reason this fix is needed at all — if they're gone, the bug (and the
# reason for this pin) may no longer apply, which is worth knowing either way.
if bodyhas "$BACKUPS_SVC" "fn restore_inner" "no-same-owner"; then
  ok "restore_inner still extracts with --no-same-owner (confirms the fix is still needed, not stale)"
else
  bad "restore_inner no longer passes --no-same-owner — re-verify whether chown_restored_tree calls are still necessary"
fi

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
