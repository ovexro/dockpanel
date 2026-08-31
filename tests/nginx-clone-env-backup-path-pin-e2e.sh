#!/usr/bin/env bash
# nginx-clone-env-backup-path-pin-e2e.sh — s438
#
# Pins the two remaining defense-in-depth findings from the s436
# `dockpanel-fanout` round ([[project_dockpanel_tech_debt_p173]], Findings 1
# and 2). Both were confirmed NOT reachable via the panel API today — every
# current caller sources its domain/filepath from a DB-validated column, not
# straight from an admin-supplied field — but the handlers themselves had no
# validation of their own, so a future caller (or a future backend change
# that trusts them) would inherit the gap silently.
#
#   §A  nginx.rs: clone_site/get_env/set_env were 3 of ~28 domain-taking
#       handlers that skipped `is_valid_domain` while every sibling calls it
#       first. The live systemd unit's `ReadWritePaths` allowlist includes
#       `/etc/systemd/system` and `/var/spool/cron` — so an unvalidated
#       domain segment (`../../etc/systemd/system`) is root code execution
#       via cron/systemd-unit overwrite, not merely a stray file write.
#   §B  remote_backup.rs's upload() gated the backup filepath with a lexical
#       `starts_with()` check that "/var/backups/dockpanel/../../etc/shadow"
#       satisfies — the OS resolves the `..` on open. ProtectSystem=strict
#       blocks writes outside the allowlist but not READS, so an unguarded
#       path here is arbitrary root-readable-file exfiltration (shadow,
#       Let's Encrypt private keys, .env secrets) to an attacker-supplied
#       S3/SFTP destination.
#
# Pure source analysis: no box, no network, no build.

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

echo
echo "=============================================="
echo "  nginx clone/env + remote-backup path validation — source pins (s438)"
echo "=============================================="
echo

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

