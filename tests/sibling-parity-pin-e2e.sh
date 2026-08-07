#!/usr/bin/env bash
# sibling-parity-pin-e2e.sh — s323
#
# A fix applied to ONE instance of a pattern owes a grep for the pattern.
#
# Three defects found in one session, all the same shape: a hardening was written,
# was correct, was commented — and its siblings never got it. In each case the
# hardened copy stood next to the unhardened one for months.
#
#   §A  ONE FAIL-CLOSED CRONTAB READER. `crontab` has no partial-update verb, so
#       every writer reads the whole file and writes the whole file back, and an
#       empty read is indistinguishable from "no entries". `routes/crons.rs` grew
#       a fail-closed reader that refuses to treat a failed `crontab -l` as empty.
#       `services/wordpress.rs::set_auto_update` kept
#       `.output().await.map(|o| …stdout…).unwrap_or_default()`, which collapses a
#       spawn failure AND a non-zero exit into "" — then piped that back as root's
#       complete new crontab. Reached from POST /api/sites/{id}/wordpress/auto-update,
#       an ordinary authenticated route naming ONE site, with a box-wide blast
#       radius: every tenant's jobs and every system entry, gone.
#   §B  EVERY SSL WRITE RE-RENDERS THE FULL VHOST. The agent's SSL routes receive a
#       3-field request and invent the other nineteen, so they render a vhost with
#       WAF off, no CSP, and an unversioned php-fpm socket. v2.18.0 added a
#       compensating full rebuild to provision, renew and force-renew. `upload_ssl`
#       is the fourth sibling and was missed, so uploading a certificate stripped a
#       hardened site's directives — and 502'd it outright if it ran PHP.
#   §C  THE PRE-DELETE BACKUP RUNS FIRST. It sat AFTER the call whose agent handler
#       does remove_dir_all on the webroot, and `create_backup` refuses when the
#       site root is missing — so the snapshot taken "before permanent deletion"
#       failed every single time, into a `let _ =`.
#   §D  CONTEXT. Arms that must be green at BOTH tags.

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

echo
echo "=============================================="
echo "  sibling parity — source pins (s323)"
echo "=============================================="
echo

