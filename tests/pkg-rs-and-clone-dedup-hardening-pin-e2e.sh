#!/usr/bin/env bash
# Regression pins for the s459 small-carries bundle: a session picked
# specifically because it had been declined at the opener 3 sessions running
# (the standing rotation kept winning instead) — no finder/skeptic fan-out
# this time, just implementing already-diagnosed carries.
#
#   F1  pkg.rs had 4 sites wrapping the SANDBOXED safe_command(...) builder
#       in a tokio::time::timeout with no kill_on_drop(true) — the
#       established timeout-orphan class this project has now fixed 9x
#       elsewhere, never swept in this specific file's safe_command() (as
#       opposed to safe_command_unsandboxed()) call sites: installed_php_
#       version, php_streams, run_ok, which. Fixed: kill_on_drop(true) added
#       to each.
#   F2  The harder, 9-session-carried fix: safe_command_unsandboxed()'s
#       systemd-run --wait timeout only ever killed the LOCAL waiting
#       client — the remote transient unit it started runs in its own
#       cgroup, fully decoupled, and kept running orphaned forever on
#       timeout (live-proven in a prior session). Fixed: a new
#       UnsandboxedCommand::output_with_timeout() in safe_cmd.rs assigns the
#       transient unit a caller-known name via --unit=NAME (composes with
#       --collect, which only means "GC on exit", not naming), and on
#       timeout fires a best-effort, un-awaited `systemctl stop --no-block
#       <unit>` before returning an operator-actionable error naming the
#       unit. All 6 of pkg.rs's safe_command_unsandboxed(...) call sites
#       (enable_php_stream, transact's main transaction, transact's
#       autoremove, refresh_index, escape_hatch_works, add_repo) converted
#       from an external tokio::time::timeout wrapper to this method.
#       spawn_streaming() (the live-log-streaming path) deliberately left
#       untouched — different completion semantics, out of scope.
#   F3  routes/nginx.rs::clone_site reimplemented, inline, the exact same
#       rsync+chown-split logic as services/staging.rs::clone_files (a
#       second implementation of the same "clone a site's files" feature,
#       carried as known duplication since s456). Fixed: clone_site now
#       delegates the copy+chown work to clone_files, keeping only its own
#       du -sb size computation and JSON response as route-specific code.
#   F4  install-agent.sh's firewall-port-opening step checked `command -v
#       ufw` before checking firewalld's actual active state — the inverse
#       of the check order setup.sh's own detect_firewall() already uses,
#       and the same split-brain class (s265) this project has fixed
#       everywhere else: a box with ufw merely INSTALLED but firewalld
#       ACTUALLY ENFORCING would silently configure the firewall nobody is
#       using. Fixed: check `firewall-cmd --state` first, matching setup.sh.
#
# Pure source analysis; no box, no network, no build.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

