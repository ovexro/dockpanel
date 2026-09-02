#!/usr/bin/env bash
#
# Regression pin for VOLUME OWNERSHIP on one-click app templates, and for the
# nineteenth unresolvable image reference.
#
# WHAT HAPPENED. `deploy_app` creates every declared volume as a directory under
# APP_DATA_DIR, running as root, mode 0755, and bind-mounts it. Nothing chowned
# it. That is correct for most images — they start as root and chown their own
# data directory in their entrypoint, which is exactly what postgres, mysql and
# mongo do before dropping privileges.
#
# It is wrong for an image that declares a non-root `Config.User`, because that
# process starts ALREADY DROPPED and can never chown its way out. It cannot write
# the directory the template just mounted for it.
#
# MEASURED, not argued. Every catalogue image's config blob was read from the
# registry (no pulls) at s342: 42 of 148 images run non-root, and 31 of those
# declare a volume — 21% of the catalogue could not persist a byte. Proven end to
# end on prom/prometheus:v3, whose `Config.User` is the NAME `nobody`:
#
#   root-owned 0755 dir (what deploy_app made)  -> exited 1, "permission denied"
#                                                  on /prometheus/queries.active,
#                                                  ZERO files in the volume
#   chowned to the uid the image itself names   -> running, FIVE files including
#                                                  the WAL and the lock
#
# WHY THE FIX IS SHAPED THE WAY IT IS. `Config.User` is frequently a NAME, and a
# name is defined by the image's own /etc/passwd and by nothing on the host — 24
# of the 31 victims name a user rather than a uid. So the resolver reads
# /etc/passwd out of the image itself, by creating a container and copying the
# file out without ever starting it. The two lazy alternatives are both refused
# on purpose: 0777 would put a world-writable data directory on a host that runs
# other people's containers, and guessing a uid hands one tenant's data to
# another account. When the name cannot be resolved the directory is left
# root-owned and the agent says so — the same state as before, but no longer a
# silent one.
#
# ALSO PINNED HERE: the Readarr template, withdrawn s342. Its image published an
# OCI index with ZERO manifests on both tags, so `docker pull` exited 1 while the
# Sonarr and Radarr controls returned four manifests each, and upstream is
# archived. ⚠ v2.96.0's sweep could not see this class: the manifest request
# returns HTTP 200 with a well-formed index that merely happens to be empty, and
# that sweep classified by error MESSAGE. An absence arm keeps it withdrawn.
#
# WRAP TOLERANCE. Every arm that reads Rust canonicalises the source by deleting
# whitespace first, and no pattern ever spells a CLOSING delimiter — that is
# where rustfmt inserts its trailing comma, and spelling one is how #384 got
# shipped into the suite written to pin #384, twice (lessons #384, #391).
#
# PROSE SAFETY. Arms run against COMMENT-STRIPPED source, because this file's own
# header narrates the very tokens it searches for, and the subject file carries a
# comment explaining the withdrawal (lesson #149, [[feedback_source_pin_prose_trap]]).
#
# Pure source analysis: no box, no network, no build, no Docker daemon.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

CATALOGUE=panel/agent/src/services/docker_apps.rs
[ -f "$CATALOGUE" ] || { echo "missing $CATALOGUE"; exit 1; }

# Strip line comments only. A block-comment stripper is what deleted 485 lines of
# real code until s294 (a `/*` inside a string literal opened a comment that ran
# to the next `*/`), so this deliberately does the narrow, safe thing.
code() { sed 's://.*$::' "$1"; }

CODE=$(code "$CATALOGUE")
# Canonical form: whitespace deleted, so the single-line and the rustfmt-wrapped
# spellings are the same string.
FLAT=$(printf '%s' "$CODE" | tr -d ' \t\n')

echo "§0 the subject parses (a truncated subject must not read as a clean sweep)"

DECLS=$(printf '%s\n' "$CODE" | grep -c 'AppTemplateDef {' || true)
if [ "$DECLS" -ge 100 ]; then
  ok "catalogue parsed: $DECLS AppTemplateDef declarations survive the strip"
