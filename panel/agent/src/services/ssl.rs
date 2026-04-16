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

/// Load existing ACME account or create a new one.
pub async fn load_or_create_account(email: &str) -> Result<Account, String> {
    if Path::new(ACME_ACCOUNT_PATH).exists() {
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
        tracing::info!("Loaded existing ACME account");
        Ok(account)
    } else {
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

        // Save credentials
        let json = serde_json::to_string_pretty(&creds)
            .map_err(|e| format!("Failed to serialize ACME creds: {e}"))?;
        if let Some(parent) = Path::new(ACME_ACCOUNT_PATH).parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        tokio::fs::write(ACME_ACCOUNT_PATH, json)
            .await
            .map_err(|e| format!("Failed to save ACME account: {e}"))?;

        // Restrict ACME account key permissions to owner-only (0o600)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) = tokio::fs::set_permissions(
                ACME_ACCOUNT_PATH,
                std::fs::Permissions::from_mode(0o600),
            ).await {
                tracing::error!("Failed to set ACME account key permissions: {e}");
            }
        }

        tracing::info!("Created new ACME account for {email}");
        Ok(account)
    }
}

/// Provision a Let's Encrypt certificate for a domain using HTTP-01 challenge.
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
        .map_err(|e| format!("Failed to create ACME order: {e}"))?;

    let state = order.state();
    let needs_challenge = matches!(state.status, OrderStatus::Pending);

    if !needs_challenge && !matches!(state.status, OrderStatus::Ready) {
        return Err(format!("Unexpected order status: {:?}", state.status));
    }

    if needs_challenge {
        // Get authorizations and solve HTTP-01 challenge
        let mut authorizations = order.authorizations();
        while let Some(result) = authorizations.next().await {
            let mut authz = result.map_err(|e| format!("Failed to get authorization: {e}"))?;

            match authz.status {
                AuthorizationStatus::Valid => continue,
                AuthorizationStatus::Pending => {}
                status => return Err(format!("Unexpected authorization status: {status:?}")),
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
                .map_err(|e| format!("Failed to set challenge ready: {e}"))?;
        }
    }

    // Poll until order is ready for finalization
    use instant_acme::RetryPolicy;
    let timeout = std::time::Duration::from_secs(60);

    order
        .poll_ready(&RetryPolicy::new().timeout(timeout))
        .await
        .map_err(|e| format!("Order not ready: {e}"))?;

    // Finalize — generates CSR internally and returns private key PEM
    let private_key_pem = order
        .finalize()
        .await
        .map_err(|e| format!("Failed to finalize order: {e}"))?;

    // Poll for certificate
    let cert_chain_pem = order
        .poll_certificate(&RetryPolicy::new().timeout(timeout))
        .await
        .map_err(|e| format!("Failed to get certificate: {e}"))?;

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
    tokio::fs::write(&key_path, &private_key_pem)
        .await
        .map_err(|e| format!("Failed to write key: {e}"))?;

    // Restrict key permissions
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = tokio::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).await {
            tracing::error!("Failed to set key file permissions for {}: {}", domain, e);
        }
    }

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
        .map_err(|e| format!("ACME order: {e}"))?;

    let state = order.state();
    if !matches!(state.status, OrderStatus::Pending | OrderStatus::Ready) {
        return Err(format!("Unexpected order status: {:?}", state.status));
    }

    // Build Cloudflare client
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("HTTP client: {e}"))?;
    let cf_api = "https://api.cloudflare.com/client/v4";
    let mut headers = reqwest::header::HeaderMap::new();
    if let Some(email) = cf_api_email {
        headers.insert("X-Auth-Email", email.parse().map_err(|_| "Invalid CF email")?);
        headers.insert("X-Auth-Key", cf_api_token.parse().map_err(|_| "Invalid CF token")?);
    } else {
        headers.insert(
            "Authorization",
            format!("Bearer {cf_api_token}").parse().map_err(|_| "Invalid CF token")?,
        );
    }

    let mut created_records: Vec<String> = Vec::new();

    if matches!(state.status, OrderStatus::Pending) {
        let mut authorizations = order.authorizations();
        while let Some(result) = authorizations.next().await {
            let mut authz = result.map_err(|e| format!("Authorization: {e}"))?;

            match authz.status {
                AuthorizationStatus::Valid => continue,
                AuthorizationStatus::Pending => {}
                status => {
                    cleanup_cf_records(&client, cf_api, cf_zone_id, &headers, &created_records).await;
                    return Err(format!("Auth status: {status:?}"));
                }
            }

            let mut challenge = authz
                .challenge(ChallengeType::Dns01)
                .ok_or_else(|| {
                    "No DNS-01 challenge (Let's Encrypt may require HTTP-01 for this domain)".to_string()
                })?;

            let key_auth = challenge.key_authorization();
            let txt_value = key_auth.dns_value();

            // Create TXT record: _acme-challenge.{domain}
            let record_name = format!("_acme-challenge.{domain}");
            tracing::info!("DNS-01: creating TXT {record_name} = {txt_value}");

            let resp = client
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
                .map_err(|e| format!("CF create TXT: {e}"))?;

            let resp_json: serde_json::Value = resp.json().await
                .map_err(|e| format!("CF parse: {e}"))?;

            if resp_json.get("success").and_then(|v| v.as_bool()) != Some(true) {
                cleanup_cf_records(&client, cf_api, cf_zone_id, &headers, &created_records).await;
                let errs = resp_json.get("errors").cloned().unwrap_or_default();
                return Err(format!("CF TXT create failed: {errs}"));
            }

            if let Some(rid) = resp_json.pointer("/result/id").and_then(|v| v.as_str()) {
                created_records.push(rid.to_string());
            }

            // Wait for DNS propagation (Cloudflare is fast, but ACME servers cache)
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;

            if let Err(e) = challenge.set_ready().await {
                cleanup_cf_records(&client, cf_api, cf_zone_id, &headers, &created_records).await;
                return Err(format!("Challenge ready: {e}"));
            }
        }
    }

    // Poll until order is ready (120s timeout for DNS propagation)
    use instant_acme::RetryPolicy;
    let timeout = std::time::Duration::from_secs(120);
    let poll_result = order.poll_ready(&RetryPolicy::new().timeout(timeout)).await;

    // Always clean up TXT records
    cleanup_cf_records(&client, cf_api, cf_zone_id, &headers, &created_records).await;

    poll_result.map_err(|e| format!("Order not ready: {e}"))?;

    // Finalize
    let private_key_pem = order
        .finalize()
        .await
        .map_err(|e| format!("Finalize: {e}"))?;

    let cert_chain_pem = order
        .poll_certificate(&RetryPolicy::new().timeout(timeout))
        .await
        .map_err(|e| format!("Certificate fetch: {e}"))?;

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
    tokio::fs::write(&key_path, &private_key_pem)
        .await
        .map_err(|e| format!("Write key: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = tokio::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).await;
    }

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

/// Regenerate nginx config with SSL enabled and reload.
pub async fn enable_ssl_for_site(
    templates: &Tera,
    domain: &str,
    site_config: &SiteConfig,
) -> Result<(), String> {
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

    let config_path = format!("/etc/nginx/sites-enabled/{domain}.conf");
    let tmp_path = format!("{config_path}.tmp");
    tokio::fs::write(&tmp_path, &rendered)
        .await
        .map_err(|e| format!("Failed to write nginx config: {e}"))?;
    tokio::fs::rename(&tmp_path, &config_path)
        .await
        .map_err(|e| format!("Failed to rename nginx config: {e}"))?;

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
    Ok(())
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