# Strip comments and the test module. Only exits at a #[cfg(test)] line
# immediately followed by `mod ` — a real test module, not a cfg-gated
# function/const variant elsewhere in the file (project_dockpanel_lessons_p182).
code() {
  awk '
    held == 1 {
      if ($0 ~ /^mod /) { exit }
      print heldline
      held = 0
    }
    /^#\[cfg\(test\)\]$/ { held = 1; heldline = $0; next }
    { print }
  ' "$1" | grep -vE '^[[:space:]]*(///|//!|//|\*|/\*)'
}
has()   { [ -n "$(code "$1" | grep -F -- "$2")" ]; }
hasre() { [ -n "$(code "$1" | grep -E -- "$2")" ]; }

# One named function's body, comment-stripped. Anchored (^ ... \() — GNU
# awk's \b silently matches nothing in default ERE mode. NOTE: this only
# scopes precisely for TOP-LEVEL (non-impl-method) functions, whose closing
# brace sits at column 0 — every pkg.rs function pinned below is top-level,
# so this is exact there. safe_cmd.rs's UnsandboxedCommand methods are
# INSIDE an impl block (their own closing brace is indented, not column 0),
# so fnbody over-captures for them — deliberately NOT used for safe_cmd.rs
# method bodies below; whole-file hasre()/has() is used there instead.
fnbody()    { code "$1" | awk "/$2/,/^}/"; }
bodyhas()   { [ -n "$(fnbody "$1" "$2" | grep -F -- "$3")" ]; }
bodyhasre() { [ -n "$(fnbody "$1" "$2" | grep -E -- "$3")" ]; }

PKG=panel/agent/src/services/pkg.rs
SAFE_CMD=panel/agent/src/safe_cmd.rs
NGINX_ROUTE=panel/agent/src/routes/nginx.rs
STAGING=panel/agent/src/services/staging.rs
INSTALL_AGENT_SH=scripts/install-agent.sh

for f in "$PKG" "$SAFE_CMD" "$NGINX_ROUTE" "$STAGING" "$INSTALL_AGENT_SH"; do
  [ -f "$f" ] || { echo "missing source file: $f"; exit 1; }
done

echo "── 1. F1: the 4 safe_command(...) (sandboxed) sites in pkg.rs now kill_on_drop ──"
if bodyhasre "$PKG" "^pub async fn installed_php_version" 'kill_on_drop\(true\)'; then
  ok "installed_php_version() sets kill_on_drop(true)"
else
  bad "installed_php_version() is missing kill_on_drop(true)"
fi
if bodyhasre "$PKG" "^pub async fn php_streams" 'kill_on_drop\(true\)'; then
  ok "php_streams() sets kill_on_drop(true)"
else
  bad "php_streams() is missing kill_on_drop(true)"
fi
if bodyhasre "$PKG" "^async fn run_ok" 'kill_on_drop\(true\)'; then
  ok "run_ok() sets kill_on_drop(true)"
else
  bad "run_ok() is missing kill_on_drop(true)"
fi
if bodyhasre "$PKG" "^async fn which" 'kill_on_drop\(true\)'; then
  ok "which() sets kill_on_drop(true)"
else
  bad "which() is missing kill_on_drop(true)"
fi
# Whole-file count control: exactly 4 kill_on_drop(true) sites in pkg.rs —
# a regression that ADDS one without pairing it correctly, or that silently
# drops one, both change this count.
KOD_COUNT=$(code "$PKG" | grep -Fc 'kill_on_drop(true)')
if [ "$KOD_COUNT" -eq 4 ]; then
  ok "pkg.rs has exactly 4 kill_on_drop(true) sites total"
else
  bad "pkg.rs has $KOD_COUNT kill_on_drop(true) sites, expected 4 — a site was added or removed"
fi

echo
echo "── 2. F2: safe_cmd.rs gains output_with_timeout() + insert_unit_arg(), used everywhere ──"
if hasre "$SAFE_CMD" 'pub async fn output_with_timeout'; then
  ok "UnsandboxedCommand::output_with_timeout exists"
else
  bad "output_with_timeout is missing"
fi
if hasre "$SAFE_CMD" 'fn insert_unit_arg\(argv: &mut Vec<std::ffi::OsString>\)'; then
  ok "insert_unit_arg helper exists with the expected signature"
else
  bad "insert_unit_arg is missing or its signature changed"
fi
if bodyhasre "$SAFE_CMD" "^fn insert_unit_arg\(" 'position\(\|a\| a == "--"\)'; then
  ok "insert_unit_arg scans for the -- separator rather than assuming a fixed index"
else
  bad "insert_unit_arg no longer scans for the -- separator — may have regressed to a fixed-index insert"
fi
if bodyhasre "$SAFE_CMD" "^fn insert_unit_arg\(" '--unit='; then
  ok "insert_unit_arg inserts a --unit= argument"
else
  bad "insert_unit_arg no longer inserts --unit="
fi
if hasre "$SAFE_CMD" 'insert_unit_arg\(&mut self\.argv\)'; then
  ok "output_with_timeout actually calls insert_unit_arg on its own argv"
else
  bad "output_with_timeout no longer calls insert_unit_arg — the unit-naming fix may be dead code"
fi
if hasre "$SAFE_CMD" '"stop", "--no-block", &stop_unit'; then
  ok "the timeout branch runs systemctl stop --no-block on the named unit"
else
  bad "the timeout branch no longer stops the named unit — the remote-unit-orphan bug may have regressed"
fi
# The stop must be fire-and-forget (tokio::spawn, not awaited inline) so
# cleanup can never delay/mask the timeout error itself. Scoped to this
# fix's own literal (a bare `tokio::spawn(async move {` search is too weak
# — spawn_streaming()'s PRE-EXISTING, unrelated status-waiter task already
# has one, so that pattern alone passes even against pre-fix code).
if hasre "$SAFE_CMD" 'let stop_unit = unit\.clone\(\);'; then
  ok "the systemctl stop runs in a detached task (fire-and-forget, does not block the timeout error)"
else
  bad "the systemctl stop no longer looks detached — may now block the caller on cleanup"
fi
# spawn_streaming() must be untouched — explicitly out of scope for this fix.
SPAWN_STREAMING_KOD=$(code "$SAFE_CMD" | grep -c 'pub fn spawn_streaming')
if [ "$SPAWN_STREAMING_KOD" -eq 1 ]; then
  ok "spawn_streaming() is still present, unduplicated (control: out-of-scope surface untouched)"
else
  bad "spawn_streaming() count is $SPAWN_STREAMING_KOD, expected 1 — check for accidental duplication/removal"
fi

echo
echo "── 3. F2: every safe_command_unsandboxed(...) call site in pkg.rs routes through output_with_timeout ──"
USC_COUNT=$(code "$PKG" | grep -Fc 'safe_command_unsandboxed(')
OWT_COUNT=$(code "$PKG" | grep -Fc 'output_with_timeout(')
if [ "$USC_COUNT" -eq 6 ] && [ "$OWT_COUNT" -eq 6 ]; then
  ok "6 safe_command_unsandboxed(...) call sites, 6 output_with_timeout(...) call sites — 1:1"
else
  bad "safe_command_unsandboxed count=$USC_COUNT, output_with_timeout count=$OWT_COUNT (expected 6/6) — a call site may have been added, removed, or left on the old external-timeout pattern"
fi
# Control: no leftover instance of the OLD pattern (an external
# tokio::time::timeout directly wrapping a safe_command_unsandboxed(...)
# .output() call) — the exact shape this fix replaced everywhere.
if hasre "$PKG" 'tokio::time::timeout\(\s*(INSTALL_TIMEOUT|Duration::from_secs\(30\)),\s*$'; then
  bad "found a tokio::time::timeout(...) opener that may still directly wrap a raw .output() call — re-check it routes through output_with_timeout instead"
else
  ok "no leftover external-timeout-wrapping-raw-output() pattern in pkg.rs"
fi
for anchor in \
  "^pub async fn enable_php_stream" \
  "^async fn transact" \
  "^pub async fn refresh_index" \
  "^async fn escape_hatch_works" \
  "^pub async fn add_repo"
do
  if bodyhasre "$PKG" "$anchor" 'output_with_timeout\('; then
    ok "$anchor now uses output_with_timeout"
  else
    bad "$anchor no longer calls output_with_timeout — may have regressed to the old pattern"
  fi
done

echo
echo "── 4. F3: nginx.rs::clone_site delegates to staging.rs::clone_files ──"
if bodyhas "$NGINX_ROUTE" "^async fn clone_site" "services::staging::clone_files("; then
  ok "clone_site calls services::staging::clone_files"
else
  bad "clone_site no longer calls services::staging::clone_files — the dedup may have regressed"
fi
if bodyhasre "$NGINX_ROUTE" "^async fn clone_site" '"rsync"'; then
  bad "clone_site still has its own inline rsync call — the duplicate implementation was not actually removed"
else
  ok "clone_site no longer runs its own inline rsync (control: real dedup, not just an added delegate call)"
fi
if bodyhasre "$NGINX_ROUTE" "^async fn clone_site" 'www-data:www-data'; then
  bad "clone_site still has its own inline chown-to-www-data logic — the duplicate implementation was not actually removed"
else
  ok "clone_site no longer runs its own inline chown (control: chown logic now lives only in clone_files)"
fi
# clone_files itself must be untouched — the delegation target, not the
# thing being changed.
if hasre "$STAGING" 'pub async fn clone_files\(source_domain: &str, target_domain: &str\)'; then
  ok "staging.rs::clone_files signature is unchanged (control: delegation target untouched)"
else
  bad "staging.rs::clone_files signature changed — re-check the delegation call still matches"
fi
# clone_site's own post-copy work (the one thing that should NOT have moved
# into clone_files) must still be present.
if bodyhas "$NGINX_ROUTE" "^async fn clone_site" '"du"'; then
  ok "clone_site still computes size via du (control: route-specific tail preserved)"
else
  bad "clone_site's own du -sb size computation is missing"
fi

echo
echo "── 5. F4: install-agent.sh checks firewalld's ACTIVE STATE before ufw ──"
if hasre "$INSTALL_AGENT_SH" 'firewall-cmd --state'; then
  ok "install-agent.sh checks firewall-cmd --state (actually enforcing), not just command -v"
else
  bad "install-agent.sh no longer checks firewall-cmd --state"
fi
# Order matters: the firewall-cmd branch must appear BEFORE the ufw branch
# in the firewall-port-opening block, so a box with both installed but only
# firewalld running is handled correctly.
FW_BLOCK_FILE=$(mktemp)
awk '/# Allow agent port through firewall/,/^fi$/' "$INSTALL_AGENT_SH" > "$FW_BLOCK_FILE"
FIREWALLD_LINE=$(grep -n 'firewall-cmd --state' "$FW_BLOCK_FILE" | head -1 | cut -d: -f1)
UFW_LINE=$(grep -n 'ufw allow' "$FW_BLOCK_FILE" | head -1 | cut -d: -f1)
rm -f "$FW_BLOCK_FILE"
if [ -n "$FIREWALLD_LINE" ] && [ -n "$UFW_LINE" ] && [ "$FIREWALLD_LINE" -lt "$UFW_LINE" ]; then
  ok "the firewalld --state check appears BEFORE the ufw allow line (correct precedence)"
else
  bad "could not confirm firewalld is checked before ufw (firewalld_line=$FIREWALLD_LINE, ufw_line=$UFW_LINE) — re-verify by hand"
fi

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
