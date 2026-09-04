#!/usr/bin/env bash
# Regression pins for the s460 rotation: services/smtp.rs + routes/smtp.rs
# (never a PRIMARY finder/skeptic target) and routes/updates.rs (same),
# LOC-ranked and pre-selection-checked per feedback_dockpanel_audit_scope.
#
#   Topic 1 (routes/updates.rs) — the whole System Updates feature
#     (list/count/apply) was 100% hardcoded to apt/apt-get, with zero use of
#     this project's own pkg.rs dual-family abstraction. On an RHEL-family
#     agent (a platform this project explicitly supports and tests
#     elsewhere), list_updates/update_count silently returned "0 updates" —
#     read as fully patched — because apt-family commands simply don't
#     exist there. Fixed: dnf/yum check-update support (exit code 0/100/
#     other correctly distinguished — check-update's exit status is NOT a
#     plain success/failure boolean), a 3-token line parser requiring a
#     name.arch dot (rejects a false-positive 3-word non-package line —
#     caught by this pin suite's own mutation test on the parser), an
#     rpm -qa lookup for current-version display, and a needs-restarting -r
#     fallback for reboot_required on RHEL (no /var/run/reboot-required
#     there). apply_updates now selects apt-get/dnf/yum via pkg::installer()
#     instead of a hardcoded apt-get binary.
#   Topic 2 (services/smtp.rs) — two CONFIRMED findings in configure():
#     (a) from_name was validated for injection and pushed fleet-wide by the
#     backend, but never written into the msmtp config or the PHP
#     sendmail_path ini — a "From Name" the UI promises applies "to all
#     sites on this server" had no effect on any real application mail,
#     only the one-off admin Test Email. Fixed: msmtp's from_full_name
#     directive (verified against `man msmtp` on this box — envelope `from`
#     is address-only; from_full_name is the separate directive that names
#     the display name msmtp adds to an auto-generated From header).
#     (b) Every SMTP settings save unconditionally truncated
#     /var/log/msmtp.log via fs::write(path, "") — the frontend always
#     re-PUTs all 7 smtp_* keys on Save (even an unrelated field edit),
#     which pushes to every online fleet member, wiping the mail relay log
#     on every save, including the save an operator makes while debugging a
#     mail problem using that exact log. Fixed: only create the file when
#     it doesn't already exist.
#   Off-menu, completeness-critic-found — routes/php.rs had the textually
#     identical vulnerable pattern v2.213.0 (the immediately prior ship) had
#     just fixed in pkg.rs: wrapping safe_command_unsandboxed(...).output()
#     in a caller-side tokio::time::timeout only kills the LOCAL systemd-run
#     waiter on expiry, leaving the actual privileged apt/dnf transaction
#     running orphaned in its own transient unit forever. Three live sites
#     (install_version, uninstall_version's purge, its apt autoremove
#     follow-up), all using the same 300s INSTALL_TIMEOUT-equal duration as
#     pkg.rs's own fixed sites. Fixed: converted to
#     UnsandboxedCommand::output_with_timeout(), which also stops the named
#     transient unit on timeout.
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
fnbody()    { code "$1" | awk "/$2/,/^}/"; }
bodyhas()   { [ -n "$(fnbody "$1" "$2" | grep -F -- "$3")" ]; }
bodyhasre() { [ -n "$(fnbody "$1" "$2" | grep -E -- "$3")" ]; }
count()     { code "$1" | grep -Fc -- "$2"; }

UPDATES=panel/agent/src/routes/updates.rs
SMTP=panel/agent/src/services/smtp.rs
PHP=panel/agent/src/routes/php.rs

for f in "$UPDATES" "$SMTP" "$PHP"; do
  [ -f "$f" ] || { echo "missing source file: $f"; exit 1; }
done

echo "── 1. Topic 1: routes/updates.rs gains a dnf/yum path via pkg.rs ──"
if hasre "$UPDATES" 'use crate::services::pkg::\{self, Installer, PkgMgr\};'; then
  ok "updates.rs imports the pkg.rs dual-family abstraction"
else
  bad "updates.rs no longer imports pkg::{Installer,PkgMgr} — RHEL path may be gone"
fi
if bodyhasre "$UPDATES" "^async fn rpm_check_update" 'Some\(0\) \| Some\(100\)'; then
  ok "rpm_check_update distinguishes check-update's 0/100/error exit codes"
else
  bad "rpm_check_update no longer distinguishes 0 (no updates) from 100 (updates) from a real error"
fi
if bodyhas "$UPDATES" "^fn parse_rpm_update_line" "rsplit_once('.')?"; then
  ok "parse_rpm_update_line requires the name.arch dot (rejects a false-positive 3-word line)"
else
  bad "parse_rpm_update_line's dot requirement is missing — may misparse a non-package summary line as one"
fi
if bodyhasre "$UPDATES" "^async fn list_updates" 'PkgMgr::Rpm'; then
  ok "list_updates branches on PkgMgr::Rpm"
else
  bad "list_updates no longer branches on PkgMgr::Rpm — may be apt-only again"
fi
if bodyhasre "$UPDATES" "^async fn update_count" 'PkgMgr::Rpm'; then
  ok "update_count branches on PkgMgr::Rpm"
else
  bad "update_count no longer branches on PkgMgr::Rpm — may be apt-only again"
fi
if bodyhasre "$UPDATES" "^async fn apply_updates" 'pkg::installer\(\).await'; then
  ok "apply_updates selects its binary via pkg::installer() rather than a hardcoded apt-get"
