use std::path::Path;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tokio::io::AsyncWriteExt;
use crate::safe_cmd::safe_command;

type HmacSha256 = Hmac<Sha256>;

/// Validate that a filepath is within the allowed backup directory and contains no traversal.
fn validate_backup_path(filepath: &str) -> Result<(), String> {
    if !filepath.starts_with("/var/backups/dockpanel/") {
        return Err("Path must be within /var/backups/dockpanel/".to_string());
    }
    if filepath.contains("..") {
        return Err("Path must not contain '..'".to_string());
    }
    Ok(())
}

/// Appended after the ciphertext of every file this build encrypts: a
/// 32-byte HMAC-SHA256 tag over the ciphertext, followed by this 8-byte
/// marker. AES-256-CBC alone has no integrity check — PKCS7 padding bounds
/// only the final block, so a bit-flip anywhere earlier decrypts silently
/// into corrupted plaintext instead of failing. Encrypt-then-MAC catches
/// that: `decrypt_file` verifies the tag before it ever hands the
/// ciphertext to openssl.
///
/// A backup made before this fix has no trailer at all — its last 8 bytes
/// are AES output, not this marker, for all practical purposes (odds of a
/// collision are 1 in 2^64). `decrypt_file` treats that absence as "this
/// predates the integrity tag" and falls back to the old, unauthenticated
/// decrypt path so existing backups keep working.
const MAC_MAGIC: &[u8; 8] = b"DPHMAC01";
const MAC_TAG_LEN: usize = 32;
const MAC_TRAILER_LEN: usize = MAC_TAG_LEN + 8;

/// Derive a MAC key from the backup passphrase, independent of the AES key
/// `openssl -pbkdf2` derives internally from that same passphrase (a value
/// this process never sees). Domain-separated by a fixed label so the tag
/// can't be turned into an oracle against the encryption key.
fn derive_mac_key(passphrase: &str) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(passphrase.as_bytes())
        .expect("HMAC accepts a key of any length");
    mac.update(b"dockpanel-backup-hmac-v1");
    mac.finalize().into_bytes().to_vec()
}

fn compute_tag(passphrase: &str, ciphertext: &[u8]) -> Vec<u8> {
    let key = derive_mac_key(passphrase);
    let mut mac = HmacSha256::new_from_slice(&key).expect("HMAC accepts a key of any length");
    mac.update(ciphertext);
    mac.finalize().into_bytes().to_vec()
}

/// Constant-time tag verification (via `hmac`'s own `verify_slice`).
fn verify_tag(passphrase: &str, ciphertext: &[u8], tag: &[u8]) -> bool {
    let key = derive_mac_key(passphrase);
    let mut mac = match HmacSha256::new_from_slice(&key) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(ciphertext);
    mac.verify_slice(tag).is_ok()
}

