#!/usr/bin/env bash
# Regression pins for the s454 dockpanel-fanout run over
# panel/agent/src/services/security.rs + panel/agent/src/routes/security.rs
# (workflow wf_ec0595a4-efd; project_dockpanel_tech_debt ledger p193).
#
#   S1  parse_ssh_config()/modify_sshd_config() only ever read/wrote
#       /etc/ssh/sshd_config, never following an `Include` directive into
#       /etc/ssh/sshd_config.d/*.conf. OpenSSH's own rule is "the first
#       obtained value wins", and the stock `Include` line ships BEFORE the
#       main file's own commented defaults — live-verified on this exact box:
#       GET /security/overview reported ssh_port:22 while sshd actually
#       listened on 1571 (port.conf), and 50-cloud-init.conf silently
#       overrode PasswordAuthentication the same way. A write through the
#       old code path would report success while sshd kept the old value.
#   S2  add_firewall_rule/remove_firewall_rule/apply_fix("block_port") shelled
#       to `ufw` unconditionally, even though get_firewall_status and
#       change_ssh_port had already been fixed (s265) to detect firewalld.
#       Live-verified 424/UFW_MISSING on this exact box (firewalld active,
#       ufw not installed) — a first-class setup.sh target (RHEL family).
#   S3  get_login_audit() hardcoded /var/log/auth.log and swallowed a missing
#       file into an empty Vec (Ok(vec![])) — indistinguishable from a
#       genuinely quiet RHEL host, which logs the same events to
#       /var/log/secure instead. Two sibling hardcodes found in the same
#       sweep: diagnostics.rs's brute-force scan (silently never fires on
#       RHEL) and routes/logs.rs's log-sizes listing (cosmetic).
#   S4  Completeness critic, off the original menu: is_safe_hook_command
#       (command_filter.rs, gates git-deploy post_deploy_cmd/pre_build_cmd at
#       the sh -c sink in routes/git_build.rs::run_hook) was the one function
#       in this file NOT routed through normalize_for_blocklist — executed
#       proof: is_safe_hook_command("r'm' -rf /") returned true, and `sh -c
#       "r'm' -rf /"` runs `rm -rf /`. The backend's independent copy,
#       is_safe_shell_command (routes/mod.rs), had no normalization at all.
#
# Pure source analysis except where noted; no box, no network, no build.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

# Strip comments and the test module, so a pin can never be satisfied (or
# tripped) by prose describing the very thing it forbids.
code()  { sed '/#\[cfg(test)\]/q' "$1" | grep -vE '^[[:space:]]*(///|//!|//|\*|/\*)'; }
has()   { [ -n "$(code "$1" | grep -F -- "$2")" ]; }
hasre() { [ -n "$(code "$1" | grep -E -- "$2")" ]; }

# One named function's body, comment-stripped. NEVER pipe this straight into
# `grep -q` — a producer this long can still be writing when `-q` closes the
# read end on its first match, and under `pipefail` that SIGPIPE (not the
# match) becomes the `if`'s exit status. `bodyhas`/`bodyhasre` avoid it the
# same way `has`/`hasre` above do: grep without `-q` drains the pipe fully,
# so the producer never gets closed out from under it.
fnbody()    { code "$1" | awk "/$2/,/^}/"; }
bodyhas()   { [ -n "$(fnbody "$1" "$2" | grep -F -- "$3")" ]; }
bodyhasre() { [ -n "$(fnbody "$1" "$2" | grep -E -- "$3")" ]; }

SEC_SVC=panel/agent/src/services/security.rs
SEC_ROUTE=panel/agent/src/routes/security.rs
FIREWALL=panel/agent/src/services/firewall.rs
CMDFILTER=panel/agent/src/services/command_filter.rs
LOGS_SVC=panel/agent/src/services/logs.rs
DIAG=panel/agent/src/services/diagnostics.rs
AGENT_LOGS_ROUTE=panel/agent/src/routes/logs.rs
BACKEND_MOD=panel/backend/src/routes/mod.rs

for f in "$SEC_SVC" "$SEC_ROUTE" "$FIREWALL" "$CMDFILTER" "$LOGS_SVC" "$DIAG" "$AGENT_LOGS_ROUTE" "$BACKEND_MOD"; do
  [ -f "$f" ] || { echo "missing source file: $f"; exit 1; }
done

echo "── 1. S1: SSH config parsing/writing follows Include drop-ins ──"

if has "$SEC_SVC" "fn linearized_sshd_lines"; then
  ok "security.rs linearizes sshd_config + its Includes before parsing/writing"
else
  bad "linearized_sshd_lines is gone — Include-blindness may be back"
fi

if has "$SEC_SVC" "fn parse_ssh_config_at"; then
  ok "parse_ssh_config is path-parameterized for testability"
else
  bad "parse_ssh_config_at is gone"
fi

if bodyhas "$SEC_SVC" "fn parse_ssh_config_at" "linearized_sshd_lines"; then
  ok "parse_ssh_config_at reads through the linearized (Include-aware) view"
else
  bad "parse_ssh_config_at no longer calls linearized_sshd_lines — regressed to single-file read"
fi

if bodyhas "$SEC_SVC" "fn modify_sshd_config_at" "linearized_sshd_lines"; then
  ok "modify_sshd_config_at resolves the governing file via the linearized view before writing"
else
  bad "modify_sshd_config_at no longer resolves an Include-aware target — a write could silently miss the file that actually governs the key"