else
  bad "catalogue did not parse — only $DECLS declarations; every arm below is meaningless"
fi

STRIPPED_LINES=$(printf '%s\n' "$CODE" | wc -l)
RAW_LINES=$(wc -l < "$CATALOGUE")
if [ "$STRIPPED_LINES" -eq "$RAW_LINES" ]; then
  ok "comment strip removed no LINES ($RAW_LINES) — it cannot have eaten a declaration"
else
  bad "comment strip changed the line count ($RAW_LINES -> $STRIPPED_LINES)"
fi

echo
echo "§1 deploy_app works out who the volume belongs to BEFORE it creates one"

case "$FLAT" in
  *'letvolume_owner=iftemplate.volumes.is_empty()'*)
    ok "ownership is resolved once per deploy, not once per volume" ;;
  *) bad "deploy_app does not resolve a volume owner before the volume loop" ;;
esac

case "$FLAT" in
  *'resolve_volume_owner(&docker,template.image).await'*)
    ok "the resolver is called with the template's own image" ;;
  *) bad "resolve_volume_owner is never called on the deploy path" ;;
esac

case "$FLAT" in
  *'.config?'*'.user'*) ok "the owner comes from the image's own Config.User" ;;
  *) bad "nothing reads Config.User — the uid can only be a guess" ;;
esac

echo
echo "§2 the chown lands AFTER the prefix check, never before"

# The canonicalisation check is what proves the path is one of ours. A chown is
# precisely the operation a symlink escape would want to borrow, so its position
# relative to that check is load-bearing, not stylistic.
ESCAPE_AT=$(printf '%s\n' "$CODE" | grep -n 'escapes allowed prefix' | head -1 | cut -d: -f1)
CHOWN_AT=$(printf '%s\n' "$CODE" | grep -n 'chown_to(&resolved_str' | head -1 | cut -d: -f1)
if [ -n "$ESCAPE_AT" ] && [ -n "$CHOWN_AT" ] && [ "$CHOWN_AT" -gt "$ESCAPE_AT" ]; then
  ok "chown at line $CHOWN_AT follows the prefix check at line $ESCAPE_AT"
else
  bad "chown does not follow the prefix check (escape=$ESCAPE_AT chown=$CHOWN_AT)"
fi

case "$FLAT" in
  *'chown_to(&resolved_str'*)
    ok "the chown targets the CANONICALISED path, not the path as written" ;;
  *) bad "the chown does not target the canonicalised path" ;;
esac

echo
echo "§3 a NAME is resolved from inside the image — the whole point of the fix"

case "$FLAT" in
  *'read_file_from_image(docker,image,"/etc/passwd")'*)
    ok "login names resolve from the image's own /etc/passwd" ;;
  *) bad "nothing reads /etc/passwd from the image; a name can only be guessed" ;;
esac

case "$FLAT" in
  *'read_file_from_image(docker,image,"/etc/group")'*)
    ok "group names resolve from the image's own /etc/group" ;;
  *) bad "nothing reads /etc/group from the image" ;;
esac

# The probe must never START the container it creates to read a file out of.
# ⚠ The haystack is already whitespace-free, so the window's own boundaries must
# be spelled that way too — spelling them with spaces extracts nothing, and an
# empty window is why the guard below exists rather than a bare grep (s302).
PROBE=$(printf '%s' "$FLAT" | sed -n 's/.*asyncfnread_file_from_image\(.*\)fnpasswd_lookup.*/\1/p')
if [ -z "$PROBE" ]; then
  bad "could not isolate read_file_from_image — the arm below would pass on nothing"
elif grep -q 'start_container' <<<"$PROBE"; then
  bad "the file probe STARTS the container; it must only create and remove it"
else
  ok "the file probe never starts the container it reads from"
fi

case "$PROBE" in
  *'remove_container'*) ok "the probe container is always removed again" ;;
  *) bad "the probe container is never removed — every deploy would leak one" ;;
