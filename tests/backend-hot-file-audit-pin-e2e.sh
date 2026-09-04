#!/usr/bin/env bash
# backend-hot-file-audit-pin-e2e.sh — s463
#
# panel/backend/src/{routes,services} had never had its own exhaustion status
# directly re-verified (agent crate: s461, panel/cli/src: s462). A LOC-rank +
# two-direction pre-selection check (git-log + memory-mention grep, filtering
# a cosmetic mass-commit that touches nearly every backend file) surfaced 5
# files with genuinely zero prior dedicated scrutiny: dns.rs, security.rs +
# security_hardening.rs, update.rs + panel_update.rs. A full finder/skeptic/
# critics fan-out found 4 real, independently-verified defects across those
# files plus one off-menu completeness-critic find in databases.rs:
#
#   §A dns.rs `update_record`/`delete_record` interpolated an unvalidated
#      `record_id` path segment straight into the outbound Cloudflare API
#      URL — a "../../../zones/OTHER/dns_records/RECID"-shaped id (percent-
#      encoded to survive the reverse proxy) reroutes the request, carrying
#      the zone's own decrypted CF token, to an arbitrary Cloudflare API
#      path. PowerDNS's record_id is a different, safely-parsed hex-encoded
#      composite (pdns_parse_record_id) — unaffected, left alone.
#   §B security.rs `compliance_report` built an HTML report by raw
#      format!-interpolating scan-derived title/description/remediation
#      with no escaping, served as text/html and opened in a new tab by the
#      frontend's own "Download Report" link — a stored XSS reachable the
#      moment a compromised hosted site names a file with HTML in its path
#      (malware/secrets scans record raw filesystem paths verbatim).
#   §C (off-menu, completeness critic) databases.rs `create()` authorizes
#      via the fleet-wide SITE_CALLER_PREDICATE (any site an admin manages,
#      including a tenant's site on the local box) but every OTHER handler
#      — list/credentials/remove/get_db_info (the chokepoint for 11 more
#      handlers)/pitr_config — required strict self-ownership, so a database
#      an admin creates on a site they don't personally own becomes
#      permanently unmanageable by its own creator. Same bug-class shape
#      this file family has been fixed for 5 times before.
#   §D panel_update.rs `build_fleet_plan` scoped fleet-update plans to
#      `servers.user_id = caller` with no admin-widening — a fleet-wide
#      update run silently skipped every server a DIFFERENT admin had
#      registered, reporting success with no indication the plan was
#      incomplete. Sibling call sites (servers.rs::list, dashboard.rs) had
#      already established the admin-widened convention; this one never got
#      it.
#   §E update.rs `apply_fleet`'s `include_panel` path reached
#      `start_panel_update` with only a shape check on target_version —
#      none of `apply_update`'s direction/advertised-match guard
#      (`reject_apply_target`), so it could downgrade the panel to any
#      syntactically-valid version, not merely a stale-advertised one.
#   §F panel_update.rs `start_panel_update`'s concurrent-apply guard was
#      read-check-then-later-write, with the multi-minute create_snapshot
#      pipeline running entirely inside the gap — two requests within that
#      window both passed the check and both spawned their own snapshot +
#      detached update.sh.
#   §G panel_update.rs `rollback_to_snapshot` had no in-flight check at
#      all, so `/api/update/rollback` could race update.sh's binary swap
#      against restore-snapshot.sh's binary swap on the same files.
#
# Read-only/authz-widening changes mostly; the one behavior addition beyond
# tightening is databases.rs's admin-visibility widening, which grants
# nothing create() didn't already grant — it makes read/manage consistent
# with what create() already authorized.

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

echo
echo "=================================================="
echo "  Backend hot-file audit — source pins (s463)"
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

DNS=panel/backend/src/routes/dns.rs
DNS_C=$(code "$DNS")
SECURITY=panel/backend/src/routes/security.rs
SECURITY_C=$(code "$SECURITY")
DATABASES=panel/backend/src/routes/databases.rs
DATABASES_C=$(code "$DATABASES")
PANEL_UPDATE=panel/backend/src/services/panel_update.rs
PANEL_UPDATE_C=$(code "$PANEL_UPDATE")
UPDATE=panel/backend/src/routes/update.rs
UPDATE_C=$(code "$UPDATE")

UPDATE_RECORD_BODY=$(fnbody "$DNS_C" "update_record")
DELETE_RECORD_BODY=$(fnbody "$DNS_C" "delete_record")
COMPLIANCE_BODY=$(fnbody "$SECURITY_C" "compliance_report")
DB_LIST_BODY=$(fnbody "$DATABASES_C" "list")
DB_CREDENTIALS_BODY=$(fnbody "$DATABASES_C" "credentials")
DB_REMOVE_BODY=$(fnbody "$DATABASES_C" "remove")
DB_GET_INFO_BODY=$(fnbody "$DATABASES_C" "get_db_info")
DB_PITR_CONFIG_BODY=$(fnbody "$DATABASES_C" "pitr_config")
BUILD_FLEET_PLAN_BODY=$(fnbody "$PANEL_UPDATE_C" "build_fleet_plan")
START_UPDATE_BODY=$(fnbody "$PANEL_UPDATE_C" "start_panel_update")
ROLLBACK_BODY=$(fnbody "$PANEL_UPDATE_C" "rollback_to_snapshot")
APPLY_FLEET_BODY=$(fnbody "$UPDATE_C" "apply_fleet")

