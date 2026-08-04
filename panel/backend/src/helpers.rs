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
    // The stored Cloudflare token / global API key is encrypted at rest (see
    // dns::create_zone). Decrypt here — the single choke point every CF caller funnels
    // through — so no read site can forget to. The legacy fallback returns any
    // pre-encryption plaintext value (and a freshly-entered token during validation)
    // unchanged, so the round trip is transparent.
    let token = crate::services::secrets_crypto::decrypt_credential_from_env(token);
    let token = token.as_str();
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

/// True if an IPv4 address is loopback / private / link-local / CGNAT / unspecified
/// / broadcast — i.e. one an SSRF guard must reject.
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
/// link-local (fe80::/10), OR is an IPv4-mapped address whose embedded v4 is
/// internal (`::ffff:127.0.0.1`, `::ffff:169.254.169.254`, …). Rust's
/// `Ipv6Addr::is_loopback()` is false for the mapped forms, so those must be
/// normalized explicitly — that was the SSRF-validator gap.
fn v6_is_internal(v6: std::net::Ipv6Addr) -> bool {
    if v6.is_loopback() || v6.is_unspecified() {
        return true;
    }
    if let Some(v4) = v6.to_ipv4_mapped() {
        if v4_is_internal(v4) {
            return true;
        }
    }
    let seg = v6.segments();
    (seg[0] & 0xfe00) == 0xfc00 // ULA fc00::/7
        || (seg[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
}

/// True if an IP is one an SSRF guard must reject (loopback / private / link-local /
/// ULA / CGNAT / IPv4-mapped-internal / unspecified).
fn ip_is_internal(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => v4_is_internal(v4),
        std::net::IpAddr::V6(v6) => v6_is_internal(v6),
    }
}

/// True if `entry` — a plain IP or a CIDR block like `10.0.0.0/8` — covers `client`.
/// Families must match; a v4 entry never covers a v6 client. Anything unparseable
/// covers nothing, so a typo denies rather than admits (see `valid_panel_ip_entry`,
/// which rejects such entries at write time so an operator cannot lock themselves out).
fn ip_entry_covers(entry: &str, client: std::net::IpAddr) -> bool {
    let entry = entry.trim();
    let Some((net, bits)) = entry.split_once('/') else {
        return entry
            .parse::<std::net::IpAddr>()
            .map(|ip| ip == client)
            .unwrap_or(false);
    };
    let (Ok(net), Ok(bits)) = (net.trim().parse::<std::net::IpAddr>(), bits.trim().parse::<u32>())
    else {
        return false;
    };
    match (net, client) {
        (std::net::IpAddr::V4(n), std::net::IpAddr::V4(c)) => {
            if bits > 32 {
                return false;
            }
            // A /0 must not shift by 32 (UB-adjacent, and `checked_shl` returns None).
            let mask = if bits == 0 { 0 } else { u32::MAX << (32 - bits) };
            u32::from(n) & mask == u32::from(c) & mask
        }
        (std::net::IpAddr::V6(n), std::net::IpAddr::V6(c)) => {
            if bits > 128 {
                return false;
            }
            let mask = if bits == 0 { 0 } else { u128::MAX << (128 - bits) };
            u128::from(n) & mask == u128::from(c) & mask
        }
        _ => false,
    }
}

/// True if `client` is covered by any entry of a comma-separated panel IP allowlist.
/// The caller decides what an EMPTY list means (today: no restriction at all).
pub fn panel_ip_allowed(list: &str, client: &str) -> bool {
    let Ok(client) = client.trim().parse::<std::net::IpAddr>() else {
        // No usable client address (e.g. no `x-real-ip` from the proxy) and a
        // non-empty allowlist: deny. An allowlist that cannot identify the caller
        // must fail CLOSED.
        return false;
    };
    list.split(',')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .any(|e| ip_entry_covers(e, client))
}

/// True if `entry` is a form `panel_ip_allowed` can actually match. Used to reject a
/// malformed allowlist at write time — otherwise a typo silently locks every admin out
/// of the panel, and the only way back in is editing the database by hand.
pub fn valid_panel_ip_entry(entry: &str) -> bool {
    let entry = entry.trim();
    match entry.split_once('/') {
        None => entry.parse::<std::net::IpAddr>().is_ok(),
        Some((net, bits)) => match (net.trim().parse::<std::net::IpAddr>(), bits.trim().parse::<u32>()) {
            (Ok(std::net::IpAddr::V4(_)), Ok(b)) => b <= 32,
            (Ok(std::net::IpAddr::V6(_)), Ok(b)) => b <= 128,
            _ => false,
        },
    }
}

/// True if a redirect target `host` is internal: a literal internal IP, `localhost`,
/// OR a hostname that resolves (blocking) to any internal IP. For use inside a reqwest
/// redirect-policy closure, which is synchronous and cannot `.await` — so this resolves
/// with the blocking std resolver. That is acceptable here: it only runs when a monitored
/// target actually returns a 3xx (rare), inside the background uptime task. Fails CLOSED
/// (treats an unresolvable target as internal) so a redirect is never followed blindly.
pub fn host_resolves_internal_blocking(host: &str, port: u16) -> bool {
    let h = host.trim_matches(|c| c == '[' || c == ']');
    if h.eq_ignore_ascii_case("localhost") {
        return true;
    }
    if let Ok(ip) = h.parse::<std::net::IpAddr>() {
        return ip_is_internal(ip);
    }
    use std::net::ToSocketAddrs;
    match (h, port).to_socket_addrs() {
        Ok(addrs) => addrs.into_iter().any(|a| ip_is_internal(a.ip())),
        Err(_) => true, // cannot resolve → do not follow
    }
}

/// SSRF protection: validate that a URL does not resolve to an internal/private address.
///
/// Checks loopback, private (RFC 1918), link-local, ULA, CGNAT, IPv4-mapped-IPv6,
/// and unspecified addresses. Resolves DNS to catch bypass via hostnames that map
/// to internal IPs.
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
                if ip_is_internal(addr.ip()) {
                    return Err("URL resolves to a private/internal address".to_string());
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

/// Cache for [`detect_public_ip_cached`]: the resolved address and when it was read.
static PUBLIC_IP_CACHE: std::sync::Mutex<Option<(String, std::time::Instant)>> =
    std::sync::Mutex::new(None);

/// How long a detected public IP stays fresh. A box's public address changes
/// approximately never; five minutes is short enough that a real change is picked
/// up quickly and long enough that an interactive preflight is effectively free.
const PUBLIC_IP_TTL: std::time::Duration = std::time::Duration::from_secs(300);

/// [`detect_public_ip`] with a short-lived cache.
///
/// The uncached version makes an outbound HTTPS request to api.ipify.org on EVERY
/// call. That is fine for a once-per-provision check, but the DNS preflight runs
/// as the user types a domain — uncached, a single create-site form would issue
/// dozens of external requests and get rate-limited.
pub async fn detect_public_ip_cached() -> String {
    if let Ok(guard) = PUBLIC_IP_CACHE.lock() {
        if let Some((ip, at)) = guard.as_ref() {
            if at.elapsed() < PUBLIC_IP_TTL && !ip.is_empty() {
                return ip.clone();
            }
        }
    }

    let ip = detect_public_ip().await;

    // Never cache a failure — a transient network blip would otherwise pin an
    // empty address for the whole TTL and silently disable every DNS check.
    if !ip.is_empty() {
        if let Ok(mut guard) = PUBLIC_IP_CACHE.lock() {
            *guard = Some((ip.clone(), std::time::Instant::now()));
        }
    }
    ip
}

/// Parse the certificate expiry string the agent returns for SSL
/// provision / renew / DNS-01 responses.
///
/// The agent builds this with `x509_parser`'s `not_after.to_datetime().to_string()`
/// (agent `services/ssl.rs::get_cert_expiry`). That is a `time::OffsetDateTime`
/// `Display`, which renders as `"<date> <time> <offset>"` — e.g.
/// `2026-10-23 09:41:07.0 +00:00:00`. It never emits a literal `UTC`.
///
/// Every panel-side call site used to parse it with `"%Y-%m-%d %H:%M:%S%.f UTC"`,
/// which cannot match that shape, so the parse always failed and `sites.ssl_expiry`
/// was left NULL forever. That silently starved three features that read it: the
/// dashboard SSL countdown, `alert_engine::check_ssl_expiry` (whose query is
/// `WHERE ssl_enabled = TRUE AND ssl_expiry IS NOT NULL`, so it matched no rows at
/// all), and the ARI renewal bookkeeping in `auto_healer`.
///
/// Tolerant by design — panel and agent are separately-versioned binaries and a
/// fleet can run a mix, so accept the legacy `UTC` spelling and RFC 3339 too rather
/// than trade one brittle format for another.
/// Rewrite a trailing `+HH:MM:00` offset to `+HH:MM` so chrono can consume it.
/// Returns `None` when the string doesn't end in a zero-seconds offset, leaving the
/// caller to try its other formats.
fn strip_zero_offset_seconds(s: &str) -> Option<String> {
    // "+00:00:00" — 9 bytes, all ASCII, so byte indexing is safe.
    let tail = s.as_bytes();
    if tail.len() < 9 {
        return None;
    }
    let t = &tail[tail.len() - 9..];
    let shaped = matches!(t[0], b'+' | b'-')
        && t[1].is_ascii_digit()
        && t[2].is_ascii_digit()
        && t[3] == b':'
        && t[4].is_ascii_digit()
        && t[5].is_ascii_digit()
        && t[6] == b':'
        && t[7].is_ascii_digit()
        && t[8].is_ascii_digit();
    // Only whole-minute offsets: `:00` seconds carry no information to lose.
    if shaped && t[7] == b'0' && t[8] == b'0' {
        Some(s[..s.len() - 3].to_string())
    } else {
        None
    }
}

pub fn parse_agent_cert_expiry(raw: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }

    // The current agent format. chrono's `%z` family stops after `+HH:MM`, so the
    // trailing `:SS` the `time` crate always prints would be left unconsumed and the
    // whole parse rejected. Drop that group first — but only when it is `:00`, so a
    // genuinely sub-minute offset falls through instead of being silently shifted.
    let normalised = strip_zero_offset_seconds(s);
    if let Ok(dt) = chrono::DateTime::parse_from_str(
        normalised.as_deref().unwrap_or(s),
        "%Y-%m-%d %H:%M:%S%.f %:z",
    ) {
        return Some(dt.with_timezone(&chrono::Utc));
    }

    // RFC 3339 / ISO 8601, in case the agent ever serialises a chrono type instead.
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&chrono::Utc));
    }

    // Legacy spelling kept so a newer panel never regresses an older agent.
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f UTC") {
        return Some(dt.and_utc());
    }

    None
}

