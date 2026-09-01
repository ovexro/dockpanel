//! SSRF guard for `repo_url` at the point this process actually dials it.
//!
//! `panel/backend`'s `validate_repo_url_not_internal` (`helpers.rs`) checks the
//! same class of URL, but it runs on the PANEL host at write time — a git
//! deploy's `repo_url` is stored, then dialed later (immediately, on a
//! scheduled pull, or on a redeploy) by THIS process, on a DIFFERENT host,
//! via a plain `git clone`/`git fetch` subprocess. A DNS answer that differs
//! between "how the panel resolved this hostname" and "how this fleet member
//! resolves it right now" sails straight through the panel's check untouched
//! — the two checks don't even share a network vantage point, let alone a
//! moment in time. `helpers.rs`'s `resolve_validated`/`pinned_client` close
//! this class for `reqwest` call sites by pinning the resolved IP into the
//! HTTP connection itself; a subprocess `git clone` has no such hook, so the
//! best available guard is the same resolve-then-check `validate_host_not_internal`
//! already uses for `check_tcp`/`check_ping` elsewhere in this codebase —
//! same accepted residual (a same-host, same-instant TOCTOU window is far
//! narrower than the unbounded cross-host gap this closes).
//!
//! Ported (not shared — the agent and backend are separate crates in this
//! workspace) from `panel/backend/src/helpers.rs`; keep the two in sync if
//! the internal-address rule set changes.

/// True if an IPv4 address is loopback / private / link-local / unspecified /
/// broadcast / the metadata block (169.x) / CGNAT (100.64.0.0/10).
fn v4_is_internal(v4: std::net::Ipv4Addr) -> bool {
    let o = v4.octets();
    v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_unspecified()
        || v4.is_broadcast()
        || o[0] == 169 // link-local/metadata block (broad; matches the original guard)
        || (o[0] == 100 && (o[1] & 0xC0) == 0x40) // CGNAT 100.64.0.0/10
}

/// True if an IPv6 address is loopback / unspecified / ULA (fc00::/7) /
/// link-local (fe80::/10), OR embeds an internal IPv4 via a transition
/// mechanism (IPv4-mapped, NAT64 well-known, IPv4-compatible, 6to4).
fn v6_is_internal(v6: std::net::Ipv6Addr) -> bool {
    if v6.is_loopback() || v6.is_unspecified() {
        return true;
    }
    let seg = v6.segments();
    let low_v4 = std::net::Ipv4Addr::new(
        (seg[6] >> 8) as u8,
        (seg[6] & 0xff) as u8,
        (seg[7] >> 8) as u8,
        (seg[7] & 0xff) as u8,
    );
    let is_mapped = seg[0] == 0 && seg[1] == 0 && seg[2] == 0 && seg[3] == 0 && seg[4] == 0 && seg[5] == 0xffff;
    let is_nat64 = seg[0] == 0x0064 && seg[1] == 0xff9b && seg[2] == 0 && seg[3] == 0 && seg[4] == 0 && seg[5] == 0;
    let is_v4compat = seg[0] == 0 && seg[1] == 0 && seg[2] == 0 && seg[3] == 0 && seg[4] == 0 && seg[5] == 0;
    if (is_mapped || is_nat64 || is_v4compat) && v4_is_internal(low_v4) {
        return true;
    }
    if seg[0] == 0x2002 {
        let v4 = std::net::Ipv4Addr::new(
            (seg[1] >> 8) as u8,
            (seg[1] & 0xff) as u8,
            (seg[2] >> 8) as u8,
            (seg[2] & 0xff) as u8,
        );
        if v4_is_internal(v4) {
            return true;
        }
    }
    (seg[0] & 0xfe00) == 0xfc00 // ULA fc00::/7
        || (seg[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
}

fn ip_is_internal(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => v4_is_internal(v4),
        std::net::IpAddr::V6(v6) => v6_is_internal(v6),
    }
}

/// Resolve `host` and reject if it (or any address it resolves to) is internal.
async fn validate_host_not_internal(host: &str, port: u16) -> Result<(), String> {
    let host = host.trim();
    if host.is_empty() {
        return Err("Host is required".to_string());
    }
    let bare = host.trim_matches(|c| c == '[' || c == ']');
    if let Ok(ip) = bare.parse::<std::net::IpAddr>() {
        if ip_is_internal(ip) {
            return Err("Host points to a private/internal address".to_string());
        }
        return Ok(());
    }
    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".localhost") {
        return Err("Host points to a local address".to_string());
    }
    match tokio::net::lookup_host((bare, port)).await {
        Ok(addrs) => {
            let mut resolved_any = false;
            for addr in addrs {
                resolved_any = true;
                if ip_is_internal(addr.ip()) {
                    return Err("Host resolves to a private/internal address".to_string());
                }
            }
            if !resolved_any {
                return Err("Host could not be resolved".to_string());
            }
        }
        Err(_) => {
            return Err("Host could not be resolved".to_string());
        }
    }
    Ok(())
}

