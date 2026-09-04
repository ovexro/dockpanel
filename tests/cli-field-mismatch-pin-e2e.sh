#!/usr/bin/env bash
# cli-field-mismatch-pin-e2e.sh — s461
#
# ONE BUG CLASS: a CLI display command reads a JSON field name the agent
# never emits, so `unwrap_or(default)` swallows the mismatch silently —
# no error, no panic, just permanently wrong/blank output.
#
# Found by a dockpanel-fanout finder/skeptic pair (panel/cli/src, the
# never-fully-audited command files) plus a completeness-critic off-menu
# find in a sibling file:
#
#   - `dockpanel security` (overview): read `firewall_status`/`fail2ban_status`
#     (strings) and `ssl_coverage`/`scan_date` (fields that don't exist at
#     all) — the agent's `SecurityOverview` has `firewall_active`/
#     `fail2ban_running` (bools) and no ssl_coverage/scan_date concept.
#   - `dockpanel security firewall`: read `enabled` (agent emits `active`)
#     and `port`/`proto` on each rule (agent's `FirewallRule` has only
#     `number, to, action, from` — action is literally "ALLOW IN"/"DENY IN",
#     never bare "allow").
#   - `dockpanel security scan`: read `risk_level` (doesn't exist on
#     `ScanResult`) and each finding's `message` (agent's `Finding` has
#     `title`/`description`, no `message`). The real severity vocabulary is
#     "critical"/"warning"/"info" (security_scanner.rs), not "low/medium/high".
#   - `dockpanel backup list <domain>` (off-menu, completeness critic): read
#     `created` — the agent emits `created_at`, and the file's OWN sibling
#     functions (`cmd_db_backup_list`, `cmd_vol_backup_list`) already read
#     `created_at` correctly, so this is a same-file regression check too.
#
# Fix: rename every CLI read to match the agent's actual struct field
# names (cross-checked directly against the struct definitions, not
# guessed), derive `risk_level` client-side from the findings actually
# returned (the agent never computes one), and match the real
# critical/warning/info severity vocabulary instead of the fictional
# low/medium/high one. Read-only display paths only — no mutating
# command's request body or success-check changed.
#
# §A cmd_security_overview reads the real SecurityOverview field names.
# §B cmd_firewall_list reads the real FirewallStatus/FirewallRule field names.
# §C cmd_security_scan reads Finding's real fields and derives risk locally.
# §D cmd_backup_list reads created_at, matching its own sibling functions.

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

echo
echo "=================================================="
echo "  CLI field-mismatch — source pins (s461)"
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

SECURITY=panel/cli/src/commands/security.rs
SECURITY_C=$(code "$SECURITY")
BACKUP=panel/cli/src/commands/backup.rs
BACKUP_C=$(code "$BACKUP")

OVERVIEW_BODY=$(fnbody "$SECURITY_C" "cmd_security_overview")
FIREWALL_BODY=$(fnbody "$SECURITY_C" "cmd_firewall_list")
SCAN_BODY=$(fnbody "$SECURITY_C" "cmd_security_scan")
BACKUP_LIST_BODY=$(fnbody "$BACKUP_C" "cmd_backup_list")
DB_BACKUP_LIST_BODY=$(fnbody "$BACKUP_C" "cmd_db_backup_list")
VOL_BACKUP_LIST_BODY=$(fnbody "$BACKUP_C" "cmd_vol_backup_list")

# ── §A cmd_security_overview reads real SecurityOverview fields ─────────
echo "── §A cmd_security_overview: real field names, not fictional ones ──"

if [ -n "$OVERVIEW_BODY" ]; then
  ok "A0 cmd_security_overview exists"
else
  bad "A0 cmd_security_overview is missing — every arm below is measuring nothing"
fi

if has "$OVERVIEW_BODY" '"firewall_active"'; then
  ok "A1 reads firewall_active (the agent's real bool field)"
else
  bad "A1 does not read firewall_active — still on the wrong field name"
fi

if lacks "$OVERVIEW_BODY" '"firewall_status"'; then
  ok "A2 no longer reads the nonexistent firewall_status"
else
  bad "A2 still reads firewall_status — this field is never emitted"
fi

if has "$OVERVIEW_BODY" '"fail2ban_running"'; then
  ok "A3 reads fail2ban_running (the agent's real bool field)"
else
  bad "A3 does not read fail2ban_running — still on the wrong field name"
fi

if lacks "$OVERVIEW_BODY" '"fail2ban_status"'; then
  ok "A4 no longer reads the nonexistent fail2ban_status"
else
  bad "A4 still reads fail2ban_status — this field is never emitted"
fi

if lacks "$OVERVIEW_BODY" '"ssl_coverage"' && lacks "$OVERVIEW_BODY" '"scan_date"'; then
  ok "A5 dropped ssl_coverage/scan_date — SecurityOverview has neither field, ever"
