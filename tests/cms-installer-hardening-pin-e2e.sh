#!/usr/bin/env bash
# Regression pins for this session's dockpanel-fanout run over
# panel/agent/src/services/cms.rs + routes/cms.rs — a genuine LOC-ranked
# rotation pick (never previously a PRIMARY finder/skeptic target), landed
# on after 32 higher-priority candidates were disqualified by the
# two-direction pre-selection check. 1 topic (finder/skeptic + completeness
# + setup critics).
#
#   F1  services/cms.rs::run_cmd/run_cmd_in (the shared helper every one of
#       the 5 CMS installers routes through for composer/drush/php/curl/
#       unzip) had zero kill_on_drop and zero tokio::time::timeout anywhere
#       — the established timeout-orphan class already fixed 8x elsewhere
#       in this project (docker_apps.rs/pkg.rs/database_backup.rs/
#       wp_vulnerability.rs/backups.rs/security.rs/database.rs/
#       image_scanner.rs), never swept here. Fixed: kill_on_drop(true) +
#       tokio::time::timeout on both helpers, plus ensure_composer()'s and
#       install_joomla()'s own direct curl/unzip calls.
#   F2  Off-menu, setup-critic-found: services/pkg.rs::enable_php_stream was
#       MISCITED by both finder and skeptic as an "already fixed" precedent
#       for F1 — it actually has tokio::time::timeout with ZERO
#       kill_on_drop anywhere in the file (`grep -c kill_on_drop pkg.rs` ->
#       0), because it spawns via safe_command_unsandboxed(), whose
#       UnsandboxedCommand::output()/spawn_streaming() build their own inner
#       tokio::process::Command that no caller can reach to chain
#       .kill_on_drop(true) onto. Fixed AT THE SOURCE in safe_cmd.rs (not
#       per-caller) — this closes the gap for every one of the ~48
#       safe_command_unsandboxed call sites across 8 files, not just
#       enable_php_stream, as a natural consequence of the correct fix
#       location.
#   F3  Laravel .env writer (replace_env_line, consumed by phpdotenv):
#       quoted a value only if it contained a space — never for '#', which
#       phpdotenv's EntryParser treats as an unconditional comment-start in
#       UNQUOTED_STATE (no whitespace precondition), silently truncating
#       everything after it. Fixed: unconditional double-quoting with \\ "
#       $ all escaped (a bare $ inside phpdotenv's DOUBLE_QUOTED_STATE
#       starts variable interpolation unless escaped) — verified against
#       the real vlucas/phpdotenv library (composer require), round-
#       tripping '#', space, '"', "'", '\', '$', and all of them combined
#       in one value.
#   F4  CodeIgniter .env writer (set_ci_env, CI4's OWN DotEnv.php — not
#       phpdotenv): never quoted any value at all. CI4's sanitizeValue()
#       throws an uncaught InvalidArgumentException for any unquoted value
#       containing whitespace, and per the skeptic this fires in
#       Boot::bootWeb() BEFORE CI4 installs its own exception handler — a
#       raw fatal on every request, not a graceful 500. Fixed: unconditional
#       double-quoting, but with a DIFFERENT escape set than F3 (\\ and "
#       only, deliberately leaving $ un-escaped) — CI4's own unquoting
#       regex only recognizes \\ and \" as escapes; a \$ would desync that
#       regex rather than produce a literal $. Verified against CI4's own
#       sanitizeValue()/resolveNestedVariables() source. Residual,
#       CI4-side, unfixable-from-here limitation documented in code: a
#       value containing a literal ${SOME_SET_ENV_VAR} substring still
#       interpolates, since CI4 provides no escape for it at all.
#   F5  ensure_composer() downloaded composer.phar via bare curl with zero
#       integrity check before chmod +x'ing it as the install engine for
#       4/5 CMS installers. Fixed: fetches the sha256 sidecar
#       getcomposer.org itself publishes at <url>.sha256, verifies before
#       chmod — fails closed (removes the phar) on any mismatch or fetch
#       failure.
#   F6  install_joomla() downloaded the release zip via bare curl with zero
#       integrity check. Unlike composer.phar, GitHub does not publish a
#       discrete checksum FILE for release assets — Joomla instead
#       publishes a per-package SHA-256 table inside the release notes'
#       free-text body (confirmed stable across 8 sampled releases incl.
#       betas/RCs). Fixed: switched tag-resolution from the old `curl -sI`
#       HEAD-redirect trick to GitHub's Releases JSON API (one call now
#       yields both tag_name and the checksum table), extracts the SHA-256
#       tied to the EXACT filename about to be downloaded, verifies before
#       unzip — fails closed on any parse/fetch/mismatch failure.
#   F7  install_drupal()'s db_url ("mysql://{user}:{pass}@{host}/{db}",
#       passed as one --db-url= argv to drush) had db_pass validated only
#       for newline/CR/null, and db_host had ZERO validation at all, unlike
#       every other credential field. Empirically verified against PHP's
#       actual parse_url() (which Drupal's Connection::
#       createConnectionOptionsFromUrl() feeds this through with no
#       urldecode() anywhere) that '/' and '?' break parsing outright and
#       '#' does too in the password — but silently TRUNCATES the host into
#       a URL fragment when in db_host, worse than a clean failure.
#       Percent-encoding was investigated and rejected (same parser never
#       decodes escapes, so an encoded char reaches the DB driver as a
#       literal "%23" instead of round-tripping). Fixed: reject '#' '/' '?'
#       in db_pass (added to the existing denylist), new allowlist
#       validation for db_host (previously unvalidated).
#
# Deliberately NOT touched (verified correct/out-of-scope, not re-flagged
# here):
#   - Joomla's --admin-user=/--admin-username= dual flags: confirmed by both
#     finder and skeptic against Joomla's actual setup.xml form definition
#     to be two genuinely distinct required fields, not a redundant pair.
#   - SSRF-via-registry-host in image_scanner.rs/sbom_scanner.rs
#     (completeness critic off-menu find): the SAME finding already
#     deferred in the prior session's ledger, not new — not re-fixed here.
#
# Pure source analysis; no box, no network, no build.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

