#!/usr/bin/env bash
# pdns-install-scope-pin-e2e.sh — installing PowerDNS on a second managed server
# must not silently steal the credentials the first server's DNS zones depend on
#
#   Found by s422's fan-out setup critic, scoped and fixed at s423.
#   install_powerdns is ServerScope-scoped (per-server, same class as
#   Redis/Fail2Ban/Cloudflare Tunnel) but pdns_api_url/pdns_api_key live in the
#   global `settings` table with no server_id at all. Installing on a second
#   server silently overwrote the first server's credentials
#   (`ON CONFLICT (key) DO UPDATE`) — every dns_zones row using the powerdns
#   provider would then point at the wrong authoritative server, with no error
#   anywhere. Fix: a `pdns_server_id` ownership row, checked before install and
#   cleared on a matching uninstall.
#
# Pure source analysis: no box, no network, no build.
#
# NO PIPES INTO `grep -q` — under `set -o pipefail` grep -q closes the pipe on
# its first match and the arm goes red on correct code. Every arm uses a
# here-string.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

SYSTEM=panel/backend/src/routes/system.rs
[ -f "$SYSTEM" ] || bad "MISSING SUBJECT FILE: $SYSTEM"

echo "== §A  install_powerdns refuses a second server while another owns the credentials =="

INSTALL=$(awk '/^pub async fn install_powerdns\(/{i=1} i{print} i && /^}$/{exit}' "$SYSTEM")
NINST=$(grep -c . <<< "$INSTALL")
if [ "$NINST" -ge 60 ]; then
  ok "A1-control install_powerdns body extracted — $NINST lines"
else
  bad "A1-control install_powerdns body extracted — only $NINST lines (the extractor broke)"
fi

# A2: the ServerScope id is actually bound (was `_server_id`, discarded, before
# this fix — an underscore-prefixed binding proves nothing downstream can use it).
if grep -qE 'ServerScope\(server_id, agent\): ServerScope' <<< "$INSTALL"; then
  ok "A2 server_id is bound (not discarded as _server_id)"
else
  bad "A2 server_id must be bound — an underscored _server_id cannot be checked against the recorded owner"
fi

# A3: the pre-flight ownership check reads pdns_server_id from settings.
if grep -qE "SELECT value FROM settings WHERE key = 'pdns_server_id'" <<< "$INSTALL"; then
  ok "A3 reads the recorded pdns_server_id owner"
else
  bad "A3 must read pdns_server_id before installing — no way to detect a second-server install otherwise"
fi

# A4: a mismatched owner is rejected with a CONFLICT, not silently allowed through.
if grep -qE 'owner_id != server_id' <<< "$INSTALL" && grep -qE 'StatusCode::CONFLICT' <<< "$INSTALL"; then
  ok "A4 a mismatched owner_id is rejected with 409 CONFLICT"
else
  bad "A4 a mismatched owner must be rejected — otherwise a second install silently overwrites the first"
fi

# A5: POSITIONAL — the ownership check must run BEFORE the agent install call,
# not after (an after-the-fact check can't prevent the overwrite it exists to
# stop; it would only be able to report it once already done).
CHECK_LINE=$(grep -n "SELECT value FROM settings WHERE key = 'pdns_server_id'" <<< "$INSTALL" | head -1 | cut -d: -f1)
AGENT_CALL_LINE=$(grep -n 'post_long("/services/install/powerdns"' <<< "$INSTALL" | head -1 | cut -d: -f1)
if [ -n "$CHECK_LINE" ] && [ -n "$AGENT_CALL_LINE" ] && [ "$CHECK_LINE" -lt "$AGENT_CALL_LINE" ]; then
  ok "A5 the ownership check (line $CHECK_LINE) runs BEFORE the agent install call (line $AGENT_CALL_LINE)"
else
  bad "A5 the ownership check must precede the install call — check=${CHECK_LINE:-none} install=${AGENT_CALL_LINE:-none}"
fi

# A6: a successful install records which server now owns the credentials —
# without this, every install looks "ownerless" and the guard never engages.
if grep -qE "INSERT INTO settings \(key, value, updated_at\) VALUES \('pdns_server_id', \\\$1, NOW\(\)\)" <<< "$INSTALL" \
   && grep -qE '\.bind\(server_id\.to_string\(\)\)' <<< "$INSTALL"; then
  ok "A6 a successful install records pdns_server_id = this server"
else
  bad "A6 must persist pdns_server_id on success — otherwise the guard has nothing to compare against next time"
fi

echo "== §B  uninstall_powerdns frees the ownership slot, but only when it actually owns it =="

UNINSTALL=$(awk '/^pub async fn uninstall_powerdns\(/{i=1} i{print} i && /^}$/{exit}' "$SYSTEM")
NUNIN=$(grep -c . <<< "$UNINSTALL")
if [ "$NUNIN" -ge 30 ]; then
  ok "B1-control uninstall_powerdns body extracted — $NUNIN lines"
else
  bad "B1-control uninstall_powerdns body extracted — only $NUNIN lines (the extractor broke)"
fi

# B2: uninstall does NOT use the shared generic install_service_with_log helper
# — that helper never touches settings, so routing through it would leave a
# stale pdns_server_id forever and block installing PowerDNS anywhere else.
# Matches the actual CALL SITE (a bare identifier would also match this
# function's own explanatory comment about NOT using the helper — the
# source-pin prose trap).
if grep -qE '^\s*install_service_with_log\(&state, agent, claims\.sub' <<< "$UNINSTALL"; then
  bad "B2 uninstall_powerdns must NOT delegate to install_service_with_log — it never clears settings, permanently locking the ownership slot"
else
  ok "B2 uninstall_powerdns has its own settings-clearing logic (does not delegate to the generic helper)"
fi

# B3: the clear is conditioned on ownership match — an uninstall on a server
# that never owned the install must not wipe another server's live credentials.
if grep -qE 'owned_by_this_server' <<< "$UNINSTALL" \
   && grep -qE "id_str\.parse::<Uuid>\(\)\.map\(\|id\| id == server_id\)" <<< "$UNINSTALL"; then
  ok "B3 the settings clear is gated on this server actually owning the recorded install"
else
  bad "B3 must gate the clear on ownership — an unconditional clear would let uninstalling on the WRONG server wipe the real owner's live credentials"
fi

# B4: the clear actually removes the ownership row itself, not just the
# credentials — leaving pdns_server_id behind would permanently lock the slot
# even after a legitimate uninstall.
if grep -qE "DELETE FROM settings WHERE key IN \('pdns_api_url', 'pdns_api_key', 'pdns_server_id'\)" <<< "$UNINSTALL"; then
  ok "B4 the clear removes pdns_server_id along with the credentials"
else
  bad "B4 the clear must remove pdns_server_id too — otherwise no other server can ever install PowerDNS again"
fi

echo
printf 'pdns-install-scope: \033[32m%d passed\033[0m, \033[31m%d failed\033[0m\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
