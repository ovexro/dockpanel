#!/usr/bin/env bash
# cli-password-args-pin-e2e.sh — s441
#
# ONE PROPERTY: an operator-chosen database password never has to be typed
# as a bare CLI argument.
#
# `dockpanel db create` and `dockpanel backup db-create` took a required
# clap `--password <VALUE>` flag (CWE-214): the value lands in shell history
# and stays visible to any local user via `ps`/`/proc/<pid>/cmdline` for the
# life of the process. `iac.rs`'s own password print is a DIFFERENT case
# (a server-GENERATED password, printed after the fact — nothing prior ever
# existed to leak); these two took a HUMAN-CHOSEN secret in as an argument,
# the inverse shape.
#
# Fix: `--password` becomes optional, a new `--password-stdin` reads one
# line from stdin instead, and a shared `resolve_password` helper falls
# back to an interactive masked prompt (via `rpassword`, which reads
# /dev/tty directly so it isn't confused by stdin redirection) when neither
# flag is given. `--password` still works for scripts that already use it
# — this closes the door on it being the ONLY way, not on it existing.
#
# §A `password` is optional (`Option<String>`) on both commands, not a bare
#    required `String` — the shape that forced argv exposure.
# §B `--password-stdin` exists on both and conflicts with `--password`
#    (clap `conflicts_with`, so a caller can't send contradictory input).
# §C the shared `resolve_password` helper exists and implements all three
#    tiers: explicit value, stdin line, interactive prompt.
# §D both command functions actually CALL `resolve_password` — position:
#    before the password is embedded in the outgoing request body, so the
#    resolved (not raw) value is what gets sent.
# §E the CLI crate depends on `rpassword` for the masked prompt.
# §F docs no longer document `--password` as the only/required path.

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

echo
echo "=================================================="
echo "  CLI password args — source pins (s441)"
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

has()  { grep -qE -- "$2" <<< "$1"; }

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

# An `enum NAME { ... }` body, bounded on its own braces — scopes the
# variant search below so a same-named variant on a DIFFERENT enum earlier
# in the file (SitesCmd::Create precedes DbCmd::Create) can't be matched by
# accident.
enumbody() {
  awk -v en="$2" '
    index($0, "enum " en " {") && !started { started=1 }
    started {
      n=gsub(/\{/,"{"); m=gsub(/\}/,"}"); depth += n - m; print
      if (opened || n>0) opened=1
      if (opened && depth<=0) exit
    }
  ' <<< "$1"
}

# clap struct-variant bodies are not `fn NAME(...)` — bound instead on the
# variant's OWN name and its opening/closing braces. Call on an enumbody
# result, never on a whole-file body — see enumbody's own comment.
variantbody() {
  awk -v v="$2" '
    index($0, v " {") && !started { started=1 }
    started {
      n=gsub(/\{/,"{"); m=gsub(/\}/,"}"); depth += n - m; print
      if (opened || n>0) opened=1
      if (opened && depth<=0) exit
    }
  ' <<< "$1"
}

offset() {
  local body="$1" needle="$2"
  local head="${body%%"$needle"*}"
  if [ "$head" = "$body" ]; then
    echo -1
  else
    echo "${#head}"
  fi
}

before() {
  local body="$1" first="$2" second="$3"
  local o1 o2
  o1=$(offset "$body" "$first")
  o2=$(offset "$body" "$second")
  [ "$o1" != -1 ] && [ "$o2" != -1 ] && [ "$o1" -lt "$o2" ]
}

MAIN=panel/cli/src/main.rs
MAIN_C=$(code "$MAIN")
DB=panel/cli/src/commands/db.rs
DB_C=$(code "$DB")
BACKUP=panel/cli/src/commands/backup.rs
BACKUP_C=$(code "$BACKUP")
MOD=panel/cli/src/commands/mod.rs
MOD_C=$(code "$MOD")

DBCMD_ENUM=$(enumbody "$MAIN_C" "DbCmd")
BACKUPCMD_ENUM=$(enumbody "$MAIN_C" "BackupCmd")
DB_CREATE_VARIANT=$(variantbody "$DBCMD_ENUM" "Create")
BACKUP_DBCREATE_VARIANT=$(variantbody "$BACKUPCMD_ENUM" "DbCreate")
RESOLVE_BODY=$(fnbody "$MOD_C" "resolve_password")
CMD_DB_CREATE_BODY=$(fnbody "$DB_C" "cmd_db_create")
CMD_DB_BACKUP_CREATE_BODY=$(fnbody "$BACKUP_C" "cmd_db_backup_create")

# ── §A password is Option<String>, not a bare required String ───────────
echo "── §A --password is optional on both commands ──"

if has "$DB_CREATE_VARIANT" 'password:\s*Option<String>'; then
  ok "A1 DbCmd::Create's password field is Option<String>"
else
  bad "A1 DbCmd::Create's password is not Option<String> — still a forced argv value"
fi

if has "$BACKUP_DBCREATE_VARIANT" 'password:\s*Option<String>'; then
  ok "A2 BackupCmd::DbCreate's password field is Option<String>"
