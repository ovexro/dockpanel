//! AES-256-GCM encryption for the Secrets Manager.
//!
//! Derives an encryption key from the JWT_SECRET using SHA-256 with a fixed salt.
//! Each secret gets a random 12-byte nonce, prepended to the ciphertext.
//! Format: base64(nonce || ciphertext || tag)

use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, AeadCore,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use sha2::{Digest, Sha256};

/// Derive an AES-256 key from the JWT secret.
fn derive_key(jwt_secret: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"dockpanel-secrets-v1:");
    hasher.update(jwt_secret.as_bytes());
    let result = hasher.finalize();
    let mut key = [0u8; 32];
    key.copy_from_slice(&result);
    key
}

/// Encrypt a plaintext string. Returns base64-encoded (nonce + ciphertext).
pub fn encrypt(plaintext: &str, jwt_secret: &str) -> Result<String, String> {
    let key = derive_key(jwt_secret);
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| format!("Cipher init failed: {e}"))?;

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
pub fn decrypt(encrypted_b64: &str, jwt_secret: &str) -> Result<String, String> {
    let key = derive_key(jwt_secret);
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| format!("Cipher init failed: {e}"))?;

    let combined = B64.decode(encrypted_b64)
        .map_err(|e| format!("Base64 decode failed: {e}"))?;

    if combined.len() < 12 {
        return Err("Ciphertext too short".into());
    }

    let (nonce_bytes, ciphertext) = combined.split_at(12);
    let nonce = aes_gcm::Nonce::from_slice(nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| "Decryption failed (wrong key or corrupted data)".to_string())?;

    String::from_utf8(plaintext)
        .map_err(|e| format!("UTF-8 decode failed: {e}"))
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
}