#[cfg(test)]
mod cert_expiry_tests {
    use super::parse_agent_cert_expiry;
    use chrono::{Datelike, Timelike};

    /// THE regression test. This exact string is what
    /// `time::OffsetDateTime`'s `Display` produces, which is what the agent
    /// sends. If this ever fails, `ssl_expiry` is silently NULL again and the
    /// SSL countdown + expiry alerts go dark with no error anywhere.
    #[test]
    fn parses_the_format_the_agent_actually_sends() {
        let dt = parse_agent_cert_expiry("2026-10-23 09:41:07.0 +00:00:00")
            .expect("agent's time::OffsetDateTime Display form must parse");
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.month(), 10);
        assert_eq!(dt.day(), 23);
        assert_eq!(dt.hour(), 9);
        assert_eq!(dt.minute(), 41);
        assert_eq!(dt.second(), 7);
    }

    #[test]
    fn parses_without_fractional_seconds() {
        assert!(parse_agent_cert_expiry("2026-10-23 09:41:07 +00:00:00").is_some());
    }

    #[test]
    fn parses_a_non_utc_offset_and_normalises_to_utc() {
        let dt = parse_agent_cert_expiry("2026-10-23 11:41:07.0 +02:00:00")
            .expect("offset form must parse");
        assert_eq!(dt.hour(), 9, "must be converted to UTC, not read as wall time");
    }

    #[test]
    fn still_parses_the_legacy_utc_spelling() {
        assert!(parse_agent_cert_expiry("2026-10-23 09:41:07.0 UTC").is_some());
    }

    #[test]
    fn parses_rfc3339() {
        assert!(parse_agent_cert_expiry("2026-10-23T09:41:07Z").is_some());
    }

    #[test]
    fn rejects_junk_instead_of_defaulting() {
        assert!(parse_agent_cert_expiry("").is_none());
        assert!(parse_agent_cert_expiry("   ").is_none());
        assert!(parse_agent_cert_expiry("not a date").is_none());
        assert!(parse_agent_cert_expiry("2026-10-23").is_none());
    }
}