PASS=0; FAIL=0; SKIP=0
ok()   { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

# ⚠ NEVER call `bad` inside `cmd | while read` — the pipeline puts the loop in a
# SUBSHELL and the FAIL increment dies with it, so the arm prints ✗ and the suite
# still exits 0 (s322). Use `done < <(…)`.

code() {
  perl -0777 -pe '
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
    s{^\s*///.*$}{}gm;
  ' "$1"
}

has()  { grep -qE -- "$2" <<< "$1"; }

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

CRONTAB_SVC=panel/agent/src/services/crontab.rs
WP_SVC=panel/agent/src/services/wordpress.rs
PANEL_SITES=panel/backend/src/routes/sites.rs
PANEL_SSL=panel/backend/src/routes/ssl.rs

for f in "$CRONTAB_SVC" "$WP_SVC" "$PANEL_SITES" "$PANEL_SSL"; do
  [ -f "$f" ] || { echo "  FATAL: $f missing — wrong tree?"; exit 1; }
done

# ── §A one fail-closed crontab reader ───────────────────────────────────────
echo "§A  one fail-closed crontab reader"

# Membership is derived from the WORLD — every crontab invocation in the crate —
# not from a list somebody remembered to update. A new writer joins this census by
# EXISTING (lesson #274).
CRON_FILES=""
while read -r f; do
  [ -n "$f" ] || continue
  code "$f" | grep -q 'safe_command\(_sync\)\?("crontab")' && CRON_FILES="${CRON_FILES}${f}"$'\n'
done < <(grep -rl 'safe_command\(_sync\)\?("crontab")' panel/agent/src --include='*.rs' 2>/dev/null)

CRON_N=$(grep -c . <<< "$CRON_FILES" || true)
if [ "${CRON_N:-0}" -lt 2 ]; then
  bad "A0 found only ${CRON_N:-0} file(s) invoking crontab — the enumeration is broken, §A is measuring nothing"
else
  ok "A0 enumerated $CRON_N files invoking crontab (census derived from the tree, not a hardcoded list)"
fi

# A WRITER is an invocation passing "-" (read the new crontab from stdin). Only the
# shared module may be one; everything else must read.
WRITERS=""
while read -r f; do
  [ -n "$f" ] || continue
  if code "$f" | grep -qE '"crontab"\)[^;]*"-"[^;]*\)' \
     || code "$f" | tr '\n' ' ' | grep -qE 'safe_command\("crontab"\)[^;]{0,200}args\(\["-u", "root", "-"\]\)'; then
    WRITERS="${WRITERS}${f}"$'\n'
  fi
done < <(printf '%s' "$CRON_FILES")

STRAY=$(grep -v "^${CRONTAB_SVC}$" <<< "$WRITERS" | grep -c . || true)
if [ "${STRAY:-0}" -eq 0 ]; then
  ok "A1 the only crontab WRITER in the crate is services/crontab.rs"
else
  while read -r f; do
    [ -n "$f" ] || continue
    bad "A1 $f writes the crontab directly instead of via services/crontab.rs — a failed read there deletes every entry on the box"
  done < <(grep -v "^${CRONTAB_SVC}$" <<< "$WRITERS")
fi

CT=$(code "$CRONTAB_SVC")
READER=$(fnbody "$CT" "read_crontab")
if [ -z "$READER" ]; then
  bad "A2 could not extract read_crontab — the arms below measure nothing"
elif has "$READER" 'no crontab for'; then
  ok "A2 the reader distinguishes 'no crontab for' from every other failure"
else
  bad "A2 the reader no longer special-cases the genuinely-absent crontab — it will refuse on a fresh box"
fi

if [ -z "$READER" ]; then
  bad "A3 skipped — no reader"
elif has "$READER" 'o\.status\.success\(\)'; then
  ok "A3 the reader inspects the exit status (a non-zero exit is not an empty crontab)"
else
  bad "A3 the reader ignores the exit status — the exact collapse this module exists to prevent"
fi

# A4 is the arm that would have caught the original defect: the vulnerable shape is
# a crontab read whose failure becomes a default value.
SETAU=$(fnbody "$(code "$WP_SVC")" "set_auto_update")
if [ -z "$SETAU" ]; then
  bad "A4 could not extract set_auto_update — this arm measures nothing"
elif has "$SETAU" 'unwrap_or_default\(\)'; then
  bad "A4 set_auto_update collapses a failed crontab read into an empty string again"
elif has "$SETAU" 'crontab::read_crontab'; then
  ok "A4 set_auto_update reads through the shared fail-closed reader"
else
  bad "A4 set_auto_update does not use the shared reader — it has its own crontab read again"
fi

# ── §B every SSL write re-renders the full vhost ────────────────────────────
echo
echo "§B  every SSL write re-renders the full vhost"

SITES=$(code "$PANEL_SITES")
UPLOAD=$(fnbody "$SITES" "upload_ssl")
if [ -z "$UPLOAD" ]; then
  bad "B0 could not extract upload_ssl — §B measures nothing"
elif has "$UPLOAD" 'rebuild_vhost_after_ssl'; then
  ok "B1 upload_ssl re-renders the full vhost after handing the cert to the agent"
else
  bad "B1 upload_ssl does not rebuild the vhost — uploading a cert strips WAF/CSP and 502s a PHP site"
fi

REBUILDERS=$(grep -rc 'rebuild_vhost_after_ssl' panel/backend/src --include='*.rs' 2>/dev/null | awk -F: '{s+=$2} END {print s+0}')
if [ "$REBUILDERS" -ge 5 ]; then
  ok "B2 the rebuild is referenced $REBUILDERS times (definition + every SSL-writing sibling)"
else
  bad "B2 only $REBUILDERS references to rebuild_vhost_after_ssl — a sibling lost its compensation"
fi

# ── §C the pre-delete backup runs first ─────────────────────────────────────
echo
echo "§C  the pre-delete backup runs before anything destructive"

# The route is DELETE /api/sites/{id}; the Rust handler is `remove`. Named from
# SOURCE rather than from the route, because the first draft of this arm asked
# for `delete_site` and its extraction guard is the only reason that showed up.
DEL=$(fnbody "$SITES" "remove")
if [ -z "$DEL" ]; then
  bad "C0 could not extract the site-delete handler (fn remove) — §C measures nothing"
else
  ok "C0 site-delete handler body extracted ($(wc -l <<< "$DEL") lines)"
  # Ordering, not presence. Presence was never the problem — the call was there
  # the whole time, forty lines too late.
  BK=$(grep -n '/backups/' <<< "$DEL" | head -1 | cut -d: -f1)
  NG=$(grep -n 'nginx/sites/' <<< "$DEL" | tail -1 | cut -d: -f1)
  DB=$(grep -n '/databases/' <<< "$DEL" | head -1 | cut -d: -f1)
  if [ -z "$BK" ]; then
    bad "C1 delete_site no longer takes a pre-delete backup at all"
  elif [ -z "$NG" ] || [ -z "$DB" ]; then
    bad "C1 could not locate the destructive calls in delete_site — this arm is measuring nothing"
  elif [ "$BK" -lt "$NG" ] && [ "$BK" -lt "$DB" ]; then
    ok "C1 the backup (line $BK of the body) precedes the database delete ($DB) and the site-files delete ($NG)"
  else
    bad "C1 the pre-delete backup at body-line $BK runs AFTER a destructive call (db $DB, files $NG) — create_backup refuses once the site root is gone, so it captures nothing"
  fi

  if has "$DEL" 'let _ = agent[[:space:]]*$|let _ = agent\.post\([[:space:]]*&format!\("/backups'; then
    bad "C2 the pre-delete backup's result is discarded again — a permanent failure would be invisible"
  else
    ok "C2 the pre-delete backup's result is not silently discarded"
  fi

  if has "$DEL" '"databases": predelete_dbs\.specs'; then
    ok "C3 the pre-delete backup includes the site's databases (still alive at that point)"
  else
    bad "C3 the pre-delete backup omits the databases — a files-only snapshot of a site being destroyed"
  fi
fi

# ── §D context ──────────────────────────────────────────────────────────────
echo
echo "§D  context (green at both tags)"

if has "$SITES" 'pub async fn remove\('; then
  ok "D1 the site-delete handler still exists"
else
  bad "D1 the site-delete handler is gone — §C measures a deleted feature"
fi

if has "$(code "$PANEL_SSL")" 'fn rebuild_vhost_after_ssl'; then
  ok "D2 the canonical rebuild helper still exists in routes/ssl.rs"
else
  bad "D2 rebuild_vhost_after_ssl is gone — §B cannot be satisfied"
fi

echo
echo "----------------------------------------------"
printf '  PASS %d  FAIL %d  SKIP %d\n' "$PASS" "$FAIL" "$SKIP"
echo "----------------------------------------------"
echo

[ "$FAIL" -eq 0 ] || exit 1
exit 0
