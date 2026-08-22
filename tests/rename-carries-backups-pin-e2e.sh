#!/usr/bin/env bash
#
# Regression pin for "a rename takes the backups with it" — s389.
#
# THE DEFECT. Renaming a site migrated the webroot, the vhost, the parked vhost,
# the certificates, the nginx logs, the PHP-FPM pools, the fail2ban jail and the
# redirect configs — ten numbered steps — and left every backup tree behind under
# the old name. That is worse than a disk leak, because the panel's own table
# stores a filename and NO path: every reader rebuilds the directory from the
# LIVE domain. So the instant a rename completed, every archive the site had
# became unrestorable and undownloadable while the panel went on listing it, and
# the nightly retention sweep then resolved the new name, got NotFound, treated
# it as "already gone", deleted the row and counted it as pruned. Records
# destroyed, files stranded, success logged.
#
# TWO REFUSALS came with the fix, and both are pinned because both are the kind
# of guard that is easy to delete for looking over-cautious:
#
#   * A destination that already holds files is refused BEFORE anything moves.
#     Not hypothetical — every completed site delete manufactures one, because
#     the panel writes a pre-delete archive and records no row for it while the
#     cascade takes the rows that would have named it.
#   * A site with backups will not rename onto an agent too old to carry them,
#     because such an agent answers success and leaks anyway.
#
# ⛔ EVERY ARM HERE IS SCOPED TO ONE FUNCTION BODY. A file-scoped arm on this
# subject is worthless: `nginx.rs` holds eleven other `fs::rename` calls and the
# tree paths live in a helper ABOVE the handler, so a whole-file `has` would be
# satisfied by a sibling however the carry itself is rewritten. Every subject is
# also floored on its post-strip size, because a `fnbody` whose pattern stops
# matching yields an EMPTY subject and every absence arm beneath it then passes
# green for code that is no longer there.
#
# Pure source analysis: no box, no network, no build.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0; FAIL=0
ok()  { printf '\033[32m✓\033[0m %s\n' "$1"; PASS=$((PASS+1)); }
bad() { printf '\033[31m✗\033[0m %s — %s\n' "$1" "$2"; FAIL=$((FAIL+1)); }
eq()  { [ "$2" = "$3" ] && ok "$1" || bad "$1" "expected '$3', got '$2'"; }
has() { case "$2" in *"$3"*) ok "$1" ;; *) bad "$1" "missing: $3" ;; esac; }
hasnt(){ case "$2" in *"$3"*) bad "$1" "present but must not be: $3" ;; *) ok "$1" ;; esac; }
before() { # $1 label, $2 subject, $3 earlier, $4 later
  local a b
  a=$(printf '%s' "$2" | $G -boF -- "$3" | head -1 | cut -d: -f1)
  b=$(printf '%s' "$2" | $G -boF -- "$4" | head -1 | cut -d: -f1)
  if [ -z "$a" ] || [ -z "$b" ]; then
    bad "$1" "could not locate both landmarks (earlier='${a:-none}' later='${b:-none}') — the arm measured nothing"
  elif [ "$a" -lt "$b" ]; then
    ok "$1"
  else
    bad "$1" "the earlier landmark sits at $a, AFTER the later one at $b"
  fi
}

# ugrep's --ignore-files shim honours .gitignore, so use the real binary.
G=/usr/bin/grep

subj()   { sed -E -e 's://.*$::' -e 's:^[[:space:]]*--.*$::' "$1" | tr -d ' \n\\'; }
subjin() { sed -E -e 's://.*$::' -e 's:^[[:space:]]*--.*$::' | tr -d ' \n\\'; }
fnbody() { awk -v p="$2" 'index($0,p){f=1} f{print} f && /^}$/{exit}' "$1"; }
occ()    { printf '%s' "$1" | $G -oF -- "$2" | wc -l | tr -d ' '; }

ANGINX=panel/agent/src/routes/nginx.rs
ADBR=panel/agent/src/routes/database_backup.rs
BSITES=panel/backend/src/routes/sites.rs
BDBR=panel/backend/src/routes/databases.rs
BSCHED=panel/backend/src/services/backup_scheduler.rs
BDASH=panel/backend/src/routes/dashboard.rs
BORCH=panel/backend/src/routes/backup_orchestrator.rs
BEXEC=panel/backend/src/services/backup_policy_executor.rs

for f in "$ANGINX" "$ADBR" "$BSITES" "$BDBR" "$BSCHED" "$BDASH" "$BORCH" "$BEXEC"; do
  [ -f "$f" ] || { bad "SETUP" "$f missing"; exit 1; }
done

