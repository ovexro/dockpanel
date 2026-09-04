#!/usr/bin/env bash
# cli-io-crate-audit-pin-e2e.sh — s462
#
# panel/cli/src/commands/{db.rs, backup.rs, iac.rs} — the 3 files the s461
# dockpanel-fanout explicitly excluded ("already partially checked" — true
# only for the s440/441 CWE-214 pass). A full finder/skeptic/critics run
# this session found 7 real, independently-verified defects across those
# 3 files plus one off-menu completeness-critic find:
#
#   §A db.rs `rand_byte()` silently returned 0 on /dev/urandom failure —
#      would produce the fully predictable password "aaaaaaaaaaaaaaaa" for
#      a real database with no error surfaced anywhere.
#   §B backup.rs `cmd_backup_verify`'s database lane hardcoded
#      `db_type: "postgres"` — a mysql/mariadb backup could never be
#      successfully verified through the CLI; it always attempted (and
#      failed) a Postgres restore against it.
#   §C backup.rs `cmd_backup_health` folded every top-level entry under
#      /var/backups/dockpanel into "site backups", including the agent's
#      own databases/volumes/wp-snapshots/snapshots roots and an unrelated
#      ops db/ dump directory — live-reproduced ~9x/~50000x off on this box.
#   §D iac.rs `cmd_apply` silently dropped every firewall rule in an
#      exported file — no create, no update, no message, ever.
#   §E iac.rs `cmd_apply` silently no-op'd `ssl: true` when no --email was
#      given — site creation still printed a bare success tick.
#   §F iac.rs `cmd_apply --dry-run` unconditionally reported every cron as
#      "to create", even when byte-identical to what's already scheduled —
#      the one resource type that DOES reconcile on a real apply, so the
#      dry-run plan was actively wrong, not just incomplete.
#   §G (off-menu, completeness critic) agent/services/remote_backup.rs's
#      `run_sftp()` had no `kill_on_drop(true)` — its sibling `s3_curl()`,
#      ~280 lines earlier in the SAME file, already carried this exact fix
#      (v2.204.0) for the same reason: a timed-out/dropped future otherwise
#      leaves sftp/sshpass running detached against the remote host.
#
# Read-only display/plan-time paths mostly; the one behavior change beyond
# messaging is db_type now being a real, caller-supplied value instead of
# a hardcoded literal, and rand_byte's caller now handling failure instead
# of silently proceeding with a zero byte.

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

echo
echo "=================================================="
echo "  CLI/agent io-crate audit — source pins (s462)"
echo "=================================================="
echo

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

