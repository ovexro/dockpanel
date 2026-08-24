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
  code "$f" | grep -c 'safe_command\(_sync\)\?("crontab")' >/dev/null && CRON_FILES="${CRON_FILES}${f}"$'\n'
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
  if code "$f" | grep -cE '"crontab"\)[^;]*"-"[^;]*\)' >/dev/null \
     || code "$f" | tr '\n' ' ' | grep -cE 'safe_command\("crontab"\)[^;]{0,200}args\(\["-u", "root", "-"\]\)' >/dev/null; then
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

# ── B3/B4 — the arm B2 could not be ────────────────────────────────────────
#
# B2 is a COUNT, and a count cannot say WHICH doors need compensating. At s398
# `routes/mail.rs` grew TWO calls to `/ssl/provision/{domain}` with no rebuild
# and B2 stayed green the whole time, because the total never fell below five.
# Measured consequence on a box: adding a mail domain for a name that also had a
# website re-rendered that website as STATIC — 403 on every PHP request — and
# dropped its `limit_req_zone`, while the panel went on reporting `runtime = php`.
#
# So DERIVE the doors instead of counting the cure. A panel file that calls an
# agent SSL route which RE-RENDERS the vhost (`/ssl/provision`,
# `/ssl/provision-dns01`, `/ssl/{domain}/renew` — NOT `/ssl/status`,
# `/ssl/profiles` or `/ssl/{domain}/renewal-info`, which render nothing) must put
# the site's real configuration back afterwards.
#
# ⚠ The per-door token is deliberately a DIFFERENT literal per file rather than
# one shared pattern: `build_nginx_body` is DEFINED in routes/sites.rs, so an arm
# spelling that token there could never fail — it would be satisfied by the
# definition it was meant to police.
# `routes/mod.rs` is excluded on purpose: it matches as the ROUTE TABLE
# (`/api/ssl/{id}/renew`), not as a caller of the agent.
RENDER_DOORS=$(grep -rlE 'ssl/provision|/ssl/\{[a-z_]*\}/renew|/ssl/\{\}/renew' panel/backend/src --include='*.rs' 2>/dev/null \
               | grep -vE '/routes/mod\.rs$' | sort || true)
DOOR_N=$(printf '%s\n' "$RENDER_DOORS" | grep -c . || true)
if [ "${DOOR_N:-0}" -lt 4 ]; then
  bad "B3 only $DOOR_N vhost-rendering SSL doors enumerated — the derivation broke, this arm measures nothing"
else
  ok "B3 $DOOR_N panel files call a vhost-rendering agent SSL route"
  B4_BAD=""
  for f in $RENDER_DOORS; do
    case "$f" in
      *routes/ssl.rs)                COMP='rebuild_vhost_after_ssl\(' ;;
      *routes/sites.rs)              COMP='rebuild_vhost_for_site\(' ;;
      *routes/mail.rs)               COMP='rebuild_vhost_for_domain\(' ;;
      *services/auto_healer.rs)      COMP='build_nginx_body\(' ;;
      *services/security_scanner.rs) COMP='build_nginx_body\(' ;;
      *) B4_BAD="$B4_BAD $f(UNKNOWN-DOOR)"; continue ;;
    esac
    grep -qE -- "$COMP" "$f" 2>/dev/null || B4_BAD="$B4_BAD $f"
  done
  if [ -n "$B4_BAD" ]; then
    bad "B4 SSL-writing door(s) with no vhost compensation:$B4_BAD — an SSL write there publishes a vhost with no limits and no hardening (UNKNOWN-DOOR = a new door nobody has decided about)"
  else
    ok "B4 every vhost-rendering SSL door puts the site's configuration back"
  fi
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

  # FLATTENED (#383). This used to be an alternation of hand-guessed wrap shapes,
  # which only ever covers the wraps somebody thought of: the `.await` chain break
  # was covered, the parent-attached wrap rustfmt emits for a chain without one was
  # not. An ABSENCE arm that cannot match fails SILENTLY. Flatten, then allow the
  # bounded gaps flattening leaves — ' *' only, never '.*'.
  DEL_FLAT=$(tr '\n\t' '  ' <<< "$DEL" | tr -s ' ')
  if has "$DEL_FLAT" 'let _ = agent *\. *post\( *&format!\( *"/backups'; then
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

