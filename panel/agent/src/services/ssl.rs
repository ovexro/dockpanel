use instant_acme::{
    Account, AccountCredentials, AuthorizationStatus, CertificateIdentifier, ChallengeType,
    Identifier, LetsEncrypt, NewAccount, NewOrder, OrderStatus,
};
use rustls::pki_types::CertificateDer;
use std::path::Path;
use tera::Tera;

use crate::routes::nginx::SiteConfig;
use crate::services::nginx;

const ACME_ACCOUNT_PATH: &str = "/etc/dockpanel/ssl/acme-account.json";
const SSL_DIR: &str = "/etc/dockpanel/ssl";
const ACME_WEBROOT: &str = "/var/www/acme";

/// Write a secret file (a TLS private key) with 0600 applied AT CREATION,
/// closing the write-then-chmod window where the agent (running as root under
/// the default 022 umask) leaves the key briefly world/group-readable (0644) —
/// a local disclosure race on a shared box. `.mode(0o600)` sets the permission
/// as the file is created; the trailing `set_permissions` re-tightens an
/// already-existing file (mode() is ignored when the file already exists).
pub async fn write_key_file(path: &str, content: &str) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        use tokio::io::AsyncWriteExt;
        let mut f = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .await
            .map_err(|e| format!("Failed to open key file: {e}"))?;
        f.write_all(content.as_bytes())
            .await
            .map_err(|e| format!("Failed to write key: {e}"))?;
        let _ = tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        tokio::fs::write(path, content)
            .await
            .map_err(|e| format!("Failed to write key: {e}"))
    }
}

/// Normalise a DNS name for comparison: trim, drop ONE trailing root dot,
/// lowercase. Both sides go through this — `is_valid_domain` permits uppercase,
/// so the site's own domain needs it as much as the certificate's names do.
fn normalise_dns_name(name: &str) -> String {
    let n = name.trim();
    n.strip_suffix('.').unwrap_or(n).to_ascii_lowercase()
}

/// Does one name presented by a certificate cover `domain`?
///
/// A wildcard covers EXACTLY ONE label and only the leftmost one, per RFC 6125.
/// `*.example.com` covers `app.example.com` and does NOT cover
/// `app.staging.example.com` or the bare apex. Getting this wrong in the
/// permissive direction is what makes a panel report TLS that browsers reject;
/// getting it wrong in the strict direction would refuse working certificates,
/// so both halves are pinned by tests below.
fn dns_name_covers(name: &str, domain: &str) -> bool {
    if name == domain {
        return true;
    }
    let Some(base) = name.strip_prefix("*.") else {
        return false;
    };
    // `*.com` and partial wildcards (`w*.example.com`) are not names we accept.
    if base.is_empty() || base.contains('*') || !base.contains('.') {
        return false;
    }
    match domain.split_once('.') {
        Some((label, rest)) => !label.is_empty() && !label.contains('*') && rest == base,
        None => false,
    }
}

/// The DNS names a parsed certificate presents.
///
/// The Common Name is a FALLBACK, consulted only when the certificate carries no
/// subjectAltName extension at all — which is what every browser does, and what
/// makes a `CN=other.example.com, SAN=real.example.com` certificate correctly
/// refused for `other.example.com`.
fn presented_dns_names(cert: &x509_parser::certificate::X509Certificate<'_>) -> Vec<String> {
    if let Ok(Some(san)) = cert.subject_alternative_name() {
        let names: Vec<String> = san
            .value
            .general_names
            .iter()
            .filter_map(|gn| match gn {
                x509_parser::extensions::GeneralName::DNSName(n) => Some(normalise_dns_name(n)),
                _ => None,
            })
            .collect();
        if !names.is_empty() {
            return names;
        }
    }
    cert.subject()
        .iter_common_name()
        .filter_map(|cn| cn.as_str().ok())
        .map(normalise_dns_name)
        .collect()
}

/// Check that an uploaded certificate actually names the site it is being
/// installed for. On success returns the names it presents, so the caller can
/// show them back to the operator.
///
/// Three decisions here are load-bearing and each of them is the opposite of an
/// obvious alternative that was measured and rejected:
///
/// 1. **Only the FIRST `CERTIFICATE` block is asked**, because that is the one
///    nginx binds as the leaf. Measured against nginx 1.24.0: a bundle of
///    `[wrong.crt, right.crt]` paired with `wrong.key` passes `nginx -t` and
///    then serves `wrong.crt`. A rule that accepted the bundle because SOME
///    block covered the domain would wave through exactly the silent mismatch
///    this function exists to stop.
/// 2. **Blocks are filtered by label, and a non-certificate block is refused
///    outright.** The certificate field was checked with a bare
///    `contains("BEGIN CERTIFICATE")`, which a key-then-certificate paste
///    satisfies. That paste lands in `fullchain.pem`, written with a plain
///    `fs::write` at **0644** while the key beside it is deliberately 0600 —
///    driven on a fresh box and confirmed. ⚠ Do NOT overstate this: the key is
///    not actually reachable by other local accounts, because `/etc/dockpanel`
///    and `/etc/dockpanel/ssl` are both **0700**, so traversal stops above the
///    file. The protection comes from the directory, not from the write, and it
///    survives only as long as that stays true. The sharper half of the same
///    paste is that it blinds the expiry ladder: the status reader decodes the
///    first PEM block whatever its label and gets no date out of a key, so a
///    first upload in that shape records no expiry at all — the row the
///    countdown, the alerts and the healer all filter OUT.
/// 3. **There is no `is_ca()` filter.** `openssl req -x509` stamps
///    `basicConstraints CA:TRUE` on its output, so every self-signed staging
///    certificate is a "CA" by that test and an `is_ca()` leaf filter refuses
///    all of them. Note for whoever writes fixtures: `rcgen` defaults to
///    `IsCa::NoCa`, so a suite built only on rcgen defaults stays GREEN with
///    that filter wrongly restored — one fixture below sets it by hand.
pub fn cert_covers_domain(cert_pem: &str, domain: &str) -> Result<Vec<String>, String> {
    let want = normalise_dns_name(domain);
    let mut leaf: Option<Vec<String>> = None;

    for block in x509_parser::pem::Pem::iter_from_buffer(cert_pem.as_bytes()) {
        let block = block.map_err(|e| format!("the certificate could not be read: {e}"))?;
        if block.label != "CERTIFICATE" {
            return Err(format!(
                "the certificate field contains a {} block. Paste only the certificate here, with \
                 its chain if it has one — the private key belongs in its own field, where it is \
                 stored with the permissions a key needs and where this site's expiry can still \
                 be read.",
                block.label.to_ascii_lowercase()
            ));
        }
        if leaf.is_none() {
            let cert = block
                .parse_x509()
                .map_err(|e| format!("the certificate could not be read: {e}"))?;
            leaf = Some(presented_dns_names(&cert));
        }
    }

    let Some(names) = leaf else {
        return Err("no certificate was found in the certificate field.".to_string());
    };

    if names.iter().any(|n| dns_name_covers(n, &want)) {
        return Ok(names);
    }
    let presented = if names.is_empty() {
        "no host name at all".to_string()
    } else {
        names.join(", ")
    };
    Err(format!(
        "this certificate is for {presented}, not {want}. A certificate has to name the site it \
         secures or browsers reject it whatever the panel says — check that you pasted the \
         certificate for {want} and not for another site."
    ))
}