# ── §A dns.rs: record_id validated before URL interpolation ─────────────
echo "── §A dns.rs: CF record_id is allowlisted (32-char hex) before use ──"

if has "$UPDATE_RECORD_BODY" 'record_id\.len\(\) != 32' && has "$UPDATE_RECORD_BODY" 'is_ascii_hexdigit'; then
  ok "A1 update_record validates record_id as 32-char hex on the cloudflare arm"
else
  bad "A1 update_record's cloudflare arm no longer validates record_id shape"
fi

if has "$DELETE_RECORD_BODY" 'record_id\.len\(\) != 32' && has "$DELETE_RECORD_BODY" 'is_ascii_hexdigit'; then
  ok "A2 delete_record validates record_id as 32-char hex on the cloudflare arm"
else
  bad "A2 delete_record's cloudflare arm no longer validates record_id shape"
fi

# Positive control: the PowerDNS arm's record_id (a hex-encoded composite of
# variable length, decoded via pdns_parse_record_id) must NOT be forced
# through the same fixed-32-hex check — it was never the bug and a blanket
# fix would break every PowerDNS record update/delete.
if has "$UPDATE_RECORD_BODY" 'pdns_parse_record_id'; then
  ok "A3 (control) update_record's powerdns arm still parses record_id via pdns_parse_record_id"
else
  bad "A3 (control) powerdns record parsing regressed"
fi

# ── §B security.rs: compliance_report escapes scan-derived HTML ─────────
echo "── §B security.rs: compliance_report HTML-escapes findings before interpolation ──"

if has "$SECURITY_C" 'fn html_escape'; then
  ok "B1 an html_escape helper exists in security.rs"
else
  bad "B1 no html_escape helper found"
fi

if has "$COMPLIANCE_BODY" 'html_escape\(severity\)' \
  && has "$COMPLIANCE_BODY" 'html_escape\(title\)' \
  && has "$COMPLIANCE_BODY" 'html_escape\(description\)'; then
  ok "B2 severity/title/description are escaped before interpolation"
else
  bad "B2 one of severity/title/description is no longer escaped — regressed"
fi

if has "$COMPLIANCE_BODY" 'remediation\.as_deref\(\)\.map\(html_escape\)'; then
  ok "B3 remediation is escaped via its Option map, not interpolated raw"
else
  bad "B3 remediation is no longer escaped"
fi

# Negative control: the OLD unescaped format! shape (bare {severity}/{title}/
# {description} placeholders with no escape call) must be gone.
if lacks "$COMPLIANCE_BODY" '"<tr><td[^"]*\{severity\}'; then
  ok "B4 (control) the old raw {severity}-in-format! shape is gone"
else
  bad "B4 (control) findings_html still interpolates a raw {severity} placeholder"
fi

# ── §C databases.rs: read/manage handlers admin-widened to match create() ─
echo "── §C databases.rs: list/credentials/remove/get_db_info/pitr_config admin-widened ──"

for pair in "DB_LIST_BODY:C1:list" "DB_CREDENTIALS_BODY:C2:credentials" "DB_REMOVE_BODY:C3:remove" "DB_GET_INFO_BODY:C4:get_db_info"; do
  var="${pair%%:*}"; rest="${pair#*:}"; id="${rest%%:*}"; name="${rest#*:}"
  body="${!var}"
  if has "$body" "u\.role = 'admin'" && has "$body" 'sv\.is_local OR sv\.user_id = u\.id'; then
    ok "$id $name() carries the admin-widening EXISTS clause"
  else
    bad "$id $name() is missing the admin-widening EXISTS clause — creator/reader mismatch regressed"
  fi
done

if has "$DB_PITR_CONFIG_BODY" "u\.role = 'admin'" && has "$DB_PITR_CONFIG_BODY" 'sv\.is_local OR sv\.user_id = u\.id'; then
  ok "C5 pitr_config's inline ownership check carries the same admin-widening"
else
  bad "C5 pitr_config's inline check is missing the admin-widening"
fi

# Positive control: create()'s own predicate (the source of truth these were
# widened to match) must still be there, unchanged.
if has "$DATABASES_C" 'SITE_CALLER_PREDICATE'; then
  ok "C6 (control) create() still uses helpers::SITE_CALLER_PREDICATE"
else
  bad "C6 (control) create()'s own predicate regressed"
fi

