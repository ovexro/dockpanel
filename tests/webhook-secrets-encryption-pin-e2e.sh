#!/usr/bin/env bash
# webhook-secrets-encryption-pin-e2e.sh — s431
#
# Regression pins for the 5 sibling plaintext webhook/verify secrets deferred
# from s430's notification-secrets rollout: `deploy_configs.webhook_secret`,
# `git_deploys.webhook_secret`, `extensions.webhook_secret`,
# `whmcs_config.webhook_secret`, `webhook_endpoints.verify_secret` were all
# plain columns, with zero `secrets_crypto` reference anywhere in the files
# that wrote or read them.
#
# ⚠ The s430 ledger's own design note characterized deploy_configs/git_deploys
# as "compare-only — should be HASHED, mirroring extensions.api_key_hash".
# That was wrong: Deploy.tsx and GitDeploys.tsx read `config.webhook_secret`
# from the GET/list response on EVERY page load to render the webhook URL to
# paste into the git host — not shown once at creation like an API key. A
# hash-only design would have silently broken that display on the very next
# page load after creation. All five subjects here are reversible-encrypt.
#
# ⚠ A second defect a naive port of the s430 pattern would have shipped: three
# of the five columns (deploy_configs.webhook_secret VARCHAR(64),
# git_deploys.webhook_secret VARCHAR(64), webhook_endpoints.verify_secret
# VARCHAR(200)) are sized for the PLAINTEXT. Base64(nonce+ciphertext+tag) is
# longer than the plaintext it wraps — encrypting in place without widening
# would overflow the column and fail every write. Migration
# 20260830010000_widen_webhook_secret_columns.sql widens all three to TEXT.
#
#   §A  webhook_gateway.rs::create_endpoint ENCRYPTS verify_secret before the
#       INSERT; receive_webhook DECRYPTS it before classify_signature. No
#       update route exists for this field (endpoints are delete+recreate).
#   §B  extensions.rs::create/rotate_secret ENCRYPT webhook_secret, returning
#       the plaintext in the one-time response; test_webhook DECRYPTS before
#       signing. services::extensions::emit_event has no AppState (PgPool
#       only), so it decrypts via decrypt_credential_from_env, matching
#       notifications.rs/uptime.rs's s430 precedent.
#   §C  whmcs.rs::update_config ENCRYPTS (decrypting the prior value first, so
#       the preserve-vs-mint comparison still runs on plaintext); get_config
#       and the inbound webhook receiver DECRYPT.
#   §D  deploy.rs::set_config ENCRYPTS on write, then decrypts whatever
#       RETURNING * actually gives back (not the freshly-generated local) —
#       ON CONFLICT's DO UPDATE never touches this column, so an
#       update-existing call must show the ORIGINAL secret, not a new one
#       that was never stored. get_config and the webhook receiver DECRYPT.
#   §E  git_deploys.rs::create ENCRYPTS; list/get_one/update DECRYPT
#       alongside the existing mask_github_token() call; the webhook receiver
#       DECRYPTS before the SHA256 hash-compare.
#   §F  credential_reencrypt.rs's self-enforcing registry carries all 5 new
#       subjects in SIMPLE_SUBJECTS and pairs each writer module in
#       COVERED_MODULES.
#   §G  the three too-narrow columns are widened to TEXT.
#
# Pure source analysis: no box, no network, no build.
set -uo pipefail
cd "$(dirname "$0")/.."

GATEWAY=panel/backend/src/routes/webhook_gateway.rs
EXTENSIONS=panel/backend/src/routes/extensions.rs
EXT_SVC=panel/backend/src/services/extensions.rs
WHMCS=panel/backend/src/routes/whmcs.rs
DEPLOY=panel/backend/src/routes/deploy.rs
GITDEPLOYS=panel/backend/src/routes/git_deploys.rs
REENCRYPT=panel/backend/src/services/credential_reencrypt.rs
MIGRATION=panel/backend/migrations/20260830010000_widen_webhook_secret_columns.sql

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }
has()  { grep -qE -- "$2" "$1" 2>/dev/null && ok "$3" || bad "$3"; }
# Multi-line variant: several calls below are formatted with each argument on
# its own line, so the pattern must match ACROSS lines. -z null-separates
# input (the whole file becomes one "line"), -P gives Perl regex where \s
# already spans newlines.
hasml() { grep -qPzo -- "$2" "$1" 2>/dev/null && ok "$3" || bad "$3"; }
count() { grep -oE -- "$2" "$1" 2>/dev/null | wc -l | tr -d ' '; }

echo "== §A  webhook_gateway.rs: encrypt on create, decrypt on receive =="

has "$GATEWAY" 'encrypted_verify_secret = verify_secret' \
  "A1 create_endpoint encrypts verify_secret before binding"