#[cfg(test)]
mod ssrf_tests {
    use super::{ip_is_internal, v4_is_internal, v6_is_internal};
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn v4_internal_ranges_blocked() {
        for s in ["127.0.0.1", "10.1.2.3", "192.168.1.1", "172.16.0.1", "169.254.169.254", "0.0.0.0", "100.64.0.1"] {
            assert!(v4_is_internal(s.parse::<Ipv4Addr>().unwrap()), "{s} should be internal");
        }
    }

    #[test]
    fn v4_public_allowed() {
        for s in ["8.8.8.8", "1.1.1.1", "93.184.216.34", "100.63.255.255", "100.128.0.1"] {
            assert!(!v4_is_internal(s.parse::<Ipv4Addr>().unwrap()), "{s} should be public");
        }
    }

    #[test]
    fn v6_mapped_loopback_and_metadata_blocked() {
        // The exact gap: IPv4-mapped IPv6 whose is_loopback() is false.
        assert!(v6_is_internal("::ffff:127.0.0.1".parse::<Ipv6Addr>().unwrap()));
        assert!(v6_is_internal("::ffff:169.254.169.254".parse::<Ipv6Addr>().unwrap()));
        assert!(v6_is_internal("::ffff:10.0.0.1".parse::<Ipv6Addr>().unwrap()));
    }