# Strip comments and the test module. NOT the naive "sed-quit-at-first-
# #[cfg(test)]" shape — see project_dockpanel_lessons_p182 (s456): only exit
# at a #[cfg(test)] line immediately followed by `mod `, a real test module,
# not a cfg-gated function/const variant elsewhere in the file.
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

# One named function's body, comment-stripped. Function-name patterns MUST be
# anchored (^ ... \() — GNU awk's \b silently matches nothing in default ERE
# mode (project_dockpanel_lessons_p182, s456).
fnbody()    { code "$1" | awk "/$2/,/^}/"; }
bodyhas()   { [ -n "$(fnbody "$1" "$2" | grep -F -- "$3")" ]; }
bodyhasre() { [ -n "$(fnbody "$1" "$2" | grep -E -- "$3")" ]; }
countin()   { fnbody "$1" "$2" | grep -Fc -- "$3"; }

CMS=panel/agent/src/services/cms.rs
CMS_ROUTE=panel/agent/src/routes/cms.rs
PKG=panel/agent/src/services/pkg.rs
SAFE_CMD=panel/agent/src/safe_cmd.rs

for f in "$CMS" "$CMS_ROUTE" "$PKG" "$SAFE_CMD"; do
  [ -f "$f" ] || { echo "missing source file: $f"; exit 1; }
done

echo "── 1. F1: run_cmd/run_cmd_in take a timeout and kill_on_drop the child ──"
if bodyhasre "$CMS" "^async fn run_cmd\(" 'kill_on_drop\(true\)'; then
  ok "run_cmd sets kill_on_drop(true)"
else
  bad "run_cmd is missing kill_on_drop(true)"
fi
if bodyhasre "$CMS" "^async fn run_cmd\(" 'tokio::time::timeout'; then
  ok "run_cmd wraps its spawn in tokio::time::timeout"
else
  bad "run_cmd has no timeout wrapper"
fi
if bodyhasre "$CMS" "^async fn run_cmd_in\(" 'kill_on_drop\(true\)'; then
  ok "run_cmd_in sets kill_on_drop(true)"
else
  bad "run_cmd_in is missing kill_on_drop(true)"
fi
if bodyhasre "$CMS" "^async fn run_cmd_in\(" 'tokio::time::timeout'; then
  ok "run_cmd_in wraps its spawn in tokio::time::timeout"