has "$GATEWAY" '\.bind\(encrypted_verify_secret\)' \
  "A2 the INSERT binds the encrypted local, not the raw Option"
if grep -qE '^\s*\.bind\(verify_secret\)$' "$GATEWAY"; then
  bad "A3 CONTROL: raw verify_secret is no longer bound directly"
else
  ok "A3 CONTROL: raw verify_secret is no longer bound directly"
fi
has "$GATEWAY" 'decrypted_verify_secret = endpoint\.verify_secret\.as_deref\(\)\.map' \
  "A4 receive_webhook decrypts endpoint.verify_secret"
hasml "$GATEWAY" 'classify_signature\(\s*&endpoint\.verify_mode,\s*decrypted_verify_secret\.as_deref\(\)' \
  "A5 classify_signature is called with the DECRYPTED value, not the raw field"

echo
echo "== §B  extensions.rs + services/extensions.rs: encrypt on write, decrypt on send =="

hasml "$EXTENSIONS" 'encrypted_webhook_secret = crate::services::secrets_crypto::encrypt_credential\(\s*&webhook_secret' \
  "B1 create() encrypts the freshly-minted webhook_secret"
has "$EXTENSIONS" '\.bind\(&encrypted_webhook_secret\)' \
  "B2 create()'s INSERT binds the encrypted local"
has "$EXTENSIONS" '"webhook_secret": webhook_secret,' \
  "B3 create()'s one-time response still returns the PLAINTEXT (not the ciphertext)"
hasml "$EXTENSIONS" 'encrypted_new_secret = crate::services::secrets_crypto::encrypt_credential\(\s*&new_secret' \
  "B4 rotate_secret() encrypts the freshly-minted secret"
has "$EXTENSIONS" '\.bind\(&encrypted_new_secret\)' \
  "B5 rotate_secret()'s UPDATE binds the encrypted local"
hasml "$EXTENSIONS" 'webhook_secret = crate::services::secrets_crypto::decrypt_credential_or_legacy\(\s*&webhook_secret' \
  "B6 test_webhook decrypts before computing the HMAC signature"
has "$EXT_SVC" 'decrypt_credential_from_env\(&webhook_secret\)' \
  "B7 emit_event decrypts via the AppState-free helper (no AppState in this fn)"

echo
echo "== §C  whmcs.rs: encrypt on save, decrypt on read + inbound verify =="

hasml "$WHMCS" 'cur_secret = cur_secret\s*\.map\(\|s\| crate::services::secrets_crypto::decrypt_credential_or_legacy' \
  "C1 update_config decrypts the prior value before the preserve-vs-mint decision"
hasml "$WHMCS" 'encrypted_webhook_secret = crate::services::secrets_crypto::encrypt_credential\(\s*&webhook_secret' \
  "C2 update_config encrypts before the INSERT/DO UPDATE"
has "$WHMCS" '\.bind\(&encrypted_webhook_secret\)' \
  "C3 the SQL bind chain uses the encrypted local"
has "$WHMCS" '"webhook_secret": webhook_secret' \
  "C4 the save response still returns PLAINTEXT"
has "$WHMCS" 'let webhook = webhook\.map\(\|w\| \{' \
  "C5 get_config decrypts before serializing"
has "$WHMCS" 'let secret = secret\.map\(\|s\| \{' \
  "C6 the inbound webhook receiver decrypts before the ct_eq compare"

echo
echo "== §D  deploy.rs: encrypt on write, decrypt what RETURNING * actually holds =="

hasml "$DEPLOY" 'encrypted_webhook_secret = crate::services::secrets_crypto::encrypt_credential\(\s*&webhook_secret' \
  "D1 set_config encrypts the freshly-minted secret"
has "$DEPLOY" '\.bind\(&encrypted_webhook_secret\)' \
  "D2 the INSERT...ON CONFLICT binds the encrypted local"
hasml "$DEPLOY" 'config\.webhook_secret = crate::services::secrets_crypto::decrypt_credential_or_legacy\(\s*&config\.webhook_secret' \
  "D3 set_config decrypts the RETURNING-* value in place (not the pre-write local)"
# Ordering/correctness control: ON CONFLICT's DO UPDATE never assigns
# webhook_secret, so an update-existing call must return the ORIGINAL stored
# secret, not the freshly-generated one that was silently discarded.
has "$DEPLOY" 'repo_url = \$2, branch = \$3, deploy_script = \$4, auto_deploy = \$5, atomic_deploy = \$7, keep_releases = \$8, updated_at = NOW\(\)' \
  "D4 CONTROL: DO UPDATE's SET list still omits webhook_secret (confirms decrypt-what-came-back is the correct fix, not a stale assumption)"
has "$DEPLOY" 'let config = config\.map\(\|mut c\| \{' \
  "D5 get_config decrypts before returning"