esac

echo
echo "§4 the two refused shortcuts stay refused"

# 0777 on a multi-tenant host, and a guessed uid, are the two cheap fixes that
# would have made this a one-line change. Neither may appear.
if grep -qE '0o777|0777' <<<"$FLAT"; then
  bad "a world-writable mode appears in the catalogue — that is the refused shortcut"
else
  ok "no world-writable fallback anywhere in the file"
fi

# Recursion is allowed in exactly ONE place and refused everywhere else, and the
# difference is what is underneath rather than a matter of taste.
#
#   deploy_app  may be pointed at a directory a previous deployment wrote, so it
#               chowns the directory ITSELF and nothing below it.
#   the migration has just refused to touch a non-empty directory and then filled
#               it with `docker cp`, so every path below it is one it created
#               seconds ago — and it must chown all of them, because `docker cp`
#               writes them as root (#110, s360).
#
# ⚠ THE ARM THIS REPLACES WAS A FALSE GREEN. It grepped for `chown-R`, `WalkDir`
# or one exact `read_dir().map()` spelling, so it printed "the chown is not
# recursive" against a file that had just grown a recursive chown written as an
# ordinary stack walk. An absence arm keyed on three spellings of a thing cannot
# see the fourth (lesson #490). Count the CALL SITES instead — a name has one
# spelling and the compiler enforces it.
#
# The test module is raw source too and calls this function itself (s342), so the
# count is taken over the shipping half only.
SRC_ONLY=$(printf '%s\n' "$CODE" | sed '/#\[cfg(test)\]/,$d')
FLAT_SRC=$(printf '%s' "$SRC_ONLY" | tr -d ' \t\n')
SRC_LINES=$(printf '%s\n' "$SRC_ONLY" | wc -l)
if [ "$SRC_LINES" -lt 1000 ] || [ "$SRC_LINES" -ge "$STRIPPED_LINES" ]; then
  bad "could not isolate the shipping half ($SRC_LINES of $STRIPPED_LINES lines) — the count below would be meaningless"
else
  ok "shipping half isolated: $SRC_LINES of $STRIPPED_LINES lines precede the test module"
fi

RECURSIVE_CALLS=$(printf '%s' "$FLAT_SRC" | grep -o 'chown_migrated_tree(' | wc -l)
if [ "$RECURSIVE_CALLS" -eq 2 ]; then
  ok "the recursive chown has exactly one call site plus its definition"
else
  bad "chown_migrated_tree appears $RECURSIVE_CALLS times in shipping code, expected 2 — a second caller means some other path recurses over a directory it did not create"
fi

case "$FLAT" in
  *'chown_to(&resolved_str'*)
    ok "the DEPLOY path still chowns the directory alone" ;;
  *) bad "deploy_app no longer uses the non-recursive chown" ;;
esac

if grep -qE 'chown-R' <<<"$FLAT"; then
  bad "a shell 'chown -R' appears; recursion must be the walk that refuses to follow symlinks"
else
  ok "no shell chown -R anywhere in the file"
fi

echo
echo "§4b the migration chowns what it wrote, and cannot walk out through a symlink"

# GitHub #110, second act. v2.111.0 rescued the data out of the writable layer and
# then handed it back owned by root, so every non-root image died on its first
# write — n8n on `EACCES .../crash.journal`, then `SQLITE_READONLY`. Reproduced end
# to end at s360 with the real image before any of this was written.
case "$FLAT" in
  *'chown_migrated_tree(&resolved,owner)'*)
    ok "the migration chowns the whole tree it just created" ;;
  *) bad "the migration does not chown its tree — docker cp wrote it as root, so a non-root app cannot write the data just rescued (#110)" ;;
esac

case "$FLAT" in
  *'std::fs::symlink_metadata(&path)?'*)
    ok "the walk stats without following symlinks" ;;
  *) bad "the walk follows symlinks and could chown outside the volume entirely" ;;
esac

