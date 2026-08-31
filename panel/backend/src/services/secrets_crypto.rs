//! AES-256-GCM encryption for the Secrets Manager and credential encryption.
//!
//! Derives encryption keys from the JWT_SECRET (or SECRETS_ENCRYPTION_KEY env var)
//! using SHA-256 with distinct salts for separation of concerns.
//! Each secret gets a random 12-byte nonce, prepended to the ciphertext.
//! Format: base64(nonce || ciphertext || tag)

// Suppress transitive deprecation warnings from aes-gcm 0.10's use of
// generic-array 0.14; upgrading requires aes-gcm to publish a version
// against generic-array 1.x.
#![allow(deprecated)]

use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, AeadCore,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

/// The environment variable that overrides key derivation for BOTH the
/// credential path and the Secrets Manager vault.
pub const ENCRYPTION_KEY_ENV: &str = "SECRETS_ENCRYPTION_KEY";

const VAULT_SALT: &[u8] = b"dockpanel-secrets-v1:";
const CREDENTIAL_JWT_SALT: &[u8] = b"dockpanel-credential-v1:";
const CREDENTIAL_ENV_SALT: &[u8] = b"dockpanel-credential-encryption-v1:";
const VAULT_SOURCE_SALT: &[u8] = b"dockpanel-secrets-encryption:";
const BACKUP_ENCRYPTION_SALT: &[u8] = b"dockpanel-backup-encryption-v2:";

/// Read `SECRETS_ENCRYPTION_KEY`, treating **set-but-empty as unset**.
///
/// The empty-string guard is not cosmetic. `routes::secrets::get_encryption_key`
/// used a bare `unwrap_or_else`, so `SECRETS_ENCRYPTION_KEY=` (exported but
/// blank, which is what a half-filled `api.env` produces) keyed the whole vault
/// off the empty string while its sibling here treated the same value as unset.
/// The two subsystems disagreed about what the operator had configured.
/// The guard itself, split out from the environment read so a test can drive it.
///
/// It is deliberately a separate function: with the filter inlined into
/// `encryption_key_override`, the only way to exercise it is to mutate
/// process-global environment from a parallel test thread, so the arm that
/// claimed to cover it in fact called a *different* function and stayed green
/// when the guard was deleted.
fn normalize_env_key(raw: Option<String>) -> Option<String> {
    raw.filter(|k| !k.is_empty())
}

fn encryption_key_override() -> Option<String> {
    normalize_env_key(std::env::var(ENCRYPTION_KEY_ENV).ok())
}

/// Legacy key derivation (SHA-256 only) — kept for decrypting existing data.
fn derive_key_legacy(secret: &str, salt: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update(secret.as_bytes());
    let result = hasher.finalize();
    let mut key = [0u8; 32];
    key.copy_from_slice(&result);
    key
}

/// Derive an AES-256 key from a secret with a specific salt prefix using HKDF.
/// HKDF provides proper key derivation with extract-then-expand, unlike raw SHA-256.
fn derive_key_with_salt(secret: &str, salt: &[u8]) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(Some(salt), secret.as_bytes());
    let mut key = [0u8; 32];
    hk.expand(b"dockpanel-encryption", &mut key)
        .expect("HKDF expand failed: output length is valid");
    key
}

/// Derive an AES-256 key from a vault key SOURCE string.
///
/// The argument is the output of [`vault_key_primary`], not the raw JWT secret.
fn derive_key(key_source: &str) -> [u8; 32] {
    derive_key_with_salt(key_source, VAULT_SALT)
}

