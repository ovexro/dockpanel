#!/usr/bin/env bash
# Regression pins for s260 — site backups carry their databases.
#
# s259 drove a restore on a real box and found the defect this closes: a site
# backup was `tar czf` of the webroot and nothing else, so restoring a WordPress
# site returned its files and none of its content. The file marker came back;
# the deleted post did not; the panel called it a success.
#
# Four properties are pinned here, each one a way the fix could silently rot:
#
#   A. THE ARCHIVE CARRIES THE DATABASE. create_backup takes the site's
#      databases and stages a dump of each into the archive. A site with none
#      still produces exactly the archive it always did.
#   B. A MISSING DATABASE IS REPORTED, NEVER SWALLOWED. A dump that fails, or a
#      database the panel refuses to touch, is named in the result and recorded
#      on the backup row — because "is my content in here?" has to be answerable
#      later, not only at the moment the backup ran.
#   C. THE DUMPS NEVER REACH THE DOCUMENT ROOT. They are the site's entire
#      content in plaintext and the webroot is served to the public, so restore
#      extracts them to staging first and excludes them from the file pass.
#   D. A PARTIAL RESTORE IS A FAILURE, NOT A SUCCESS WITH A FOOTNOTE. Files
#      restored + database not is the dangerous state: the site is running
#      restored files against its previous content and the operator must be told.
#
# The behavioural half of this lives in `cargo test -p dockpanel-agent
# db_payload_tests`, which round-trips a real archive. This script needs no
# running panel.
#   run: bash tests/site-backup-databases-pin-e2e.sh
set -u

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
AGENT_SVC="$REPO/panel/agent/src/services/backups.rs"
AGENT_RT="$REPO/panel/agent/src/routes/backups.rs"
API="$REPO/panel/backend/src/routes/backups.rs"
UI="$REPO/panel/frontend/src/pages/Backups.tsx"
CLI="$REPO/panel/cli/src/commands/backup.rs"
DOC="$REPO/docs/guides/backups.md"
CLIDOC="$REPO/docs/cli-reference.md"
MIGRATION="$REPO/panel/backend/migrations/20260726100000_site_backup_databases.sql"
DBBACKUP="$REPO/panel/agent/src/services/database_backup.rs"
SCHED="$REPO/panel/backend/src/services/backup_scheduler.rs"
POLICY="$REPO/panel/backend/src/services/backup_policy_executor.rs"

PASS=0; FAIL=0
ok()   { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }
has()  { grep -q -- "$2" "$1" 2>/dev/null && ok "$3" || bad "$3"; }
hasnt(){ grep -q -- "$2" "$1" 2>/dev/null && bad "$3" || ok "$3"; }
hasE() { grep -qE -- "$2" "$1" 2>/dev/null && ok "$3" || bad "$3"; }

echo
echo "A. The archive carries the database"
hasE "$AGENT_SVC" 'pub async fn create_backup\(domain: &str, databases: &\[DbSpec\]' \
     "create_backup accepts the site's databases"
has  "$AGENT_SVC" 'pub struct DbSpec' \
     "a spec type carries container/name/engine/user/password down to the agent"
has  "$AGENT_SVC" 'stage_one_dump' \
     "each database is dumped into the archive payload"
hasE "$AGENT_SVC" 'dump_mysql|dump_postgres' \
     "dumping reuses the hardened per-engine dump paths"
has  "$AGENT_RT" 'body: Option<Json<SiteDbBody>>' \
     "the agent route takes the databases as an OPTIONAL body (no body = files only, as before)"
hasE "$API" 'agent\.post\(&agent_path, Some\(agent_body\)\)' \
     "the panel actually sends the databases to the agent"
has  "$API" 'async fn site_databases' \
     "the panel resolves a site's databases (the agent cannot — it has no DB access)"
has  "$API" 'decrypt_credential_or_legacy' \
     "database credentials are decrypted before being handed down"

echo
echo "B. A missing database is reported, never swallowed"
has  "$AGENT_SVC" 'pub struct DbFailure' \
     "a failed database is a structured result, not a log line"
has  "$AGENT_SVC" 'databases_failed' \
     "the backup result names the databases that are NOT in the archive"
has  "$API" 'databases_included, databases_expected' \
     "the backup row records what the archive holds vs what the site had"
has  "$MIGRATION" 'databases_included' \
     "a migration adds the columns"
has  "$MIGRATION" 'DEFAULT 0' \
     "pre-existing rows default to 0 — true of every archive made before this ship"
has  "$API" 'Backup created WITHOUT' \
     "an incomplete backup does not report a clean success"
has  "$API" 'is not fully provisioned yet' \
     "a database the panel refuses to touch is still counted and named"
hasE "$API" 'container_id\.as_deref\(\)\.unwrap_or\(""\)\.is_empty\(\)' \
     "the v2.19.0 empty-container_id guard is mirrored (never dump a name-collided container)"

