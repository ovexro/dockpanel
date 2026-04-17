//! Agent TLS: load (or generate) the self-signed cert used for inbound TLS,
//! and expose the SHA-256 fingerprint of the DER-encoded cert so the central
//! panel can pin it (Trust On First Use).
//!
//! Cert location: `/etc/dockpanel/ssl/agent.{crt,key}`. `install-agent.sh`
//! generates these at install time via openssl; this module generates them
//! at first boot if missing (dev / direct-binary installs).

use axum_server::tls_rustls::RustlsConfig;
use sha2::{Digest, Sha256};
use std::path::Path;

const CERT_PATH: &str = "/etc/dockpanel/ssl/agent.crt";
const KEY_PATH: &str = "/etc/dockpanel/ssl/agent.key";

/// Load the on-disk cert + key, generating a fresh pair if either is missing.
/// Returns the Rustls config ready for axum-server plus the hex SHA-256
/// fingerprint of the cert's DER bytes.
pub async fn load_or_generate() -> Result<(RustlsConfig, String), String> {
    if !Path::new(CERT_PATH).exists() || !Path::new(KEY_PATH).exists() {
        generate_self_signed()?;
        tracing::info!("Generated agent TLS cert at {CERT_PATH}");
    }

    let cert_pem = std::fs::read(CERT_PATH)
        .map_err(|e| format!("read {CERT_PATH}: {e}"))?;
    let key_pem = std::fs::read(KEY_PATH)
        .map_err(|e| format!("read {KEY_PATH}: {e}"))?;

    let config = RustlsConfig::from_pem(cert_pem.clone(), key_pem)
        .await
        .map_err(|e| format!("build RustlsConfig: {e}"))?;

    let fingerprint = fingerprint_from_pem(&cert_pem)?;
    Ok((config, fingerprint))
}

/// SHA-256 of the first certificate's DER bytes, lowercase hex.
pub fn fingerprint_from_pem(pem: &[u8]) -> Result<String, String> {
    let mut reader = std::io::BufReader::new(pem);
    let first = rustls_pemfile::certs(&mut reader)
        .next()
        .ok_or_else(|| "no certificate in PEM".to_string())
        .and_then(|r| r.map_err(|e| format!("parse PEM: {e}")))?;
    let digest = Sha256::digest(first.as_ref());
    Ok(hex::encode(digest))
}

fn generate_self_signed() -> Result<(), String> {
    let cert = rcgen::generate_simple_self_signed(vec!["dockpanel-agent".to_string()])
        .map_err(|e| format!("rcgen: {e}"))?;

    std::fs::create_dir_all("/etc/dockpanel/ssl")
        .map_err(|e| format!("mkdir /etc/dockpanel/ssl: {e}"))?;
    std::fs::write(CERT_PATH, cert.cert.pem())
        .map_err(|e| format!("write {CERT_PATH}: {e}"))?;
    std::fs::write(KEY_PATH, cert.signing_key.serialize_pem())
        .map_err(|e| format!("write {KEY_PATH}: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(KEY_PATH, std::fs::Permissions::from_mode(0o600)).ok();
        std::fs::set_permissions(CERT_PATH, std::fs::Permissions::from_mode(0o644)).ok();
    }

    Ok(())
}