else
  bad "run_cmd_in has no timeout wrapper"
fi
# Every call site actually passes a timeout argument (regression against a
# future call site added without one). Anchored on [[:space:]] rather than
# \s — this grep's \s does not reliably combine with an alternation group
# (empirically confirmed while writing this pin: \s silently failed to
# match "let _ = run_cmd_in(" once the (_in)? alternation was added,
# [[:space:]] did not). Excludes the 2 function definitions (their lines
# start "async fn run_cmd", not "run_cmd(" / "let x = run_cmd(").
CALL_COUNT=$(code "$CMS" | grep -cE '^[[:space:]]*(let [a-z_]+ = )?run_cmd(_in)?\(')
TIMEOUT_ARG_COUNT=$(code "$CMS" | grep -vE '^const ' | grep -cE '(COMPOSER|CLI_INSTALL|DOWNLOAD|QUICK)_TIMEOUT')
if [ "$CALL_COUNT" -eq 12 ] && [ "$TIMEOUT_ARG_COUNT" -ge 12 ]; then
  ok "all $CALL_COUNT run_cmd(_in) call sites pass a *_TIMEOUT constant ($TIMEOUT_ARG_COUNT non-const-def references)"
else
  bad "call-site/timeout-arg count mismatch (calls=$CALL_COUNT, expected 12; timeout-refs=$TIMEOUT_ARG_COUNT) — a call site may be missing its timeout, or one was added/removed"
fi

echo
echo "── 2. F1: ensure_composer()'s and install_joomla()'s direct spawns are also guarded ──"
if bodyhasre "$CMS" "^pub async fn ensure_composer" 'kill_on_drop\(true\)'; then
  ok "ensure_composer sets kill_on_drop(true) on its curl call(s)"
else
  bad "ensure_composer is missing kill_on_drop(true)"
fi
if bodyhasre "$CMS" "^pub async fn ensure_composer" 'tokio::time::timeout'; then
  ok "ensure_composer wraps its curl call(s) in tokio::time::timeout"
else
  bad "ensure_composer has no timeout wrapper"
fi

echo
echo "── 3. F2: safe_cmd.rs's UnsandboxedCommand sets kill_on_drop centrally ──"
if bodyhasre "$SAFE_CMD" "^    pub async fn output\(&mut self\)" 'cmd\.kill_on_drop\(true\)'; then
  ok "UnsandboxedCommand::output() sets kill_on_drop(true) on the inner systemd-run Command"
else
  bad "UnsandboxedCommand::output() is missing kill_on_drop(true) — pkg.rs::enable_php_stream (and ~48 other call sites) regress"
fi
if bodyhasre "$SAFE_CMD" "^    pub fn spawn_streaming\(&mut self\)" 'cmd\.kill_on_drop\(true\)'; then
  ok "UnsandboxedCommand::spawn_streaming() sets kill_on_drop(true) too"
else
  bad "UnsandboxedCommand::spawn_streaming() is missing kill_on_drop(true)"
fi
# pkg.rs itself is untouched (the fix is centralized) — confirm it still has
# ZERO kill_on_drop of its own, proving the fix genuinely lives in the
# shared helper and not duplicated/reverted at the call site.
if hasre "$PKG" 'kill_on_drop'; then
  bad "pkg.rs now has its own kill_on_drop call — the centralized safe_cmd.rs fix may have been bypassed by a direct per-call one (not wrong, but re-check this pin's premise)"
else
  ok "pkg.rs has no kill_on_drop of its own (control: the fix is genuinely centralized in safe_cmd.rs, not duplicated here)"
fi
if bodyhasre "$PKG" "^pub async fn enable_php_stream" 'safe_command_unsandboxed'; then
  ok "enable_php_stream still spawns via safe_command_unsandboxed (control: it's the same call path the safe_cmd.rs fix protects)"
else
  bad "enable_php_stream no longer calls safe_command_unsandboxed — re-check whether this pin's premise still holds"
fi

# Literal (grep -F, not regex) substrings of the actual Rust source text —
# built via single-quoted bash strings so no shell/regex escaping games are
# needed. BACKSLASH is the raw 4-char sequence \\\\ (the source text of the
# string literal "\\\\", i.e. "replace two backslashes with one escaped
# pair"). DQUOTE is the raw 4-char sequence \\\" (source text of "\\\"").
# DOLLAR is the raw 3-char sequence \\$ (source text of "\\$").
BACKSLASH_ESC='\\\\'
DQUOTE_ESC='\\\"'
DOLLAR_ESC='\\$'