echo
echo "C. The dumps never reach the document root"
has  "$AGENT_SVC" 'const PAYLOAD_DIR' \
     "the payload lives in its own archive directory"
has  "$AGENT_SVC" '"--exclude", PAYLOAD_DIR' \
     "the file-restore pass excludes the payload"
has  "$AGENT_SVC" 'async fn extract_payload' \
     "the payload is extracted to staging first, separately from the files"
has  "$AGENT_SVC" 'from_mode(0o700)' \
     "staging is root-only — the dumps are the site's content in plaintext"
hasE "$AGENT_SVC" 'remove_dir_all\(&?stage' \
     "staging is always removed, including on the error path"
has  "$AGENT_SVC" 'site root contains a reserved' \
     "a site that already owns the payload path is refused, not shadowed"
has  "$AGENT_SVC" "cannot be restored into the site directory" \
     "the single-file restore explicitly refuses the database payload"

echo
echo "D. A partial restore is a failure"
hasE "$AGENT_SVC" 'self\.files_restored && self\.databases_failed\.is_empty\(\)' \
     "ok() requires BOTH the files and every database"
has  "$AGENT_RT" 'StatusCode::INTERNAL_SERVER_ERROR, Json(payload)' \
     "the agent answers a partial restore with a failure carrying the report"
has  "$API" 'Restore failed — files restored, database not' \
     "the panel names the dangerous state explicitly"
has  "$API" 'backup.restore_partial' \
     "a partial restore is a distinct activity-log event"
has  "$API" 'This backup contains files only' \
     "restoring a files-only archive onto a site with databases warns BEFORE it starts"

echo
echo "G. EVERY path that creates a site backup includes the databases"
# The manual button and the scheduled job must not disagree. A scheduled backup
# that quietly omitted the database would be this exact defect surviving in the
# path people rely on most — and nobody is watching when it runs.
has  "$API" 'pub async fn site_database_specs' \
     "one shared resolver, reachable from the schedulers as well as the route"
hasnt "$SCHED" 'agent_path, None' \
     "the schedule path no longer creates a backup with no databases"
has  "$SCHED" 'site_database_specs' \
     "the schedule path resolves the site's databases"
has  "$SCHED" 'is INCOMPLETE' \
     "an incomplete scheduled backup is logged loudly, since nobody is watching"
has  "$SCHED" 'databases_included, databases_expected' \
     "…and its backup row records what the archive really holds"
hasnt "$POLICY" 'create"), None' \
     "the backup-policy path no longer creates a backup with no databases"
has  "$POLICY" 'site_database_specs' \
     "the backup-policy path resolves the site's databases too"

echo
echo "F. The restore actually reaches a client binary that exists"
# Found by driving this drill on a real box: the panel provisions `mariadb:11`,
# which dropped the mysql-named client symlinks, and restore_mysql was the only
# call site still invoking the old name. Dumping worked; restoring never did.
hasE "$DBBACKUP" '"mariadb", "-u", user, db_name' \
     "restore_mysql invokes the client the container actually has"
hasnt "$DBBACKUP" '"mysql", "-u", user, db_name' \
     "it no longer invokes a binary MariaDB 11 does not ship"
has  "$DBBACKUP" 'produced no error output' \
     "a failure with empty stderr still reports a reason, never a bare 'failed:'"
has  "$AGENT_RT" 'obj.insert("error".into()' \
     "a failed restore also carries a plain-language summary under the key clients read"
has  "$AGENT_SVC" 'supplied no database credentials' \
     "a caller that sent no credentials is told THAT, not that the site lost its database"
has  "$CLI" 'supplied no database credentials' \
     "the CLI recognises that reason and explains the way out"

echo
echo "E. The surfaces tell the same story"
has  "$UI" 'function backupContents' \
     "the UI derives what each archive actually holds"
has  "$UI" 'const c = backupContents(backup);' \
     "every backup row renders what that archive actually holds"
has  "$UI" 'databases_included: number;' \
     "the row model carries the archive's real contents from the API"
has  "$UI" 'Files only' \
     "an archive with no database is labelled"
hasnt "$UI" 'Databases are
    not included' \
     "the UI no longer states databases are excluded as a blanket fact"
has  "$CLI" 'Content: files only' \
     "the CLI says what it produced"
has  "$CLI" 'cannot resolve a site' \
     "the CLI explains WHY it cannot include databases (it authenticates to the agent)"
hasnt "$DOC" '\*\*Databases are not included\.\*\*' \
     "the guide no longer says databases are excluded from a site backup"
has  "$DOC" 'and a dump of every database attached to' \
     "the guide states what a backup now contains"
has  "$DOC" 'contain files only' \
     "the guide is explicit that older archives hold no database"
has  "$CLIDOC" 'never include databases' \
     "the CLI reference states the CLI limitation"

echo
printf 'PASS %d  FAIL %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