# ── §E one wp-cli invocation ────────────────────────────────────────────────
# s372. `services/wordpress.rs` owns the wp-cli invocation: it installs the
# binary at a known path, passes `--allow-root` (the agent runs as root and
# wp-cli REFUSES outright without it) and decides the plugin/theme skip policy.
# `services/wp_vulnerability.rs` hand-rolled a second one and got both halves
# wrong — it shelled out to `php` against a phar under the agent's private tmp,
# which nothing in this tree writes and `PrivateTmp=yes` puts out of reach, and
# its one non-phar arm omitted `--allow-root`. So every wp-cli call in the
# vulnerability scanner failed, the plugin-list branch was an `if success` with
# no `else`, and a scan that had never read a plugin was recorded as
# `total_vulns: 0` — a clean bill of health for a site nothing had looked at.
#
# Membership is derived from the crate, not from a list: a new invocation joins
# this census by EXISTING (#274), and E2 asserts checked == total so a site that
# skips the flag cannot hide behind the ones that carry it (#565).
echo
echo "§E  one wp-cli invocation"

WP_VULN=panel/agent/src/services/wp_vulnerability.rs
[ -f "$WP_VULN" ] || { echo "  FATAL: $WP_VULN missing — wrong tree?"; exit 1; }

# E0 — the census, printed as its own positive control (#480).
WPCLI_FILES=""
while read -r f; do
  [ -n "$f" ] || continue
  WPCLI_FILES="${WPCLI_FILES}${f}"$'\n'
done < <(grep -rlE 'safe_command(_sync|_unsandboxed)?\((WP_CLI|"wp")\)' panel/agent/src --include='*.rs' 2>/dev/null | sort)

WPCLI_N=$(grep -c . <<< "$WPCLI_FILES" || true)
if [ "${WPCLI_N:-0}" -lt 1 ]; then
  bad "E0 found no wp-cli invocation anywhere in the agent — the enumeration is broken, §E is measuring nothing"
else
  ok "E0 enumerated ${WPCLI_N} file(s) invoking wp-cli: $(tr '\n' ' ' <<< "$WPCLI_FILES" | sed 's/ $//')"
fi

# E1 — only the owning module may invoke it.
WP_STRAY=$(grep -v "^${WP_SVC}$" <<< "$WPCLI_FILES" | grep -c . || true)
if [ "${WP_STRAY:-0}" -eq 0 ]; then
  ok "E1 services/wordpress.rs is the only module invoking wp-cli"
else
  while read -r f; do
    [ -n "$f" ] || continue
    bad "E1 $f invokes wp-cli directly instead of through services/wordpress.rs — a second copy is how the last one drifted onto a binary that does not exist"
  done < <(grep -v "^${WP_SVC}$" <<< "$WPCLI_FILES")
fi

# E2 — EVERY invocation carries --allow-root, asserted in both directions.
# Each call site is bounded by the code's own punctuation — from the
# `safe_command(...)` to that statement's `.output()` — never a fixed -A n (#172).
WP_TOTAL=0; WP_ROOTED=0
while read -r stmt; do
  [ -n "$stmt" ] || continue
  WP_TOTAL=$((WP_TOTAL+1))
  case "$stmt" in *'--allow-root'*) WP_ROOTED=$((WP_ROOTED+1));; esac
done < <(code "$WP_SVC" | perl -0777 -ne 'while (/safe_command(?:_sync|_unsandboxed)?\((?:WP_CLI|"wp")\)(.*?)\.output\(\)/gs) { my $s=$1; $s =~ s/\s+/ /g; print "$s\n" }')

if [ "$WP_TOTAL" -eq 0 ]; then
  bad "E2 extracted no wp-cli call statements from services/wordpress.rs — this arm measures nothing"
elif [ "$WP_ROOTED" -eq "$WP_TOTAL" ]; then
  ok "E2 all $WP_TOTAL wp-cli call site(s) pass --allow-root (the agent runs as root; wp-cli refuses without it)"
else
  bad "E2 only $WP_ROOTED of $WP_TOTAL wp-cli call sites pass --allow-root — the ones that do not fail outright as root, exactly as the vulnerability scanner's did"
fi