case "$FLAT" in
  *'libc::lchown('*)
    ok "entries are lchowned, so a symlink is changed and never its target" ;;
  *) bad "no lchown — a symlink in the app's own data would be followed" ;;
esac

# The precondition that BOUNDS the recursion. If this refusal ever goes, the walk
# above stops being a walk over files we created and becomes one over somebody
# else's data. The two must never drift apart.
case "$FLAT" in
  *'Refusingtomigrate'*)
    ok "the migration still refuses a non-empty directory — what makes the recursion bounded" ;;
  *) bad "the non-empty refusal is gone, so the recursive chown is no longer bounded by construction" ;;
esac

echo
echo "§4c the repair reaches installs the migration already broke — on ALL three doors"

# v2.113.2 fixed the migration and could not undo it: the migration runs only for a
# volume that is MISSING, and the bad migration is what supplied the mount. So the
# harmed population and the fixed population were disjoint, and the only remedy was
# an operator running chown -R over SSH. This is the half that reaches them.
REPAIR_CALLS=$(printf '%s' "$FLAT_SRC" | grep -o 'repair_root_owned_volumes(' | wc -l)
if [ "$REPAIR_CALLS" -eq 4 ]; then
  ok "the repair is wired to all three recreate doors (plus its definition)"
else
  bad "repair_root_owned_volumes appears $REPAIR_CALLS times in shipping code, expected 4 — update_app, change_container_image and update_env must ALL call it or one door silently leaves apps broken"
fi

# The narrowest rule that repairs what we did: reclaim root and nothing else. An
# ownership somebody chose deliberately is not ours to rewrite.
# s449: rewritten around fd-relative syscalls (openat/fstatat, all
# O_NOFOLLOW/AT_SYMLINK_NOFOLLOW) to close a TOCTOU — a path-based
# `symlink_metadata` then `read_dir` re-resolves the path twice, and an app
# with write access to its own bind mount can swap a directory for a symlink
# in between (project_dockpanel_tech_debt_p185 carry F). Keys on the new
# spelling; the capability (root-only filter) is unchanged.
case "$FLAT" in
  *'st.st_uid==0'*)
    ok "the repair reclaims ONLY root-owned paths" ;;
  *) bad "the repair is not filtered to root-owned paths — it would rewrite an ownership a human chose" ;;
esac

# F fix: every path lookup in the walk must be fd-relative (openat/fstatat/
# fchownat with O_NOFOLLOW/AT_SYMLINK_NOFOLLOW), never a path string handed
# to std::fs — the latter re-resolves from '/' and reopens the TOCTOU window.
case "$FLAT" in
  *'openat(dir_fd,name.as_ptr(),libc::O_NOFOLLOW'*)
    ok "the walk opens each entry O_NOFOLLOW, relative to its already-open parent" ;;
  *) bad "the walk does not open entries O_NOFOLLOW against an open parent fd — the TOCTOU is back" ;;
esac
case "$FLAT" in
  *'fstatat(dir_fd,name.as_ptr(),&mutst,libc::AT_SYMLINK_NOFOLLOW)'*)
    ok "the walk stats each entry fd-relative, never following a symlink" ;;
  *) bad "the walk does not stat entries via fstatat/AT_SYMLINK_NOFOLLOW" ;;
esac

# A root-owned entry with nlink>1 is a planted hardlink to a real host file
# (fs.protected_hardlinks=0 lets an unprivileged process create one inside
# its own volume) — chowning it would repaint that other name too.
case "$FLAT" in
  *'st.st_nlink>1'*)
    ok "the walk skips a root-owned entry with more than one link" ;;
  *) bad "the walk has no nlink guard — a hardlinked host file inside the volume would be repainted" ;;
esac

# Root-run images have nothing to repair, and resolve_volume_owner is what says so.
case "$FLAT" in
  *'letSome(owner)=resolve_volume_owner(docker,image).awaitelse{'*)
    ok "a root-run image is skipped — the resolver's None is the discriminator" ;;
  *) bad "the repair does not consult resolve_volume_owner, so it would act on root-run images" ;;