/// Options controlling an ACME order — profile selection + ARI replacement chain.
/// `None` or all-None fields means "classic, no prior cert" (backwards-compatible).
#[derive(Default, Clone)]
pub struct ProvisionOpts<'a> {
    /// ACME profile to request ("classic", "tlsserver", "shortlived"). If the
    /// CA doesn't support profiles, pass None — otherwise the order will fail.
    pub profile: Option<&'a str>,
    /// PEM of the certificate being replaced (RFC 9773 ARI `replaces` hint).
    /// When set, the CA can correlate the renewal with the prior issuance.
    pub replaces_pem: Option<&'a str>,
}

#[derive(serde::Serialize)]
pub struct CertInfo {
    pub cert_path: String,
    pub key_path: String,
    pub expiry: Option<String>,
    /// Echoes the profile used for this order (None if none was requested).
    pub profile: Option<String>,
}

#[derive(serde::Serialize)]
pub struct ProfileInfo {
    pub name: String,
    pub description: String,
}

/// ARI (RFC 9773) suggestion: when the CA wants us to renew and when it wants
/// us to check back for a refreshed suggestion.
#[derive(serde::Serialize)]
pub struct AriSuggestion {
    /// Start of the suggested renewal window.
    pub renewal_at: chrono::DateTime<chrono::Utc>,
    /// End of the suggested renewal window.
    pub renewal_before: chrono::DateTime<chrono::Utc>,
    /// When to re-fetch ARI (CA-hinted retry-after).
    pub recheck_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Serialize)]
pub struct CertStatus {
    pub domain: String,
    pub has_cert: bool,
    pub issuer: Option<String>,
    pub not_after: Option<String>,
    pub days_remaining: Option<i64>,
}

/// Outcome of persisting freshly-minted ACME account credentials.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Persisted {
    /// These credentials are the ones now on disk.
    Written,
    /// Somebody else's credentials were already there. Ours are abandoned, and
    /// the caller MUST adopt what is on disk instead of using what it minted.
    Adopted,
}

/// Write account credentials only if no account file exists yet.
///
/// `create_new` is the whole point: the create-or-fail is atomic in the kernel,
/// so however many writers race, exactly one is told `Written` and every other
/// is told `Adopted`. A plain write would let the last one silently overwrite
/// the account every earlier certificate was issued under.
///
/// The 0600 is applied AT CREATION rather than by a following `set_permissions`,
/// for the reason `write_key_file` above already documents: this file is an ACME
/// account key, so the write-then-chmod window left the credential that can
/// order and revoke this box's certificates briefly world-readable.
pub(crate) async fn persist_account_credentials(
    path: &str,
    json: &str,
) -> Result<Persisted, String> {
    if let Some(parent) = Path::new(path).parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }

    let mut opts = tokio::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    // `mode` here is tokio's own inherent method on its OpenOptions, so this
    // needs no `OpenOptionsExt` import — adding one is an unused-import warning,
    // not a fix. The permission is asserted on the file that lands, in
    // `the_account_file_is_owner_only_from_creation`, because reading this line
    // cannot tell you which `mode` resolved.
    #[cfg(unix)]
    opts.mode(0o600);

    match opts.open(path).await {
        Ok(mut f) => {
            use tokio::io::AsyncWriteExt;
            f.write_all(json.as_bytes())
                .await
                .map_err(|e| format!("Failed to save ACME account: {e}"))?;
            f.flush()
                .await
                .map_err(|e| format!("Failed to save ACME account: {e}"))?;
            Ok(Persisted::Written)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(Persisted::Adopted),
        Err(e) => Err(format!("Failed to save ACME account: {e}")),
    }
}

/// Read the stored ACME account, or `None` when no account file exists.
async fn stored_account() -> Result<Option<Account>, String> {
    if !Path::new(ACME_ACCOUNT_PATH).exists() {
        return Ok(None);
    }
    let json = tokio::fs::read_to_string(ACME_ACCOUNT_PATH)
        .await
        .map_err(|e| format!("Failed to read ACME account: {e}"))?;
    let creds: AccountCredentials = serde_json::from_str(&json)
        .map_err(|e| format!("Failed to parse ACME account: {e}"))?;
    let account = Account::builder()
        .map_err(|e| format!("Failed to build ACME client: {e}"))?
        .from_credentials(creds)
        .await
        .map_err(|e| format!("Failed to load ACME account: {e}"))?;
    Ok(Some(account))
}

/// Load the existing ACME account or create one.
///
/// ⛔ THE INVARIANT: the account handed back is ALWAYS the one whose credentials
/// are on disk. It is not a tidiness property — a certificate issued under an
/// account key that is nowhere on disk can never be renewed. Every renewal door
/// attaches the prior certificate as an RFC 9773 ARI `replaces` hint, the CA
/// answers `unauthorized` because that account did not request the certificate
/// being replaced, and `provision_cert` has no path that retries without the
/// hint — so the order is refused, every cycle, until the certificate expires
/// and the site goes dark behind a panel that reported a successful issuance.
///
/// ⭐ HOW THAT USED TO HAPPEN, and it needed no operator mistake: this was a
/// check-then-act with a network round trip in the middle. Two sites created
/// together on a fresh box each spawn an auto-SSL task; both looked, both saw no
/// account file, both created a DIFFERENT account at Let's Encrypt, and the
/// second `write` clobbered the first. Measured on a throwaway box: two
/// "Created new ACME account" lines 443 ms apart, after which the certificates
/// issued inside that window answered 422 to every renewal for ever, while one
/// issued after it renewed 200. The window is the whole account-creation round
/// trip, and creating two sites at once is ordinary first-run behaviour.
///
/// Two mechanisms, because they cover different racers:
///   1. The mutex serialises the doors inside THIS process, which is where the
///      race actually happened — seven call sites, all of them reachable from a
///      `tokio::spawn`.
///   2. `create_new` covers a second process (an agent restart overlapping a
///      running one), which no in-process lock can see. Losing that race is not
///      an error: the loser adopts the winner's account and abandons its own.
pub async fn load_or_create_account(email: &str) -> Result<Account, String> {
    // Held across the whole load-or-create, including the round trip to the CA.
    // It is taken once per boot in practice; the cost is bounded by the very
    // thing it prevents happening twice.
    static ACCOUNT_INIT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _serialise = ACCOUNT_INIT.lock().await;

    if let Some(account) = stored_account().await? {
        tracing::info!("Loaded existing ACME account");
        return Ok(account);
    }

    let (account, creds) = Account::builder()
        .map_err(|e| format!("Failed to build ACME client: {e}"))?
        .create(
            &NewAccount {
                contact: &[&format!("mailto:{email}")],
                terms_of_service_agreed: true,
                only_return_existing: false,
            },
            LetsEncrypt::Production.url().to_string(),
            None,
        )
        .await
        .map_err(|e| format!("Failed to create ACME account: {e}"))?;

    let json = serde_json::to_string_pretty(&creds)
        .map_err(|e| format!("Failed to serialize ACME creds: {e}"))?;

    match persist_account_credentials(ACME_ACCOUNT_PATH, &json).await? {
        Persisted::Written => {
            tracing::info!("Created new ACME account for {email}");
            Ok(account)
        }
        Persisted::Adopted => {
            // Another PROCESS wrote first. Returning the account just minted
            // would issue certificates under a key that is nowhere on disk,
            // which is precisely the defect this function was rewritten to
            // remove — so adopt the stored one and let ours go unused.
            tracing::warn!(
                "An ACME account was written by another process first — adopting it and \
                 abandoning the one just created, so every certificate is issued under the \
                 account whose key is on disk"
            );
            stored_account().await?.ok_or_else(|| {
                "ACME account file disappeared between creation and read".to_string()
            })
        }
    }
}