else
  bad "A2 BackupCmd::DbCreate's password is not Option<String> — still a forced argv value"
fi

# ── §B --password-stdin exists and conflicts with --password ────────────
echo "── §B --password-stdin exists and is mutually exclusive with --password ──"

if has "$DB_CREATE_VARIANT" 'password_stdin:\s*bool'; then
  ok "B1 DbCmd::Create has a password_stdin flag"
else
  bad "B1 DbCmd::Create has no password_stdin flag"
fi

if has "$DB_CREATE_VARIANT" 'conflicts_with\s*=\s*"password_stdin"'; then
  ok "B2 DbCmd::Create's --password conflicts_with --password-stdin"
else
  bad "B2 no conflicts_with guard — a caller could pass both flags with silently ambiguous precedence"
fi

if has "$BACKUP_DBCREATE_VARIANT" 'password_stdin:\s*bool'; then
  ok "B3 BackupCmd::DbCreate has a password_stdin flag"
else
  bad "B3 BackupCmd::DbCreate has no password_stdin flag"
fi

if has "$BACKUP_DBCREATE_VARIANT" 'conflicts_with\s*=\s*"password_stdin"'; then
  ok "B4 BackupCmd::DbCreate's --password conflicts_with --password-stdin"
else
  bad "B4 no conflicts_with guard on the backup db-create variant"
fi

# ── §C resolve_password implements all three resolution tiers ──────────
echo "── §C resolve_password: explicit value, stdin, interactive prompt ──"

if [ -n "$RESOLVE_BODY" ]; then
  ok "C1 resolve_password exists"
else
  bad "C1 resolve_password is missing — every arm below is measuring nothing"
fi

if has "$RESOLVE_BODY" 'if let Some\(p\) = password'; then
  ok "C2 an explicit --password value is honored first (backward compatible)"
else
  bad "C2 no explicit-value tier — existing scripts using --password would break"
fi

if has "$RESOLVE_BODY" 'password_stdin' && has "$RESOLVE_BODY" 'read_line'; then
  ok "C3 the stdin tier reads a line when password_stdin is set"
else
  bad "C3 no stdin-reading tier found"
fi

if has "$RESOLVE_BODY" 'rpassword::prompt_password'; then
  ok "C4 the fallback tier is an interactive masked prompt"
else
  bad "C4 no interactive prompt fallback — a caller with neither flag would get... what?"
fi

if before "$RESOLVE_BODY" "password_stdin" "rpassword::prompt_password"; then
  ok "C5 stdin is tried before falling through to the interactive prompt"
else
  bad "C5 resolution order is wrong — the prompt could fire even with --password-stdin set"
fi

# ── §D both commands actually CALL resolve_password ──────────────────────
echo "── §D cmd_db_create / cmd_db_backup_create resolve before sending ──"

if has "$CMD_DB_CREATE_BODY" 'resolve_password'; then
  ok "D1 cmd_db_create calls resolve_password"
else
  bad "D1 cmd_db_create never calls resolve_password — the Option<String> plumbing is decorative"
fi

if before "$CMD_DB_CREATE_BODY" "resolve_password" 'json!({'; then
  ok "D2 resolve_password runs before the request body is built"
else
  bad "D2 resolve_password runs after (or the request body doesn't wait for it) — position wrong"
fi

if has "$CMD_DB_BACKUP_CREATE_BODY" 'resolve_password'; then
  ok "D3 cmd_db_backup_create calls resolve_password"
else
  bad "D3 cmd_db_backup_create never calls resolve_password"
fi

if before "$CMD_DB_BACKUP_CREATE_BODY" "resolve_password" 'json!({'; then
  ok "D4 resolve_password runs before the request body is built"
else
  bad "D4 resolve_password runs after (or the request body doesn't wait for it) — position wrong"
fi

# ── §E the CLI crate actually depends on rpassword ───────────────────────
echo "── §E Cargo.toml declares the prompt dependency ──"

if grep -qE '^rpassword\s*=' panel/cli/Cargo.toml; then
  ok "E1 panel/cli/Cargo.toml depends on rpassword"
else
  bad "E1 no rpassword dependency in panel/cli/Cargo.toml — C4 above would not even compile"
fi

# ── §F docs reflect the new, safer default ────────────────────────────────
echo "── §F docs no longer document --password as the only path ──"

DOC=docs/cli-reference.md
if [ -f "$DOC" ] && grep -q -- "--password-stdin" "$DOC"; then
  ok "F1 $DOC documents --password-stdin"
else
  bad "F1 $DOC does not mention --password-stdin"
fi

if [ -f "$DOC" ] && ! grep -qE '\| `--password` \| Yes \|' "$DOC"; then
  ok "F2 $DOC no longer lists --password as Required"
else
  bad "F2 $DOC's db create table still marks --password as Required"
fi

echo
echo "=================================================="
echo "  PASS=$PASS FAIL=$FAIL"
echo "=================================================="

[ "$FAIL" -eq 0 ]