else
  bad "apply_updates no longer calls pkg::installer() — may have regressed to hardcoded apt-get"
fi
if bodyhas "$UPDATES" "^async fn apply_updates" 'safe_command_unsandboxed("apt-get", &[])'; then
  bad "apply_updates still hardcodes safe_command_unsandboxed(\"apt-get\", ...) — the RHEL branch may be dead code"
else
  ok "apply_updates no longer hardcodes the apt-get binary directly"
fi
if bodyhasre "$UPDATES" "^async fn reboot_required" 'needs-restarting'; then
  ok "reboot_required has an RHEL fallback (needs-restarting -r)"
else
  bad "reboot_required lost its RHEL fallback — reboot banner will never show on RPM boxes"
fi

echo
echo "── 2. Topic 1 control: the new RPM parser is unit-tested, not just source-shaped ──"
if grep -qF 'fn parses_a_real_dnf_check_update_line' "$UPDATES"; then
  ok "parse_rpm_update_line has a Rust unit test for a real dnf line"
else
  bad "no unit test found for parse_rpm_update_line"
fi
if grep -qF 'fn skips_section_headers_and_the_metadata_banner' "$UPDATES"; then
  ok "parse_rpm_update_line has a unit test for non-package lines"
else
  bad "no unit test found guarding against section-header false positives"
fi

echo
echo "── 3. Topic 2: services/smtp.rs — from_name reaches msmtp via from_full_name ──"
if bodyhas "$SMTP" "^pub fn configure" 'from_full_name'; then
  ok "configure() writes msmtp's from_full_name directive"
else
  bad "configure() no longer writes from_full_name — from_name is dead again"
fi
if bodyhasre "$SMTP" "^pub fn configure" 'from_name\.is_empty\(\)'; then
  ok "configure() guards on from_name being empty before emitting the directive"
else
  bad "configure()'s empty-from_name guard is missing"
fi
# Control: from_full_name must be INSIDE the account block (after `from`,
# before `user`), not in `defaults` — msmtp scopes it per-account like `from`.
# The template is a raw string with a REAL newline between the two lines,
# so this checks adjacency by line number rather than a single-line regex.
FROM_LINE=$(grep -n 'from[[:space:]]*{from}$' "$SMTP" | head -1 | cut -d: -f1)
NAME_LINE=$(grep -n '^{from_name_line}user' "$SMTP" | head -1 | cut -d: -f1)
if [ -n "$FROM_LINE" ] && [ -n "$NAME_LINE" ] && [ "$NAME_LINE" -eq $((FROM_LINE + 1)) ]; then
  ok "from_full_name is emitted immediately after from, inside the account block"
else
  bad "from_full_name's placement in the config template changed (from=$FROM_LINE, name=$NAME_LINE) — re-verify it is still inside the account block"
fi

echo
echo "── 4. Topic 2: services/smtp.rs — msmtp.log is no longer truncated on every save ──"
if bodyhasre "$SMTP" "^pub fn configure" '!std::path::Path::new\("/var/log/msmtp\.log"\)\.exists\(\)'; then
  ok "configure() only creates msmtp.log when it does not already exist"
else
  bad "configure() no longer guards the msmtp.log write — may truncate on every save again"
fi
# Control: exactly one write to this path, and it must be the guarded one —
# a regression that adds a second unconditional write would slip past a
# has()-only check.
WRITE_COUNT=$(count "$SMTP" '/var/log/msmtp.log", "").ok();')
if [ "$WRITE_COUNT" -eq 1 ]; then
  ok "exactly one write to msmtp.log in the file (the guarded one)"
else
  bad "found $WRITE_COUNT writes to msmtp.log, expected 1 — an unguarded second write may have been added"
fi

echo
echo "── 5. Off-menu: routes/php.rs propagates output_with_timeout to its 3 remaining sites ──"
if bodyhasre "$PHP" "^async fn install_version\(" 'output_with_timeout'; then
  ok "install_version() uses output_with_timeout"
else
  bad "install_version() does not use output_with_timeout — orphaned-unit fix not propagated"
fi
if bodyhasre "$PHP" "^async fn uninstall_version\(" 'output_with_timeout'; then
  ok "uninstall_version() uses output_with_timeout (covers both the purge and the autoremove follow-up)"
else
  bad "uninstall_version() does not use output_with_timeout — orphaned-unit fix not propagated"
fi
# Control: zero remaining instances of the OLD double-wrap pattern
# (tokio::time::timeout(...) around a plain .output() call) anywhere in the
# file — this is the exact shape v2.213.0 fixed in pkg.rs and this session
# fixed here; a regression re-introducing it anywhere would slip past the
# two positive checks above alone.
OLD_PATTERN_COUNT=$(code "$PHP" | grep -Pzo 'tokio::time::timeout\(\s*std::time::Duration::from_secs\(300\),\s*safe_command_unsandboxed' 2>/dev/null | grep -ac 'tokio::time::timeout' || true)
if [ "${OLD_PATTERN_COUNT:-0}" -eq 0 ]; then
  ok "no remaining caller-side tokio::time::timeout wrapping a plain safe_command_unsandboxed(...).output() in php.rs"
else
  bad "found $OLD_PATTERN_COUNT remaining old-shape timeout wraps in php.rs — the orphaned-unit bug may still exist somewhere"
fi

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