/// Provision a Let's Encrypt certificate for a domain using HTTP-01 challenge.
/// Marks an error the CERTIFICATE AUTHORITY produced, as opposed to one this
/// machine produced.
///
/// The two are worth telling apart because only one of them is the operator's to
/// act on. A rejected challenge, a rate limit, an order the CA will not accept —
/// those are answers, and repeating them to the operator is useful. A full disk
/// or an unwritable directory here is an agent fault, and describing it as
/// anything the CA said would be a lie in the one direction that matters: it
/// sends somebody to check their DNS while their server is out of space.
///
/// ⚠ Marked POSITIVELY, and everything unmarked is treated as local. A new arm
/// added below therefore defaults to "this machine's fault", which keeps its
/// incident id — the safe direction. The inverse default would quietly hand
/// every future error to the operator as though the CA had spoken.
pub const CA_DECLINED: &str = "[ca] ";

/// The same idea for the OTHER party the DNS-01 door talks to.
///
/// `provision_cert_dns01` has three ways to fail, not two: the CA can decline,
/// this machine can break, and — uniquely on this door — the DNS provider can
/// refuse. A token without `DNS:Edit` is the commonest real failure here, and it
/// is the operator's to fix, so hiding it behind an incident id wastes the one
/// sentence that would have helped. But it is NOT something the CA said, and
/// marking it `CA_DECLINED` would make the panel attribute Cloudflare's refusal
/// to Let's Encrypt — a lie about who declined, in a message written to be
/// trusted.
///
/// ⚠ Marked POSITIVELY, exactly like [`CA_DECLINED`]: an arm added below with no
/// marker defaults to "this machine's fault" and keeps its incident id.
pub const DNS_PROVIDER_DECLINED: &str = "[dns] ";

pub async fn provision_cert(
    account: &Account,
    domain: &str,
    opts: Option<&ProvisionOpts<'_>>,
) -> Result<CertInfo, String> {
    tracing::info!("Provisioning SSL for {domain}");

    // Create order with optional profile + ARI replaces hint
    let identifier = Identifier::Dns(domain.to_string());
    let identifiers = [identifier];
    let mut new_order = NewOrder::new(&identifiers);
    if let Some(o) = opts {
        if let Some(p) = o.profile {
            new_order = new_order.profile(p);
        }
        if let Some(pem) = o.replaces_pem {
            match cert_identifier_from_pem(pem) {
                Ok(owned) => new_order = new_order.replaces(owned),
                Err(e) => tracing::warn!("ARI replaces skipped ({domain}): {e}"),
            }
        }
    }
    let profile_used = opts.and_then(|o| o.profile).map(String::from);
    let mut order = account
        .new_order(&new_order)
        .await
        .map_err(|e| format!("{CA_DECLINED}The CA refused the certificate order: {e}"))?;

    let state = order.state();
    let needs_challenge = matches!(state.status, OrderStatus::Pending);

    if !needs_challenge && !matches!(state.status, OrderStatus::Ready) {
        return Err(format!("{CA_DECLINED}The CA returned an unexpected order status: {:?}", state.status));
    }

    if needs_challenge {
        // Get authorizations and solve HTTP-01 challenge
        let mut authorizations = order.authorizations();
        while let Some(result) = authorizations.next().await {
            let mut authz = result.map_err(|e| format!("{CA_DECLINED}The CA would not return an authorization: {e}"))?;

            match authz.status {
                AuthorizationStatus::Valid => continue,
                AuthorizationStatus::Pending => {}
                status => return Err(format!("{CA_DECLINED}The CA returned an unexpected authorization status: {status:?}")),
            }

            let mut challenge = authz
                .challenge(ChallengeType::Http01)
                .ok_or("No HTTP-01 challenge found")?;

            let token = challenge.token.clone();
            let key_auth = challenge.key_authorization();

            // Write challenge file to ACME webroot
            let challenge_dir = format!("{ACME_WEBROOT}/.well-known/acme-challenge");
            tokio::fs::create_dir_all(&challenge_dir)
                .await
                .map_err(|e| format!("Failed to create challenge dir: {e}"))?;
            let challenge_path = format!("{challenge_dir}/{token}");
            tokio::fs::write(&challenge_path, key_auth.as_str())
                .await
                .map_err(|e| format!("Failed to write challenge file: {e}"))?;

            tracing::info!("Challenge file written for {domain}");

            // Tell ACME server the challenge is ready
            challenge
                .set_ready()
                .await
                .map_err(|e| format!("{CA_DECLINED}The CA would not accept the challenge as ready: {e}"))?;
        }
    }

    // Poll until order is ready for finalization
    use instant_acme::RetryPolicy;
    let timeout = std::time::Duration::from_secs(60);

    order
        .poll_ready(&RetryPolicy::new().timeout(timeout))
        .await
        .map_err(|e| format!("{CA_DECLINED}The CA did not validate the challenge: {e}"))?;

    // Finalize — generates CSR internally and returns private key PEM
    let private_key_pem = order
        .finalize()
        .await
        .map_err(|e| format!("{CA_DECLINED}The CA would not finalize the order: {e}"))?;

    // Poll for certificate
    let cert_chain_pem = order
        .poll_certificate(&RetryPolicy::new().timeout(timeout))
        .await
        .map_err(|e| format!("{CA_DECLINED}The CA would not hand back the certificate: {e}"))?;

    // Save certificate and private key
    let cert_dir = format!("{SSL_DIR}/{domain}");
    tokio::fs::create_dir_all(&cert_dir)
        .await
        .map_err(|e| format!("Failed to create cert dir: {e}"))?;

    let cert_path = format!("{cert_dir}/fullchain.pem");
    let key_path = format!("{cert_dir}/privkey.pem");

    tokio::fs::write(&cert_path, &cert_chain_pem)
        .await
        .map_err(|e| format!("Failed to write cert: {e}"))?;
    // Write the private key 0600-at-creation (no world-readable 0644 window).
    write_key_file(&key_path, &private_key_pem).await?;

    // Clean up challenge files
    let challenge_dir = format!("{ACME_WEBROOT}/.well-known/acme-challenge");
    if let Ok(mut entries) = tokio::fs::read_dir(&challenge_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            tokio::fs::remove_file(entry.path()).await.ok();
        }
    }

    // Parse expiry for response
    let expiry = get_cert_expiry(&cert_path).await;

    tracing::info!("SSL certificate provisioned for {domain}");

    Ok(CertInfo {
        cert_path,
        key_path,
        expiry,
        profile: profile_used,
    })
}