/// Every key a CREDENTIAL ciphertext could have been written under, in the
/// order to try them. `credential_key_candidates(..)[0]` is what
/// [`encrypt_credential`] writes with today.
///
/// ⚠ This is deliberately a UNION, never an if/else, and that is the whole
/// point of the function. Until v2.112.0 `derive_credential_key` was an
/// if/else yielding exactly ONE key per process: with `SECRETS_ENCRYPTION_KEY`
/// set it returned `HKDF(env)`, and `HKDF(jwt)` — the key every existing
/// install had actually encrypted with — appeared in NO arm of the decrypt
/// chain. Setting the variable on a running panel was therefore silent,
/// irreversible data loss, and `decrypt_credential_or_legacy` fail-opened the
/// wreckage into its callers as base64 text where a password belonged.
///
/// `env_key` is passed in rather than read here so the ordering can be tested
/// without mutating process-global environment from parallel test threads.
fn credential_key_candidates(jwt_secret: &str, env_key: Option<&str>) -> Vec<[u8; 32]> {
    let mut keys = Vec::with_capacity(4);
    // Current, when an operator has set the variable.
    if let Some(env) = env_key {
        keys.push(derive_key_with_salt(env, CREDENTIAL_ENV_SALT));
    }
    // UNCONDITIONAL. What production encrypts with when the variable is unset,
    // and what every install that predates the variable is still holding.
    keys.push(derive_key_with_salt(jwt_secret, CREDENTIAL_JWT_SALT));
    // Pre-HKDF legacy derivations, likewise unconditional / env-conditional.
    keys.push(derive_key_legacy(jwt_secret, CREDENTIAL_JWT_SALT));
    if let Some(env) = env_key {
        keys.push(derive_key_legacy(env, CREDENTIAL_ENV_SALT));
    }
    keys
}

/// Every key SOURCE string a VAULT ciphertext could have been written under.
///
/// Same defect, second subsystem: `routes::secrets::get_encryption_key` returns
/// the env var verbatim when set, else `hex(sha256(salt || jwt))`, and
/// [`decrypt`]'s two arms are both derived from whatever that one string is —
/// so the pre-env derivation was in no arm at all and setting the variable
/// stranded every vault secret, including the CMS admin passwords site
/// provisioning writes there and the values `deploy` injects into a site's
/// environment.
pub fn vault_key_sources(jwt_secret: &str, env_key: Option<&str>) -> Vec<String> {
    let mut sources = Vec::with_capacity(2);
    if let Some(env) = env_key {
        sources.push(env.to_string());
    }
    // UNCONDITIONAL — see above.
    sources.push(hex::encode(derive_key_legacy(jwt_secret, VAULT_SOURCE_SALT)));
    sources
}

/// The vault key source to ENCRYPT with. Always `vault_key_sources(..)[0]`.
pub fn vault_key_primary(jwt_secret: &str) -> String {
    let env = encryption_key_override();
    vault_key_sources(jwt_secret, env.as_deref())
        .into_iter()
        .next()
        .expect("vault_key_sources always yields the unconditional derivation")
}

/// The v2 backup-encryption PASSPHRASE: `hex(HKDF(jwt_secret, BACKUP_ENCRYPTION_SALT))`.
///
/// Callers pass this to `openssl enc -pass stdin`, which runs its own PBKDF2
/// (100k iterations, random salt per file) to turn a passphrase into an
/// AES-256 key — that stretching step was always fine. What v1
/// (`services::backup_policy_executor::derive_backup_encryption_key_v1`) got
/// wrong was the passphrase's VALUE: a literal substring of `JWT_SECRET`, the
/// same secret every session token is HMAC-signed with. Learning a v1 backup
/// passphrase handed an attacker 32 bytes of the panel's auth signing key —
/// coupling the blast radius of "one old backup file leaked" to "every
/// session on this install can be forged." HKDF is a one-way PRF: this
/// string reveals nothing about `jwt_secret`, and `jwt_secret` reveals
/// nothing about this string without redoing the derivation.
pub fn derive_backup_encryption_key_v2(jwt_secret: &str) -> String {
    hex::encode(derive_key_with_salt(jwt_secret, BACKUP_ENCRYPTION_SALT))
}