# Whole-file floors first, then every function-body subject is floored too.
for pair in "$ANGINX:40000" "$ADBR:4000" "$BSITES:60000" "$BDBR:20000" \
            "$BSCHED:6000" "$BDASH:9000" "$BORCH:35000" "$BEXEC:12000"; do
  f=${pair%:*}; min=${pair##*:}
  n=$(subj "$f" | wc -c)
  [ "$n" -ge "$min" ] || { bad "SETUP" "$f has $n chars of code, expected >= $min"; exit 1; }
done

F_ANGINX=$(subj "$ANGINX")
F_TREES=$(fnbody  "$ANGINX" "fn domain_backup_trees("   | subjin)
F_OCC=$(fnbody    "$ANGINX" "fn dir_is_occupied("        | subjin)
F_UNDO=$(fnbody   "$ANGINX" "fn undo_backup_carry("      | subjin)
F_RENAME=$(fnbody "$ANGINX" "async fn rename_site("      | subjin)
F_PURGEDUMP=$(fnbody "$ADBR" "async fn purge_dump("      | subjin)
F_ADBRRT=$(fnbody "$ADBR"   "pub fn router("             | subjin)
F_BSITES=$(subj "$BSITES")
F_BRENAME=$(fnbody "$BSITES" "pub async fn rename_domain(" | subjin)
F_BREMOVE=$(fnbody "$BSITES" "pub async fn remove("      | subjin)
F_PURGE=$(fnbody  "$BDBR"   "pub(crate) async fn purge_dumps_if_unclaimed(" | subjin)
F_SCHED=$(subj "$BSCHED")
F_DASH=$(subj "$BDASH")
F_ORCH=$(subj "$BORCH")
F_EXEC=$(subj "$BEXEC")

for pair in "F_TREES:200" "F_OCC:100" "F_UNDO:200" "F_RENAME:5000" \
            "F_PURGEDUMP:60" "F_ADBRRT:300" "F_BRENAME:2000" "F_BREMOVE:4000" \
            "F_PURGE:600"; do
  v=${pair%:*}; min=${pair##*:}
  n=$(printf '%s' "${!v}" | wc -c)
  [ "$n" -ge "$min" ] || { bad "SETUP" "$v resolved $n chars, expected >= $min — the pattern stopped matching"; exit 1; }
done

echo "── §A  the three domain-keyed trees are named in ONE place"
has "A1 the archive root"      "$F_TREES" 'letroot="/var/backups/dockpanel";'
has "A2 the restic repo, keyed with the substitution its writer uses" "$F_TREES" 'restic/{}",old_domain.replace('"'"'.'"'"',"_")'
has "A3 the wordpress snapshots" "$F_TREES" 'wp-snapshots/{old_domain}'
# Two per tree — an old path and a new one — so three trees is six. Counted
# rather than merely present: adding a fourth tree without teaching the undo and
# the refusals about it is the way this grows a hole.
eq  "A4 three trees, each with an old and a new path" "$(occ "$F_TREES" 'format!("{root}/')" 6

echo "── §B  the handler actually uses them, and the result of the move is READ"
has "B1 the handler resolves the trees once"  "$F_RENAME" 'domain_backup_trees(&old_domain,new_domain)'
# ⭐ THE ARM THAT MATTERS. `.ok()` here is the whole defect wearing the fix's
# clothes: it moves what it can, discards ENOTEMPTY, and answers success.
has "B2 the move is matched, not discarded"   "$F_RENAME" 'matchstd::fs::rename(old_tree,new_tree){'
hasnt "B3 and never swallowed"                "$F_RENAME" 'std::fs::rename(old_tree,new_tree).ok()'
has "B4 a moved tree is remembered so it can be put back" "$F_RENAME" 'carried.push((old_tree.clone(),new_tree.clone()));'
has "B5 a moved tree is re-hardened, because rename preserves the source mode" "$F_RENAME" 'services::backups::secure_backup_tree();'

echo "── §C  the refusals"
# Two in the handler: the step-0 precondition and the errno split on the move.
eq  "C1 two caller-fixable refusals, not one" "$(occ "$F_RENAME" 'StatusCode::CONFLICT')" 2
has "C2 a collision is refused"               "$F_RENAME" 'dir_is_occupied(new_tree)'
# Anchored on the `if` and the opening brace, so the whole condition is pinned
# rather than a substring of it. A substring arm is satisfied by
# `if false && <the same condition>`, which disables the refusal while leaving
# every literal in place — found by mutation, not by reading.
has "C3 only when there is something to move" "$F_RENAME" 'ifstd::path::Path::new(old_tree).exists()&&dir_is_occupied(new_tree){'
# ⛔ The precondition is worthless below the first move: by then the cheap
# refusal is gone and the webroot has already changed name.
before "C4 the precondition runs BEFORE the webroot moves" "$F_RENAME" \
  'dir_is_occupied(new_tree)' 'std::fs::rename(&old_dir,&new_dir)'
has "C5 doubt counts as occupied"             "$F_OCC" 'Err(_)=>true'
has "C6 and only NotFound counts as free"     "$F_OCC" 'ErrorKind::NotFound=>false'

echo "── §D  the rollback is symmetric"
# 1 definition + 3 call sites: the carry's own failure, and BOTH nginx -t
# branches. Counted, not merely present — deleting one call leaves the other two.
eq  "D1 the undo is wired at every abort"     "$(occ "$F_ANGINX" 'undo_backup_carry(')" 4
has "D2 it puts trees back most-recent-first" "$F_UNDO" 'carried.iter().rev()'
has "D3 and never silently"                   "$F_UNDO" 'tracing::error!('

echo "── §E  the panel keeps the rows in step with the files"
has "E1 a minimum agent is named"             "$F_BRENAME" 'BACKUP_CARRY_MIN_AGENT'
has "E2 the gate reads the agent's own answer, not a column that is NULL on the local agent" \
    "$F_BRENAME" 'agent.get("/health")'
# ⛔ Scoped so a site with NOTHING to lose still renames on any agent version.
has "E3 the refusal is gated on the site having backups" "$F_BRENAME" 'SELECTCOUNT(*)FROMbackupsWHEREsite_id=$1'
has "E4 the rows are re-prefixed to match the moved files" "$F_BRENAME" 'UPDATEbackupsSETfilename=$1||substring(filenamefrom$3)'
# ⛔ The agent answers this endpoint with a bare ARRAY. Reading a `backups` key
# would make the check report "nothing left behind" for every input it can see —
# an assertion that cannot fail is worse than no assertion.
has "E5 the leftover check reads the array the agent actually sends" "$F_BRENAME" 'v.as_array()'
hasnt "E6 not a key that is never there"      "$F_BRENAME" 'v.get("backups")'
# ⛔ FROZEN ON PURPOSE, and this is the only thing standing between the gate and a
# routine release bump. `BACKUP_CARRY_MIN_AGENT` is a historical fact — the release
# in which the carry shipped — and it happens to equal the version of that release,
# so it looks exactly like the ten other places a version bump rewrites. Sed it
# forward and the panel starts refusing renames on agents that carry backups
# perfectly well, with no other symptom. It moves only if the carry itself changes.
eq  "E7 the minimum agent is frozen at the release that shipped the carry" \
    "$($G -oF -- 'BACKUP_CARRY_MIN_AGENT:&str="2.142.0";' <<< "$F_BSITES" | wc -l | tr -d ' ')" 1

echo "── §F  a failed upload is RECORDED, so retention can still reach the file"
hasnt "F1 the scheduler no longer drops the row" "$F_SCHED" 'dontrecordinDBsincetheuploadfailed'
has "F2 the exhausted upload is carried past the INSERT" "$F_SCHED" 'upload_failed=Some(last_err.clone());'
has "F3 and the run still fails afterwards"   "$F_SCHED" 'ifletSome(why)=upload_failed{'
# ⛔ The ordering IS the fix. Returning before the INSERT restores the defect
# with every literal in this file still present.
before "F4 the row is written BEFORE the run reports failure" "$F_SCHED" \
  'INSERTINTObackups(' 'ifletSome(why)=upload_failed{'
has "F5 the destination is recorded whatever the upload did" "$F_SCHED" '.bind(row.dest_id)'
hasnt "F6 and not only when it succeeded"     "$F_SCHED" '.bind(ifuploaded_remote{row.dest_id}else{None})'

echo "── §G  a local-only archive does not read as a fresh backup"
# The row unit F adds would otherwise silence three staleness readers, which is
# a narrowing of an existing alarm — the site whose destination is down is
# exactly the one they exist to name.
has "G1 the dashboard tile"       "$F_DASH" '(b.destination_idISNULLORb.uploaded)'
has "G2 the backup-health list"   "$F_ORCH" '(b.destination_idISNULLORb.uploaded)'
has "G3 the 48-hour notification" "$F_EXEC" '(b.destination_idISNULLORb.uploaded)'

echo "── §H  site delete takes the database dumps with it"
has "H1 the delete holds the NAME, not just the container" "$F_BREMOVE" '"SELECTname,container_idFROMdatabasesWHEREsite_id=$1",'
hasnt "H2 and no longer only the container"  "$F_BREMOVE" '"SELECTcontainer_idFROMdatabasesWHEREsite_id=$1'
has "H3 the purge is reached from site delete" "$F_BREMOVE" 'purge_dumps_if_unclaimed('
# ⛔ THE ONLY GUARD, AND IT IS SILENT WHEN WRONG. The probe has no
# self-exclusion term, so above the cascade every row answers the question about
# ITSELF: nothing is purged, and the log line reads exactly like a correct
# decision. No error, no failing request, no symptom.
before "H4 the purge runs AFTER the cascade" "$F_BREMOVE" \
  'DELETEFROMsitesWHEREid=$1' 'purge_dumps_if_unclaimed('
has "H5 doubt still keeps the files"         "$F_PURGE" '.unwrap_or(true);'
has "H6 and the probe still asks about this host" "$F_PURGE" 'WHEREd.name=$1AND(s.server_id=$2ORs.server_idISNULL)'

echo "── §I  the one database name that collides with its own route"
has "I1 the static path has a handler that needs no capture" "$F_ADBRRT" '.route("/db-backups/dump",post(dump).delete(purge_dump))'
hasnt "I2 and not the one that asks for a parameter the URL cannot supply" "$F_ADBRRT" '.delete(purge))'
has "I3 it delegates rather than re-implementing the name check" "$F_PURGEDUMP" 'purge(Path("dump".to_string())).await'

printf '\n%s\n' "passed: $PASS   failed: $FAIL"
[ "$FAIL" -eq 0 ]