/// Provision a Let's Encrypt certificate using DNS-01 challenge via Cloudflare.
/// Supports wildcard certificates (*.domain + domain).
pub async fn provision_cert_dns01(
    account: &Account,
    domain: &str,
    cf_zone_id: &str,
    cf_api_token: &str,
    cf_api_email: Option<&str>,
    wildcard: bool,
    opts: Option<&ProvisionOpts<'_>>,
) -> Result<CertInfo, String> {
    let label = if wildcard { "wildcard" } else { "dns01" };
    tracing::info!("Provisioning SSL ({label}) for {domain}");

    // Build identifiers
    let mut ids = vec![Identifier::Dns(domain.to_string())];
    if wildcard {
        ids.push(Identifier::Dns(format!("*.{domain}")));
    }

    let mut new_order = NewOrder::new(&ids);
    if let Some(o) = opts {
        if let Some(p) = o.profile {
            new_order = new_order.profile(p);
        }
        if let Some(pem) = o.replaces_pem {
            match cert_identifier_from_pem(pem) {
                Ok(owned) => new_order = new_order.replaces(owned),
                Err(e) => tracing::warn!("ARI replaces skipped ({domain}): {e}"),
            }
        }
    }
    let profile_used = opts.and_then(|o| o.profile).map(String::from);
    let mut order = account
        .new_order(&new_order)
        .await
        .map_err(|e| format!("{CA_DECLINED}The CA refused the certificate order: {e}"))?;

    let state = order.state();
    if !matches!(state.status, OrderStatus::Pending | OrderStatus::Ready) {
        return Err(format!("{CA_DECLINED}The CA returned an unexpected order status: {:?}", state.status));
    }

    // Build Cloudflare client
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("HTTP client: {e}"))?;
    let cf_api = "https://api.cloudflare.com/client/v4";
    let mut headers = reqwest::header::HeaderMap::new();
    if let Some(email) = cf_api_email {
        headers.insert(
            "X-Auth-Email",
            email.parse().map_err(|_| {
                format!("{DNS_PROVIDER_DECLINED}The Cloudflare account email for this zone is not a usable header value")
            })?,
        );
        headers.insert(
            "X-Auth-Key",
            cf_api_token.parse().map_err(|_| {
                format!("{DNS_PROVIDER_DECLINED}The Cloudflare API key for this zone is not a usable header value")
            })?,
        );
    } else {
        headers.insert(
            "Authorization",
            format!("Bearer {cf_api_token}").parse().map_err(|_| {
                format!("{DNS_PROVIDER_DECLINED}The Cloudflare API token for this zone is not a usable header value")
            })?,
        );
    }

    let mut created_records: Vec<String> = Vec::new();

    if matches!(state.status, OrderStatus::Pending) {
        let mut authorizations = order.authorizations();
        while let Some(result) = authorizations.next().await {
            let mut authz = match result {
                Ok(a) => a,
                Err(e) => {
                    cleanup_cf_records(&client, cf_api, cf_zone_id, &headers, &created_records).await;
                    return Err(format!("{CA_DECLINED}The CA would not return an authorization: {e}"));
                }
            };

            match authz.status {
                AuthorizationStatus::Valid => continue,
                AuthorizationStatus::Pending => {}
                status => {
                    cleanup_cf_records(&client, cf_api, cf_zone_id, &headers, &created_records).await;
                    return Err(format!("{CA_DECLINED}The CA returned an unexpected authorization status: {status:?}"));
                }
            }

            let mut challenge = match authz.challenge(ChallengeType::Dns01) {
                Some(c) => c,
                None => {
                    cleanup_cf_records(&client, cf_api, cf_zone_id, &headers, &created_records).await;
                    return Err(format!(
                        "{CA_DECLINED}The CA offered no DNS-01 challenge for this domain"
                    ));
                }
            };

            let key_auth = challenge.key_authorization();
            let txt_value = key_auth.dns_value();

            // Create TXT record: _acme-challenge.{domain}
            let record_name = format!("_acme-challenge.{domain}");
            tracing::info!("DNS-01: creating TXT {record_name} = {txt_value}");

            let resp = match client
                .post(&format!("{cf_api}/zones/{cf_zone_id}/dns_records"))
                .headers(headers.clone())
                .json(&serde_json::json!({
                    "type": "TXT",
                    "name": &record_name,
                    "content": &txt_value,
                    "ttl": 120,
                }))
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    cleanup_cf_records(&client, cf_api, cf_zone_id, &headers, &created_records).await;
                    return Err(format!("{DNS_PROVIDER_DECLINED}Cloudflare could not be reached to publish the challenge record: {e}"));
                }
            };

            let resp_json: serde_json::Value = match resp.json().await {
                Ok(j) => j,
                Err(e) => {
                    cleanup_cf_records(&client, cf_api, cf_zone_id, &headers, &created_records).await;
                    return Err(format!("{DNS_PROVIDER_DECLINED}Cloudflare returned a response that could not be read: {e}"));
                }
            };

            if resp_json.get("success").and_then(|v| v.as_bool()) != Some(true) {
                cleanup_cf_records(&client, cf_api, cf_zone_id, &headers, &created_records).await;
                let errs = resp_json.get("errors").cloned().unwrap_or_default();
                return Err(format!("{DNS_PROVIDER_DECLINED}Cloudflare refused to create the challenge record: {errs}"));
            }

            if let Some(rid) = resp_json.pointer("/result/id").and_then(|v| v.as_str()) {
                created_records.push(rid.to_string());
            }

            // Wait for DNS propagation (Cloudflare is fast, but ACME servers cache)
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;

            if let Err(e) = challenge.set_ready().await {
                cleanup_cf_records(&client, cf_api, cf_zone_id, &headers, &created_records).await;
                return Err(format!("{CA_DECLINED}The CA would not accept the challenge as ready: {e}"));
            }
        }
    }

    // Poll until order is ready (120s timeout for DNS propagation)
    use instant_acme::RetryPolicy;
    let timeout = std::time::Duration::from_secs(120);
    let poll_result = order.poll_ready(&RetryPolicy::new().timeout(timeout)).await;

    // Always clean up TXT records
    cleanup_cf_records(&client, cf_api, cf_zone_id, &headers, &created_records).await;

    poll_result.map_err(|e| format!("{CA_DECLINED}The CA did not validate the DNS-01 challenge: {e}"))?;

    // Finalize
    let private_key_pem = order
        .finalize()
        .await
        .map_err(|e| format!("{CA_DECLINED}The CA would not finalize the order: {e}"))?;

    let cert_chain_pem = order
        .poll_certificate(&RetryPolicy::new().timeout(timeout))
        .await
        .map_err(|e| format!("{CA_DECLINED}The CA would not hand back the certificate: {e}"))?;

    // Save cert (use base domain for directory)
    let cert_dir = format!("{SSL_DIR}/{domain}");
    tokio::fs::create_dir_all(&cert_dir)
        .await
        .map_err(|e| format!("Create cert dir: {e}"))?;

    let cert_path = format!("{cert_dir}/fullchain.pem");
    let key_path = format!("{cert_dir}/privkey.pem");

    tokio::fs::write(&cert_path, &cert_chain_pem)
        .await
        .map_err(|e| format!("Write cert: {e}"))?;
    // Write the private key 0600-at-creation (no world-readable 0644 window).
    write_key_file(&key_path, &private_key_pem).await?;

    let expiry = get_cert_expiry(&cert_path).await;
    tracing::info!("SSL ({label}) provisioned for {domain}");

    Ok(CertInfo { cert_path, key_path, expiry, profile: profile_used })
}

/// Clean up Cloudflare TXT records created during DNS-01 challenge.
async fn cleanup_cf_records(
    client: &reqwest::Client,
    cf_api: &str,
    zone_id: &str,
    headers: &reqwest::header::HeaderMap,
    record_ids: &[String],
) {
    for rid in record_ids {
        match client
            .delete(&format!("{cf_api}/zones/{zone_id}/dns_records/{rid}"))
            .headers(headers.clone())
            .send()
            .await
        {
            Ok(_) => tracing::info!("DNS-01: cleaned up TXT record {rid}"),
            Err(e) => tracing::warn!("DNS-01: failed to clean up TXT {rid}: {e}"),
        }
    }
}

/// Get certificate expiry date from PEM file.
async fn get_cert_expiry(cert_path: &str) -> Option<String> {
    let pem_data = tokio::fs::read(cert_path).await.ok()?;
    let (_, pem) = x509_parser::pem::parse_x509_pem(&pem_data).ok()?;
    let cert = pem.parse_x509().ok()?;
    let not_after = cert.validity().not_after.to_datetime();
    Some(not_after.to_string())
}