/// Does this stored value look like something we wrote, rather than a legacy
/// plaintext credential?
///
/// A value we wrote is `base64(nonce || ciphertext || tag)`: 12 bytes of nonce
/// plus a 16-byte GCM tag, so 28 bytes is the floor even for empty plaintext.
/// This is what lets a failed decrypt be classified instead of swallowed —
/// "this was never encrypted" and "this is encrypted with a key I cannot
/// derive" are the same `Err` without it.
fn looks_like_ciphertext(value: &str) -> bool {
    match B64.decode(value) {
        Ok(bytes) => bytes.len() >= 12 + 16,
        Err(_) => false,
    }
}

/// Encrypt a plaintext string. Returns base64-encoded (nonce + ciphertext).
pub fn encrypt(plaintext: &str, jwt_secret: &str) -> Result<String, String> {
    let mut key = derive_key(jwt_secret);
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| { key.zeroize(); format!("Cipher init failed: {e}") })?;
    key.zeroize();

    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|e| format!("Encryption failed: {e}"))?;

    // Combine nonce + ciphertext
    let mut combined = Vec::with_capacity(12 + ciphertext.len());
    combined.extend_from_slice(&nonce);
    combined.extend_from_slice(&ciphertext);

    Ok(B64.encode(&combined))
}

/// Decrypt a base64-encoded (nonce + ciphertext) string.
/// Tries HKDF-derived key first, falls back to legacy SHA-256 key for existing data.
pub fn decrypt(encrypted_b64: &str, jwt_secret: &str) -> Result<String, String> {
    let combined = B64.decode(encrypted_b64)
        .map_err(|e| format!("Base64 decode failed: {e}"))?;

    if combined.len() < 12 {
        return Err("Ciphertext too short".into());
    }

    let (nonce_bytes, ciphertext) = combined.split_at(12);
    let nonce = aes_gcm::Nonce::from_slice(nonce_bytes);

    // Try HKDF key first (new encryption)
    let mut key = derive_key(jwt_secret);
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| { key.zeroize(); format!("Cipher init failed: {e}") })?;
    key.zeroize();

    if let Ok(plaintext) = cipher.decrypt(nonce, ciphertext) {
        return String::from_utf8(plaintext)
            .map_err(|e| format!("UTF-8 decode failed: {e}"));
    }

    // Fall back to legacy SHA-256 key for data encrypted before HKDF migration
    let mut legacy_key = derive_key_legacy(jwt_secret, b"dockpanel-secrets-v1:");
    let legacy_cipher = Aes256Gcm::new_from_slice(&legacy_key)
        .map_err(|e| { legacy_key.zeroize(); format!("Cipher init failed: {e}") })?;
    legacy_key.zeroize();

    let plaintext = legacy_cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| "Decryption failed (wrong key or corrupted data)".to_string())?;

    String::from_utf8(plaintext)
        .map_err(|e| format!("UTF-8 decode failed: {e}"))
}

/// Encrypt a credential (DB password, SMTP password, DKIM key, etc.).
/// Uses a separate key derivation from the vault encryption.
pub fn encrypt_credential(plaintext: &str, jwt_secret: &str) -> Result<String, String> {
    let env = encryption_key_override();
    let mut key = credential_key_candidates(jwt_secret, env.as_deref())
        .into_iter()
        .next()
        .expect("credential_key_candidates always yields the unconditional derivation");
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| { key.zeroize(); format!("Cipher init failed: {e}") })?;
    key.zeroize();

    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|e| format!("Encryption failed: {e}"))?;

    let mut combined = Vec::with_capacity(12 + ciphertext.len());
    combined.extend_from_slice(&nonce);
    combined.extend_from_slice(&ciphertext);

    Ok(B64.encode(&combined))
}