fi

# The "first obtained value wins" precedence must actually be respected, not
# just read Include lines without early-stopping per keyword.
if bodyhasre "$SEC_SVC" "fn parse_ssh_config_at" 'if !port_set|if !pw_set|if !root_set'; then
  ok "parse_ssh_config_at stops at the FIRST match per keyword (OpenSSH's own precedence)"
else
  bad "parse_ssh_config_at may take the LAST match instead of the first — inverts sshd's own precedence"
fi

echo
echo "── 2. S2: firewall rule add/remove/block-port speak firewalld too ──"

if bodyhas "$SEC_SVC" "pub async fn add_firewall_rule" "Firewall::Firewalld"; then
  ok "add_firewall_rule branches on the detected firewall"
else
  bad "add_firewall_rule no longer checks for firewalld — back to a ufw-only 424 on RHEL"
fi

if bodyhas "$SEC_SVC" "pub async fn remove_firewall_rule" "Firewall::Firewalld"; then
  ok "remove_firewall_rule branches on the detected firewall"
else
  bad "remove_firewall_rule no longer checks for firewalld"
fi

for fn in add_port remove_port remove_service rich_rule_spec add_rich_rule_raw remove_rich_rule_raw; do
  if has "$FIREWALL" "fn $fn"; then
    ok "firewall.rs exposes $fn"
  else
    bad "firewall.rs is missing $fn — firewalld parity primitive lost"
  fi
done

if has "$SEC_SVC" "fn firewalld_entries"; then
  ok "firewalld_status and remove_firewall_rule share one numbered entry list (services+ports+rich rules)"
else
  bad "firewalld_entries is gone — the numbers the Security page shows and the ones remove_firewall_rule resolves can drift apart again"
fi

echo
echo "── 3. S3: the auth log path is RHEL-aware, and a missing file is a real error ──"

if has "$LOGS_SVC" "fn resolve_auth_log_path"; then
  ok "logs.rs exposes a shared resolve_auth_log_path()"
else
  bad "resolve_auth_log_path is gone"
fi

if bodyhas "$SEC_SVC" "pub async fn get_login_audit" "resolve_auth_log_path"; then
  ok "get_login_audit resolves the auth log path instead of hardcoding /var/log/auth.log"
else
  bad "get_login_audit hardcodes the Debian-only auth log path again"
fi

if bodyhasre "$SEC_SVC" "pub async fn get_login_audit" '\.map_err\('; then
  ok "get_login_audit propagates a read failure as an Err, not a silent empty Vec"
else
  bad "get_login_audit swallows a missing/unreadable log into Ok(vec![]) again — indistinguishable from a genuinely quiet host"
fi

if bodyhas "$SEC_SVC" "pub async fn get_login_audit" "unwrap_or_default"; then
  bad "get_login_audit still has an unwrap_or_default() fallback on the read — the silent-empty-success path may be back"
else
  ok "get_login_audit has no silent unwrap_or_default() on the auth log read"
fi

# Two sibling hardcodes found in the same sweep — same fix, different files.
if has "$DIAG" 'resolve_auth_log_path()'; then
  ok "diagnostics.rs's brute-force scan uses the RHEL-aware path"
else
  bad "diagnostics.rs still hardcodes /var/log/auth.log — its brute-force finding silently never fires on RHEL"
fi

if has "$AGENT_LOGS_ROUTE" 'resolve_auth_log_path()'; then
  ok "routes/logs.rs's log-sizes listing uses the RHEL-aware path"
else
  bad "routes/logs.rs's log-sizes listing still hardcodes /var/log/auth.log"
fi

# No stray direct hardcode of the Debian-only path anywhere in the agent
# crate's non-comment code, outside the one place that still legitimately
# needs the literal string: resolve_auth_log_path's own existence probe.
STRAY=""
while IFS= read -r f; do
  [ -z "$f" ] && continue
  [ "$f" = "$LOGS_SVC" ] && continue
  if has "$f" '"/var/log/auth.log"'; then
    STRAY="$STRAY $f"
  fi
done <<EOF
$(grep -rl '/var/log/auth\.log' panel/agent/src/ --include="*.rs" 2>/dev/null || true)
EOF
if [ -z "$STRAY" ]; then
  ok "no other agent-crate file still hardcodes the Debian-only auth log path"
else
  bad "these files still hardcode /var/log/auth.log directly:$STRAY"
fi

echo
echo "── 4. S4: hook/shell command validators close the quote-split bypass ──"

if bodyhas "$CMDFILTER" "pub fn is_safe_hook_command" "normalize_for_blocklist(command)"; then
  ok "is_safe_hook_command routes its dangerous-pattern scan through normalize_for_blocklist"
else
  bad "is_safe_hook_command no longer normalizes — r'm' -rf / bypasses the blocklist and sh -c runs it for real"
fi

if has "$BACKEND_MOD" "fn normalize_for_blocklist"; then
  ok "the backend has its own normalize_for_blocklist"
else
  bad "the backend's normalize_for_blocklist is gone"
fi

if bodyhas "$BACKEND_MOD" "pub fn is_safe_shell_command" "normalize_for_blocklist(cmd)"; then
  ok "is_safe_shell_command routes through normalize_for_blocklist"
else
  bad "is_safe_shell_command no longer normalizes — same quote-split bypass as command_filter.rs had"
fi

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