/// Get SSL certificate status for a domain.
pub async fn get_cert_status(domain: &str) -> CertStatus {
    let cert_path = format!("{SSL_DIR}/{domain}/fullchain.pem");

    if !Path::new(&cert_path).exists() {
        return CertStatus {
            domain: domain.to_string(),
            has_cert: false,
            issuer: None,
            not_after: None,
            days_remaining: None,
        };
    }

    let (issuer, not_after, days_remaining) = match tokio::fs::read(&cert_path).await {
        Ok(pem_data) => {
            if let Ok((_, pem)) = x509_parser::pem::parse_x509_pem(&pem_data) {
                if let Ok(cert) = pem.parse_x509() {
                    let issuer = cert.issuer().to_string();
                    let not_after_dt = cert.validity().not_after.to_datetime();
                    let not_after_str = not_after_dt.to_string();
                    let expiry_ts = not_after_dt.unix_timestamp();
                    let now_ts = chrono::Utc::now().timestamp();
                    let days = (expiry_ts - now_ts) / 86400;
                    (Some(issuer), Some(not_after_str), Some(days))
                } else {
                    (None, None, None)
                }
            } else {
                (None, None, None)
            }
        }
        Err(_) => (None, None, None),
    };

    CertStatus {
        domain: domain.to_string(),
        has_cert: true,
        issuer,
        not_after,
        days_remaining,
    }
}

/// Every certificate this host actually holds, whether the panel issued it or not.
///
/// The diagnostics scanner has always walked this directory — that is where
/// "SSL certificate expiring soon: {domain}" comes from — but nothing else could
/// enumerate it, so the panel could raise a finding about a certificate it was
/// unable to list anywhere. An administrator was told to fix something no screen
/// could show them.
///
/// Deliberately reports what is on DISK and nothing else: whether a certificate
/// belongs to a site, who owns it, and whether it can be renewed here are all
/// questions the agent has no database to answer. It lists; the panel decides.
pub async fn list_cert_status() -> Vec<CertStatus> {
    let mut out = Vec::new();

    let mut entries = match tokio::fs::read_dir(SSL_DIR).await {
        Ok(e) => e,
        // No directory means no certificates, which is an empty list and not an
        // error — a fresh box has never issued one.
        Err(_) => return out,
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        if !entry.file_type().await.map(|ft| ft.is_dir()).unwrap_or(false) {
            continue;
        }
        let domain = entry.file_name().to_string_lossy().to_string();
        // The ACME account material lives beside the certificates and is not one.
        if domain.starts_with('.') {
            continue;
        }
        let status = get_cert_status(&domain).await;
        if status.has_cert {
            out.push(status);
        }
    }

    out.sort_by(|a, b| a.domain.cmp(&b.domain));
    out
}

/// Regenerate nginx config with SSL enabled and reload.
/// Switch a site's vhost to the HTTPS template and, if it is a WordPress site
/// still advertising plain HTTP, move its canonical URL across with it.
///
/// The two halves belong together. The HTTPS template redirects `:80` to `:443`,
/// so from the moment this returns, the stored URL is the only thing deciding
/// what the site tells visitors to use. Every path that gives a site a
/// certificate — first provision, manual retry, DNS-01, an uploaded custom cert,
/// renewal, git deploys, Docker apps — comes through here, which is why the
/// promotion lives here and not in any one of them.
pub async fn enable_ssl_for_site(
    templates: &Tera,
    domain: &str,
    site_config: &SiteConfig,
) -> Result<crate::services::wordpress::CanonicalUrlOutcome, String> {
    let ssl_config = SiteConfig {
        runtime: site_config.runtime.clone(),
        root: site_config.root.clone(),
        proxy_port: site_config.proxy_port,
        php_socket: site_config.php_socket.clone(),
        ssl: Some(true),
        ssl_cert: Some(format!("{SSL_DIR}/{domain}/fullchain.pem")),
        ssl_key: Some(format!("{SSL_DIR}/{domain}/privkey.pem")),
        rate_limit: site_config.rate_limit,
        max_upload_mb: site_config.max_upload_mb,
        php_memory_mb: site_config.php_memory_mb,
        php_max_workers: site_config.php_max_workers,
        custom_nginx: site_config.custom_nginx.clone(),
        php_preset: site_config.php_preset.clone(),
        app_command: site_config.app_command.clone(),
        fastcgi_cache: site_config.fastcgi_cache,
        redis_cache: site_config.redis_cache,
        redis_db: site_config.redis_db,
        waf_enabled: site_config.waf_enabled,
        waf_mode: site_config.waf_mode.clone(),
        csp_policy: site_config.csp_policy.clone(),
        permissions_policy: site_config.permissions_policy.clone(),
        bot_protection: site_config.bot_protection.clone(),
    };

    let rendered = nginx::render_site_config(templates, domain, &ssl_config)
        .map_err(|e| format!("Template render error: {e}"))?;

    // A site the operator took offline gets its parked body updated instead, so
    // the certificate is already in it when the site is enabled again. Writing
    // into service here is what used to bring a disabled site back the moment it
    // gained a certificate.
    let target = nginx::vhost_target(domain);
    let config_path = target.path().to_string();
    let tmp_path = format!("{config_path}.tmp");
    tokio::fs::write(&tmp_path, &rendered)
        .await
        .map_err(|e| format!("Failed to write nginx config: {e}"))?;
    tokio::fs::rename(&tmp_path, &config_path)
        .await
        .map_err(|e| format!("Failed to rename nginx config: {e}"))?;

    if !target.is_live() {
        tracing::info!(
            "Site {domain} is disabled: its parked configuration now carries the certificate, \
             and nginx was not reloaded"
        );
        // The canonical URL is left alone deliberately: nothing is being served
        // over the new certificate yet, so moving WordPress to HTTPS now would
        // point it at a name answering 503.
        return Ok(crate::services::wordpress::CanonicalUrlOutcome::Untouched);
    }

    let test_result = nginx::test_config()
        .await
        .map_err(|e| format!("Failed to test nginx: {e}"))?;

    if !test_result.success {
        // Rollback — write non-SSL config
        let fallback = nginx::render_site_config(templates, domain, site_config)
            .map_err(|e| format!("Rollback render error: {e}"))?;
        tokio::fs::write(&config_path, &fallback).await.ok();
        nginx::reload().await.ok();
        return Err(format!("SSL nginx config invalid: {}", test_result.stderr));
    }

    nginx::reload()
        .await
        .map_err(|e| format!("Nginx reload failed: {e}"))?;

    tracing::info!("Nginx updated with SSL for {domain}");

    // The certificate is live either way — a site whose canonical URL could not
    // be moved is still served, so this never fails the enable. It is reported
    // instead, because "serving HTTPS while telling everyone to use HTTP" is a
    // state somebody has to be told about.
    let canonical = crate::services::wordpress::promote_site_url_to_https(domain).await;
    match &canonical {
        crate::services::wordpress::CanonicalUrlOutcome::Promoted => {
            tracing::info!("WordPress canonical URL for {domain} moved to HTTPS");
        }
        crate::services::wordpress::CanonicalUrlOutcome::Failed(e) => {
            tracing::warn!("Could not move WordPress canonical URL for {domain} to HTTPS: {e}");
        }
        crate::services::wordpress::CanonicalUrlOutcome::Untouched => {}
    }
    Ok(canonical)
}

// ── ACME profile + ARI (RFC 9773) helpers ────────────────────────────────

/// List ACME profiles advertised in the server directory. Empty vec means
/// the CA doesn't support the profiles extension; callers should fall back
/// to the default profile.
pub fn list_profiles(account: &Account) -> Vec<ProfileInfo> {
    account
        .profiles()
        .map(|p| ProfileInfo {
            name: p.name.to_string(),
            description: p.description.to_string(),
        })
        .collect()
}