/// Try every candidate key against one ciphertext.
///
/// Returns `Ok(Some(plaintext))` on success, `Ok(None)` when the value is
/// well-formed but no key opens it, and `Err` when the value was never our
/// ciphertext at all. Callers need those three cases kept apart: only the
/// middle one means "encrypted with a key this process cannot derive".
fn try_keys(encrypted_b64: &str, keys: Vec<[u8; 32]>) -> Result<Option<String>, String> {
    let combined = B64
        .decode(encrypted_b64)
        .map_err(|e| format!("Base64 decode failed: {e}"))?;

    if combined.len() < 12 {
        return Err("Ciphertext too short".into());
    }

    let (nonce_bytes, ciphertext) = combined.split_at(12);
    let nonce = aes_gcm::Nonce::from_slice(nonce_bytes);

    for mut key in keys {
        let cipher = match Aes256Gcm::new_from_slice(&key) {
            Ok(c) => c,
            Err(e) => {
                key.zeroize();
                return Err(format!("Cipher init failed: {e}"));
            }
        };
        key.zeroize();

        if let Ok(plaintext) = cipher.decrypt(nonce, ciphertext) {
            return String::from_utf8(plaintext)
                .map(Some)
                .map_err(|e| format!("UTF-8 decode failed: {e}"));
        }
    }

    Ok(None)
}

/// Decrypt a credential. Returns the plaintext string.
///
/// Tries every derivation in [`credential_key_candidates`], so a panel that
/// gains (or loses) `SECRETS_ENCRYPTION_KEY` can still read everything it wrote
/// before the change.
pub fn decrypt_credential(encrypted_b64: &str, jwt_secret: &str) -> Result<String, String> {
    let env = encryption_key_override();
    match try_keys(
        encrypted_b64,
        credential_key_candidates(jwt_secret, env.as_deref()),
    )? {
        Some(plaintext) => Ok(plaintext),
        None => Err("Decryption failed (wrong key or corrupted data)".to_string()),
    }
}

/// Decrypt a Secrets Manager vault value, trying every key source the value
/// could have been written under.
///
/// Takes the RAW jwt secret, not a derived key — deriving one source and
/// handing it to [`decrypt`] is precisely what made setting
/// `SECRETS_ENCRYPTION_KEY` unsurvivable for the vault.
pub fn decrypt_vault(encrypted_b64: &str, jwt_secret: &str) -> Result<String, String> {
    let env = encryption_key_override();
    let mut keys = Vec::new();
    for source in vault_key_sources(jwt_secret, env.as_deref()) {
        keys.push(derive_key_with_salt(&source, VAULT_SALT));
        keys.push(derive_key_legacy(&source, VAULT_SALT));
    }
    match try_keys(encrypted_b64, keys)? {
        Some(plaintext) => Ok(plaintext),
        None => Err("Decryption failed (wrong key or corrupted data)".to_string()),
    }
}

/// Re-encrypt a stored credential under the CURRENT primary key.
///
/// Returns `Ok(None)` when the value is already readable under the primary key
/// (nothing to do) and `Err` when no candidate opens it. This is the migration
/// half: without it, adding `SECRETS_ENCRYPTION_KEY` leaves every existing row
/// permanently dependent on a fallback arm, and there is no way to retire the
/// old derivation.
pub fn reencrypt_credential(
    encrypted_b64: &str,
    jwt_secret: &str,
) -> Result<Option<String>, String> {
    let env = encryption_key_override();
    let candidates = credential_key_candidates(jwt_secret, env.as_deref());

    // Already under the primary key? Then this row needs no rewrite.
    if try_keys(encrypted_b64, vec![candidates[0]])?.is_some() {
        return Ok(None);
    }

    match try_keys(encrypted_b64, candidates)? {
        Some(plaintext) => encrypt_credential(&plaintext, jwt_secret).map(Some),
        None => Err("no candidate key opens this value".to_string()),
    }
}