    #[test]
    fn v6_ula_and_link_local_blocked() {
        assert!(v6_is_internal("fc00::1".parse::<Ipv6Addr>().unwrap()));
        assert!(v6_is_internal("fd00:ec2::254".parse::<Ipv6Addr>().unwrap()));
        assert!(v6_is_internal("fe80::1".parse::<Ipv6Addr>().unwrap()));
        assert!(v6_is_internal("::1".parse::<Ipv6Addr>().unwrap()));
        assert!(v6_is_internal("::".parse::<Ipv6Addr>().unwrap()));
    }

    #[test]
    fn v6_public_allowed() {
        assert!(!v6_is_internal("2606:4700:4700::1111".parse::<Ipv6Addr>().unwrap()));
        assert!(!ip_is_internal("2001:4860:4860::8888".parse::<std::net::IpAddr>().unwrap()));
    }
}

#[cfg(test)]
mod panel_ip_allowlist_tests {
    use super::{panel_ip_allowed, valid_panel_ip_entry};

    #[test]
    fn exact_addresses_match_either_family() {
        assert!(panel_ip_allowed("203.0.113.4", "203.0.113.4"));
        assert!(!panel_ip_allowed("203.0.113.4", "203.0.113.5"));
        assert!(panel_ip_allowed("2001:db8::1", "2001:db8::1"));
        // Written differently, same address — string equality (the pre-v2.46.0
        // behaviour) would reject this one.
        assert!(panel_ip_allowed("2001:0db8::0001", "2001:db8::1"));
    }