/// Fetch ACME Renewal Information (RFC 9773) for a certificate on disk.
/// Returns None when the cert can't be parsed, when the CA doesn't support
/// ARI, or on transient fetch failures — callers fall back to a static
/// threshold in that case.
pub async fn fetch_ari(account: &Account, cert_pem_path: &str) -> Option<AriSuggestion> {
    let pem_bytes = tokio::fs::read(cert_pem_path).await.ok()?;
    let cert_der = first_cert_der(&pem_bytes)?;
    let cert_der_ref = CertificateDer::from(cert_der.as_slice());
    let ident = CertificateIdentifier::try_from(&cert_der_ref).ok()?;

    match account.renewal_info(&ident).await {
        Ok((info, retry_after)) => {
            let start = offset_to_chrono(info.suggested_window.start)?;
            let end = offset_to_chrono(info.suggested_window.end)?;
            let recheck = chrono::Utc::now()
                + chrono::Duration::from_std(retry_after).unwrap_or(chrono::Duration::hours(6));
            Some(AriSuggestion {
                renewal_at: start,
                renewal_before: end,
                recheck_at: recheck,
            })
        }
        Err(e) => {
            tracing::debug!("ARI fetch failed ({cert_pem_path}): {e}");
            None
        }
    }
}

/// Build a `CertificateIdentifier` from a cert PEM. Parses the first
/// certificate in the chain (the leaf).
fn cert_identifier_from_pem(pem: &str) -> Result<CertificateIdentifier<'static>, String> {
    let der = first_cert_der(pem.as_bytes()).ok_or("no PEM certificate found")?;
    let der_ref = CertificateDer::from(der.as_slice());
    CertificateIdentifier::try_from(&der_ref)
        .map(|id| id.into_owned())
        .map_err(|e| format!("cert identifier: {e:?}"))
}

/// Return the first DER-encoded certificate from a PEM blob.
fn first_cert_der(pem_bytes: &[u8]) -> Option<Vec<u8>> {
    let (_, pem) = x509_parser::pem::parse_x509_pem(pem_bytes).ok()?;
    Some(pem.contents)
}

fn offset_to_chrono(dt: time::OffsetDateTime) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::<chrono::Utc>::from_timestamp(dt.unix_timestamp(), dt.nanosecond())
}

// ─── Registered certificates (#104) ──────────────────────────────────────

/// Where a certificate registered BY NAME lives: one directory per alias,
/// holding the same `fullchain.pem` + `privkey.pem` pair a domain directory
/// holds, referenced from a vhost by alias rather than by the domain it fronts.
///
/// A SIBLING root, deliberately not a subdirectory of the per-domain tree, and
/// the placement is the whole design:
///
/// - `list_cert_status`, the diagnostics expiry check and the security scanner
///   all walk the per-domain tree treating EVERY directory as a domain and
///   offering a renew-by-name fix for it. An alias must never be enumerated as a
///   domain, so it must not sit where those walks look.
/// - `unexpose_domain` deletes a domain's own directory when its stack is
///   removed. A registered certificate is shared across claims and has to
///   survive any one stack's removal.
/// - The per-domain renew door replaces whatever sits at the domain's path with
///   an ACME certificate. The registry is never at that path, so no renewal
///   door can reach a certificate the operator supplied — nothing renews a
///   stack's certificate today, and this keeps it that way by construction.
///
/// `/etc/dockpanel` is already in the unit's writable set, so the new root
/// needs no sandbox change.
pub const SSL_REGISTRY_DIR: &str = "/etc/dockpanel/ssl-registry";

/// What a registered certificate says about itself — the metadata the panel
/// records so a screen can show it without ever holding the PEM.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CertMetadata {
    /// The names the leaf presents, through the same SAN-then-CN rule
    /// `cert_covers_domain` uses.
    pub dns_names: Vec<String>,
    pub issuer: String,
    pub not_before: chrono::DateTime<chrono::Utc>,
    pub not_after: chrono::DateTime<chrono::Utc>,
    /// SHA-256 of the leaf's DER, lowercase hex. What `openssl x509
    /// -fingerprint -sha256` prints, minus the colons.
    pub fingerprint_sha256: String,
}

/// Is `alias` a name a registered certificate may carry?
///
/// A DNS-label shape: lowercase letters, digits and inner hyphens, 1–64 bytes.
/// It becomes a directory name on this box and is spliced into a vhost, so it
/// has to satisfy the vhost renderer's path check as well — the grammar is a
/// strict subset of that character class and can never spell `..`. Uppercase
/// is refused rather than folded: the panel stores the alias lowercase, and an
/// agent that silently folded would answer for a name the panel's row does not
/// carry.
pub fn is_valid_cert_alias(alias: &str) -> bool {
    let b = alias.as_bytes();
    if b.is_empty() || b.len() > 64 {
        return false;
    }
    let inner = |c: u8| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-';
    let edge = |c: u8| c.is_ascii_lowercase() || c.is_ascii_digit();
    edge(b[0]) && edge(b[b.len() - 1]) && b.iter().all(|&c| inner(c))
}

/// The certificate and key paths a registered alias resolves to, in that
/// order. One spelling: the upload writes here, the vhost renderer names it,
/// and the delete guard greps vhosts for it — three readers of one string.
pub fn registry_paths(alias: &str) -> (String, String) {
    (
        format!("{SSL_REGISTRY_DIR}/{alias}/fullchain.pem"),
        format!("{SSL_REGISTRY_DIR}/{alias}/privkey.pem"),
    )
}

/// Read the FIRST certificate block of a PEM and describe it.
///
/// The first block is the leaf — the one nginx binds — for the reason
/// `cert_covers_domain` documents at length, and the names come from the same
/// helper so a registered certificate and an uploaded one can never disagree
/// about what a certificate presents. A non-certificate block is refused with
/// the same sentence family: a pasted key is the ordinary slip, and it would
/// otherwise land in a 0644 file.
pub fn cert_metadata(cert_pem: &str) -> Result<CertMetadata, String> {
    use sha2::Digest;

    if !cert_pem.contains("-----BEGIN") {
        return Err("no certificate was found in the certificate field.".to_string());
    }
    // The first PEM block, whatever it is labelled — judged below, because a
    // key pasted here is the ordinary slip and deserves its sentence.
    let (_, block) = x509_parser::pem::parse_x509_pem(cert_pem.as_bytes())
        .map_err(|e| format!("the certificate could not be read: {e}"))?;
    match block.label.as_str() {
        "CERTIFICATE" => {}
        other => {
            return Err(format!(
                "the certificate field contains a {} block. Paste only the certificate here, with \
                 its chain if it has one — the private key belongs in its own field, where it is \
                 stored with the permissions a key needs.",
                other.to_ascii_lowercase()
            ));
        }
    }
    let (_, cert) = x509_parser::parse_x509_certificate(&block.contents)
        .map_err(|e| format!("the certificate could not be read: {e}"))?;
    let validity = cert.validity();
    let not_before = offset_to_chrono(validity.not_before.to_datetime())
        .ok_or_else(|| "the certificate's validity start is not a date".to_string())?;
    let not_after = offset_to_chrono(validity.not_after.to_datetime())
        .ok_or_else(|| "the certificate's expiry is not a date".to_string())?;
    Ok(CertMetadata {
        dns_names: presented_dns_names(&cert),
        issuer: cert.issuer().to_string(),
        not_before,
        not_after,
        fingerprint_sha256: hex::encode(sha2::Sha256::digest(&block.contents)),
    })
}

