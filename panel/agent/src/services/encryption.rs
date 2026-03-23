use std::path::Path;
use tokio::process::Command;

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

/// Encrypt a file using AES-256-CBC with PBKDF2 (openssl).
/// Returns the path to the encrypted file (original path + ".enc").
pub async fn encrypt_file(filepath: &str, key: &str) -> Result<String, String> {
    validate_backup_path(filepath)?;

    let path = Path::new(filepath);
    if !path.exists() {
        return Err(format!("File not found: {filepath}"));
    }

    let enc_path = format!("{filepath}.enc");

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(600),
        Command::new("openssl")
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
                &format!("pass:{key}"),
            ])
            .output(),
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
    let meta = std::fs::metadata(&enc_path)
        .map_err(|e| format!("Failed to read encrypted file: {e}"))?;
    if meta.len() == 0 {
        std::fs::remove_file(&enc_path).ok();
        return Err("Encryption produced empty output".to_string());
    }

    // Remove the unencrypted original
    std::fs::remove_file(filepath).ok();

    tracing::info!("File encrypted: {enc_path} ({} bytes)", meta.len());
    Ok(enc_path)
}

/// Decrypt an encrypted file. Returns the path to the decrypted file.
pub async fn decrypt_file(enc_filepath: &str, key: &str) -> Result<String, String> {
    validate_backup_path(enc_filepath)?;

    let path = Path::new(enc_filepath);
    if !path.exists() {
        return Err(format!("File not found: {enc_filepath}"));
    }

    // Strip .enc suffix to get original filename
    let dec_path = if enc_filepath.ends_with(".enc") {
        enc_filepath[..enc_filepath.len() - 4].to_string()
    } else {
        format!("{enc_filepath}.dec")
    };

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(600),
        Command::new("openssl")
            .args([
                "enc",
                "-d",
                "-aes-256-cbc",
                "-pbkdf2",
                "-iter",
                "100000",
                "-in",
                enc_filepath,
                "-out",
                &dec_path,
                "-pass",
                &format!("pass:{key}"),
            ])
            .output(),
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