    #[test]
    fn cidr_ranges_cover_their_members() {
        assert!(panel_ip_allowed("10.0.0.0/8", "10.11.12.13"));
        assert!(!panel_ip_allowed("10.0.0.0/8", "11.0.0.1"));
        assert!(panel_ip_allowed("192.168.1.0/24", "192.168.1.255"));
        assert!(!panel_ip_allowed("192.168.1.0/24", "192.168.2.1"));
        assert!(panel_ip_allowed("2001:db8::/32", "2001:db8:dead:beef::1"));
        assert!(!panel_ip_allowed("2001:db8::/32", "2001:db9::1"));
    }

    #[test]
    fn prefix_boundaries_do_not_overflow() {
        // /0 covers everything of its family; a 32-bit shift would be UB-adjacent.
        assert!(panel_ip_allowed("0.0.0.0/0", "8.8.8.8"));
        assert!(panel_ip_allowed("::/0", "2001:db8::1"));
        // /32 and /128 are single hosts.
        assert!(panel_ip_allowed("8.8.8.8/32", "8.8.8.8"));
        assert!(!panel_ip_allowed("8.8.8.8/32", "8.8.8.9"));
        // Out-of-range prefixes match nothing rather than panicking.
        assert!(!panel_ip_allowed("8.8.8.8/33", "8.8.8.8"));
        assert!(!panel_ip_allowed("2001:db8::/129", "2001:db8::1"));
    }

    #[test]
    fn families_do_not_cross() {
        assert!(!panel_ip_allowed("0.0.0.0/0", "2001:db8::1"));
        assert!(!panel_ip_allowed("::/0", "8.8.8.8"));
    }

    #[test]
    fn lists_are_split_and_trimmed_and_any_entry_admits() {
        assert!(panel_ip_allowed(" 203.0.113.4 , 10.0.0.0/8 ", "10.1.1.1"));
        assert!(panel_ip_allowed("203.0.113.4,10.0.0.0/8", "203.0.113.4"));
        assert!(!panel_ip_allowed("203.0.113.4,10.0.0.0/8", "8.8.8.8"));
        // Empty entries are ignored, not treated as wildcards.
        assert!(!panel_ip_allowed(",,", "8.8.8.8"));
    }

    #[test]
    fn an_unusable_client_address_is_denied_not_admitted() {
        // No X-Real-IP from the proxy: an allowlist that cannot identify the
        // caller must fail CLOSED.
        assert!(!panel_ip_allowed("10.0.0.0/8", ""));
        assert!(!panel_ip_allowed("10.0.0.0/8", "not-an-ip"));
        assert!(!panel_ip_allowed("0.0.0.0/0", ""));
    }

    #[test]
    fn garbage_entries_admit_nobody() {
        assert!(!panel_ip_allowed("not-an-ip", "8.8.8.8"));
        assert!(!panel_ip_allowed("10.0.0.0/abc", "10.0.0.1"));
    }

    #[test]
    fn validation_accepts_what_the_matcher_understands() {
        for good in ["203.0.113.4", "10.0.0.0/8", "2001:db8::1", "2001:db8::/32", "0.0.0.0/0"] {
            assert!(valid_panel_ip_entry(good), "{good} should validate");
        }
        for bad in ["", "not-an-ip", "10.0.0.0/33", "2001:db8::/129", "10.0.0.0/", "10.0.0.0/-1"] {
            assert!(!valid_panel_ip_entry(bad), "{bad} should be rejected");
        }
    }
}

