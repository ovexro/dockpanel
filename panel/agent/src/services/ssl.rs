use instant_acme::{
    Account, AccountCredentials, AuthorizationStatus, ChallengeType, Identifier, LetsEncrypt,
    NewAccount, NewOrder, OrderStatus,
};
use std::path::Path;
use tera::Tera;

use crate::routes::nginx::SiteConfig;
use crate::services::nginx;

const ACME_ACCOUNT_PATH: &str = "/etc/dockpanel/ssl/acme-account.json";
const SSL_DIR: &str = "/etc/dockpanel/ssl";
const ACME_WEBROOT: &str = "/var/www/acme";

#[derive(serde::Serialize)]
pub struct CertInfo {
    pub cert_path: String,
    pub key_path: String,
    pub expiry: Option<String>,
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

        tracing::info!("Created new ACME account for {email}");
        Ok(account)
    }
}

/// Provision a Let's Encrypt certificate for a domain using HTTP-01 challenge.
pub async fn provision_cert(account: &Account, domain: &str) -> Result<CertInfo, String> {
    tracing::info!("Provisioning SSL for {domain}");

    // Create order
    let identifier = Identifier::Dns(domain.to_string());
    let mut order = account
        .new_order(&NewOrder::new(&[identifier]))
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
        tokio::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))
            .await
            .ok();
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
    })
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