# E3 — nothing anywhere runs a phar through the php interpreter. Anchored on the
# INVOCATION, not on the filename, so `ensure_cli`'s download URL is not a hit.
PHAR_HITS=$(for f in $(grep -rl 'safe_command' panel/agent/src --include='*.rs'); do
  code "$f" | perl -0777 -ne 'while (/safe_command(?:_sync|_unsandboxed)?\("php"\)(.*?)\.output\(\)/gs) { print "HIT\n" if $1 =~ /\.phar/ }'
done | grep -c . || true)
if [ "${PHAR_HITS:-0}" -eq 0 ]; then
  ok "E3 no agent code runs a .phar through the php interpreter"
else
  bad "E3 ${PHAR_HITS} php invocation(s) name a .phar — the agent's tmp is private, so a phar staged there is unreachable and the call can only fail"
fi

# E4 — the arm that would have caught the original defect: the scanner takes its
# plugin list from the shared entry point and PROPAGATES the failure.
SCAN=$(fnbody "$(code "$WP_VULN")" "scan_site")
if [ -z "$SCAN" ]; then
  bad "E4 could not extract scan_site — this arm measures nothing"
elif ! has "$SCAN" 'wp_at_root'; then
  bad "E4 scan_site no longer goes through the shared wp-cli entry point — it has its own invocation again"
elif has "$SCAN" 'status\.success\(\)'; then
  bad "E4 scan_site branches on an exit status again — the shape whose missing else recorded an unrun scan as total_vulns: 0"
else
  ok "E4 scan_site lists plugins through the shared entry point and propagates a failure instead of reporting zero"
fi

# E5 — wp-cli's `update` field is a STATUS; the version lives in `update_version`.
if [ -z "$SCAN" ]; then
  bad "E5 skipped — no scan_site"
elif has "$SCAN" 'update_version'; then
  ok "E5 the reported latest_version comes from update_version, not from the update status word"
else
  bad "E5 scan_site reports a version taken from the update STATUS again — 'available' is not a version, and the outdated test inverts"
fi

# E6 — the repaired failure has to REACH the operator. `error.rs` passes an
# agent's sentence through on a 4xx and replaces anything else with
# "Operation failed. Reference: <uuid>" (#556). `scan_site` could not fail
# before, so this route's error arm was dead code; teaching it to fail armed a
# collapse that had never been observed (#506). The count is printed, not
# asserted, because the other routes in that file are a pre-existing carry —
# but it is printed so the size stays visible on every run.
WP_ROUTES=panel/agent/src/routes/wordpress.rs
if [ ! -f "$WP_ROUTES" ]; then
  bad "E6 $WP_ROUTES missing — this arm measures nothing"
else
  SCAN_ARM=$(fnbody "$(code "$WP_ROUTES")" "vuln_scan")
  COLLAPSING=$(awk '/^async fn |^pub async fn /{fn=$0; sub(/^.*fn /,"",fn); sub(/\(.*/,"",fn)} /StatusCode::INTERNAL_SERVER_ERROR/{print fn}' "$WP_ROUTES" | sort -u | grep -c . || true)
  if [ -z "$SCAN_ARM" ]; then
    bad "E6 could not extract vuln_scan — this arm measures nothing"
  elif has "$SCAN_ARM" 'INTERNAL_SERVER_ERROR'; then
    bad "E6 vuln_scan returns a 5xx again — the panel replaces it with 'Operation failed. Reference: <uuid>', so a scan that could not run is opaque instead of merely wrong"
  else
    ok "E6 a scan that cannot run reports its reason as a 4xx, so the sentence survives the panel boundary (${COLLAPSING} other route(s) in that file still collapse — pre-existing carry)"
  fi
fi

# E7 — the one hardening fix that does not go through wp-cli writes into
# `wp-content/uploads`, which WordPress core DOES NOT SHIP; it appears on the
# first media upload. A bare write there failed with ENOENT on every freshly
# installed site — the moment an operator hardens one. Creating it is only half
# the fix: a root-owned uploads directory stops WordPress writing media at all,
# so the chown is asserted with it.
# Subject is the extracted helper, not `apply_hardening` — extracting a helper
# moves the subject of every pin that measured it (#555), and this arm was
# written against the inline version one commit earlier.
UPLOADS=$(fnbody "$(code "$WP_VULN")" "block_php_uploads")
# Flattened, because the chown spans four source lines and `has` is line-based —
# a multi-line pattern in a line-based grep can never fire (#409).
UPLOADS_FLAT=$(tr '\n' ' ' <<< "$UPLOADS" | tr -s ' ')
if [ -z "$UPLOADS" ]; then
  bad "E7 could not extract block_php_uploads — this arm measures nothing"