// ── Provisioning-log access control ─────────────────────────────────────
//
// `provision_logs` is one process-wide map shared by nine unrelated features:
// service installs, mail installs, system updates, site provisioning, backups
// and restores, migration imports, container deploys, git deploys and rollbacks.
// It is a single flat keyspace — nothing namespaces a site id apart from a
// deploy id — so any endpoint that looks a caller-supplied uuid up in it can
// reach every other feature's stream unless it proves ownership itself.
//
// Five endpoints did that five different ways, and the two weakest were open:
// the service-install stream looked the id up with no ownership test at all,
// and the git-deploy stream consulted the owner table but *fell through* when
// the id was absent — which was the common case, because only three of the
// sixteen sites that created a log ever recorded an owner for it. Together
// those two meant any signed-in account could read any other tenant's site
// provisioning stream, and that stream deliberately carries a generated CMS
// admin password in cleartext for the couple of minutes the install takes
// (`routes::sites`, the "credentials" step). The comment there justified
// emitting it on the grounds that the stream was owner-scoped. That was true
// of the endpoint its author was looking at and false of the siblings.
//
// So ownership stops being each caller's responsibility. A log cannot be
// created without an owner, and cannot be read except by that owner.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// The shared provisioning-log map: id -> (history, live channel, created-at).
pub type ProvisionLogs = Arc<
    Mutex<
        HashMap<
            uuid::Uuid,
            (
                Vec<crate::routes::sites::ProvisionStep>,
                tokio::sync::broadcast::Sender<crate::routes::sites::ProvisionStep>,
                Instant,
            ),
        >,
    >,
>;

/// Owner table for the map above: id -> the user whose log it is.
pub type ProvisionOwners = Arc<Mutex<HashMap<uuid::Uuid, uuid::Uuid>>>;

/// Create a provisioning log and record who owns it.
///
/// Deliberately returns nothing. Every feature emits by locking the map and
/// taking `get_mut(&id)`, which is what lets the terminal `remove` actually
/// close the stream: the map holds the last sender, so dropping the entry ends
/// every receiver. Handing a `Sender` clone back would let a caller outlive the
/// entry and hold the channel open, and the SSE stream would hang instead of
/// finishing.
///
/// Both maps are taken under one lock scope, logs first. The cleanup task in
/// `main` drops owner rows whose log has been evicted (`owners.retain(|id, _|
/// map.contains_key(id))`) and takes the same two locks in the same order, so
/// it can neither interleave with a registration and strand a live log without
/// an owner — which the reader below would then refuse to serve to the very
/// user watching it — nor deadlock against one.
///
/// `capacity` is the broadcast channel depth; callers that emit fine-grained
/// output (apt line-by-line) need far more than callers that emit a dozen steps.
pub fn register_provision_log(
    logs: &ProvisionLogs,
    owners: &ProvisionOwners,
    id: uuid::Uuid,
    owner: uuid::Uuid,
    capacity: usize,
) {
    let (tx, _) = tokio::sync::broadcast::channel(capacity);
    let mut logs = logs.lock().unwrap_or_else(|e| e.into_inner());
    let mut owners = owners.lock().unwrap_or_else(|e| e.into_inner());
    logs.insert(id, (Vec::new(), tx, Instant::now()));
    owners.insert(id, owner);
}

/// Drop a provisioning log and its owner together.
///
/// For the callers that discard a log outright rather than letting their
/// terminal step retire it — deleting the migration a log belongs to, say.
/// Removing only the log would leave the owner row behind until the next sweep.
pub fn forget_provision_log(logs: &ProvisionLogs, owners: &ProvisionOwners, id: uuid::Uuid) {
    let mut logs = logs.lock().unwrap_or_else(|e| e.into_inner());
    let mut owners = owners.lock().unwrap_or_else(|e| e.into_inner());
    logs.remove(&id);
    owners.remove(&id);
}

/// Resolve a provisioning log for a caller, or refuse.
///
/// Returns the history so far plus a receiver for everything still to come.
///
/// An id with no owner recorded is refused. That is the whole point: a future
/// feature that adds a log without going through `register_provision_log` gets
/// a stream nobody can read, which is a bug its author will notice on the first
/// run — the previous arrangement gave them a stream *everybody* could read,
/// which nobody would notice at all.
///
/// "Not yours" and "no such log" deliberately return the same 404. Separating
/// them would turn the endpoint into an oracle for which uuids are live jobs.
pub fn open_provision_log(
    logs: &ProvisionLogs,
    owners: &ProvisionOwners,
    id: uuid::Uuid,
    caller: uuid::Uuid,
    missing: &str,
) -> Result<
    (
        Vec<crate::routes::sites::ProvisionStep>,
        tokio::sync::broadcast::Receiver<crate::routes::sites::ProvisionStep>,
    ),
    crate::error::ApiError,