/// Extract `(host, port)` from a `repo_url` in any of the four shapes
/// `is_valid_repo_url` accepts: `https://`, `http://`, `ssh://`, and the
/// scp-like `git@host:path` shorthand (no scheme, always user `git`, no
/// port — matches `panel/backend/src/helpers.rs::repo_url_authority`).
fn repo_url_authority(url: &str) -> Result<(String, u16), String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("Repository URL is required".to_string());
    }

    if let Some(rest) = url.strip_prefix("git@") {
        let host = rest.split(':').next().unwrap_or("");
        let host = host.split('/').next().unwrap_or(host);
        return if host.is_empty() {
            Err("Repository URL has no hostname".to_string())
        } else {
            Ok((host.to_string(), 22))
        };
    }

    let parsed = url::Url::parse(url).map_err(|_| "Repository URL is not valid".to_string())?;

    if !matches!(parsed.scheme(), "http" | "https" | "ssh") {
        return Err("Repository URL must use https, http, ssh, or git@host:path".to_string());
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| "Repository URL has no hostname".to_string())?
        .to_string();
    let port = parsed.port_or_known_default().unwrap_or(22);
    Ok((host, port))
}

/// SSRF guard for a git `repo_url`, at the point THIS process is about to
/// dial it via a `git clone`/`git fetch` subprocess. Call this immediately
/// before spawning that subprocess — not earlier, not once at startup — so
/// the resolve is as close to the actual connection as this process can get.
pub async fn validate_repo_url_not_internal(url: &str) -> Result<(), String> {
    let (host, port) = repo_url_authority(url)?;
    validate_host_not_internal(&host, port).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v4_internal_ranges() {
        assert!(v4_is_internal("127.0.0.1".parse().unwrap()));
        assert!(v4_is_internal("10.1.2.3".parse().unwrap()));
        assert!(v4_is_internal("192.168.1.1".parse().unwrap()));
        assert!(v4_is_internal("169.254.169.254".parse().unwrap())); // cloud metadata
        assert!(v4_is_internal("100.64.0.1".parse().unwrap())); // CGNAT
        assert!(!v4_is_internal("8.8.8.8".parse().unwrap()));
        assert!(!v4_is_internal("140.82.112.3".parse().unwrap())); // github.com-ish
    }

    #[test]
    fn v6_internal_ranges() {
        assert!(v6_is_internal("::1".parse().unwrap()));
        assert!(v6_is_internal("fe80::1".parse().unwrap()));
        assert!(v6_is_internal("fc00::1".parse().unwrap()));
        assert!(v6_is_internal("::ffff:127.0.0.1".parse().unwrap())); // v4-mapped loopback
        assert!(!v6_is_internal("2001:4860:4860::8888".parse().unwrap()));
    }

    #[test]
    fn repo_url_authority_parses_every_shape() {
        assert_eq!(
            repo_url_authority("https://github.com/owner/repo.git").unwrap(),
            ("github.com".to_string(), 443)
        );
        assert_eq!(
            repo_url_authority("ssh://git@10.0.0.1:2222/owner/repo.git").unwrap(),
            ("10.0.0.1".to_string(), 2222)
        );
        assert_eq!(
            repo_url_authority("git@github.com:owner/repo.git").unwrap(),
            ("github.com".to_string(), 22)
        );
        assert!(repo_url_authority("").is_err());
    }

    #[tokio::test]
    async fn literal_internal_ip_repo_urls_rejected_across_every_shape() {
        for u in [
            "http://127.0.0.1/x.git",
            "https://169.254.169.254/x.git", // cloud metadata
            "ssh://git@10.0.0.1/x.git",
            "git@192.168.1.1:owner/repo.git",
        ] {
            assert!(
                validate_repo_url_not_internal(u).await.is_err(),
                "expected {u} to be rejected"
            );
        }
    }

    #[tokio::test]
    async fn malformed_and_unresolvable_are_rejected_not_silently_admitted() {
        assert!(validate_repo_url_not_internal("").await.is_err());
        assert!(validate_repo_url_not_internal("ftp://example.com/x.git").await.is_err());
        assert!(
            validate_repo_url_not_internal("https://this-host-does-not-exist.invalid/x.git")
                .await
                .is_err()
        );
    }
}