# Strip comments before matching (per [[feedback_source_pin_prose_trap]] — this
# file's own header spells the tokens the arms grep for).
code() {
  perl -0777 -pe '
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*///.*$}{}gm;
    s{^\s*//.*$}{}gm;
  ' "$1"
}

has()  { grep -qE -- "$2" <<< "$1"; }
flat() { tr '\n' ' ' <<< "$1" | tr -s ' '; }

# A function body, bounded on ITS OWN braces (s323 fix — a window ending at
# the successor's match position swallows the next function's declaration).
fnbody() {
  awk -v fn="$2" '
    index($0, "fn " fn) && !started { started=1 }
    started {
      n=gsub(/\{/,"{"); m=gsub(/\}/,"}"); depth += n - m; print
      if (opened || n>0) opened=1
      if (opened && depth<=0) exit
    }
  ' <<< "$1"
}

NGINX=panel/agent/src/routes/nginx.rs
BACKUP=panel/agent/src/routes/remote_backup.rs

for f in "$NGINX" "$BACKUP"; do
  [ -f "$f" ] || { bad "SETUP subject missing: $f"; exit 1; }
done

NGINX_SRC=$(code "$NGINX")
BACKUP_SRC=$(code "$BACKUP")

# ── §A  nginx.rs: clone_site/get_env/set_env now validate their domain(s) ────

CLONE=$(flat "$(fnbody "$NGINX_SRC" "clone_site")")
if [ -z "$CLONE" ]; then
  bad "A0 could not extract nginx::clone_site"
else
  ok "A0 nginx::clone_site extracted"
  if has "$CLONE" 'is_valid_domain\(&body\.source_domain\)' && has "$CLONE" 'is_valid_domain\(&body\.target_domain\)'; then
    ok "A1 clone_site validates BOTH source_domain and target_domain"
  else
    bad "A1 clone_site is missing domain validation on one or both fields — rsync --delete runs on an unvalidated path"
  fi
  CHK_AT=$(grep -boE 'is_valid_domain\(&body\.(source|target)_domain\)' <<< "$CLONE" | head -1 | cut -d: -f1)
  RSYNC_AT=$(grep -bo 'safe_command("rsync")' <<< "$CLONE" | head -1 | cut -d: -f1)
  if [ -n "$CHK_AT" ] && [ -n "$RSYNC_AT" ] && [ "$CHK_AT" -lt "$RSYNC_AT" ]; then
    ok "A2 the validation precedes the rsync call"
  else
    bad "A2 the validation does not precede the rsync call (check@${CHK_AT:-none} rsync@${RSYNC_AT:-none})"
  fi
fi

GETENV=$(flat "$(fnbody "$NGINX_SRC" "get_env")")
if [ -z "$GETENV" ]; then
  bad "A3 could not extract nginx::get_env"
else
  ok "A3 nginx::get_env extracted"
  if has "$GETENV" 'is_valid_domain\(&domain\)'; then
    ok "A4 get_env validates domain before building the .env path"
  else
    bad "A4 get_env has no domain validation — reads an arbitrary root-readable file via a crafted domain segment"
  fi
  if has "$GETENV" '-> Result<Json<serde_json::Value>, ApiErr>'; then
    ok "A5 get_env's return type was widened to Result so it can actually reject a bad domain"
  else
    bad "A5 get_env's signature no longer returns a Result — it cannot reject anything, regardless of what A4 found"
  fi
fi

SETENV=$(flat "$(fnbody "$NGINX_SRC" "set_env")")
if [ -z "$SETENV" ]; then
  bad "A6 could not extract nginx::set_env"
else
  ok "A6 nginx::set_env extracted"
  if has "$SETENV" 'is_valid_domain\(&domain\)'; then
    ok "A7 set_env validates domain before writing the .env path"
  else
    bad "A7 set_env has no domain validation — writes an arbitrary path via a crafted domain segment"
  fi
fi

# A8 — the census floor. 25 sibling handlers already called is_valid_domain
# before this fix (finder- and skeptic-verified at s436); this fix adds 4 new
# call sites (clone_site x2, get_env, set_env). A future removal of any of
# these checks drops the total below this floor; a future ADDITION does not
# break it, so this is deliberately a floor, not an exact-equality census
# (the heavier enumeration [[project_dockpanel_lessons_p60]] describes is not
# proportionate to a DiD-only, not-currently-reachable finding).
N_CALLS=$(grep -oF 'is_valid_domain(' "$NGINX" | wc -l)
if [ "$N_CALLS" -ge 31 ]; then
  ok "A8 nginx.rs calls is_valid_domain at least 31 times (found $N_CALLS) — the 4 new call sites are present"
else
  bad "A8 nginx.rs calls is_valid_domain only $N_CALLS times, expected >= 31 — a validation call site was lost"
fi

# ── §B  remote_backup.rs: upload() rejects '..' in the filepath ──────────────

UPLOAD=$(flat "$(fnbody "$BACKUP_SRC" "upload")")
if [ -z "$UPLOAD" ]; then
  bad "B0 could not extract remote_backup::upload"
else
  ok "B0 remote_backup::upload extracted"
  if has "$UPLOAD" 'starts_with\("/var/backups/dockpanel/"\)' && has "$UPLOAD" 'body\.filepath\.contains\("\.\."\)'; then
    ok "B1 upload rejects BOTH a non-prefixed path AND a lexical '..' traversal"
  else
    bad "B1 upload is missing the prefix check, the '..' rejection, or both — a crafted filepath can read outside /var/backups/dockpanel/"
  fi
  # B2 — the two conditions must be OR'd together in one guard, not two
  # separate ifs where an early return on the first could let a later
  # "and only if that passed" mutation quietly drop the second (both must
  # gate the SAME single rejection).
  if has "$UPLOAD" 'starts_with\("/var/backups/dockpanel/"\) \|\| body\.filepath\.contains\("\.\."\)'; then
    ok "B2 both conditions are OR'd into a single rejection guard"
  else
    bad "B2 the prefix check and the '..' rejection are not combined into one guard — re-read upload(), a mutation may have split them"
  fi
fi

echo
echo "----------------------------------------------"
printf '  PASS %d  FAIL %d\n' "$PASS" "$FAIL"
echo "----------------------------------------------"
echo

[ "$FAIL" -eq 0 ] || exit 1
exit 0