else
  bad "A5 still reads a field SecurityOverview does not have"
fi

if has "$OVERVIEW_BODY" '"ssh_root_login"' && has "$OVERVIEW_BODY" '"ssh_password_auth"'; then
  ok "A6 shows ssh_root_login/ssh_password_auth — real fields, replacing the dead ones"
else
  bad "A6 does not surface the SSH hardening fields the agent actually provides"
fi

# ── §B cmd_firewall_list reads real FirewallStatus/FirewallRule fields ──
echo "── §B cmd_firewall_list: real field names, not fictional ones ──"

if has "$FIREWALL_BODY" '"active"'; then
  ok "B1 reads active (FirewallStatus's real bool field)"
else
  bad "B1 does not read active — still on the wrong field name"
fi

if lacks "$FIREWALL_BODY" '\["enabled"\]'; then
  ok "B2 no longer reads the nonexistent enabled field"
else
  bad "B2 still reads enabled — FirewallStatus has no such field"
fi

if lacks "$FIREWALL_BODY" '"port"' && lacks "$FIREWALL_BODY" '"proto"'; then
  ok "B3 dropped port/proto — FirewallRule never had either field"
else
  bad "B3 still reads port/proto — FirewallRule is {number, to, action, from} only"
fi

if has "$FIREWALL_BODY" '"to"'; then
  ok "B4 reads to (FirewallRule's real destination field)"
else
  bad "B4 does not read to — the one field that actually names the rule's target"
fi

if has "$FIREWALL_BODY" 'to_lowercase\(\).*contains\("allow"\)'; then
  ok "B5 action check tolerates the real \"ALLOW IN\"/\"DENY IN\" values"
else
  bad "B5 action check still expects a bare \"allow\" the agent never sends"
fi

# ── §C cmd_security_scan reads Finding's real fields, derives risk locally ──
echo "── §C cmd_security_scan: real Finding fields + client-derived risk ──"

if lacks "$SCAN_BODY" 'result\["risk_level"\]'; then
  ok "C1 no longer reads risk_level directly off the wire — ScanResult has no such field"
else
  bad "C1 still reads result[\"risk_level\"] — always unwrap_or(\"unknown\") in production"
fi

if has "$SCAN_BODY" 'max_by_key' && has "$SCAN_BODY" '"severity"'; then
  ok "C2 derives an overall risk level from the findings actually returned"
else
  bad "C2 no client-side risk derivation found — the Risk level line has no data source left"
fi

if has "$SCAN_BODY" '"critical"' && has "$SCAN_BODY" '"warning"'; then
  ok "C3 severity handling uses the real critical/warning/info vocabulary"
else
  bad "C3 severity handling still assumes the fictional low/medium/high vocabulary"
fi

if has "$SCAN_BODY" '"title"'; then
  ok "C4 reads title (Finding's real field)"
else
  bad "C4 does not read title — still expecting Finding to have a message field"
fi

if lacks "$SCAN_BODY" 'finding\["message"\]'; then
  ok "C5 no longer reads finding[\"message\"] — Finding has no such field"
else
  bad "C5 still reads finding[\"message\"] — every finding line would print blank"
fi

# ── §D cmd_backup_list reads created_at, matching its own siblings ───────
echo "── §D cmd_backup_list: created_at, matching cmd_db_backup_list/cmd_vol_backup_list ──"

if has "$BACKUP_LIST_BODY" '"created_at"'; then
  ok "D1 cmd_backup_list reads created_at"
else
  bad "D1 cmd_backup_list still reads the wrong field — CREATED column always prints '-'"
fi

if lacks "$BACKUP_LIST_BODY" '\["created"\]'; then
  ok "D2 cmd_backup_list no longer reads the nonexistent created field"
else
  bad "D2 cmd_backup_list still reads [\"created\"] — BackupInfo has no such field"
fi

# Positive controls: the sibling functions were ALREADY correct and must
# stay that way — this is the exact contrast that exposed the bug (two
# adjacent functions in the same file got it right, one didn't).
if has "$DB_BACKUP_LIST_BODY" '"created_at"'; then
  ok "D3 (control) cmd_db_backup_list still reads created_at — unchanged, was never broken"
else
  bad "D3 (control) cmd_db_backup_list regressed off created_at"
fi

if has "$VOL_BACKUP_LIST_BODY" '"created_at"'; then
  ok "D4 (control) cmd_vol_backup_list still reads created_at — unchanged, was never broken"
else
  bad "D4 (control) cmd_vol_backup_list regressed off created_at"
fi

echo
echo "=================================================="
echo "  PASS=$PASS FAIL=$FAIL"
echo "=================================================="

[ "$FAIL" -eq 0 ]
