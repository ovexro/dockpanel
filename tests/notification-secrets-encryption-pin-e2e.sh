#!/usr/bin/env bash
# notification-secrets-encryption-pin-e2e.sh — s430
#
# Regression pins for closing the "cleartext credential hole" a s251-era audit
# flagged and nobody fixed for ~180 sessions: `alert_rules.notify_pagerduty_key`/
# `notify_webhook_url`/`notify_slack_url`/`notify_discord_url` and
# `monitors.alert_slack_url`/`alert_discord_url` were plain columns, with zero
# `secrets_crypto` reference anywhere in the files that wrote or read them.
#
#   §A  alerts.rs::upsert_rules ENCRYPTS all four notify_* fields (via
#       encrypt_credential, the credential-shaped derivation — NOT the vault's
#       encrypt/decrypt) before they ever reach SQL, on plaintext already
#       validated by the SSRF check above it.
#   §B  alerts.rs::get_rules DECRYPTS the same four fields (via
#       decrypt_credential_or_legacy, the migration-safe fallback) before
#       serializing the response — the same-account caller reading their own
#       settings back is the expected case that function exists for.
#   §C  notifications.rs::get_user_channels and uptime.rs::send_alerts — both
#       take only a PgPool (no AppState/jwt_secret) — decrypt via
#       decrypt_credential_from_env, matching that function's own documented
#       use case.
#   §D  monitors.rs's create/update ENCRYPT alert_slack_url/alert_discord_url;
#       list/create/update DECRYPT them back via the shared
#       decrypt_monitor_alert_urls helper. create's "inherit from global
#       alert_rules" fallback decrypts BEFORE the SSRF check runs on it.
#   §E  credential_reencrypt.rs's self-enforcing registry carries all 6 new
#       subjects in SIMPLE_SUBJECTS and pairs each writer module in
#       COVERED_MODULES — `every_credential_writer_is_covered` and
#       `subject_tokens_match_the_sweep` (both in that file's own test module)
#       assert this mechanically; §E here just pins the specific entries so a
#       regression names which one went missing, not just "a Rust test failed".
#
# Pure source analysis: no box, no network, no build.
set -uo pipefail
cd "$(dirname "$0")/.."

ALERTS=panel/backend/src/routes/alerts.rs
MONITORS=panel/backend/src/routes/monitors.rs
NOTIFICATIONS=panel/backend/src/services/notifications.rs
UPTIME=panel/backend/src/services/uptime.rs
REENCRYPT=panel/backend/src/services/credential_reencrypt.rs

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }
has()  { grep -qE -- "$2" "$1" 2>/dev/null && ok "$3" || bad "$3"; }
count() { grep -oE -- "$2" "$1" 2>/dev/null | wc -l | tr -d ' '; }

echo "== §A  alerts.rs::upsert_rules encrypts all four notify_* fields =="

N=$(count "$ALERTS" 'encrypt_credential\(s, jwt_secret\)')
if [ "$N" -ge 1 ]; then
  ok "A1 upsert_rules calls encrypt_credential (shared closure, $N call site)"
else
  bad "A1 upsert_rules calls encrypt_credential"
fi
has "$ALERTS" 'let enc_slack_url = encrypt_opt\(&body\.notify_slack_url\)\?' \
  "A2 notify_slack_url is routed through the encrypting closure"
has "$ALERTS" 'let enc_discord_url = encrypt_opt\(&body\.notify_discord_url\)\?' \
  "A3 notify_discord_url is routed through the encrypting closure"
has "$ALERTS" 'let enc_pagerduty_key = encrypt_opt\(&body\.notify_pagerduty_key\)\?' \
  "A4 notify_pagerduty_key is routed through the encrypting closure"
has "$ALERTS" 'let enc_webhook_url = encrypt_opt\(&body\.notify_webhook_url\)\?' \
  "A5 notify_webhook_url is routed through the encrypting closure"
has "$ALERTS" '\.bind\(&enc_slack_url\)' \
  "A6 the SQL bind chain uses the ENCRYPTED local, not the raw body field"
# Negative control: the raw plaintext must not still be what gets bound.
if grep -qE '\.bind\(&body\.notify_slack_url\)' "$ALERTS"; then
  bad "A7 CONTROL: raw body.notify_slack_url is no longer bound directly (old plaintext bind removed)"
else
  ok "A7 CONTROL: raw body.notify_slack_url is no longer bound directly"
fi

echo
echo "== §B  alerts.rs::get_rules decrypts all four before serializing =="

N=$(count "$ALERTS" 'decrypt_credential_or_legacy')
if [ "$N" -ge 4 ]; then
  ok "B1 get_rules calls decrypt_credential_or_legacy at least 4 times ($N found)"
else
  bad "B1 get_rules calls decrypt_credential_or_legacy at least 4 times (found $N)"