echo
echo "── 4. F3: Laravel .env quoting round-trips '#', space, quotes, backslash, \$ ──"
if bodyhas "$CMS" "^fn quote_laravel_env_value" "$BACKSLASH_ESC"; then
  ok "quote_laravel_env_value escapes backslash"
else
  bad "quote_laravel_env_value does not escape backslash"
fi
if bodyhas "$CMS" "^fn quote_laravel_env_value" "$DQUOTE_ESC"; then
  ok "quote_laravel_env_value escapes the double-quote"
else
  bad "quote_laravel_env_value does not escape the double-quote"
fi
if bodyhas "$CMS" "^fn quote_laravel_env_value" "$DOLLAR_ESC"; then
  ok "quote_laravel_env_value escapes \$ (prevents phpdotenv variable interpolation)"
else
  bad "quote_laravel_env_value does not escape \$ — an unescaped \$var in a password would be interpolated by phpdotenv"
fi
if bodyhas "$CMS" "^fn replace_env_line" "quote_laravel_env_value(value)"; then
  ok "replace_env_line quotes UNCONDITIONALLY now (not only when the value contains a space)"
else
  bad "replace_env_line no longer routes through quote_laravel_env_value — the '#'-truncation bug may have regressed"
fi

echo
echo "── 5. F4: CodeIgniter .env quoting escapes \\\\/\" but deliberately NOT \$ ──"
if bodyhas "$CMS" "^fn quote_ci4_env_value" "$BACKSLASH_ESC"; then
  ok "quote_ci4_env_value escapes backslash"
else
  bad "quote_ci4_env_value does not escape backslash"
fi
if bodyhas "$CMS" "^fn quote_ci4_env_value" "$DQUOTE_ESC"; then
  ok "quote_ci4_env_value escapes the double-quote"
else
  bad "quote_ci4_env_value does not escape the double-quote"
fi
if bodyhas "$CMS" "^fn quote_ci4_env_value" "$DOLLAR_ESC"; then
  bad "quote_ci4_env_value now escapes \$ — CI4's sanitizeValue() has no \\\$ escape and would desync its unquoting regex (this is CORRECT for phpdotenv, WRONG for CI4 — do not unify the two functions)"
else
  ok "quote_ci4_env_value correctly leaves \$ un-escaped (CI4-specific: it has no \\\$ escape unlike phpdotenv)"
fi
if bodyhas "$CMS" "^fn set_ci_env" "quote_ci4_env_value(value)"; then
  ok "set_ci_env quotes UNCONDITIONALLY now (previously quoted nothing, ever)"
else
  bad "set_ci_env no longer routes through quote_ci4_env_value — the space-crash bug may have regressed"
fi
# Regression control: the app.baseURL call site used to pre-wrap its own
# value in single quotes BEFORE set_ci_env existed to quote unconditionally
# — if that literal single-quote wrapping is still present, set_ci_env would
# now double-quote it (producing a broken "'https://...'" value).
if hasre "$CMS" "app\.baseURL.*'https"; then
  bad "app.baseURL's call site still pre-wraps in single quotes — now DOUBLE-quoted by set_ci_env, producing a broken value"
else
  ok "app.baseURL's call site no longer pre-wraps in single quotes (control: no double-quoting regression)"
fi

echo
echo "── 6. F5: ensure_composer() verifies composer.phar's sha256 before chmod +x ──"
if bodyhas "$CMS" "^pub async fn ensure_composer" ".sha256"; then
  ok "ensure_composer fetches the .sha256 sidecar"
else
  bad "ensure_composer no longer fetches a checksum sidecar"
fi
if bodyhasre "$CMS" "^pub async fn ensure_composer" 'Sha256::digest'; then
  ok "ensure_composer computes the downloaded phar's own sha256"
else
  bad "ensure_composer no longer computes the phar's sha256"
fi
if bodyhasre "$CMS" "^pub async fn ensure_composer" 'if actual != expected'; then
  ok "ensure_composer compares actual vs. expected before proceeding"