/// Does this private key belong to this certificate?
///
/// nginx answers this question only at `nginx -t`, after both files are on
/// disk, and for a REGISTERED certificate there may be no vhost to test yet —
/// so a mismatched pair would sit in the registry looking healthy until the
/// first claim reloaded nginx into a refusal. Asked here instead, before
/// anything is written, by comparing the key's public half against the leaf's
/// SubjectPublicKeyInfo — the same comparison rustls makes before it will serve
/// a pair. The crypto provider this goes through is the one the binary already
/// installs at start-up; nothing new is linked.
pub fn key_matches_cert(cert_pem: &str, key_pem: &str) -> Result<(), String> {
    let chain: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_pem.as_bytes())
        .collect::<Result<_, _>>()
        .map_err(|e| format!("the certificate could not be read: {e}"))?;
    if chain.is_empty() {
        return Err("no certificate was found in the certificate field.".to_string());
    }
    let key = rustls_pemfile::private_key(&mut key_pem.as_bytes())
        .map_err(|e| format!("the private key could not be read: {e}"))?
        .ok_or_else(|| {
            "no private key was found in the private key field. Paste the PEM-encoded key \
             (a PRIVATE KEY, RSA PRIVATE KEY or EC PRIVATE KEY block) that was generated with \
             this certificate."
                .to_string()
        })?;
    let signer = rustls::crypto::aws_lc_rs::sign::any_supported_type(&key)
        .map_err(|e| format!("the private key is not a kind nginx can serve with: {e}"))?;
    match rustls::sign::CertifiedKey::new(chain, signer).keys_match() {
        Ok(()) => Ok(()),
        Err(rustls::Error::InconsistentKeys(rustls::InconsistentKeys::KeyMismatch)) => {
            Err("the private key does not belong to this certificate".to_string())
        }
        // The signer could not produce its public half, so nothing was compared —
        // a different sentence from a mismatch, which is a fact about the pair.
        Err(rustls::Error::InconsistentKeys(_)) => Err(
            "the private key could not be compared against the certificate; it is not a kind \
             nginx can serve with"
                .to_string(),
        ),
        Err(e) => Err(format!("the certificate could not be read: {e}")),
    }
}

// ⚠ These test modules must stay the LAST thing in this file. The pin suites
// blank from the first `#[cfg(test)]` to EOF and never resume, so a test module
// placed mid-file hides every production line below it from every `prod_*` arm
// in every suite that reads this file. Adding a module is fine; adding
// PRODUCTION code below one is not.
#[cfg(test)]
mod cert_coverage_tests {
    use super::*;
    use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair};

    /// A self-signed certificate presenting `sans` as subjectAltName dNSNames.
    fn cert_with_sans(sans: &[&str]) -> String {
        let key = KeyPair::generate().unwrap();
        let params =
            CertificateParams::new(sans.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap();
        params.self_signed(&key).unwrap().pem()
    }

    /// A certificate carrying a Common Name and NO subjectAltName extension —
    /// legal, deprecated, and still issued by corporate PKI.
    fn cert_with_cn_only(cn: &str) -> String {
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.distinguished_name.push(DnType::CommonName, cn);
        params.self_signed(&key).unwrap().pem()
    }

    #[test]
    fn an_exact_name_is_accepted_case_and_trailing_dot_insensitively() {
        let pem = cert_with_sans(&["Shop.Example.com"]);
        assert!(cert_covers_domain(&pem, "shop.example.com").is_ok());
        assert!(cert_covers_domain(&pem, "SHOP.EXAMPLE.COM.").is_ok());
    }

    #[test]
    fn a_certificate_for_another_site_is_refused_and_names_what_it_covers() {
        let pem = cert_with_sans(&["www.example.com", "example.com"]);
        let err = cert_covers_domain(&pem, "shop.example.com").unwrap_err();
        assert!(err.contains("www.example.com"), "{err}");
        assert!(err.contains("shop.example.com"), "{err}");
    }

    #[test]
    fn a_wildcard_covers_exactly_one_label() {
        let pem = cert_with_sans(&["*.example.com", "example.com"]);
        // one label below the wildcard base — covered
        assert!(cert_covers_domain(&pem, "app.example.com").is_ok());
        // the apex — covered only because this cert also carries the apex SAN
        assert!(cert_covers_domain(&pem, "example.com").is_ok());
        // TWO labels below — NOT covered. This is the half a permissive
        // `ends_with` rule gets wrong, and it is the shape that makes a panel
        // report TLS the browser rejects.
        assert!(cert_covers_domain(&pem, "app.staging.example.com").is_err());
    }

    #[test]
    fn a_bare_wildcard_apex_does_not_cover_the_apex() {
        let pem = cert_with_sans(&["*.example.com"]);
        assert!(cert_covers_domain(&pem, "example.com").is_err());
    }

    #[test]
    fn a_partial_wildcard_label_is_not_a_wildcard() {
        assert!(!dns_name_covers("w*.example.com", "www.example.com"));
        assert!(!dns_name_covers("*.com", "example.com"));
    }

    #[test]
    fn the_common_name_is_a_fallback_only_when_no_san_is_present() {
        let cn_only = cert_with_cn_only("legacy.example.com");
        assert!(cert_covers_domain(&cn_only, "legacy.example.com").is_ok());
        assert!(cert_covers_domain(&cn_only, "other.example.com").is_err());

        // With a SAN present the CN is ignored, exactly as browsers do.
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(vec!["real.example.com".to_string()]).unwrap();
        params
            .distinguished_name
            .push(DnType::CommonName, "cn.example.com");
        let pem = params.self_signed(&key).unwrap().pem();
        assert!(cert_covers_domain(&pem, "real.example.com").is_ok());
        assert!(cert_covers_domain(&pem, "cn.example.com").is_err());
    }

    #[test]
    fn a_self_signed_certificate_marked_ca_is_still_accepted() {
        // `openssl req -x509` stamps CA:TRUE by default, so an `is_ca()` leaf
        // filter would refuse every self-signed staging certificate. rcgen
        // defaults to NoCa, so this fixture sets it BY HAND — without that, a
        // suite of rcgen defaults stays green with the wrong filter restored.
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(vec!["staging.example.com".to_string()]).unwrap();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let pem = params.self_signed(&key).unwrap().pem();
        assert!(cert_covers_domain(&pem, "staging.example.com").is_ok());
    }

    #[test]
    fn only_the_first_certificate_block_is_asked() {
        // nginx binds the FIRST certificate in the file as the leaf. Measured
        // against nginx 1.24.0: `[wrong, right]` + the RIGHT key fails
        // `nginx -t` with a key mismatch, naming the right key — proof it bound
        // `wrong`. Paired with the WRONG key it passes and serves `wrong`. So a
        // rule that accepted the bundle because SOME block covered the domain
        // would wave through the exact mismatch this function exists to stop.
        let wrong = cert_with_sans(&["wrong.example.com"]);
        let right = cert_with_sans(&["right.example.com"]);
        let bundle = format!("{wrong}{right}");
        assert!(cert_covers_domain(&bundle, "right.example.com").is_err());
        // The same two blocks the other way round is a normal leaf+chain paste.
        let bundle = format!("{right}{wrong}");
        assert!(cert_covers_domain(&bundle, "right.example.com").is_ok());
    }

    #[test]
    fn a_private_key_in_the_certificate_field_is_refused() {
        // The old `contains("BEGIN CERTIFICATE")` check accepted this. Driven on
        // a fresh box: the key landed in `fullchain.pem` at 0644 beside a key
        // file deliberately written 0600 — reachable by nobody else only because
        // the enclosing directories are 0700, which is the directory's doing and
        // not the write's. The sharper half is that it blinded the expiry
        // ladder: the status reader decodes the first PEM block whatever its
        // label and gets no date out of a key.
        let key = KeyPair::generate().unwrap();
        let params = CertificateParams::new(vec!["shop.example.com".to_string()]).unwrap();
        let cert = params.self_signed(&key).unwrap().pem();
        let pasted = format!("{}{}", key.serialize_pem(), cert);
        let err = cert_covers_domain(&pasted, "shop.example.com").unwrap_err();
        assert!(err.contains("private key"), "{err}");
    }

    #[test]
    fn unreadable_input_is_refused_rather_than_installed() {
        assert!(cert_covers_domain("not a certificate at all", "shop.example.com").is_err());
        assert!(
            cert_covers_domain(
                "-----BEGIN CERTIFICATE-----\nnot base64\n-----END CERTIFICATE-----\n",
                "shop.example.com"
            )
            .is_err()
        );
    }
}