hasml "$DEPLOY" 'stored_secret = crate::services::secrets_crypto::decrypt_credential_or_legacy\(\s*&config\.webhook_secret' \
  "D6 the inbound webhook receiver decrypts before the ct_eq compare"
has "$DEPLOY" 'secret\.as_bytes\(\)\.ct_eq\(stored_secret\.as_bytes\(\)\)' \
  "D7 the ct_eq compare uses the DECRYPTED local, not config.webhook_secret directly"

echo
echo "== §E  git_deploys.rs: encrypt on create, decrypt on every read + inbound verify =="

hasml "$GITDEPLOYS" 'encrypted_webhook_secret = crate::services::secrets_crypto::encrypt_credential\(\s*&webhook_secret' \
  "E1 create() encrypts the freshly-minted secret"
has "$GITDEPLOYS" '\.bind\(&encrypted_webhook_secret\)' \
  "E2 the INSERT binds the encrypted local"
N=$(count "$GITDEPLOYS" 'deploy\.webhook_secret = crate::services::secrets_crypto::decrypt_credential_or_legacy')
if [ "$N" -ge 3 ]; then
  ok "E3 decrypted in place at create/get_one/update (>=3 call sites, found $N)"
else
  bad "E3 decrypted in place at create/get_one/update (found $N, want >=3)"
fi
hasml "$GITDEPLOYS" 'd\.webhook_secret = crate::services::secrets_crypto::decrypt_credential_or_legacy\(\s*&d\.webhook_secret' \
  "E4 list() decrypts each row in the same loop that masks github_token"
hasml "$GITDEPLOYS" 'stored_secret = crate::services::secrets_crypto::decrypt_credential_or_legacy\(\s*&config\.webhook_secret' \
  "E5 the inbound webhook receiver decrypts before hashing"
has "$GITDEPLOYS" 'h\.update\(stored_secret\.as_bytes\(\)\)' \
  "E6 the stored-side SHA256 hash is computed over the DECRYPTED value"

echo
echo "== §F  credential_reencrypt.rs's registry carries all 5 new subjects =="

for subj in \
  '"webhook_endpoints", "id", "verify_secret"' \
  '"extensions", "id", "webhook_secret"' \
  '"whmcs_config", "id", "webhook_secret"' \
  '"deploy_configs", "id", "webhook_secret"' \
  '"git_deploys", "id", "webhook_secret"'; do
  esc=$(printf '%s' "$subj" | sed 's/[.[\*^$()+?{|]/\\&/g')
  if grep -qE -- "\($esc\)" "$REENCRYPT"; then
    ok "F1 SIMPLE_SUBJECTS contains ($subj)"
  else
    bad "F1 SIMPLE_SUBJECTS contains ($subj)"
  fi
done

for pair in \
  '"webhook_gateway", "webhook_endpoints.verify_secret"' \
  '"extensions", "extensions.webhook_secret"' \
  '"whmcs", "whmcs_config.webhook_secret"' \
  '"deploy", "deploy_configs.webhook_secret"' \
  '"git_deploys", "git_deploys.webhook_secret"'; do
  esc=$(printf '%s' "$pair" | sed 's/[.[\*^$()+?{|]/\\&/g')
  if grep -qE -- "\($esc\)" "$REENCRYPT"; then
    ok "F2 COVERED_MODULES pairs ($pair)"
  else
    bad "F2 COVERED_MODULES pairs ($pair)"
  fi
done

echo
echo "== §G  the three plaintext-sized columns are widened to TEXT =="

has "$MIGRATION" 'ALTER TABLE deploy_configs ALTER COLUMN webhook_secret TYPE TEXT' \
  "G1 deploy_configs.webhook_secret widened to TEXT"
has "$MIGRATION" 'ALTER TABLE git_deploys ALTER COLUMN webhook_secret TYPE TEXT' \
  "G2 git_deploys.webhook_secret widened to TEXT"
has "$MIGRATION" 'ALTER TABLE webhook_endpoints ALTER COLUMN verify_secret TYPE TEXT' \
  "G3 webhook_endpoints.verify_secret widened to TEXT"
# Negative control: the original CREATE TABLE migrations must be untouched —
# this is a widen-forward, not an edit to a migration already shipped.
has "panel/backend/migrations/20260312400000_deploy_configs.sql" 'webhook_secret VARCHAR\(64\) NOT NULL' \
  "G4 CONTROL: the original deploy_configs migration is untouched (widen is a separate forward migration)"
has "panel/backend/migrations/20260318200000_git_deploys.sql" 'webhook_secret VARCHAR\(64\) NOT NULL' \
  "G5 CONTROL: the original git_deploys migration is untouched"

echo
printf 'webhook-secrets-encryption: \033[0;32m%d passed\033[0m, \033[0;31m%d failed\033[0m\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