elif ! has "$UPLOADS" 'create_dir_all'; then
  bad "E7 the uploads hardening writes .htaccess without creating wp-content/uploads — ENOENT on every fresh install, which is exactly when a site is hardened"
elif ! has "$UPLOADS_FLAT" 'safe_command\("chown"\)[^;]*uploads_dir'; then
  # Anchored on the invocation AND its argument, not on the bare word: `chown`
  # as a substring survives a rename to `chownX` and the arm reads green (#564).
  bad "E7 the uploads directory is created but never handed to the web user — a root-owned uploads dir stops WordPress writing media, which is worse than skipping the hardening"
else
  ok "E7 the uploads hardening creates its parent directory and gives it to the web user"
fi

# E8 — a hardening CHECK must read the value bound to its own constant.
# Every wp-config check used to ask two independent questions of the whole file
# — is the name present anywhere, is the value literal present anywhere — and
# answered pass when both were true of two unrelated lines. `apply_hardening`
# writes exactly those literals, so ONE hardening run pinned three of the four
# green for ever: the fix disarmed the verifier. Population is DERIVED from what
# apply_hardening actually manages, never from a list written here (#551).
HARDEN=$(fnbody "$(code "$WP_VULN")" "apply_hardening")
CHECKFN=$(fnbody "$(code "$WP_VULN")" "check_security")
MANAGED=$(grep -oE 'set_wp_constant\(&root, "[A-Z_0-9]+"' <<< "$HARDEN" | grep -oE '"[A-Z_0-9]+"' | tr -d '"' | sort -u)
MANAGED_N=$(grep -c . <<< "$MANAGED" || true)

if [ -z "$HARDEN" ] || [ -z "$CHECKFN" ]; then
  bad "E8 could not extract apply_hardening or check_security — this arm measures nothing"
elif [ "${MANAGED_N:-0}" -eq 0 ]; then
  bad "E8 derived no wp-config constants from apply_hardening — this arm measures nothing"
else
  E8_BAD=0
  while read -r c; do
    [ -n "$c" ] || continue
    if ! has "$CHECKFN" "wp_constant\(&code, \"$c\"\)"; then
      bad "E8 check_security does not read $c through wp_constant — a check that does not read its own constant's value grades something else"
      E8_BAD=$((E8_BAD+1))
    fi
    if has "$CHECKFN" "wp_config\.contains\(\"$c\""; then
      bad "E8 check_security tests the whole file for $c again — any occurrence of the value literal anywhere satisfies it, and apply_hardening writes that literal"
      E8_BAD=$((E8_BAD+1))
    fi
  done <<< "$MANAGED"
  [ "$E8_BAD" -eq 0 ] && ok "E8 all $MANAGED_N hardening-managed constant(s) are graded on the value bound to their own name"
fi

# E9 — the fix has to be REACHABLE. Both fix controls used to render only for
# `status === "fail"`, while `auto-updates` grades an absent constant as
# `warning` — so a fix that exists, works, and is advertised as auto-fixable
# had no button and was excluded from the bulk run. Same class as a route with
# no caller: the capability shipped and no operator could reach it.
WP_UI=panel/frontend/src/pages/WordPressToolkit.tsx
if [ ! -f "$WP_UI" ]; then
  bad "E9 $WP_UI missing — this arm measures nothing"
elif ! has "$(cat "$WP_UI")" 'check\.status !== "pass" && check\.auto_fixable'; then
  bad "E9 the per-check Fix button gates on something other than 'not passing' — an auto-fixable check that grades warning loses its control"
elif ! has "$(cat "$WP_UI")" 'c\.status !== "pass" && c\.auto_fixable'; then
  bad "E9 the bulk fix list is not built from every non-passing auto-fixable check — a fix outside it can never run"
elif has "$(cat "$WP_UI")" 'check\.status === "fail" && check\.auto_fixable'; then
  bad "E9 a fix control gates on 'fail' again — auto-updates never grades fail when its constant is absent, so its working backend arm becomes unreachable"
else
  ok "E9 both fix controls render for any non-passing auto-fixable check, so no working fix is stranded behind a status word"
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