fi
has "$ALERTS" 'r\.notify_slack_url = r\.notify_slack_url' "B2 notify_slack_url is decrypted in place"
has "$ALERTS" 'r\.notify_pagerduty_key = r\.notify_pagerduty_key' "B3 notify_pagerduty_key is decrypted in place"

echo
echo "== §C  contexts with only a PgPool decrypt via decrypt_credential_from_env =="

has "$NOTIFICATIONS" 'decrypt_credential_from_env' \
  "C1 notifications::get_user_channels decrypts via the AppState-free helper"
N=$(count "$NOTIFICATIONS" 'decrypt_credential_from_env')
if [ "$N" -ge 4 ]; then
  ok "C2 get_user_channels decrypts all 4 fields ($N call sites)"
else
  bad "C2 get_user_channels decrypts all 4 fields (found $N, want >=4)"
fi
has "$UPTIME" 'decrypt_credential_from_env as decrypt_env' \
  "C3 uptime::send_alerts imports the same AppState-free helper"
N=$(count "$UPTIME" 'decrypt_env')
if [ "$N" -ge 4 ]; then
  ok "C4 send_alerts decrypts pagerduty/webhook/slack/discord ($N uses)"
else
  bad "C4 send_alerts decrypts pagerduty/webhook/slack/discord (found $N, want >=4)"
fi

echo
echo "== §D  monitors.rs: encrypt on write, decrypt on every read =="

has "$MONITORS" 'fn decrypt_monitor_alert_urls' \
  "D1 a shared decrypt helper exists for Monitor's two alert-url fields"
N=$(count "$MONITORS" 'decrypt_monitor_alert_urls\(')
if [ "$N" -ge 4 ]; then
  ok "D2 the helper is called from list/create/update (>=1 call each, $N total call sites incl. its own definition)"
else
  bad "D2 the helper is called enough times (found $N call sites incl. definition, want >=4)"
fi
has "$MONITORS" 'let enc_slack_url = encrypt_opt\(&slack_url\)\?' \
  "D3 create() encrypts before the INSERT"
has "$MONITORS" 'let enc_slack_url = encrypt_opt\(&body\.alert_slack_url\)\?' \
  "D4 update() encrypts before the UPDATE"
has "$MONITORS" 'let global_slack = global_slack\.as_deref\(\)\.map' \
  "D5 create()'s inherit-from-global fallback decrypts BEFORE the SSRF check runs on it"
# Ordering control: the decrypt of the inherited global value must appear
# BEFORE the SSRF validate_url_not_internal call that consumes slack_url.
DECRYPT_LINE=$(grep -n 'let global_slack = global_slack\.as_deref' "$MONITORS" | head -1 | cut -d: -f1)
SSRF_LINE=$(grep -n 'Invalid Slack alert URL' "$MONITORS" | head -1 | cut -d: -f1)
if [ -n "$DECRYPT_LINE" ] && [ -n "$SSRF_LINE" ] && [ "$DECRYPT_LINE" -lt "$SSRF_LINE" ]; then
  ok "D6 the decrypt (line $DECRYPT_LINE) runs BEFORE the SSRF check (line $SSRF_LINE), not after"
else
  bad "D6 decrypt-before-SSRF-check ordering (decrypt=$DECRYPT_LINE ssrf=$SSRF_LINE)"
fi

echo
echo "== §E  credential_reencrypt.rs's registry carries all 6 new subjects =="

for subj in \
  '"alert_rules", "id", "notify_pagerduty_key"' \
  '"alert_rules", "id", "notify_webhook_url"' \
  '"alert_rules", "id", "notify_slack_url"' \
  '"alert_rules", "id", "notify_discord_url"' \
  '"monitors", "id", "alert_slack_url"' \
  '"monitors", "id", "alert_discord_url"'; do
  esc=$(printf '%s' "$subj" | sed 's/[.[\*^$()+?{|]/\\&/g')
  if grep -qE -- "\($esc\)" "$REENCRYPT"; then
    ok "E1 SIMPLE_SUBJECTS contains ($subj)"
  else
    bad "E1 SIMPLE_SUBJECTS contains ($subj)"
  fi
done

for pair in \
  '"alerts", "alert_rules.notify_pagerduty_key"' \
  '"alerts", "alert_rules.notify_webhook_url"' \
  '"alerts", "alert_rules.notify_slack_url"' \
  '"alerts", "alert_rules.notify_discord_url"' \
  '"monitors", "monitors.alert_slack_url"' \
  '"monitors", "monitors.alert_discord_url"'; do
  esc=$(printf '%s' "$pair" | sed 's/[.[\*^$()+?{|]/\\&/g')
  if grep -qE -- "\($esc\)" "$REENCRYPT"; then
    ok "E2 COVERED_MODULES pairs ($pair)"
  else
    bad "E2 COVERED_MODULES pairs ($pair)"
  fi
done

echo
printf 'notification-secrets-encryption: \033[0;32m%d passed\033[0m, \033[0;31m%d failed\033[0m\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