/// Vault twin of [`reencrypt_credential`].
pub fn reencrypt_vault(encrypted_b64: &str, jwt_secret: &str) -> Result<Option<String>, String> {
    let primary = vault_key_primary(jwt_secret);
    // Single-source read, deliberately: "can the PRIMARY key open this?" is
    // exactly the question, and `decrypt` is the primitive that asks it.
    if decrypt(encrypted_b64, &primary).is_ok() {
        return Ok(None);
    }

    match decrypt_vault(encrypted_b64, jwt_secret) {
        Ok(plaintext) => encrypt(&plaintext, &primary).map(Some),
        Err(e) => Err(e),
    }
}

/// Decrypt a credential, falling back to treating it as plaintext if decryption
/// fails. This handles legacy unencrypted values gracefully during migration.
///
/// ⚠ THE FALLBACK IS LOAD-BEARING AND IT IS ALSO THE FAILURE MODE. Twenty-four
/// call sites take this function's return value and use it AS a password. When
/// the value really is a legacy plaintext, handing it back is exactly right.
/// When the value is our own ciphertext that no key opens, handing it back
/// means the caller authenticates with base64 — nineteen of those sites then
/// fail against a remote service with no panel-side log at all, so the blame
/// lands on the remote service, and the five TOTP sites hard-fail into a 2FA
/// lockout.
///
/// Those two cases produce the identical `Err`, which is why this used to be a
/// bare `unwrap_or_else`. [`looks_like_ciphertext`] separates them, so a key
/// failure is now LOUD while a genuine legacy plaintext stays silent.
///
/// The return type is deliberately unchanged: making this fail closed is a
/// signature change across all 24 call sites, four of them in spawned tasks
/// with no error path today. Say what happened; do not silently change what
/// callers receive.
pub fn decrypt_credential_or_legacy(value: &str, jwt_secret: &str) -> String {
    if value.is_empty() {
        return value.to_string();
    }
    match decrypt_credential(value, jwt_secret) {
        Ok(plaintext) => plaintext,
        Err(e) => {
            if looks_like_ciphertext(value) {
                tracing::error!(
                    error = %e,
                    "Stored credential is well-formed ciphertext that NO key derivation opens. \
                     Returning the stored value verbatim — callers will receive base64 text \
                     where a credential belongs, and any service they authenticate against \
                     will reject it. This almost always means {} was added, changed or removed \
                     on a panel that already held encrypted data. Restore the previous value \
                     to recover, then re-key deliberately.",
                    ENCRYPTION_KEY_ENV
                );
            }
            value.to_string()
        }
    }
}