# ── §D panel_update.rs: build_fleet_plan sees the whole fleet for admins ──
echo "── §D panel_update.rs: build_fleet_plan admin-widened, mirroring servers.rs/dashboard.rs ──"

if has "$BUILD_FLEET_PLAN_BODY" 'is_admin: bool'; then
  ok "D1 build_fleet_plan takes an is_admin parameter"
else
  bad "D1 build_fleet_plan has no is_admin parameter — still owner-only"
fi

if has "$BUILD_FLEET_PLAN_BODY" '\(\$2 OR user_id = \$1\)'; then
  ok "D2 the fleet-plan query widens to (\$2 OR user_id = \$1)"
else
  bad "D2 the fleet-plan query still hard-scopes to a single admin's own servers"
fi

if has "$APPLY_FLEET_BODY" 'build_fleet_plan\(&state\.db, claims\.sub, claims\.role == "admin"'; then
  ok "D3 apply_fleet passes the caller's admin status through to build_fleet_plan"
else
  bad "D3 apply_fleet no longer threads admin status into build_fleet_plan"
fi

# ── §E update.rs: apply_fleet's include_panel path enforces direction ───
echo "── §E update.rs: apply_fleet(include_panel) reuses reject_apply_target ──"

if has "$APPLY_FLEET_BODY" 'if body\.include_panel' && has "$APPLY_FLEET_BODY" 'reject_apply_target'; then
  ok "E1 apply_fleet calls reject_apply_target when include_panel is set"
else
  bad "E1 apply_fleet's include_panel path still skips the direction/advertised-match guard"
fi

if has "$APPLY_FLEET_BODY" "update_available_version"; then
  ok "E2 apply_fleet reads the same advertised-version row apply_update checks"
else
  bad "E2 apply_fleet does not consult the advertised-version row"
fi

# Positive control: apply_update's own guard (the logic being reused) is
# unchanged.
if has "$UPDATE_C" 'fn reject_apply_target'; then
  ok "E3 (control) reject_apply_target itself is still defined and exported to apply_fleet"
else
  bad "E3 (control) reject_apply_target regressed"
fi

# ── §F panel_update.rs: start_panel_update reserves InFlight atomically ──
echo "── §F panel_update.rs: start_panel_update closes the create_snapshot TOCTOU ──"

if has "$START_UPDATE_BODY" 'let mut s = handle\.write\(\)\.await;' ; then
  ok "F1 the concurrent-apply guard now acquires a MUTABLE write lock up front (not read-then-later-write)"
else
  bad "F1 start_panel_update's guard no longer opens with a mutable write lock — TOCTOU reopened"
fi

# The write-lock acquisition and the InFlight reservation must be the SAME
# statement block — grep the guard block specifically (from the write-lock
# open to its matching close) rather than the whole function, so a write
# lock added elsewhere in the body for an unrelated reason can't fake this.
GUARD_BLOCK=$(awk '/let mut s = handle\.write\(\)\.await;/{f=1} f{print; if (/^    \}/) exit}' <<< "$START_UPDATE_BODY")
if has "$GUARD_BLOCK" 'AlreadyInFlight' && has "$GUARD_BLOCK" 'UpdateState::InFlight \{'; then
  ok "F2 the guard checks AND reserves InFlight inside the same write-lock hold"
else
  bad "F2 the reservation is not inside the same write-lock block as the check — race window reopened"
fi

if has "$START_UPDATE_BODY" 'UpdateState::Idle' && has "$START_UPDATE_BODY" 'Err\(e\) =>' ; then
  ok "F3 a failed snapshot releases the reservation back to Idle"
else
  bad "F3 no release-on-snapshot-failure path found — a failed attempt could permanently lock the guard"
fi

# ── §G panel_update.rs: rollback_to_snapshot gets an in-flight guard ────
echo "── §G panel_update.rs: rollback_to_snapshot no longer races a concurrent apply ──"

if has "$ROLLBACK_BODY" 'handle: UpdateStateHandle'; then
  ok "G1 rollback_to_snapshot now takes the UpdateStateHandle"
else
  bad "G1 rollback_to_snapshot has no handle parameter — still unguarded"
fi

if has "$ROLLBACK_BODY" 'AlreadyInFlight' && has "$ROLLBACK_BODY" 'UpdateState::InFlight \{'; then
  ok "G2 rollback_to_snapshot checks and reserves InFlight before calling spawn_restore"
else
  bad "G2 rollback_to_snapshot no longer reserves InFlight — race reopened"
fi

if has "$UPDATE_C" 'rollback_to_snapshot\(state\.panel_update_state\.clone\(\), state\.db\.clone\(\)'; then
  ok "G3 the rollback route threads the handle through to rollback_to_snapshot"
else
  bad "G3 the rollback route call site was not updated — likely a compile error, or the guard is dead"
fi

echo
echo "=================================================="
echo "  PASS=$PASS FAIL=$FAIL"
echo "=================================================="

[ "$FAIL" -eq 0 ]
