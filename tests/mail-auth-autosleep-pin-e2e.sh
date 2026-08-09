#!/usr/bin/env bash
# Regression pins for the s259 fresh-box behavioural audit.
#
# Every defect below was invisible to source review and to a green test suite,
# and every one was found by driving a real box the way a user would:
#
#   A. MAIL AUTHENTICATION. The Dovecot passwd-file was written 0600 root:root.
#      Dovecot's auth worker drops to the `dovecot` user, so it could not open
#      the file at all and EVERY mailbox login failed — while the panel reported
#      the account created successfully. Proven on a real box: with the shipped
#      permissions an IMAP login returns "Temporary authentication failure";
#      with 0640 root:dovecot the same login succeeds.
#   B. AUTO-SLEEP HAD NO TRAFFIC SIGNAL. Nothing wrote `last_activity_at` from
#      visitors, so "idle" meant "nobody used the panel". A container answering
#      HTTP 200 every 16 seconds was stopped on the timer and the next request
#      got a 502. The sleeper must ask nginx when the domain last served.
#   C. …and enabling auto-sleep on a default install did nothing at all, for
#      ever, because the sleeper is a step of the auto-healer, which is OFF by
#      default and configured on a different page. Storing the setting is fine;
#      reporting plain success while the loop that honours it is off is not.
#   D. A SITE BACKUP IS FILES ONLY, and the guide said otherwise — twice, plus a
#      sample restore transcript with a "Restoring database..." step no code has
#      ever performed. Restoring a WordPress site returns its files and none of
#      its content. Confirmed by restoring one: the file marker came back, the
#      deleted post did not.
#
# No running panel needed.
#   run: bash tests/mail-auth-autosleep-pin-e2e.sh
set -u

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MAIL="$REPO/panel/agent/src/routes/mail.rs"
NGINX="$REPO/panel/agent/src/routes/nginx.rs"
HEALER="$REPO/panel/backend/src/services/auto_healer.rs"
APPS_API="$REPO/panel/backend/src/routes/docker_apps.rs"
APPS_UI="$REPO/panel/frontend/src/pages/Apps.tsx"
BACKUPS_UI="$REPO/panel/frontend/src/pages/Backups.tsx"
BACKUPS_DOC="$REPO/docs/guides/backups.md"
PASS=0; FAIL=0
ok()   { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }
has()  { if grep -q "$2" "$3"; then ok "$1"; else bad "$1"; fi; }
hasnt(){ if grep -q "$2" "$3"; then bad "$1"; else ok "$1"; fi; }

echo
echo "A. Dovecot can actually read its own password file"
# The whole defect in one line: the mode must not be owner-only, because the
# reader is not the owner.
has  "the passwd-file is written 0640, not owner-only" \
     'from_mode(0o640)' "$MAIL"
hasnt "no caller writes the passwd-file owner-only any more" \
     'DOVECOT_USERS, *"" *, *0o600\|DOVECOT_USERS, &dovecot_lines.join("\\\\n"), 0o600' "$MAIL"
has  "the group is handed to dovecot" \
     'args(\["root:dovecot"' "$MAIL"
# Ownership is set on the temp file, so the real path is never momentarily
# published with permissions that lock auth out.
if awk '/async fn write_dovecot_users/,/^}/' "$MAIL" \
   | grep -n 'root:dovecot\|rename' | head -2 | paste -sd' ' - \
   | grep -c 'root:dovecot.*rename' >/dev/null; then
  ok "ownership is set BEFORE the atomic rename"
else
  bad "ownership is set BEFORE the atomic rename"
fi
has  "both writers go through the one helper" \
     'write_dovecot_users("")' "$MAIL"
has  "…including the account-sync writer" \
     'write_dovecot_users(&dovecot_lines' "$MAIL"

echo
echo "B. Auto-sleep observes real traffic"
has  "the agent reports when a domain last served a request" \
     '/nginx/last-activity/{domain}' "$NGINX"
has  "…and it is routed" \
     'route("/nginx/last-activity/{domain}", get(last_activity))' "$NGINX"
has  "…from the access log's mtime, not a parse" \
     'metadata(&log_file)' "$NGINX"
# "No log" must not be reported as "no traffic": a silent zero is exactly what
# would let the timer stop a container it knows nothing about.
has  "a domain with no access log reports unknown, not idle" \
     '"seconds_ago": seconds_ago' "$NGINX"
has  "the sleeper asks before deciding" \
     'nginx/last-activity/' "$HEALER"
has  "…and only ever moves the timestamp forward" \
     'last_activity_at IS NULL OR last_activity_at < ' "$HEALER"
has  "…and treats a null answer as unknown" \
     'and_then(|v| v.as_u64())' "$HEALER"
# The domain is now read, where the old code explicitly discarded it.
hasnt "the sleeper no longer throws the domain away" \
     'for (container_id, container_name, _domain, threshold_minutes)' "$HEALER"

echo
echo "C. A switch that will not be acted on does not report plain success"
has  "the API reports whether the healer is running" \
     '"auto_heal_enabled": auto_heal_enabled' "$APPS_API"
has  "…read from the setting the healer itself reads" \
     "key = 'auto_heal_enabled'" "$APPS_API"
has  "the UI tells the user when the setting will not run" \
     'will not run until Auto-Healing is enabled' "$APPS_UI"
has  "…and says so as a warning, not a success" \
     'auto_heal_enabled === false' "$APPS_UI"
has  "the tooltip no longer promises idleness it cannot measure" \
     'served no requests' "$APPS_UI"

echo
echo "D. A backup never overstates what it contains"
# s259 found `create_backup` was `tar czf … -C <site_root> .` and nothing else,
# while the panel's own `databases` table knew the site had one — so restoring a
# WordPress site returned its files and none of its content (the file marker came
# back, the deleted post did not), and the guide claimed the opposite.
#
# s260 CHANGED THE UNDERLYING FACT: site backups now carry a dump of each of the
# site's databases, proven on a fresh box by the same drill (post came back).
# What survives both eras is the invariant this section really pins — the docs
# and the UI state what an archive actually holds, never more. The mechanics of
# the new path are pinned in tests/site-backup-databases-pin-e2e.sh.
has  "the Backups page still distinguishes an archive with no database" \
     'Files only' "$BACKUPS_UI"
has  "…and names the consequence rather than hinting at it" \
     'will not bring back posts, pages, users or settings' "$BACKUPS_UI"
has  "…and the empty state describes what a backup now protects" \
     "files and database content" "$BACKUPS_UI"
# The guide was the worst offender: it stated the database WAS included when it
# was not, twice, and printed a sample restore transcript with a "Restoring
# database..." step that no code performed. Those exact fabrications must stay
# gone even now that the capability is real — the transcript has to match what
# the CLI prints, and the CLI still cannot restore databases.
hasnt "the guide no longer claims the database is in the tarball" \
     'includes the site files, database' "$BACKUPS_DOC"
hasnt "…nor that a restore replaces the database" \
     'replaces the site files and database' "$BACKUPS_DOC"
hasnt "…nor prints a restore step that never runs" \
     'Restoring database\.\.\.' "$BACKUPS_DOC"
hasnt "…nor claims site backups carry the site's own database" \
     "Site backups include the site's own database" "$BACKUPS_DOC"
has  "…and now states what IS included, because it finally is" \
     'and a dump of every database attached to' "$BACKUPS_DOC"
has  "…while calling out that older archives are not" \
     'contain files only' "$BACKUPS_DOC"

echo
printf 'passed %d, failed %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