/// Decrypt a credential using JWT_SECRET from environment.
/// For use in contexts that don't have access to AppState (e.g., email service, notifications).
/// Falls back to plaintext for legacy unencrypted values.
pub fn decrypt_credential_from_env(value: &str) -> String {
    if value.is_empty() {
        return value.to_string();
    }
    let jwt_secret = match std::env::var("JWT_SECRET") {
        Ok(s) => s,
        Err(_) => return value.to_string(),
    };
    decrypt_credential_or_legacy(value, &jwt_secret)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let secret = "test-jwt-secret-12345";
        let plaintext = "my-secret-api-key-value";

        let encrypted = encrypt(plaintext, secret).unwrap();
        assert_ne!(encrypted, plaintext);
        assert!(encrypted.len() > 20);

        let decrypted = decrypt(&encrypted, secret).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn wrong_key_fails() {
        let encrypted = encrypt("hello", "key1").unwrap();
        let result = decrypt(&encrypted, "key2");
        assert!(result.is_err());
    }

    #[test]
    fn different_nonces() {
        let secret = "test-secret";
        let e1 = encrypt("same", secret).unwrap();
        let e2 = encrypt("same", secret).unwrap();
        // Same plaintext should produce different ciphertexts (random nonce)
        assert_ne!(e1, e2);
    }

    #[test]
    fn credential_roundtrip() {
        let secret = "test-jwt-secret";
        let plaintext = "my-db-password-123";

        let encrypted = encrypt_credential(plaintext, secret).unwrap();
        assert_ne!(encrypted, plaintext);

        let decrypted = decrypt_credential(&encrypted, secret).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn credential_key_differs_from_vault_key() {
        // Credential encryption and vault encryption must use different keys
        let secret = "same-jwt-secret";
        let plaintext = "test-value";

        let vault_enc = encrypt(plaintext, secret).unwrap();
        let cred_enc = encrypt_credential(plaintext, secret).unwrap();

        // Cross-decryption must fail
        assert!(decrypt_credential(&vault_enc, secret).is_err());
        assert!(decrypt(&cred_enc, secret).is_err());
    }

    #[test]
    fn backup_key_v2_is_deterministic_and_hex() {
        let secret = "production-jwt-secret-value";
        let k1 = derive_backup_encryption_key_v2(secret);
        let k2 = derive_backup_encryption_key_v2(secret);
        assert_eq!(k1, k2, "the same jwt_secret must derive the same v2 backup key every time");
        assert_eq!(k1.len(), 64, "hex(32 bytes) is 64 hex characters");
        assert!(k1.chars().all(|c| c.is_ascii_hexdigit()), "must be plain hex, safe on an openssl stdin pipe");
    }

    #[test]
    fn backup_key_v2_differs_from_vault_and_credential_keys() {
        // Same domain-separation property as credential_key_differs_from_vault_key,
        // extended to the backup key — three subsystems, three keys, one jwt_secret.
        let secret = "same-jwt-secret";
        let vault = derive_key(secret);
        let cred = derive_key_with_salt(secret, CREDENTIAL_JWT_SALT);
        let backup_hex = derive_backup_encryption_key_v2(secret);
        let backup_bytes = hex::decode(&backup_hex).unwrap();
        assert_ne!(vault.to_vec(), backup_bytes, "vault and backup keys must differ");
        assert_ne!(cred.to_vec(), backup_bytes, "credential and backup keys must differ");
    }

    #[test]
    fn backup_key_v2_does_not_leak_the_jwt_secret() {
        // THE regression this function exists to fix: v1 was
        // format!("backup-enc-{}", &jwt_secret[..32]) — a literal substring of
        // the auth signing secret. v2 must contain no substring of jwt_secret
        // at all (HKDF is one-way), so a leaked backup passphrase reveals
        // nothing about the key every session token is signed with.
        let secret = "SuperSecretJwtSigningKeyThatMustNeverAppearInABackupPassphrase123";
        let key = derive_backup_encryption_key_v2(secret);
        assert!(
            !key.contains(&secret[..16]),
            "the v2 backup key must not contain any substring of jwt_secret — got {key}"
        );
        assert!(
            !key.to_lowercase().contains(&secret.to_lowercase()[..16]),
            "case-insensitive check too, since hex output is lowercase"
        );
    }

    #[test]
    fn legacy_plaintext_fallback() {
        let secret = "test-jwt-secret";
        // A plaintext value that's not valid base64 ciphertext should be returned as-is
        let legacy = "my-old-plaintext-password";
        let result = decrypt_credential_or_legacy(legacy, secret);
        assert_eq!(result, legacy);
    }

    #[test]
    fn legacy_empty_string() {
        let secret = "test-jwt-secret";
        let result = decrypt_credential_or_legacy("", secret);
        assert_eq!(result, "");
    }

    // ── The defect these exist to hold shut ────────────────────────────────
    //
    // Setting SECRETS_ENCRYPTION_KEY used to swap the ONE credential key for a
    // different one, taking the key production had actually encrypted with out
    // of the decrypt chain entirely. Every arm below names the SPECIFIC key
    // whose absence was the bug — none of them counts candidates, because an
    // arm that counts is satisfied by the members that were never at risk.

    #[test]
    fn setting_the_env_key_keeps_the_unset_primary_in_the_credential_chain() {
        let jwt = "production-jwt-secret";
        let without = credential_key_candidates(jwt, None);
        let with = credential_key_candidates(jwt, Some("an-operator-supplied-key"));

        // THE invariant. `without[0]` is what a panel that has never seen the
        // variable encrypts with; it must still be reachable after the operator
        // sets one, or every existing row becomes unreadable.
        assert!(
            with.contains(&without[0]),
            "HKDF(jwt) — the key production encrypts with today — is missing from the \
             candidate set once SECRETS_ENCRYPTION_KEY is set. This is the data-loss bug."
        );
    }

    #[test]
    fn setting_the_env_key_keeps_the_unset_primary_in_the_vault_chain() {
        let jwt = "production-jwt-secret";
        let without = vault_key_sources(jwt, None);
        let with = vault_key_sources(jwt, Some("an-operator-supplied-key"));

        assert!(
            with.contains(&without[0]),
            "the derived vault key source is missing once SECRETS_ENCRYPTION_KEY is set — \
             every existing vault secret would be unreadable."
        );
    }

    #[test]
    fn the_env_key_takes_precedence_when_set() {
        let jwt = "production-jwt-secret";
        let with = credential_key_candidates(jwt, Some("operator-key"));
        // Order matters as much as membership: candidates[0] is what we WRITE
        // with, so an operator who sets the variable must get their own key.
        assert_eq!(
            with[0],
            derive_key_with_salt("operator-key", CREDENTIAL_ENV_SALT),
            "with the variable set, new writes must use the operator's key"
        );
        assert_eq!(
            vault_key_sources(jwt, Some("operator-key"))[0],
            "operator-key",
            "with the variable set, new vault writes must use the operator's key"
        );
    }

    #[test]
    fn an_empty_env_key_is_not_a_key() {
        // `SECRETS_ENCRYPTION_KEY=` (exported but blank) must read as unset.
        // The vault half lacked this guard while the credential half had it,
        // so the two subsystems disagreed about what the operator configured.
        //
        // ⚠ This arm drives `normalize_env_key` — THE guard — and not
        // `vault_key_sources`. The first cut asserted on `vault_key_sources(jwt,
        // Some(""))`, which never consults the guard at all, so deleting the
        // guard left it GREEN. The mutation run caught it; the arm now names and
        // examines the same function.
        assert_eq!(
            normalize_env_key(Some(String::new())),
            None,
            "an exported-but-blank key must read as unset"
        );
        assert_eq!(
            normalize_env_key(Some("a-real-key".into())),
            Some("a-real-key".to_string()),
            "a real key must survive normalisation"
        );
        assert_eq!(normalize_env_key(None), None, "unset stays unset");
    }

    #[test]
    fn ciphertext_is_distinguishable_from_a_legacy_plaintext() {
        let real = encrypt_credential("pw", "jwt").unwrap();
        assert!(looks_like_ciphertext(&real), "our own ciphertext must be recognised");

        // A plaintext password. Note this one is deliberately base64-decodable
        // to prove the length floor is doing work, not just the decode.
        assert!(
            !looks_like_ciphertext("hunter2"),
            "a short legacy plaintext must not be mistaken for ciphertext"
        );
        assert!(
            !looks_like_ciphertext("not base64 at all !!!"),
            "a non-base64 legacy plaintext must not be mistaken for ciphertext"
        );
    }

    #[test]
    fn reencrypt_is_a_no_op_when_already_under_the_primary_key() {
        let jwt = "jwt";
        let current = encrypt_credential("pw", jwt).unwrap();
        assert_eq!(
            reencrypt_credential(&current, jwt).unwrap(),
            None,
            "a row already under the primary key must not be rewritten"
        );
    }

    #[test]
    fn reencrypt_refuses_a_value_no_key_opens() {
        // Well-formed ciphertext from a foreign key: must ERROR, never return a
        // rewrite. Silently rewriting here would destroy the only copy.
        let foreign = encrypt_credential("pw", "a-completely-different-jwt").unwrap();
        assert!(
            reencrypt_credential(&foreign, "jwt").is_err(),
            "an unreadable value must fail loudly, never be rewritten"
        );
    }
}