> {
    let logs = logs.lock().unwrap_or_else(|e| e.into_inner());
    let owners = owners.lock().unwrap_or_else(|e| e.into_inner());

    let owned = owners.get(&id).copied() == Some(caller);
    match logs.get(&id) {
        Some((history, tx, _)) if owned => Ok((history.clone(), tx.subscribe())),
        _ => Err(crate::error::err(
            axum::http::StatusCode::NOT_FOUND,
            missing,
        )),
    }
}

/// Refuse, at the mint, a stream whose transport can only ever reach the panel's
/// own agent — when the caller has a different server selected.
///
/// The terminal and log-stream tickets are signed with the SELECTED server's
/// agent token, because both handlers take `ServerScope`. The browser then dials
/// `window.location.host`, and nginx pins that path to the LOCAL agent socket, so
/// the receiving agent verifies a ticket signed with somebody else's key and
/// answers 401. The result was a promised capability that failed for every fleet
/// member, and failed dishonestly: the terminal reported "Connection lost" and the
/// log viewer retried every three seconds for ever, both describing a network
/// problem that did not exist.
///
/// The knowledge to say so was already here. `ServerScope` resolves the id and
/// both handlers bound it and threw it away. Comparing it against the local
/// server answers before a socket is opened.
///
/// This deliberately does NOT proxy the stream. A backend WebSocket proxy would
/// deliver a member-signed ticket to the member's own agent — which would honour
/// it, including its `domain` query parameter, which the ticket does not bind and
/// which an empty value turns into a root login shell. Today two independent
/// things confine that to the panel host. Removing both, before the domain is
/// bound into the signed ticket, would widen it to every host in the fleet.
pub async fn require_local_agent_scope(
    state: &crate::AppState,
    scoped_server_id: uuid::Uuid,
    feature: &str,
) -> Result<(), crate::error::ApiError> {
    let local_id = state.agents.local_server_id().await;

    // No local row registered yet: `ServerScope` would already have rejected the
    // request, so there is nothing left to disagree with.
    if local_id == Some(scoped_server_id) || local_id.is_none() {
        return Ok(());
    }

    let name: Option<(String,)> = sqlx::query_as("SELECT name FROM servers WHERE id = $1")
        .bind(scoped_server_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();

    let which = name
        .map(|r| r.0)
        .unwrap_or_else(|| scoped_server_id.to_string());

    Err(crate::error::err(
        axum::http::StatusCode::NOT_IMPLEMENTED,
        &format!(
            "{feature} runs on the panel host only. \"{which}\" is a fleet member, \
             and the panel cannot yet stream to one. Switch back to the panel \
             server to use it."
        ),
    ))
}

#[cfg(test)]
mod provision_log_tests {
    use super::*;

    fn maps() -> (ProvisionLogs, ProvisionOwners) {
        (
            Arc::new(Mutex::new(HashMap::new())),
            Arc::new(Mutex::new(HashMap::new())),
        )
    }

    #[test]
    fn owner_can_read_its_own_log() {
        let (logs, owners) = maps();
        let id = uuid::Uuid::new_v4();
        let owner = uuid::Uuid::new_v4();
        register_provision_log(&logs, &owners, id, owner, 8);
        assert!(open_provision_log(&logs, &owners, id, owner, "gone").is_ok());
    }

    #[test]
    fn a_stranger_cannot_read_someone_elses_log() {
        let (logs, owners) = maps();
        let id = uuid::Uuid::new_v4();
        let owner = uuid::Uuid::new_v4();
        register_provision_log(&logs, &owners, id, owner, 8);
        let stranger = uuid::Uuid::new_v4();
        assert!(open_provision_log(&logs, &owners, id, stranger, "gone").is_err());
    }

    // The regression this module exists for: a log present in the map with no
    // owner recorded beside it. The old git-deploy stream fell through such a
    // gap to the log itself; thirteen of the sixteen creation sites left one.
    #[test]
    fn an_ownerless_log_is_refused_not_served() {
        let (logs, owners) = maps();
        let id = uuid::Uuid::new_v4();
        let (tx, _) = tokio::sync::broadcast::channel(8);
        logs.lock()
            .unwrap()
            .insert(id, (Vec::new(), tx, Instant::now()));

        assert!(
            open_provision_log(&logs, &owners, id, uuid::Uuid::new_v4(), "gone").is_err(),
            "a log with no recorded owner must not be readable"
        );
    }

    #[test]
    fn registration_records_the_owner() {
        let (logs, owners) = maps();
        let id = uuid::Uuid::new_v4();
        let owner = uuid::Uuid::new_v4();
        register_provision_log(&logs, &owners, id, owner, 8);
        assert_eq!(owners.lock().unwrap().get(&id).copied(), Some(owner));
        assert!(logs.lock().unwrap().contains_key(&id));
    }

    // Absent and forbidden must be indistinguishable, or the endpoint tells a
    // stranger which uuids are live jobs.
    #[test]
    fn missing_and_forbidden_are_indistinguishable() {
        let (logs, owners) = maps();
        let live = uuid::Uuid::new_v4();
        register_provision_log(&logs, &owners, live, uuid::Uuid::new_v4(), 8);

        let stranger = uuid::Uuid::new_v4();
        let forbidden = open_provision_log(&logs, &owners, live, stranger, "gone").unwrap_err();
        let absent =
            open_provision_log(&logs, &owners, uuid::Uuid::new_v4(), stranger, "gone").unwrap_err();

        // Status *and* body — a differing sentence is as good an oracle as a
        // differing code.
        assert_eq!(forbidden.0, absent.0);
        assert_eq!(*forbidden.1, *absent.1);
    }

    // Emitting the way every feature actually emits — lock the map, `get_mut`,
    // push and send — must reach a reader that has already attached. This is
    // the real path; a test against a sender handed back by `register` would
    // pin a contract no caller uses.
    #[test]
    fn a_step_emitted_through_the_map_reaches_an_attached_reader() {
        let (logs, owners) = maps();
        let id = uuid::Uuid::new_v4();
        let owner = uuid::Uuid::new_v4();
        register_provision_log(&logs, &owners, id, owner, 8);

        let (_history, mut rx) = open_provision_log(&logs, &owners, id, owner, "gone").unwrap();

        {
            let mut map = logs.lock().unwrap();
            let (history, tx, _) = map.get_mut(&id).unwrap();
            let ev = crate::routes::sites::ProvisionStep {
                step: "s".into(),
                label: "l".into(),
                status: "done".into(),
                message: None,
            };
            history.push(ev.clone());
            tx.send(ev).unwrap();
        }

        assert_eq!(rx.try_recv().unwrap().step, "s");
    }

    // Removing the entry must end the stream. That is what the terminal
    // `remove` in every feature relies on, and it only holds while the map owns
    // the last sender.
    #[test]
    fn removing_the_entry_closes_the_reader() {
        let (logs, owners) = maps();
        let id = uuid::Uuid::new_v4();
        let owner = uuid::Uuid::new_v4();
        register_provision_log(&logs, &owners, id, owner, 8);

        let (_history, mut rx) = open_provision_log(&logs, &owners, id, owner, "gone").unwrap();
        logs.lock().unwrap().remove(&id);

        assert!(
            matches!(
                rx.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Closed)
            ),
            "dropping the map entry must close the receiver, or the SSE stream hangs"
        );
    }
}