/// Encrypt a file using AES-256-CBC with PBKDF2 (openssl), then append an
/// HMAC-SHA256 integrity tag over the resulting ciphertext.
/// Returns the path to the encrypted file (original path + ".enc").
pub async fn encrypt_file(filepath: &str, key: &str) -> Result<String, String> {
    validate_backup_path(filepath)?;

    let path = Path::new(filepath);
    if !path.exists() {
        return Err(format!("File not found: {filepath}"));
    }

    let enc_path = format!("{filepath}.enc");

    // Pass the key via stdin instead of command line to avoid exposure in process listing
    let mut child = safe_command("openssl")
        .args([
            "enc",
            "-aes-256-cbc",
            "-salt",
            "-pbkdf2",
            "-iter",
            "100000",
            "-in",
            filepath,
            "-out",
            &enc_path,
            "-pass",
            "stdin",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to run openssl: {e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(key.as_bytes()).await
            .map_err(|e| format!("Failed to write key to openssl stdin: {e}"))?;
        drop(stdin);
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(600),
        child.wait_with_output(),
    )
    .await
    .map_err(|_| "Encryption timed out (10 minutes)".to_string())?
    .map_err(|e| format!("Failed to run openssl: {e}"))?;

    if !output.status.success() {
        std::fs::remove_file(&enc_path).ok();
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Encryption failed: {stderr}"));
    }

    // Verify encrypted file exists and is non-empty
    let ciphertext = std::fs::read(&enc_path)
        .map_err(|e| format!("Failed to read encrypted file: {e}"))?;
    if ciphertext.is_empty() {
        std::fs::remove_file(&enc_path).ok();
        return Err("Encryption produced empty output".to_string());
    }

    // Append the integrity tag. Computed over the ciphertext as written by
    // openssl, so verification on the way back out needs no knowledge of
    // anything openssl did internally — only the shared passphrase.
    let tag = compute_tag(key, &ciphertext);
    let mut trailer = Vec::with_capacity(MAC_TRAILER_LEN);
    trailer.extend_from_slice(&tag);
    trailer.extend_from_slice(MAC_MAGIC);
    let mut f = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&enc_path)
        .await
        .map_err(|e| format!("Failed to open encrypted file to append integrity tag: {e}"))?;
    f.write_all(&trailer)
        .await
        .map_err(|e| format!("Failed to write integrity tag: {e}"))?;
    drop(f);

    // Remove the unencrypted original
    std::fs::remove_file(filepath).ok();

    let final_len = ciphertext.len() as u64 + trailer.len() as u64;
    tracing::info!("File encrypted: {enc_path} ({final_len} bytes)");
    Ok(enc_path)
}

/// Decrypt an encrypted file. Returns the path to the decrypted file.
///
/// Verifies the integrity tag `encrypt_file` appends BEFORE decrypting —
/// verify-then-decrypt, so a tampered or corrupted ciphertext is rejected
/// without ever running it through openssl. A file with no tag (encrypted
/// before this fix) is decrypted as before, unauthenticated.
pub async fn decrypt_file(enc_filepath: &str, key: &str) -> Result<String, String> {
    validate_backup_path(enc_filepath)?;

    let path = Path::new(enc_filepath);
    if !path.exists() {
        return Err(format!("File not found: {enc_filepath}"));
    }

    let raw = std::fs::read(enc_filepath)
        .map_err(|e| format!("Failed to read encrypted file: {e}"))?;

    let has_tag = raw.len() >= MAC_TRAILER_LEN
        && &raw[raw.len() - MAC_MAGIC.len()..] == MAC_MAGIC.as_slice();

    // When a tag is present, decrypt a scratch copy of just the ciphertext
    // (openssl's CBC framing does not tolerate the 40 extra trailer bytes).
    // Legacy files with no tag are decrypted from their own path, unchanged
    // from before this fix.
    let (openssl_input, _scratch_guard) = if has_tag {
        let split = raw.len() - MAC_TRAILER_LEN;
        let ciphertext = &raw[..split];
        let tag = &raw[split..split + MAC_TAG_LEN];
        if !verify_tag(key, ciphertext, tag) {
            return Err(
                "Backup integrity check failed — the file may be corrupted or tampered with"
                    .to_string(),
            );
        }

        let scratch_path = format!("{enc_filepath}.{:016x}.stripped", rand::random::<u64>());
        tokio::fs::write(&scratch_path, ciphertext)
            .await
            .map_err(|e| format!("Failed to stage ciphertext for decryption: {e}"))?;
        (scratch_path.clone(), Some(ScratchFile(scratch_path)))
    } else {
        tracing::warn!(
            "Decrypting {enc_filepath} without an integrity tag (backup predates the integrity \
             fix) — cannot verify it wasn't tampered with"
        );
        (enc_filepath.to_string(), None)
    };

    // Strip .enc suffix to get original filename
    let dec_path = if enc_filepath.ends_with(".enc") {
        enc_filepath[..enc_filepath.len() - 4].to_string()
    } else {
        format!("{enc_filepath}.dec")
    };

    // Pass the key via stdin instead of command line to avoid exposure in process listing
    let mut child = safe_command("openssl")
        .args([
            "enc",
            "-d",
            "-aes-256-cbc",
            "-pbkdf2",
            "-iter",
            "100000",
            "-in",
            &openssl_input,
            "-out",
            &dec_path,
            "-pass",
            "stdin",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to run openssl: {e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(key.as_bytes()).await
            .map_err(|e| format!("Failed to write key to openssl stdin: {e}"))?;
        drop(stdin);
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(600),
        child.wait_with_output(),
    )
    .await
    .map_err(|_| "Decryption timed out (10 minutes)".to_string())?
    .map_err(|e| format!("Failed to run openssl: {e}"))?;

    if !output.status.success() {
        std::fs::remove_file(&dec_path).ok();
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Decryption failed: {stderr}"));
    }

    let meta = std::fs::metadata(&dec_path)
        .map_err(|e| format!("Failed to read decrypted file: {e}"))?;
    if meta.len() == 0 {
        std::fs::remove_file(&dec_path).ok();
        return Err("Decryption produced empty output".to_string());
    }

    tracing::info!("File decrypted: {dec_path} ({} bytes)", meta.len());
    Ok(dec_path)
}

/// Best-effort cleanup of the tag-stripped scratch ciphertext, regardless of
/// which return path `decrypt_file` takes.
struct ScratchFile(String);

impl Drop for ScratchFile {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_round_trips() {
        let tag = compute_tag("correct horse battery staple", b"some ciphertext bytes");
        assert!(verify_tag("correct horse battery staple", b"some ciphertext bytes", &tag));
    }

    #[test]
    fn tag_rejects_wrong_key() {
        let tag = compute_tag("key-a", b"some ciphertext bytes");
        assert!(!verify_tag("key-b", b"some ciphertext bytes", &tag));
    }

    #[test]
    fn tag_rejects_tampered_ciphertext() {
        let tag = compute_tag("key", b"some ciphertext bytes");
        assert!(!verify_tag("key", b"some tampered!! bytes", &tag));
    }

    #[test]
    fn tag_rejects_truncated_ciphertext() {
        let tag = compute_tag("key", b"some ciphertext bytes");
        assert!(!verify_tag("key", b"some ciphertext", &tag));
    }

    #[test]
    fn mac_key_is_independent_per_passphrase() {
        assert_ne!(derive_mac_key("key-a"), derive_mac_key("key-b"));
    }
}