esac

# Same prefix discipline as every other writer into APP_DATA_DIR.
REPAIR_BODY=$(printf '%s' "$FLAT_SRC" | sed -n 's/.*asyncfnrepair_root_owned_volumes\(.*\)fnchown_root_owned_entries.*/\1/p')
if [ -z "$REPAIR_BODY" ]; then
  bad "could not isolate repair_root_owned_volumes — the arms below would pass on nothing"
else
  case "$REPAIR_BODY" in
    *'canonicalize'*'starts_with(&prefix)'*)
      ok "the repair canonicalises and re-checks the APP_DATA_DIR prefix" ;;
    *) bad "the repair writes without canonicalising and re-checking the prefix" ;;
  esac
fi

case "$FLAT" in
  *'fnuser_spec_is_root'*) ok "root images are recognised and left completely alone" ;;
  *) bad "nothing distinguishes a root image, so every deploy would chown" ;;
esac

# An empty User field is how nearly every image ships and it MEANS root. Getting
# this wrong would chown 106 templates that never needed it.
case "$FLAT" in
  *'""|"root"|"0"'*) ok "an empty Config.User is treated as root" ;;
  *) bad "an empty Config.User is not treated as root" ;;
esac

echo
echo "§5 the nineteenth dead image reference stays withdrawn"

case "$FLAT" in
  *'id:"readarr"'*) bad "the Readarr template is back; its image publishes no manifests" ;;
  *) ok "the Readarr template is absent from the catalogue" ;;
esac

case "$FLAT" in
  *'linuxserver/readarr'*) bad "the dead image reference is back in the catalogue" ;;
  *) ok "the dead image reference does not appear in the catalogue" ;;
esac

# Positive control: the sibling LinuxServer templates that DO resolve must still
# be there, or this section would pass by the catalogue having been emptied.
for sibling in 'linuxserver/sonarr' 'linuxserver/radarr'; do
  case "$FLAT" in
    *"$sibling"*) ok "control: $sibling is still present" ;;
    *) bad "control: $sibling vanished — the arms above prove nothing" ;;
  esac
done

echo
echo "§6 the published template count agrees with the catalogue"

# The count is derived, never typed. FEATURES.md owns it; every other surface
# copies from there, and a number copied to a second surface is rot by
# construction unless something checks them together.
N=$(printf '%s\n' "$CODE" | awk '
  /static TEMPLATES/ { inside = 1; next }
  inside && /^\];/   { exit }
  inside && /^        id: "/ { n++ }
  END { print n + 0 }
')
CLAIMED=$(grep -oE '^\| App templates \| [0-9]+' FEATURES.md | grep -oE '[0-9]+$' | head -1)
if [ -n "$CLAIMED" ] && [ "$N" = "$CLAIMED" ]; then
  ok "catalogue holds $N templates and FEATURES.md claims $CLAIMED"
else
  bad "catalogue holds $N templates but FEATURES.md claims ${CLAIMED:-nothing}"
fi

echo
echo "§7 the resolver's parsing is unit-tested where a mistake is silent"

TESTS=$(printf '%s' "$CODE" | tr -d ' \t\n')
for arm in \
  'fnroot_user_specs_are_recognised_in_every_spelling' \
  'fna_login_name_resolves_from_the_images_own_passwd' \
  'fnthe_tar_reader_extracts_a_file_and_refuses_junk' \
  'fnvolume_owner_resolves_real_images_both_ways'
do
  case "$TESTS" in
    *"$arm"*) ok "unit test present: ${arm#fn}" ;;
    *) bad "unit test missing: ${arm#fn}" ;;
  esac
done

echo
if [ "$FAIL" -eq 0 ]; then
  printf '\033[0;32mPASS %d  FAIL 0\033[0m\n' "$PASS"
else
  printf '\033[0;31mPASS %d  FAIL %d\033[0m\n' "$PASS" "$FAIL"
fi
exit $((FAIL > 0))