code() {
  [ -f "$1" ] || return 0
  perl -0777 -pe '
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
    s{^\s*///.*$}{}gm;
  ' "$1"
}

has()    { grep -qE -- "$2" <<< "$1"; }
lacks()  { ! grep -qE -- "$2" <<< "$1"; }

fnbody() {
  awk -v fn="$2" '
    index($0, "fn " fn "(") && !started { started=1 }
    started {
      n=gsub(/\{/,"{"); m=gsub(/\}/,"}"); depth += n - m; print
      if (opened || n>0) opened=1
      if (opened && depth<=0) exit
    }
  ' <<< "$1"
}

DB=panel/cli/src/commands/db.rs
DB_C=$(code "$DB")
BACKUP=panel/cli/src/commands/backup.rs
BACKUP_C=$(code "$BACKUP")
IAC=panel/cli/src/commands/iac.rs
IAC_C=$(code "$IAC")
MAIN=panel/cli/src/main.rs
MAIN_C=$(code "$MAIN")
REMOTE_BACKUP=panel/agent/src/services/remote_backup.rs
REMOTE_BACKUP_C=$(code "$REMOTE_BACKUP")

RAND_BYTE_BODY=$(fnbody "$DB_C" "rand_byte")
VERIFY_BODY=$(fnbody "$BACKUP_C" "cmd_backup_verify")
HEALTH_BODY=$(fnbody "$BACKUP_C" "cmd_backup_health")
APPLY_BODY=$(fnbody "$IAC_C" "cmd_apply")
SFTP_BODY=$(fnbody "$REMOTE_BACKUP_C" "run_sftp")
S3_CURL_BODY=$(fnbody "$REMOTE_BACKUP_C" "s3_curl")

# ── §A db.rs rand_byte no longer silently zero-fallbacks ────────────────
echo "── §A db.rs: rand_byte surfaces /dev/urandom failure instead of returning 0 ──"

if has "$RAND_BYTE_BODY" 'Result<u8, String>'; then
  ok "A1 rand_byte's signature returns Result<u8, String>, not a bare u8"
else
  bad "A1 rand_byte still returns a bare u8 — a failure has nowhere to go but silent 0"
fi

if lacks "$RAND_BYTE_BODY" '\.ok\(\);' && lacks "$RAND_BYTE_BODY" 'if let Ok\(mut f\)'; then
  ok "A2 rand_byte no longer swallows the open/read failure via if-let/.ok()"
else
  bad "A2 rand_byte still has the silent if-let-Ok/.ok() swallow"
fi

IAC_PW_LOOP=$(fnbody "$IAC_C" "cmd_apply" | grep -A6 'match rand_byte()')
if has "$IAC_PW_LOOP" 'Err\(e\)'; then
  ok "A3 iac.rs's password-generation loop matches rand_byte's Err arm"
else
  bad "A3 iac.rs's password loop does not handle rand_byte's Err — still assumes it can't fail"
fi

# ── §B backup.rs cmd_backup_verify: db_type is a real parameter ─────────
echo "── §B backup.rs: cmd_backup_verify's db_type is caller-supplied, not hardcoded ──"

if has "$VERIFY_BODY" 'db_type: &str' || has "$BACKUP_C" 'fn cmd_backup_verify\([^)]*db_type: &str'; then
  ok "B1 cmd_backup_verify takes a db_type parameter"
else
  bad "B1 cmd_backup_verify has no db_type parameter — still hardcoded"
fi

if lacks "$VERIFY_BODY" '"db_type": "postgres"'; then
  ok "B2 the database-verify body no longer hardcodes db_type to postgres"
else
  bad "B2 still sends a hardcoded postgres db_type — mysql/mariadb verify still always fails"
fi

if has "$VERIFY_BODY" '"db_type": db_type'; then
  ok "B3 the database-verify body forwards the real db_type value"
else
  bad "B3 the database-verify body does not forward db_type"
fi

VERIFY_ARG=$(fnbody "$MAIN_C" "Verify" | head -20)
if has "$MAIN_C" 'db_type: String,' && has "$MAIN_C" 'BackupCmd::Verify \{ r#type, name, filename, db_type \}'; then
  ok "B4 main.rs declares --db-type on Verify and threads it through dispatch"
else
  bad "B4 main.rs does not wire a db_type flag through to cmd_backup_verify"
fi

# ── §C backup.rs cmd_backup_health: scoped to real site-backup dirs ─────
echo "── §C backup.rs: cmd_backup_health excludes non-site subsystem directories ──"

if has "$BACKUP_C" 'RESERVED_TOP_LEVEL_DIRS'; then
  ok "C1 a reserved-directory exclusion list exists"
else
  bad "C1 no reserved-directory exclusion list — miscount is unfixed"
fi

if has "$BACKUP_C" '"databases"' && has "$BACKUP_C" '"volumes"' && has "$BACKUP_C" '"wp-snapshots"' && has "$BACKUP_C" '"db"'; then
  ok "C2 the exclusion list names every known non-site subsystem root (databases/volumes/wp-snapshots/db)"
else
  bad "C2 the exclusion list is missing a known non-site subsystem root"
fi

if has "$HEALTH_BODY" 'is_site_backup_dir'; then
  ok "C3 cmd_backup_health's site-dir count and file walk both consult is_site_backup_dir"
else
  bad "C3 cmd_backup_health no longer filters through is_site_backup_dir — regressed"
fi

if has "$HEALTH_BODY" 'count_files\("/var/backups/dockpanel", true\)'; then
  ok "C4 the site-backup file count passes only_site_dirs=true"
else
  bad "C4 the site-backup file count does not scope to only_site_dirs"
fi

# Positive controls: databases/ and volumes/ counts were never polluted by
# this bug (they scan their own already-scoped root) and must stay unscoped.
if has "$HEALTH_BODY" 'count_files\("/var/backups/dockpanel/databases", false\)' \
  && has "$HEALTH_BODY" 'count_files\("/var/backups/dockpanel/volumes", false\)'; then
  ok "C5 (control) databases/volumes counts remain unscoped — they were never the bug"
else
  bad "C5 (control) databases/volumes count scoping regressed"
fi

# ── §D iac.rs cmd_apply: firewall is no longer a silent no-op ───────────
echo "── §D iac.rs: cmd_apply surfaces that firewall rules are export-only ──"

if has "$APPLY_BODY" 'desired\.get\("firewall"\)'; then
  ok "D1 cmd_apply inspects the desired firewall section"
else
  bad "D1 cmd_apply still never reads desired[\"firewall\"] at all"
fi

if has "$APPLY_BODY" 'does not create, update, or remove firewall rules'; then
  ok "D2 cmd_apply prints an explicit firewall-not-applied warning"
else
  bad "D2 no firewall-not-applied warning found — still silent"
fi

# ── §E iac.rs cmd_apply: ssl-with-no-email is no longer a silent no-op ──
echo "── §E iac.rs: cmd_apply warns when ssl requested but no --email given ──"

if has "$APPLY_BODY" 'no --email was given'; then
  ok "E1 cmd_apply warns when ssl:true has no email to provision with"
else
  bad "E1 cmd_apply still silently no-ops an ssl request with no --email"
fi

# ── §F iac.rs cmd_apply --dry-run: crons are diffed, not all-create ─────
echo "── §F iac.rs: dry-run cron plan diffs against current state ──"

if has "$APPLY_BODY" 'current_crons'; then
  ok "F1 cmd_apply builds a current_crons lookup from /iac/export's own current state"
else
  bad "F1 no current_crons lookup — dry-run still has nothing to diff against"
fi

if has "$APPLY_BODY" 'already exists, unchanged'; then
  ok "F2 an unchanged cron is reported as existing, not \"to create\""
else
  bad "F2 dry-run still cannot report an unchanged cron as anything but \"to create\""
fi

# ── §G remote_backup.rs run_sftp: kill_on_drop closes the orphan gap ────
echo "── §G agent remote_backup.rs: run_sftp no longer orphans its child on timeout ──"

if has "$SFTP_BODY" 'kill_on_drop\(true\)'; then
  ok "G1 run_sftp's command builder now sets kill_on_drop(true)"
else
  bad "G1 run_sftp still has no kill_on_drop — a timed-out transfer orphans sftp/sshpass"
fi

# Positive control: the sibling s3_curl fix (v2.204.0) must still be there.
if has "$S3_CURL_BODY" 'kill_on_drop\(true\)'; then
  ok "G2 (control) s3_curl's pre-existing kill_on_drop(true) is unchanged"
else
  bad "G2 (control) s3_curl's kill_on_drop regressed"
fi

echo
echo "=================================================="
echo "  PASS=$PASS FAIL=$FAIL"
echo "=================================================="

[ "$FAIL" -eq 0 ]
