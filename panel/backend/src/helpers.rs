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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_agent_token_deterministic() {
        let hash1 = hash_agent_token("test-token-123");
        let hash2 = hash_agent_token("test-token-123");
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_hash_agent_token_different_inputs() {
        let hash1 = hash_agent_token("token-a");
        let hash2 = hash_agent_token("token-b");
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_hash_agent_token_length() {
        let hash = hash_agent_token("any-token");
        assert_eq!(hash.len(), 64); // SHA-256 = 32 bytes = 64 hex chars
    }

    #[test]
    fn test_hash_agent_token_hex_format() {
        let hash = hash_agent_token("test");
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_hash_agent_token_known_value() {
        // SHA-256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        let hash = hash_agent_token("hello");
        assert_eq!(hash, "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    }

    #[test]
    fn test_hash_empty_token() {
        let hash = hash_agent_token("");
        assert_eq!(hash.len(), 64);
    }
}

/// SSRF protection: validate that a URL does not resolve to an internal/private address.
///
/// Checks loopback, private (RFC 1918), link-local, and unspecified addresses.
/// Resolves DNS to catch bypass via hostnames that map to internal IPs.
pub async fn validate_url_not_internal(url: &str) -> Result<(), String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("URL is required".to_string());
    }
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err("URL must use http or https".to_string());
    }

    // Extract host from URL (strip scheme, take up to next / or :)
    let after_scheme = if url.starts_with("https://") { &url[8..] } else { &url[7..] };
    let host = after_scheme
        .split('/')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");

    if host.is_empty() {
        return Err("URL has no hostname".to_string());
    }

    // Block obvious internal hostnames
    if host == "localhost" || host == "127.0.0.1" || host == "::1" || host == "0.0.0.0" {
        return Err("URL points to a local address".to_string());
    }

    // Resolve hostname to IP addresses and check each one
    let lookup_host = format!("{}:80", host.trim_matches(|c| c == '[' || c == ']'));
    match tokio::net::lookup_host(&lookup_host).await {
        Ok(addrs) => {
            for addr in addrs {
                let ip = addr.ip();
                if ip.is_loopback() || ip.is_unspecified() {
                    return Err("URL resolves to loopback address".to_string());
                }
                match ip {
                    std::net::IpAddr::V4(v4) => {
                        if v4.is_private() || v4.is_link_local() || v4.octets()[0] == 169 {
                            return Err(
                                "URL resolves to private/link-local address".to_string(),
                            );
                        }
                    }
                    std::net::IpAddr::V6(v6) => {
                        if v6.is_loopback() {
                            return Err(
                                "URL resolves to loopback address".to_string(),
                            );
                        }
                    }
                }
            }
        }
        Err(_) => {
            return Err("URL hostname could not be resolved".to_string());
        }
    }

    Ok(())
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