else
  bad "ensure_composer no longer compares the computed hash against the fetched one"
fi
# Fail-closed control: a mismatch must not still chmod +x the phar.
MISMATCH_TO_CHMOD=$(fnbody "$CMS" "^pub async fn ensure_composer" | awk '/if actual != expected/,/^    }/' | grep -c 'chmod')
if [ "$MISMATCH_TO_CHMOD" -eq 0 ]; then
  ok "the mismatch branch does not chmod +x the phar (fails closed)"
else
  bad "the mismatch branch appears to still chmod +x the phar — would not fail closed"
fi

echo
echo "── 7. F6: install_joomla() verifies the release zip's sha256 before unzip ──"
if bodyhas "$CMS" "^pub async fn install_joomla" "api.github.com"; then
  ok "install_joomla resolves the release via GitHub's JSON API (not the old -sI redirect trick)"
else
  bad "install_joomla no longer uses the GitHub Releases API"
fi
if bodyhas "$CMS" "^pub async fn install_joomla" "extract_release_sha256("; then
  ok "install_joomla extracts the published checksum via extract_release_sha256"
else
  bad "install_joomla no longer extracts a checksum from the release notes"
fi
if bodyhasre "$CMS" "^fn extract_release_sha256" 'body\.find\(filename\)'; then
  ok "extract_release_sha256 ties extraction to the EXACT filename being downloaded (not a generic 'ZIP Archive' label match, which could cross-match the Update-Packages table)"
else
  bad "extract_release_sha256 no longer anchors on the exact filename — could cross-match the wrong table row"
fi
# unzip must run AFTER the hash check, not before — order matters for a
# fail-closed guarantee. Assert the checksum-mismatch return precedes unzip
# in the function body's line order.
JOOMLA_BODY_FILE=$(mktemp)
fnbody "$CMS" "^pub async fn install_joomla" > "$JOOMLA_BODY_FILE"
MISMATCH_LINE=$(grep -n 'checksum mismatch' "$JOOMLA_BODY_FILE" | head -1 | cut -d: -f1)
UNZIP_LINE=$(grep -n '"unzip"' "$JOOMLA_BODY_FILE" | head -1 | cut -d: -f1)
rm -f "$JOOMLA_BODY_FILE"
if [ -n "$MISMATCH_LINE" ] && [ -n "$UNZIP_LINE" ] && [ "$MISMATCH_LINE" -lt "$UNZIP_LINE" ]; then
  ok "the checksum-mismatch check appears BEFORE unzip in source order (fails closed)"
else
  bad "could not confirm the checksum check precedes unzip (mismatch_line=$MISMATCH_LINE, unzip_line=$UNZIP_LINE) — re-verify by hand"
fi

echo
echo "── 8. F7: routes/cms.rs — db_pass rejects '#' '/' '?', db_host gets new validation ──"
if bodyhasre "$CMS_ROUTE" "^async fn install\(" "db_pass\.contains\(\['\\\\n', '\\\\r', '\\\\0', '#', '/', '\?'\]\)"; then
  ok "db_pass validation now rejects '#' '/' '?' in addition to newline/CR/null"
else
  bad "db_pass validation no longer rejects '#' '/' '?' — install_drupal's db_url could break or silently misparse again"
fi
if hasre "$CMS_ROUTE" '^    fn is_valid_db_host'; then
  ok "a dedicated is_valid_db_host validator now exists"
else
  bad "is_valid_db_host is missing"
fi
if bodyhas "$CMS_ROUTE" "^async fn install\(" "is_valid_db_host(db_host)"; then
  ok "db_host is actually run through is_valid_db_host() in the route body (not just referenced elsewhere, e.g. the per-CMS db_host.as_deref() extraction, which existed before this fix too)"
else
  bad "db_host is not validated via is_valid_db_host() — it may still be referenced elsewhere (e.g. extraction) without ever being checked"
fi
# Positive control: db_user's existing alnum+_- charset check must be
# untouched — this pin should not have widened or narrowed an unrelated
# field while touching db_pass/db_host.
if bodyhasre "$CMS_ROUTE" "^async fn install\(" "is_valid_db_identifier\(db_user\)"; then
  ok "db_user's existing validation is untouched (control)"
else
  bad "db_user's validation appears to have changed — unintended side effect"
fi

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
