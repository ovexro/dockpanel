/// Shared helper functions used across multiple route modules.
use sha2::{Sha256, Digest};

/// Hash an agent token using SHA-256. Agent tokens are high-entropy (UUIDs)
/// so SHA-256 is sufficient — no need for slow hashing (argon2/bcrypt).
pub fn hash_agent_token(token: &str) -> String {
    let hash = Sha256::digest(token.as_bytes());
    hex::encode(hash)
}

/// Build Cloudflare API headers from credentials.
///
/// If `email` is provided, uses Global API Key auth (X-Auth-Email + X-Auth-Key).
/// Otherwise, uses Bearer token auth.
pub fn cf_headers(token: &str, email: Option<&str>) -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    if let Some(em) = email {
        if let (Ok(e_val), Ok(k_val)) = (em.parse(), token.parse()) {
            headers.insert("X-Auth-Email", e_val);
            headers.insert("X-Auth-Key", k_val);
        }
    } else if let Ok(bearer) = format!("Bearer {token}").parse() {
        headers.insert("Authorization", bearer);
    }
    headers
}

/// Detect the server's public IPv4 address.
///
/// Tries the ipify.org API first (5s timeout), falls back to local UDP socket detection.
pub async fn detect_public_ip() -> String {
    match reqwest::Client::new()
        .get("https://api.ipify.org")
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        Ok(resp) => {
            let ip = resp.text().await.unwrap_or_default().trim().to_string();
            if ip.is_empty() { String::new() } else { ip }
        }
        Err(_) => {
            use std::net::UdpSocket;
            UdpSocket::bind("0.0.0.0:0")
                .and_then(|s| { s.connect("8.8.8.8:53")?; s.local_addr() })
                .map(|a| a.ip().to_string())
                .unwrap_or_default()
        }
    }
}