#[cfg(test)]
mod acme_account_persist_tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("dp-acme-{tag}-{}", uuid::Uuid::new_v4()))
    }

    /// The defect, reproduced directly: many doors reach for the account at
    /// once on a fresh box, and each of them has DIFFERENT credentials to
    /// write, because each minted its own account at the CA.
    ///
    /// Exactly one may win. Before `create_new`, every one of them reported
    /// success and the last write decided which account survived — leaving the
    /// certificates issued under the others permanently unrenewable.
    #[tokio::test]
    async fn exactly_one_of_many_concurrent_writers_wins() {
        let dir = scratch("race");
        let path = dir.join("acme-account.json");
        let path = path.to_str().unwrap().to_string();

        let mut set = tokio::task::JoinSet::new();
        for i in 0..12 {
            let path = path.clone();
            set.spawn(async move {
                let creds = format!("{{\"account\":{i}}}");
                (i, persist_account_credentials(&path, &creds).await.unwrap())
            });
        }

        let mut written = Vec::new();
        let mut adopted = 0;
        while let Some(r) = set.join_next().await {
            let (i, outcome) = r.unwrap();
            match outcome {
                Persisted::Written => written.push(i),
                Persisted::Adopted => adopted += 1,
            }
        }

        assert_eq!(written.len(), 1, "exactly one writer may win, got {written:?}");
        assert_eq!(adopted, 11, "every other writer must be told to adopt");

        // …and the file holds the WINNER's bytes. Not a loser's, and not a mix:
        // the winner is the account whose key every later certificate is bound
        // to, so "somebody won" is not enough — it has to be the one that was
        // told it won.
        let on_disk = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(on_disk, format!("{{\"account\":{}}}", written[0]));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn an_existing_account_is_adopted_never_overwritten() {
        let dir = scratch("adopt");
        let path = dir.join("acme-account.json");
        let path = path.to_str().unwrap().to_string();

        assert_eq!(
            persist_account_credentials(&path, "{\"account\":\"first\"}").await.unwrap(),
            Persisted::Written
        );
        assert_eq!(
            persist_account_credentials(&path, "{\"account\":\"second\"}").await.unwrap(),
            Persisted::Adopted
        );
        assert_eq!(
            tokio::fs::read_to_string(&path).await.unwrap(),
            "{\"account\":\"first\"}",
            "the incumbent account must survive a later writer"
        );

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    /// The account key can order and revoke every certificate on this box, so
    /// it must never exist even briefly at the default mode.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_account_file_is_owner_only_from_creation() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("mode");
        let path = dir.join("acme-account.json");
        let path = path.to_str().unwrap().to_string();

        persist_account_credentials(&path, "{}").await.unwrap();
        let mode = tokio::fs::metadata(&path).await.unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "got {mode:o}");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}

#[cfg(test)]
mod registered_certificate_tests {
    use super::*;
    use rcgen::{CertificateParams, KeyPair};

    /// A self-signed leaf for `sans` and the key it was signed with, both PEM.
    fn pair(sans: &[&str]) -> (String, String) {
        let key = KeyPair::generate().unwrap();
        let params =
            CertificateParams::new(sans.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap();
        let cert = params.self_signed(&key).unwrap().pem();
        (cert, key.serialize_pem())
    }

    #[test]
    fn the_alias_grammar_is_a_dns_label() {
        for good in ["a", "wildcard-2026", "0", "x".repeat(64).as_str()] {
            assert!(is_valid_cert_alias(good), "{good} should be accepted");
        }
        for bad in [
            "",
            "-lead",
            "trail-",
            "Upper",
            "dot.inside",
            "under_score",
            "../etc",
            "sp ace",
            "x".repeat(65).as_str(),
        ] {
            assert!(!is_valid_cert_alias(bad), "{bad:?} should be refused");
        }
        // The vhost renderer's own path check must accept every path an alias
        // can produce — a name that passed here and was refused there would be
        // a certificate that registers but can never be served.
        let (c, k) = registry_paths("wildcard-2026");
        assert!(c.starts_with(SSL_REGISTRY_DIR) && c.ends_with("/fullchain.pem"));
        assert!(k.starts_with(SSL_REGISTRY_DIR) && k.ends_with("/privkey.pem"));
        assert!(!SSL_REGISTRY_DIR.starts_with("/etc/dockpanel/ssl/"));
    }

    #[test]
    fn metadata_describes_the_leaf() {
        let (cert, _) = pair(&["Shop.Example.com", "*.example.com"]);
        let m = cert_metadata(&cert).unwrap();
        assert_eq!(m.dns_names, vec!["shop.example.com", "*.example.com"]);
        assert!(m.not_before < m.not_after);
        assert_eq!(m.fingerprint_sha256.len(), 64);
        assert!(m.fingerprint_sha256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert!(!m.issuer.is_empty());
        // Two different certificates never share a fingerprint.
        let (other, _) = pair(&["shop.example.com"]);
        assert_ne!(cert_metadata(&other).unwrap().fingerprint_sha256, m.fingerprint_sha256);
    }

    #[test]
    fn metadata_refuses_a_key_where_a_certificate_belongs() {
        let (cert, key) = pair(&["a.example.com"]);
        let err = cert_metadata(&format!("{key}{cert}")).unwrap_err();
        assert!(err.contains("private key block"), "{err}");
        assert!(cert_metadata("").unwrap_err().contains("no certificate was found"));
        assert!(cert_metadata("not pem at all").unwrap_err().contains("no certificate was found"));
    }

    #[test]
    fn a_generated_pair_matches_and_a_second_key_does_not() {
        let (cert, key) = pair(&["a.example.com"]);
        assert_eq!(key_matches_cert(&cert, &key), Ok(()));
        // The certificate with SOMEBODY ELSE's key — the pair nginx would refuse
        // at the first reload after the claim.
        let (_, stranger) = pair(&["a.example.com"]);
        assert_eq!(
            key_matches_cert(&cert, &stranger),
            Err("the private key does not belong to this certificate".to_string())
        );
        // A chain is judged by its FIRST block, like everything else here.
        let (issuer_cert, _) = pair(&["ca.example.com"]);
        assert_eq!(key_matches_cert(&format!("{cert}{issuer_cert}"), &key), Ok(()));
        assert!(key_matches_cert(&format!("{issuer_cert}{cert}"), &key).is_err());
    }

    #[test]
    fn key_matching_refuses_what_it_cannot_read() {
        let (cert, key) = pair(&["a.example.com"]);
        assert!(key_matches_cert("", &key).unwrap_err().contains("no certificate"));
        assert!(key_matches_cert(&cert, "").unwrap_err().contains("no private key"));
        assert!(key_matches_cert(&cert, &cert).unwrap_err().contains("no private key"));
    }
}
