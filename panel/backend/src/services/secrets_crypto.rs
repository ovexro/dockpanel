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

/// Derive an AES-256 key for the Secrets Manager vault.
fn derive_key(jwt_secret: &str) -> [u8; 32] {
    derive_key_with_salt(jwt_secret, b"dockpanel-secrets-v1:")
}

/// Derive an AES-256 key for credential encryption (DB passwords, SMTP, DKIM, etc.).
/// Uses SECRETS_ENCRYPTION_KEY env var if set, otherwise falls back to jwt_secret
/// with a distinct salt to separate concerns from vault encryption.
fn derive_credential_key(jwt_secret: &str) -> [u8; 32] {
    if let Ok(key) = std::env::var("SECRETS_ENCRYPTION_KEY") {
        if !key.is_empty() {
            return derive_key_with_salt(&key, b"dockpanel-credential-encryption-v1:");
        }
    }
    derive_key_with_salt(jwt_secret, b"dockpanel-credential-v1:")
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
    let mut key = derive_credential_key(jwt_secret);
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

/// Decrypt a credential. Returns the plaintext string.
/// Tries HKDF key first, falls back to legacy SHA-256 key for existing data.
pub fn decrypt_credential(encrypted_b64: &str, jwt_secret: &str) -> Result<String, String> {
    let combined = B64.decode(encrypted_b64)
        .map_err(|e| format!("Base64 decode failed: {e}"))?;

    if combined.len() < 12 {
        return Err("Ciphertext too short".into());
    }

    let (nonce_bytes, ciphertext) = combined.split_at(12);
    let nonce = aes_gcm::Nonce::from_slice(nonce_bytes);

    // Try HKDF key first (new encryption)
    let mut key = derive_credential_key(jwt_secret);
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| { key.zeroize(); format!("Cipher init failed: {e}") })?;
    key.zeroize();

    if let Ok(plaintext) = cipher.decrypt(nonce, ciphertext) {
        return String::from_utf8(plaintext)
            .map_err(|e| format!("UTF-8 decode failed: {e}"));
    }

    // Fall back to legacy SHA-256 key (jwt_secret with credential salt)
    let mut legacy_key = derive_key_legacy(jwt_secret, b"dockpanel-credential-v1:");
    let legacy_cipher = Aes256Gcm::new_from_slice(&legacy_key)
        .map_err(|e| { legacy_key.zeroize(); format!("Cipher init failed: {e}") })?;
    legacy_key.zeroize();

    if let Ok(plaintext) = legacy_cipher.decrypt(nonce, ciphertext) {
        return String::from_utf8(plaintext)
            .map_err(|e| format!("UTF-8 decode failed: {e}"));
    }

    // Fall back to legacy SHA-256 key with SECRETS_ENCRYPTION_KEY env var
    if let Ok(env_key) = std::env::var("SECRETS_ENCRYPTION_KEY") {
        if !env_key.is_empty() {
            let mut env_legacy_key = derive_key_legacy(&env_key, b"dockpanel-credential-encryption-v1:");
            let env_legacy_cipher = Aes256Gcm::new_from_slice(&env_legacy_key)
                .map_err(|e| { env_legacy_key.zeroize(); format!("Cipher init failed: {e}") })?;
            env_legacy_key.zeroize();

            if let Ok(plaintext) = env_legacy_cipher.decrypt(nonce, ciphertext) {
                return String::from_utf8(plaintext)
                    .map_err(|e| format!("UTF-8 decode failed: {e}"));
            }
        }
    }

    Err("Decryption failed (wrong key or corrupted data)".to_string())
}

/// Decrypt a credential, falling back to treating it as plaintext if decryption fails.
/// This handles legacy unencrypted values gracefully during migration.
pub fn decrypt_credential_or_legacy(value: &str, jwt_secret: &str) -> String {
    if value.is_empty() {
        return value.to_string();
    }
    decrypt_credential(value, jwt_secret).unwrap_or_else(|_| value.to_string())
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
}
