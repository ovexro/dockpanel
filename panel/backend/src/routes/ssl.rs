use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::{AdminUser, AuthUser, ServerScope};
use crate::error::{acme_order_failure, dns_provider_failure, internal_error, err, agent_error, ApiError};
use crate::models::Site;
use crate::AppState;
use crate::services::activity;

/// Answer a failed certificate order with the reason, and everything else the
/// way it was answered before.
///
/// s386 gave the operator a sentence for a certificate this product must not
/// touch. It left the commoner failure untouched: a domain that IS ours but
/// cannot answer the challenge from this box — it points somewhere else, port 80
/// is closed, the apex of a wildcard lives elsewhere — where the agent runs a
/// real order, loses it, and the operator gets a reference number after two
/// minutes of spinner. The reference described nothing; the agent had already
/// said exactly what went wrong.
///
/// Both doors share this, and provisioning is the one that fires more often —
/// the first certificate for a domain that was never pointed here at all. Fixing
/// only the one the register named would have left the busier half saying
/// nothing, and the two are one line apart.
///
/// A 422 rather than a 4xx passthrough because nothing about the REQUEST was
/// malformed: the panel asked correctly and the world declined.
fn acme_failure_or(
    domain: &str,
    context: &str,
    e: crate::services::agent::AgentError,
) -> ApiError {
    if let Some(reason) = acme_order_failure(&e) {
        tracing::warn!(domain = %domain, reason = %reason, "{context} was declined");
        // ⚠ Says what happened and nothing about WHY. A newer agent only labels
        // what the certificate authority itself said, but the compatibility path
        // for an agent nobody has updated recognises a renewal by its wrapper
        // alone, and that wrapper is put on local faults too. So this sentence has
        // to be true when the CA declined a challenge, when it refused a rate-
        // limited order, and when the agent's own disk filled up — which means it
        // may not name a cause, and must never say "try again": on a rate limit
        // that is the one instruction that makes it worse. The hint below is
        // conditional for the same reason, and describes the commonest case
        // without asserting it.
        let reason = reason.trim_end_matches(['.', ' ']);
        return err(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!(
                "A certificate for {domain} could not be issued: {reason}. If that is a \
                 validation failure, check that {domain} resolves to this server and that \
                 port 80 is reachable from the internet."
            ),
        );
    }
    agent_error(context, e)
}

/// Answer a failed DNS-01 order with the reason, and everything else the way it
/// was answered before.
///
/// The THIRD ACME door, and it must not borrow the sentence the other two share.
/// That one offers "check that the domain resolves here and that port 80 is
/// reachable" — advice this door's own button contradicts, because an operator
/// chooses DNS-01 precisely WHEN port 80 cannot be reached. Repeating it here
/// would send somebody to open a port that has nothing to do with the failure.
///
/// Three parties can fail on this door and the panel keeps them apart, most
/// specific first. The DNS provider refusing to publish `_acme-challenge` is the
/// commonest real failure and almost always a token missing `DNS:Edit`; the CA
/// declining is the same class the sibling handles; anything unlabelled is this
/// machine's fault and keeps its incident id.
///
/// ⚠ Both hints are CONDITIONAL and neither says "try again". A declined order
/// arrives here for a rate limit too, and on a rate limit "try again" is the one
/// instruction that makes it worse (#663).
fn dns01_failure_or(
    ordered: &str,
    zone: &str,
    wildcard: bool,
    e: crate::services::agent::AgentError,
) -> ApiError {
    // What was actually ORDERED, which on a wildcard is not the site the operator
    // is looking at: the order is placed against the ZONE apex. Naming the site
    // here would describe a certificate nobody asked for.
    let subject = if wildcard {
        format!("*.{ordered}")
    } else {
        ordered.to_string()
    };

    if let Some(reason) = dns_provider_failure(&e) {
        tracing::warn!(domain = %ordered, reason = %reason, "DNS-01 SSL was refused by the DNS provider");
        let reason = reason.trim_end_matches(['.', ' ']);
        return err(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!(
                "The DNS-01 challenge for {subject} could not be completed: {reason}. If \
                 Cloudflare rejected the change, check that the credentials for the {zone} \
                 zone may edit its DNS records — a scoped token needs the DNS:Edit permission."
            ),
        );
    }

    if let Some(reason) = acme_order_failure(&e) {
        tracing::warn!(domain = %ordered, reason = %reason, "DNS-01 SSL was declined");
        let reason = reason.trim_end_matches(['.', ' ']);
        return err(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!(
                "A certificate for {subject} could not be issued: {reason}. If that is a \
                 validation failure, check that the _acme-challenge TXT record for {subject} \
                 can be published in the {zone} zone."
            ),
        );
    }

    agent_error("DNS-01 SSL", e)
}


/// After an SSL provision/renew, re-render the FULL nginx vhost from the site's
/// current DB config. The agent's SSL provision/renew only renders a SUBSET
/// (WAF / CSP / Permissions-Policy / rate-limit / custom_nginx / bot-protection
/// all default off), so without this a renewal silently strips a hardened site's
/// security directives. `build_nginx_body` is the same canonical builder every
/// other config-rebuild path uses. Best-effort: a failure leaves the site on the
/// agent's (functional, SSL-enabled) subset config and is logged.
/// PUT the site's complete vhost body. The single writer behind all three
/// rebuild entry points below, so a door can never re-render a *partial* vhost
/// by picking the wrong helper.
async fn put_full_vhost(agent: &crate::services::agent::AgentHandle, site: &Site) {
    if let Err(e) = agent
        .put(
            &format!("/nginx/sites/{}", site.domain),
            crate::routes::sites::build_nginx_body(site),
        )
        .await
    {
        tracing::warn!("Full vhost rebuild after SSL op failed for {}: {e}", site.domain);
    }
}

/// Re-render a site's full vhost after an SSL write that only knew the certificate.
///
/// ⛔ The agent's `/ssl/provision*` and `/ssl/{domain}/renew` routes do NOT patch a
/// vhost. `services::ssl::enable_ssl_for_site` RE-RENDERS it from the `SiteConfig`
/// the caller handed in, and `routes/ssl.rs::provision` hardcodes every limit and
/// every hardening field to `None`. So an SSL write from a door that does not hold
/// the site's row publishes a vhost with no rate limit, no upload cap, no custom
/// nginx and no WAF/CSP — and, when that door also *guesses* the runtime, a PHP
/// site re-rendered as a static one, which answers 403 to every PHP request while
/// the panel still shows `runtime = php`.
///
/// Measured on a box at s398: adding a mail domain for a name that also had a
/// website converted that website to static and dropped its `limit_req_zone`,
/// while an identical site on the same host with no mail domain was untouched.
///
/// ⚠ Resolved by (domain, server_id), NEVER by domain alone: `sites.domain` is
/// unique only per server (`idx_sites_domain_server`), so a name-only lookup on a
/// fleet can hand back another host's row — the defect `security_scanner` already
/// documents for its own sibling read.
///
/// A domain with no site row is not an error: a mail domain need not be a site,
/// and there is then no vhost of ours to put back.
pub(crate) async fn rebuild_vhost_for_domain(
    pool: &sqlx::PgPool,
    agent: &crate::services::agent::AgentHandle,
    domain: &str,
    server_id: Uuid,
) {
    match sqlx::query_as::<_, Site>("SELECT * FROM sites WHERE domain = $1 AND server_id = $2")
        .bind(domain)
        .bind(server_id)
        .fetch_optional(pool)
        .await
    {
        Ok(Some(site)) => {
            put_full_vhost(agent, &site).await;
            tracing::info!(
                "Rebuilt {domain}'s vhost after an SSL write that did not carry the site's settings"
            );
        }
        Ok(None) => {}
        Err(e) => tracing::warn!("Vhost rebuild after SSL write: could not load a site for {domain}: {e}"),
    }
}

/// The same compensation as the `AppState`-based sibling below, for a caller that
/// holds the site id and a pool but no `AppState` — the auto-SSL task spawned by
/// site creation. Deliberately does NOT promote the canonical URL: that door's own
/// success path already handles it.
///
/// ⚠ The sibling's name is deliberately NOT spelled anywhere in this file's prose:
/// `sibling-parity-pin-e2e.sh` §B2 counts raw occurrences of it across the crate,
/// so a comment naming it would prop that count up and mask a real call site being
/// deleted. Pins grep source, and comments are source.
pub(crate) async fn rebuild_vhost_for_site(
    pool: &sqlx::PgPool,
    agent: &crate::services::agent::AgentHandle,
    site_id: Uuid,
) {
    match sqlx::query_as::<_, Site>("SELECT * FROM sites WHERE id = $1")
        .bind(site_id)
        .fetch_optional(pool)
        .await
    {
        Ok(Some(site)) => put_full_vhost(agent, &site).await,
        Ok(None) => {}
        Err(e) => tracing::warn!("Vhost rebuild after SSL op: could not load site {site_id}: {e}"),
    }
}

pub(crate) async fn rebuild_vhost_after_ssl(
    state: &AppState,
    agent: &crate::services::agent::AgentHandle,
    site_id: Uuid,
) {
    match sqlx::query_as::<_, Site>("SELECT * FROM sites WHERE id = $1")
        .bind(site_id)
        .fetch_one(&state.db)
        .await
    {
        Ok(site) => {
            put_full_vhost(agent, &site).await;

            // This rebuild is the SECOND way a vhost can turn into an HTTPS one.
            // The agent promotes a WordPress site's canonical URL inside
            // `enable_ssl_for_site`, but a WILDCARD certificate never goes
            // through it — the agent leaves those to be applied per-site from
            // here. Without this the site would serve HTTPS, redirect to it, and
            // go on telling every visitor to use HTTP. Idempotent: the agent
            // reports `untouched` when there is nothing to move.
            if site.ssl_enabled {
                if let Err(e) = agent
                    .post(&format!("/wordpress/{}/promote-https", site.domain), None)
                    .await
                {
                    tracing::warn!(
                        "Canonical URL promotion after SSL op failed for {}: {e}",
                        site.domain
                    );
                }
            }
        }
        Err(e) => tracing::warn!("Vhost rebuild after SSL op: could not load site {site_id}: {e}"),
    }
}

#[derive(Deserialize, Default)]
pub struct ProvisionQuery {
    /// Optional ACME profile override ("classic" / "tlsserver" / "shortlived").
    /// When omitted, falls back to the `acme_default_profile` setting,
    /// which itself defaults to "classic".
    #[serde(default)]
    pub profile: Option<String>,
    /// The operator's explicit intent to replace a certificate this product
    /// did not issue, after seeing the refusal that names its issuer. Mirrors
    /// the CLI's `--force` and, like it, is the only way past the agent
    /// writer's foreign-certificate refusal — never a default, never inferred.
    #[serde(default)]
    pub force: bool,
}

/// POST /api/sites/{id}/ssl — Provision SSL certificate for a site.
pub async fn provision(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Query(q): Query<ProvisionQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: Site = sqlx::query_as(&format!("SELECT s.* FROM sites s WHERE {}", crate::helpers::SITE_CALLER_PREDICATE))
        .bind(id)
        .bind(claims.sub)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("provision", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    // Issuing writes a certificate and rewrites the vhost for this domain on
    // whichever host the handle points at. The predicate above already answered
    // WHICH SITE from the row; the host has to come from the same place.
    let agent =
        crate::helpers::agent_for_site_server(&state, site.server_id, &site.domain).await?;

    if site.status != "active" {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Site must be active before provisioning SSL",
        ));
    }

    if site.ssl_enabled {
        return Err(err(StatusCode::CONFLICT, "SSL is already enabled"));
    }

    // Per-user ACME rate limiting: max 10 certificates per hour
    let (recent_count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM sites WHERE user_id = $1 AND ssl_enabled = true \
         AND updated_at > NOW() - INTERVAL '1 hour'",
    )
    .bind(claims.sub)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("provision rate check", e))?;

    if recent_count >= 10 {
        return Err(err(
            StatusCode::TOO_MANY_REQUESTS,
            "Rate limit: max 10 SSL certificates per hour. Try again later.",
        ));
    }

    // DNS pre-flight, via the shared prerequisite checker — the SAME call the
    // create-site form and the SSL panel use to render their guidance, so what we
    // enforce here and what we advised there can never drift apart.
    //
    // Only a *blocking* verdict refuses. That is a deliberate narrowing of the
    // pre-v2.28.0 guard, which refused whenever the domain didn't resolve to this
    // exact IP: the s252 audit drove a Cloudflare-proxied domain end to end and
    // issuance SUCCEEDED, because Cloudflare forwards the ACME challenge to the
    // origin. Such a domain resolves to Cloudflare's addresses, so the old guard
    // would have refused a configuration that demonstrably works. It now warns
    // (rendered as a callout) and lets the order proceed; only a domain that
    // resolves to nothing at all — where HTTP-01 cannot possibly complete — is
    // still refused.
    let dns = crate::services::prerequisites::check_dns_points_here(&site.domain).await;
    if dns.blocks() {
        return Err(err(StatusCode::PRECONDITION_FAILED, &dns.detail));
    }

    // Get admin email for ACME registration. Validated first — an address in a
    // reserved TLD makes the CA refuse the account contact, and that failure is
    // invisible everywhere downstream (s252 F4).
    let (email,): (String,) =
        sqlx::query_as("SELECT email FROM users WHERE id = $1")
            .bind(claims.sub)
            .fetch_one(&state.db)
            .await
            .map_err(|e| internal_error("provision", e))?;
    let email = resolve_acme_contact(&state.db, &email)
        .await
        .map_err(|e| err(StatusCode::PRECONDITION_FAILED, &e))?;

    let profile = resolve_profile(&state.db, q.profile.as_deref()).await;

    // Build agent request
    let mut agent_body = serde_json::json!({
        "email": email,
        "runtime": site.runtime,
    });

    if let Some(port) = site.proxy_port {
        agent_body["proxy_port"] = serde_json::json!(port);
    }
    if let Some(ref php) = site.php_version {
        agent_body["php_socket"] = serde_json::json!(format!("unix:/run/php/php{php}-fpm.sock"));
    }
    if let Some(ref root) = site.root_path {
        agent_body["root"] = serde_json::json!(root);
    }
    if let Some(ref p) = profile {
        agent_body["profile"] = serde_json::json!(p);
    }
    if q.force {
        agent_body["force"] = serde_json::json!(true);
    }

    // Call agent to provision SSL
    let agent_path = format!("/ssl/provision/{}", site.domain);
    let result = agent
        .post(&agent_path, Some(agent_body))
        .await
        .map_err(|e| acme_failure_or(&site.domain, "SSL provisioning", e))?;

    // Parse expiry from agent response
    let ssl_expiry = result
        .get("expiry")
        .and_then(|v| v.as_str())
        .and_then(crate::helpers::parse_agent_cert_expiry);

    if ssl_expiry.is_none() {
        tracing::warn!(
            "Could not parse SSL expiry for site {} (domain: {}). Raw value: {:?}",
            id, site.domain, result.get("expiry")
        );
    }

    let cert_path = result
        .get("cert_path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let key_path = result
        .get("key_path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Update site in DB. The provenance columns are written HERE, at issuance,
    // beside `ssl_profile` — the tree's existing provision-time provenance
    // column — because this is the only moment anything knows which challenge
    // answered. HTTP-01 orders exactly one identifier, so the subject is the
    // site's own domain and the certificate is never a wildcard.
    sqlx::query(
        "UPDATE sites SET ssl_enabled = true, ssl_cert_path = $1, ssl_key_path = $2, \
         ssl_expiry = $3, ssl_profile = $4, \
         ssl_challenge = 'http-01', ssl_cert_subject = $6, ssl_wildcard = FALSE, \
         ssl_dns_zone_id = NULL, \
         ssl_renewal_at = NULL, ssl_renewal_checked_at = NULL, \
         updated_at = NOW() WHERE id = $5",
    )
    .bind(&cert_path)
    .bind(&key_path)
    .bind(ssl_expiry)
    .bind(profile.as_deref())
    .bind(id)
    .bind(&site.domain)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("provision", e))?;

    tracing::info!("SSL provisioned for {}", site.domain);

    rebuild_vhost_after_ssl(&state, &agent, id).await;

    // GAP 15: Auto-activate paused monitors now that SSL/DNS is working
    let _ = sqlx::query(
        "UPDATE monitors SET enabled = TRUE WHERE site_id = $1 AND enabled = FALSE AND status = 'pending'"
    )
    .bind(id)
    .execute(&state.db)
    .await;

    Ok(Json(serde_json::json!({
        "ok": true,
        "domain": site.domain,
        "ssl_enabled": true,
        "expiry": ssl_expiry,
    })))
}

/// POST /api/sites/{id}/ssl/dns01 — Provision SSL via DNS-01 challenge (Cloudflare).
/// Supports wildcard certificates when wildcard=true.
pub async fn provision_dns01(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: Site = sqlx::query_as(&format!("SELECT s.* FROM sites s WHERE {}", crate::helpers::SITE_CALLER_PREDICATE))
        .bind(id)
        .bind(claims.sub)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("dns01 provision", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    // The DNS-01 path issues for this domain and rebuilds its vhost, same as the
    // HTTP-01 one above — so it needs the same host, from the same row.
    let agent =
        crate::helpers::agent_for_site_server(&state, site.server_id, &site.domain).await?;

    if site.status != "active" {
        return Err(err(StatusCode::BAD_REQUEST, "Site must be active"));
    }

    if site.ssl_enabled {
        return Err(err(StatusCode::CONFLICT, "SSL is already enabled"));
    }

    // Per-user ACME rate limiting: max 10 certificates per hour
    let (recent_count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM sites WHERE user_id = $1 AND ssl_enabled = true \
         AND updated_at > NOW() - INTERVAL '1 hour'",
    )
    .bind(claims.sub)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("dns01 rate check", e))?;

    if recent_count >= 10 {
        return Err(err(
            StatusCode::TOO_MANY_REQUESTS,
            "Rate limit: max 10 SSL certificates per hour. Try again later.",
        ));
    }

    let wildcard = body.get("wildcard").and_then(|v| v.as_bool()).unwrap_or(false);
    // The operator's explicit intent to replace a certificate this product did
    // not issue — see `ProvisionQuery::force` on the HTTP-01 sibling above.
    let force = body.get("force").and_then(|v| v.as_bool()).unwrap_or(false);

    // Find the matching Cloudflare DNS zone for this domain.
    // Uses longest-suffix match to handle multi-part TLDs (e.g., example.co.uk).
    let zones: Vec<crate::routes::dns::DnsZone> = sqlx::query_as(
        "SELECT * FROM dns_zones WHERE user_id = $1 AND provider = 'cloudflare'",
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("dns01 zone lookup", e))?;

    let zone = zones.into_iter()
        .filter(|z| {
            site.domain == z.domain || site.domain.ends_with(&format!(".{}", z.domain))
        })
        .max_by_key(|z| z.domain.len())
        .ok_or_else(|| err(
            StatusCode::PRECONDITION_FAILED,
            "No Cloudflare DNS zone found for this domain. Add it in DNS management first.",
        ))?;

    let cf_zone_id = zone.cf_zone_id.as_deref()
        .ok_or_else(|| err(StatusCode::PRECONDITION_FAILED, "Zone has no Cloudflare zone ID"))?;
    let cf_api_token_enc = zone.cf_api_token.as_deref()
        .ok_or_else(|| err(StatusCode::PRECONDITION_FAILED, "Zone has no Cloudflare API token"))?;
    // Decrypt the at-rest token before handing it to the agent for the DNS-01 challenge —
    // this consumer serialises the token into the agent request body, not via cf_headers.
    let cf_api_token = crate::services::secrets_crypto::decrypt_credential_or_legacy(
        cf_api_token_enc, &state.config.jwt_secret,
    );

    // Get admin email for ACME
    let (email,): (String,) = sqlx::query_as("SELECT email FROM users WHERE id = $1")
        .bind(claims.sub)
        .fetch_one(&state.db)
        .await
        .map_err(|e| internal_error("dns01 email", e))?;
    let email = resolve_acme_contact(&state.db, &email)
        .await
        .map_err(|e| err(StatusCode::PRECONDITION_FAILED, &e))?;

    // For wildcard, provision against the zone domain
    // For single domain, provision against the site domain
    let provision_domain = if wildcard { &zone.domain } else { &site.domain };

    let profile_override = body
        .get("profile")
        .and_then(|v| v.as_str())
        .map(String::from);
    let profile = resolve_profile(&state.db, profile_override.as_deref()).await;

    let mut agent_body = serde_json::json!({
        "email": email,
        "cf_zone_id": cf_zone_id,
        "cf_api_token": cf_api_token,
        "cf_api_email": zone.cf_api_email,
        "wildcard": wildcard,
    });
    if let Some(ref p) = profile {
        agent_body["profile"] = serde_json::json!(p);
    }
    if force {
        agent_body["force"] = serde_json::json!(true);
    }

    // ⛔ THE SAME BUDGET AS EVERY OTHER DNS-01 DOOR. This is the door that ISSUES
    // a wildcard — the exact order `DNS01_ORDER_TIMEOUT_SECS` derives its 300s
    // from (two 10s propagation sleeps for `{d}` and `*.{d}`, then two 120s
    // polls ≈ 260s) — and it was the one caller still passing a bare literal 180.
    // v2.145.0 gave the three RENEWAL doors the shared budget and left issuance
    // short, so issuing a wildcard could report a false failure while the agent
    // went on to succeed. A budget below the agent's own wait is not a timeout,
    // it is a guaranteed false failure — which is what that constant already says.
    let result = agent
        .post_long(
            &format!("/ssl/provision-dns01/{provision_domain}"),
            Some(agent_body),
            DNS01_ORDER_TIMEOUT_SECS,
        )
        .await
        .map_err(|e| dns01_failure_or(provision_domain, &zone.domain, wildcard, e))?;

    // Parse response
    let ssl_expiry = result
        .get("expiry")
        .and_then(|v| v.as_str())
        .and_then(crate::helpers::parse_agent_cert_expiry);

    let cert_path = result.get("cert_path").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let key_path = result.get("key_path").and_then(|v| v.as_str()).unwrap_or("").to_string();

    // Update site in DB. `provision_domain` — NOT `site.domain` — is what was
    // ordered: for a wildcard it is the Cloudflare ZONE, and the certificate
    // covers `{zone}` and `*.{zone}`. Recording the zone ROW as well as its name
    // means the renewal reaches for the credential that issued this certificate
    // instead of re-deriving a zone from a text subject, which is how a stale
    // string could otherwise select a different tenant's Cloudflare token.
    sqlx::query(
        "UPDATE sites SET ssl_enabled = true, ssl_cert_path = $1, ssl_key_path = $2, \
         ssl_expiry = $3, ssl_profile = $4, \
         ssl_challenge = 'dns-01', ssl_cert_subject = $6, ssl_wildcard = $7, \
         ssl_dns_zone_id = $8, \
         ssl_renewal_at = NULL, ssl_renewal_checked_at = NULL, \
         updated_at = NOW() WHERE id = $5",
    )
    .bind(&cert_path)
    .bind(&key_path)
    .bind(ssl_expiry)
    .bind(profile.as_deref())
    .bind(id)
    .bind(provision_domain)
    .bind(wildcard)
    .bind(zone.id)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("dns01 update", e))?;

    rebuild_vhost_after_ssl(&state, &agent, id).await;

    let label = if wildcard { "Wildcard SSL (DNS-01)" } else { "SSL (DNS-01)" };
    tracing::info!("{label} provisioned for {}", site.domain);
    activity::log_activity(
        &state.db, claims.sub, &claims.email,
        if wildcard { "site.ssl.wildcard" } else { "site.ssl.dns01" },
        Some("site"), Some(&site.domain), None, None,
    ).await;

    Ok(Json(serde_json::json!({
        "ok": true,
        "domain": site.domain,
        "wildcard": wildcard,
        "ssl_enabled": true,
        "expiry": ssl_expiry,
    })))
}

#[derive(Deserialize)]
pub struct PreflightQuery {
    pub domain: String,
}

/// GET /api/preflight/dns?domain=… — Evaluate the DNS prerequisite for a domain.
///
/// The create-site form calls this before the user commits, and the SSL panel
/// calls it to explain a gated button. Same checker the provision guard below
/// uses, so the advice and the enforcement cannot disagree.
pub async fn preflight_dns(
    _auth: AuthUser,
    Query(q): Query<PreflightQuery>,
) -> Json<crate::services::prerequisites::PrereqResult> {
    Json(crate::services::prerequisites::check_dns_points_here(&q.domain).await)
}

/// GET /api/sites/{id}/preflight — Evaluate an existing site's prerequisites.
pub async fn preflight_site(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (domain,): (String,) =
        sqlx::query_as(&format!("SELECT s.domain FROM sites s WHERE {}", crate::helpers::SITE_CALLER_PREDICATE))
            .bind(id)
            .bind(claims.sub)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("site preflight", e))?
            .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    let dns = crate::services::prerequisites::check_dns_points_here(&domain).await;
    // The ACME contact is a prerequisite for issuance too, and it is the one the
    // user cannot discover by looking at their DNS.
    let contact = resolve_acme_contact(&state.db, &claims.email).await;

    Ok(Json(serde_json::json!({
        "domain": domain,
        "dns": dns,
        "acme_contact_ok": contact.is_ok(),
        "acme_contact_problem": contact.as_ref().err(),
    })))
}

/// GET /api/sites/{id}/ssl — Get SSL status for a site.
pub async fn status(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: Site = sqlx::query_as(&format!("SELECT s.* FROM sites s WHERE {}", crate::helpers::SITE_CALLER_PREDICATE))
        .bind(id)
        .bind(claims.sub)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("status", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    // A read, so a wrong host is not destructive — but it is still dishonest: asking
    // this box about a certificate that lives on another one returns "no certificate"
    // for a site that has a perfectly good one. `.ok()` below would swallow that into
    // a null rather than an error, which is the shape nobody notices.
    let agent =
        crate::helpers::agent_for_site_server(&state, site.server_id, &site.domain).await?;

    // Also fetch live status from agent
    let agent_path = format!("/ssl/status/{}", site.domain);
    let agent_status = agent.get(&agent_path).await.ok();

    // ⚠ `agent_status` asks the agent about `{SSL_DIR}/{site.domain}/`, so for a
    // site served by a zone WILDCARD it reports `has_cert: false` — the
    // certificate is real and is installed under the zone's name, not this
    // site's. The provenance below is what tells those two cases apart, and it
    // is the only place the panel says what the certificate actually covers.

    // WHO ISSUED IT, decided ONCE, here — not re-derived by every display.
    //
    // The renewal doors have been able to answer this since `foreign_cert_issuer`
    // shipped; nothing ever told the operator. So the site detail page asserted
    // "Enabled (Let's Encrypt)" over every enabled certificate, including one the
    // operator uploaded from a commercial CA, and the panel that had just DECLINED
    // to renew it — correctly, and with a sentence naming the issuer — went on
    // describing it as its own.
    //
    // ⛔ This is derived from the `agent_status` ALREADY IN HAND, so the honesty
    // costs no extra round trip. And it is computed HERE rather than shipped as a
    // raw issuer string for the client to match on, because "does this issuer
    // string mean Let's Encrypt" is the same question `foreign_cert_issuer` asks
    // for renewal — two spellings of the apostrophe included. A second copy of
    // that test in TypeScript is a severed pair from the day it lands.
    //
    // `unknown` is a first-class answer and MUST NOT be rendered as a CA name:
    // an unreachable agent and a wildcard child both arrive here, and both held
    // a real certificate. See `helpers::CertProvenance`.
    let (provenance, issuer) = match agent_status
        .as_ref()
        .map(crate::helpers::cert_provenance)
        .unwrap_or(crate::helpers::CertProvenance::Unknown)
    {
        crate::helpers::CertProvenance::Foreign(i) => ("foreign", Some(i)),
        crate::helpers::CertProvenance::DockPanelIssued => ("dockpanel", None),
        crate::helpers::CertProvenance::Unknown => ("unknown", None),
    };

    Ok(Json(serde_json::json!({
        "ssl_enabled": site.ssl_enabled,
        "cert_path": site.ssl_cert_path,
        "key_path": site.ssl_key_path,
        "expiry": site.ssl_expiry,
        "challenge": site.ssl_challenge,
        "cert_subject": site.ssl_cert_subject,
        "wildcard": site.ssl_wildcard,
        "agent_status": agent_status,
        "provenance": provenance,
        "issuer": issuer,
    })))
}

/// How long a DNS-01 order may take, at every layer, in seconds.
///
/// Derived from the agent's own arithmetic rather than guessed: `provision_cert_dns01`
/// sleeps 10s per authorization for DNS propagation — TWO of them for a wildcard,
/// which orders `{subject}` and `*.{subject}` — then polls readiness on a 120s
/// retry policy and polls for the certificate on another 120s. That is ~260s of
/// budgeted waiting before any slack.
///
/// ⛔ Every caller must use THIS constant. The three renewal doors disagreed:
/// `auto_healer` used plain `post` (a hard 60s cap in `AgentHandle::request`),
/// `security_scanner` the same, and the interactive Renew button
/// `post_long(.., 120)` — so a DNS-01 renewal timed out at every door while the
/// agent went on to succeed, and the panel recorded a failure, skipped the
/// expiry write and wrote a cooldown row. A budget below the agent's own wait is
/// not a timeout, it is a guaranteed false failure.
pub(crate) const DNS01_ORDER_TIMEOUT_SECS: u64 = 300;

/// Renew a certificate over DNS-01, against the name it was actually issued for.
///
/// ⚠ This reuses the agent's EXISTING `/ssl/provision-dns01/{domain}` route
/// rather than adding a renewal route of its own. That route has been a renewal
/// since v2.6.7: it orders `{domain}` (+ `*.{domain}` when `wildcard`), and
/// `provision_cert_dns01` does `create_dir_all` then `write`, which over an
/// existing directory is exactly a re-issue in place. It also deliberately skips
/// the vhost enable for a wildcard, leaving that to the panel — which is what
/// this door needs. Reusing it is why this fix reaches every installed agent as
/// it stands instead of waiting for a fleet-wide upgrade; `error.rs` states that
/// doctrine outright: "an agent is only updated when somebody updates it, so a
/// fix that lives only in the agent does not arrive."
///
/// ⚠ The zone is taken by ID, decided by the caller from what was RECORDED at
/// issuance. It is never re-derived from a text subject here, because an
/// unattended loop has no actor to scope a zone lookup to, and resolving one by
/// bare domain would hand one account's Cloudflare token to a renewal running
/// for another account's site.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn renew_over_dns01(
    pool: &sqlx::PgPool,
    jwt_secret: &str,
    agent: &crate::services::agent::AgentHandle,
    subject: &str,
    wildcard: bool,
    zone_id: Uuid,
    owner_id: Uuid,
    profile: Option<&str>,
) -> Result<serde_json::Value, String> {
    let zone: crate::routes::dns::DnsZone =
        sqlx::query_as("SELECT * FROM dns_zones WHERE id = $1")
            .bind(zone_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| format!("DNS zone lookup failed: {e}"))?
            .ok_or_else(|| "the Cloudflare zone this certificate was issued against no longer exists".to_string())?;

    let cf_zone_id = zone
        .cf_zone_id
        .as_deref()
        .ok_or_else(|| format!("the {} zone has no Cloudflare zone ID", zone.domain))?;
    let cf_api_token_enc = zone
        .cf_api_token
        .as_deref()
        .ok_or_else(|| format!("the {} zone has no Cloudflare API token", zone.domain))?;
    let cf_api_token =
        crate::services::secrets_crypto::decrypt_credential_or_legacy(cf_api_token_enc, jwt_secret);

    // The ACME contact belongs to the site's OWNER, not to whoever triggered
    // this — the unattended doors have no one to attribute it to, and the
    // interactive one is reachable by an administrator acting on another
    // account's site.
    let owner_email: String = sqlx::query_scalar("SELECT email FROM users WHERE id = $1")
        .bind(owner_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| format!("owner lookup failed: {e}"))?
        .ok_or_else(|| "the site's owner account has no email address on file".to_string())?;
    let email = resolve_acme_contact(pool, &owner_email).await?;

    let mut agent_body = serde_json::json!({
        "email": email,
        "cf_zone_id": cf_zone_id,
        "cf_api_token": cf_api_token,
        "cf_api_email": zone.cf_api_email,
        "wildcard": wildcard,
    });
    if let Some(p) = profile {
        agent_body["profile"] = serde_json::json!(p);
    }

    agent
        .post_long(
            &format!("/ssl/provision-dns01/{subject}"),
            Some(agent_body),
            DNS01_ORDER_TIMEOUT_SECS,
        )
        .await
        // s388's three-party classification, reused: name Cloudflare or the CA
        // when either of them is what refused, and stay this machine's fault
        // otherwise. ⛔ Deliberately NOT `acme_failure_or`, whose sentence tells
        // the operator to check port 80 — on the door reached precisely because
        // port 80 cannot work.
        .map_err(|e| {
            crate::error::dns_provider_failure(&e)
                .or_else(|| crate::error::acme_order_failure(&e))
                .unwrap_or_else(|| format!("the DNS-01 order for {subject} did not complete: {e}"))
        })
}

/// Write down what a SUCCESSFUL renewal actually installed.
///
/// Two of the four plans change what is true about the row and must say so:
///
/// - `Http01 { record_challenge: true }` — nothing was recorded, this pass was
///   the one degraded attempt, and it succeeded. Recording it here is what makes
///   the rule terminate instead of degrading for ever.
/// - `LastResortHttp01` — a DNS-01 certificate we could not re-order over
///   DNS-01, downgraded on purpose inside its last week. The installed
///   certificate really is single-name HTTP-01 now, so the row has to agree, or
///   the next cycle branches to DNS-01 for a certificate that is not one.
///
/// ⚠ Called only after success. A FAILED HTTP-01 attempt on an unrecorded row
/// must leave it unrecorded — the likeliest reason it failed is that the site
/// cannot answer HTTP-01, which is exactly the case that made DNS-01 the right
/// door, and recording `http-01` there would pin the wrong answer for ever.
pub(crate) async fn record_renewal_provenance(
    pool: &sqlx::PgPool,
    site_id: Uuid,
    domain: &str,
    plan: &crate::helpers::RenewalPlan,
) {
    // ⛔ TWO DIFFERENT QUESTIONS, and an earlier draft of this fix conflated them.
    // "Does this renewal change what is RECORDED about provenance?" is true for
    // only two plans. "Did this renewal install a certificate at the site's OWN
    // directory?" is true for ALL THREE HTTP-01 plans — including the ordinary
    // recorded one, which is by far the commonest.
    //
    // Gating the path on the provenance question left the biggest population
    // uncovered: a row ALREADY stamped `http-01` whose stored path still names
    // somewhere else renews for ever, gets a fresh certificate at its own name
    // every time, and has its vhost re-rendered from the stale path every time.
    // v2.145.0 manufactured exactly that shape — it stamped `http-01` on
    // unrecorded rows without touching the path — so those rows would have been
    // permanently uncorrectable. Found by driving a real box, not by reading.
    //
    // `Dns01` still returns: a wildcard's path names the ZONE, which is correct,
    // and repointing it at the site would be the defect inverted.
    let (record_provenance, losing) = match plan {
        crate::helpers::RenewalPlan::Http01 { record_challenge: true } => (true, None),
        crate::helpers::RenewalPlan::Http01 { record_challenge: false } => (false, None),
        crate::helpers::RenewalPlan::LastResortHttp01 { losing } => (true, Some(losing.clone())),
        _ => return,
    };

    // ⭐ THE PATH FOLLOWS THE CERTIFICATE, and it is the half v2.145.0 left open.
    // No renewal door writes `ssl_cert_path` — 0 occurrences in `auto_healer.rs`
    // and `security_scanner.rs` (both DO write `SET ssl_expiry`, so that is a
    // measurement, not a missing grep) — yet every door re-renders the vhost from
    // that column moments later. Both plans reaching this point ordered HTTP-01
    // for `domain`, so the agent wrote `/etc/dockpanel/ssl/{domain}/`.
    //
    // For a row whose stored path names a DIFFERENT directory — a wildcard child
    // whose path names the zone — the panel therefore renewed the certificate and
    // then pointed nginx back at the un-renewed wildcard, while stamping the NEW
    // certificate's expiry on the row. The 45-day window never reopened and the
    // site went dark at the wildcard's real expiry, behind a panel reporting a
    // successful renewal. That is Shape B of the s392 defect, still live for every
    // row whose provenance was never recorded.
    //
    // Writing it HERE fixes all four doors at once: every one of them calls this
    // before its rebuild (`ssl.rs` 962→964, `auto_healer.rs` 1159→1193,
    // `security_scanner.rs` 655→711).
    let _ = sqlx::query(
        "UPDATE sites SET ssl_cert_path = $2, ssl_key_path = $3, \
         updated_at = NOW() WHERE id = $1",
    )
    .bind(site_id)
    .bind(format!("/etc/dockpanel/ssl/{domain}/fullchain.pem"))
    .bind(format!("/etc/dockpanel/ssl/{domain}/privkey.pem"))
    .execute(pool)
    .await;

    // The provenance half, only for the two plans that change what is TRUE about
    // it. An ordinary recorded HTTP-01 renewal already says `http-01`; rewriting
    // it would be a no-op that muddies which door last decided.
    if record_provenance {
        let _ = sqlx::query(
            "UPDATE sites SET ssl_challenge = 'http-01', ssl_cert_subject = $2, \
             ssl_wildcard = FALSE, ssl_dns_zone_id = NULL, updated_at = NOW() WHERE id = $1",
        )
        .bind(site_id)
        .bind(domain)
        .execute(pool)
        .await;
    }

    if let Some(losing) = losing {
        // Say exactly what was lost. The defect this ship removes was never the
        // downgrade itself — it was that the downgrade happened in silence and
        // the panel then reported a successful renewal.
        crate::services::system_log::log_event(
            pool,
            // ⚠ "warning", not "warn". The readers — the Warnings tile, the level
            // filter and the badge — all spell it long, and a short spelling is a
            // row nothing can count, filter or colour. `system-logs-scope` S8
            // exists for exactly this and caught this line.
            "warning",
            "ssl",
            &format!("{domain}: DNS-01 certificate downgraded to a single name"),
            Some(&format!(
                "The certificate covered {losing} and could not be re-ordered over DNS-01 \
                 before it expired, so a single-name HTTP-01 certificate for {domain} was \
                 issued instead. Any other name that certificate covered is no longer \
                 covered. Restore it from the site's SSL tab once a Cloudflare zone with an \
                 API token is available."
            )),
        )
        .await;
    }
}

/// POST /api/ssl/{id}/renew — Force-renew SSL certificate (admin only).
pub async fn renew(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // ⚠ This read used to be `WHERE id = $1` with no caller and no server term,
    // while `provision`, `provision_dns01` and `status` in this same file all went
    // through the shared predicate. `AdminUser` is not a superuser here — an
    // administrator's reach stops at the local box and the machines they registered
    // themselves — so the only thing keeping this handler inside that boundary was
    // the scope extractor's check on a header, which is not an authorisation the
    // row ever agreed to. Adopting the sibling form makes the row decide, and that
    // is what then makes it safe to take the host from the row as well.
    let site: Site = sqlx::query_as(&format!(
        "SELECT s.* FROM sites s WHERE {}",
        crate::helpers::SITE_CALLER_PREDICATE
    ))
        .bind(id)
        .bind(claims.sub)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("ssl renew", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    renew_for_site(&state, &site, claims.sub, &claims.email).await
}

/// Renew the certificate for a site the caller has ALREADY been authorised to reach.
///
/// Split out of `renew` so the Diagnostics "Fix" button can reach a renewal that
/// actually happens, instead of an agent arm that does not exist. It takes a
/// [`Site`] the caller resolved through `SITE_CALLER_PREDICATE`, so the
/// authorisation decision stays with the row and stays in ONE place: reuse
/// carries the mechanism, never the gate, and this project shipped that exact
/// mistake once already on the database-import door.
pub(crate) async fn renew_for_site(
    state: &AppState,
    site: &Site,
    actor_id: Uuid,
    actor_email: &str,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = site.id;

    if !site.ssl_enabled {
        return Err(err(StatusCode::BAD_REQUEST, "SSL is not enabled for this site"));
    }

    // Renewing writes a certificate and rebuilds the vhost, both named by domain on
    // whichever host this handle points at. The row names that host; the caller's
    // selection only ever named the one they were looking at.
    let agent =
        crate::helpers::agent_for_site_server(state, site.server_id, &site.domain).await?;

    // WHICH CHALLENGE ISSUED THIS CERTIFICATE decides which door renews it, and
    // that decision has to come BEFORE the foreign-certificate check below, not
    // after it — see why there.
    let days_remaining = site
        .ssl_expiry
        .map(|e| (e - chrono::Utc::now()).num_days());
    let plan = crate::helpers::renewal_plan(&state.db, site, days_remaining).await;

    // ⛔ RENEWING IS REPLACING. `provision_cert` writes the same
    // `fullchain.pem`/`privkey.pem` an uploaded certificate occupies, so "renew"
    // aimed at a certificate this product did not issue does not refresh it — it
    // DESTROYS it and puts a 90-day Let's Encrypt certificate in its place. For a
    // commercial wildcard, a Cloudflare Origin CA certificate or a corporate PKI
    // certificate that is a paid asset replaced without consent, and for a domain
    // that cannot answer HTTP-01 from this box it is worse: the order fails and
    // the operator is left with neither.
    //
    // So the question is asked once, here, where every deliberate renewal passes:
    // the admin Renew button, and the Diagnostics Fix button that reaches this
    // same function. A positive foreign issuer is a refusal WITH A SENTENCE —
    // which is the whole point of the exercise, since the alternative outcome for
    // exactly this case was `Operation failed. Reference: {uuid}` after a wasted
    // ACME order.
    //
    // ⛔ THE NAME CHECKED MUST BE THE NAME THE ORDER IS ABOUT TO OVERWRITE. A
    // DNS-01 renewal orders against `subject` — the zone apex for a wildcard,
    // recorded at issuance, never `site.domain` — and `site.domain` typically has
    // no certificate directory of its own in that case (the agent resolves
    // `/ssl/status/{domain}` to a literal `/etc/dockpanel/ssl/{domain}/`), so
    // checking `site.domain` always answered "no certificate here" regardless of
    // what foreign certificate actually sat at the zone apex about to be
    // overwritten. Skipped for `Refuse`, which never reaches an agent order.
    let subject_domain: &str = match &plan {
        crate::helpers::RenewalPlan::Dns01 { subject, .. } => subject.as_str(),
        _ => site.domain.as_str(),
    };
    if !matches!(&plan, crate::helpers::RenewalPlan::Refuse { .. }) {
        if let Some(issuer) = crate::helpers::foreign_cert_issuer(&agent, subject_domain).await {
            return Err(err(
                StatusCode::UNPROCESSABLE_ENTITY,
                &format!(
                    "The certificate on {} was not issued by DockPanel (issuer: {}). DockPanel \
                     renews only the Let's Encrypt certificates it issued itself, and renewing \
                     this one would replace it. Install a replacement under the site's SSL tab \
                     instead, or renew it wherever it was issued.",
                    subject_domain, issuer
                ),
            ));
        }
    }

    // Agent renew now needs the same context as provision so it can rebuild
    // the nginx config after issuing the new cert.
    let (email,): (String,) = sqlx::query_as("SELECT email FROM users WHERE id = $1")
        .bind(site.user_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| internal_error("ssl renew email", e))?;
    let email = resolve_acme_contact(&state.db, &email)
        .await
        .map_err(|e| err(StatusCode::PRECONDITION_FAILED, &e))?;

    let mut agent_body = serde_json::json!({
        "email": email,
        "runtime": site.runtime,
    });
    if let Some(port) = site.proxy_port {
        agent_body["proxy_port"] = serde_json::json!(port);
    }
    if let Some(ref php) = site.php_version {
        agent_body["php_socket"] = serde_json::json!(format!("unix:/run/php/php{php}-fpm.sock"));
    }
    if let Some(ref root) = site.root_path {
        agent_body["root"] = serde_json::json!(root);
    }
    if let Some(ref p) = site.ssl_profile {
        agent_body["profile"] = serde_json::json!(p);
    }

    // `plan` was already decided above the foreign-certificate check, since that
    // check needs to know which name is about to be ordered.
    let result = match &plan {
        crate::helpers::RenewalPlan::Refuse { reason } => {
            // The idiom this file already uses for a precondition that is not an
            // agent failure — the three sibling refusals in `provision_dns01` are
            // PRECONDITION_FAILED with a plain sentence. `dns01_failure_or` is an
            // AgentError translator and cannot be reached from here at all: no
            // agent has been called yet.
            return Err(err(StatusCode::PRECONDITION_FAILED, reason));
        }
        crate::helpers::RenewalPlan::Dns01 { subject, wildcard, zone_id } => {
            renew_over_dns01(
                &state.db,
                &state.config.jwt_secret,
                &agent,
                subject,
                *wildcard,
                *zone_id,
                site.user_id,
                site.ssl_profile.as_deref(),
            )
            .await
            .map_err(|reason| err(StatusCode::UNPROCESSABLE_ENTITY, &reason))?
        }
        crate::helpers::RenewalPlan::Http01 { .. }
        | crate::helpers::RenewalPlan::LastResortHttp01 { .. } => {
            let agent_path = format!("/ssl/{}/renew", site.domain);
            agent
                .post_long(&agent_path, Some(agent_body), DNS01_ORDER_TIMEOUT_SECS)
                .await
                .map_err(|e| acme_failure_or(&site.domain, "SSL renewal", e))?
        }
    };

    // Update expiry from the renew response and clear stale ARI hints so
    // the next auto-heal cycle refetches them.
    //
    // ⚠ The provenance columns move WITH the certificate. A last-resort
    // downgrade really does leave an HTTP-01 single-name certificate installed,
    // so the row must say so — otherwise the next cycle would branch to DNS-01
    // for a certificate that is no longer one. And an unrecorded row records
    // itself here, which is what makes the whole rule converge: an unknown row
    // behaves as it does today exactly once, and is known ever after.
    if let Some(expiry_str) = result.get("expiry").and_then(|v| v.as_str()) {
        if let Some(expiry) = crate::helpers::parse_agent_cert_expiry(expiry_str) {
            let _ = sqlx::query(
                "UPDATE sites SET ssl_expiry = $1, ssl_renewal_at = NULL, \
                 ssl_renewal_checked_at = NULL, updated_at = NOW() WHERE id = $2",
            )
            .bind(expiry)
            .bind(id)
            .execute(&state.db)
            .await;
        }
    }
    record_renewal_provenance(&state.db, id, &site.domain, &plan).await;

    rebuild_vhost_after_ssl(state, &agent, id).await;

    tracing::info!("SSL renewed for {} by {}", site.domain, actor_email);
    activity::log_activity(
        &state.db, actor_id, actor_email, "ssl.renew",
        Some("site"), Some(&site.domain), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true, "domain": site.domain })))
}

/// DELETE /api/ssl/{id} — Revoke and delete SSL certificate (admin only).
pub async fn revoke(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Same unscoped read as `renew`, and this is the worse half: it deletes the
    // certificate files through the agent and then blanks every `ssl_*` column on
    // the row, so an id belonging to a site outside the caller's boundary was a
    // cross-boundary write even when the agent leg found nothing to delete.
    let site: Site = sqlx::query_as(&format!(
        "SELECT s.* FROM sites s WHERE {}",
        crate::helpers::SITE_CALLER_PREDICATE
    ))
        .bind(id)
        .bind(claims.sub)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("ssl revoke", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    if !site.ssl_enabled {
        return Err(err(StatusCode::BAD_REQUEST, "SSL is not enabled for this site"));
    }

    // The deletion names the site by domain and lands on whichever host this handle
    // points at, so it has to be the host the row names.
    let agent =
        crate::helpers::agent_for_site_server(&state, site.server_id, &site.domain).await?;

    let agent_path = format!("/ssl/{}", site.domain);
    agent
        .delete(&agent_path)
        .await
        .map_err(|e| agent_error("SSL deletion", e))?;

    // Clear SSL fields in DB
    sqlx::query(
        "UPDATE sites SET ssl_enabled = false, ssl_cert_path = NULL, ssl_key_path = NULL, \
         ssl_expiry = NULL, ssl_profile = NULL, \
         ssl_challenge = NULL, ssl_cert_subject = NULL, ssl_wildcard = NULL, \
         ssl_dns_zone_id = NULL, ssl_renewal_at = NULL, \
         ssl_renewal_checked_at = NULL, updated_at = NOW() WHERE id = $1",
    )
    .bind(id)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("ssl revoke", e))?;

    // ⛔ AFTER the clear, never before. The agent's teardown removes
    // `/etc/dockpanel/ssl/{domain}/` while `/etc/nginx/sites-enabled/{domain}.conf`
    // still carries `ssl_certificate` naming it — and `nginx -t` is WHOLE-SERVER,
    // so from that moment every site edit on the box fails and the next restart
    // leaves nginx down for every tenant. The agent's shared-directory guard does
    // not save this door: `ownership.rs` skips the site's OWN vhost, so a solo
    // site's directory really is deleted. The four sibling SSL writers have always
    // re-rendered here; this one never did.
    //
    // Placement is load-bearing. `build_nginx_body` emits the certificate keys
    // only under `if site.ssl_enabled`, which the statement above has just made
    // false, and this helper re-reads the row — so it now writes a plain HTTP
    // vhost. Called BEFORE the clear it would write the HTTPS config back,
    // naming the deleted directory, fail `nginx -t` and be rolled back: a silent
    // no-op that satisfies any arm merely asserting the call exists.
    rebuild_vhost_after_ssl(&state, &agent, id).await;

    tracing::info!("SSL revoked for {} by {}", site.domain, claims.email);
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "ssl.revoke",
        Some("site"), Some(&site.domain), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true, "domain": site.domain })))
}

// ── ACME profile + default-profile admin surface ─────────────────────────

/// GET /api/ssl/profiles — List ACME profiles advertised by the CA.
///
/// Requires an admin (the ACME account is a panel-wide resource). Returns
/// the server directory's profile list plus the currently configured
/// default. When the CA doesn't support the profiles extension, `profiles`
/// is empty; callers should hide the dropdown.
pub async fn profiles(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Use the admin's own email for ACME directory lookup. Safe because we
    // only read the server directory; no order is created.
    let email = &claims.email;
    let agent_path = format!("/ssl/profiles?email={}", urlencoding::encode(email));
    let list = agent
        .get(&agent_path)
        .await
        .map_err(|e| agent_error("ACME profiles", e))?;

    let default = sqlx::query_scalar::<_, String>(
        "SELECT value FROM settings WHERE key = 'acme_default_profile'",
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("read default profile", e))?;

    let contact_email = sqlx::query_scalar::<_, String>(
        "SELECT value FROM settings WHERE key = 'acme_contact_email'",
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("read acme contact", e))?;

    // Tell the operator, up front, which address will actually reach the CA and
    // whether their own is usable — the whole point of F4 is that this was
    // invisible until every certificate had already failed.
    let effective = resolve_acme_contact(&state.db, &claims.email).await;

    Ok(Json(serde_json::json!({
        "profiles": list,
        "default": default,
        "contact_email": contact_email,
        "login_email": claims.email,
        "login_email_usable": validate_acme_contact(&claims.email).is_ok(),
        "effective_contact": effective.as_ref().ok(),
        "contact_problem": effective.as_ref().err(),
    })))
}

/// GET /api/ssl/contact-email — Report the ACME contact situation.
///
/// Deliberately does NOT touch the agent, unlike `profiles` above. The whole
/// point of this surface is to be readable when issuance is broken, and a bad
/// contact is one of the reasons the agent's ACME directory call fails — putting
/// it behind that call would hide it exactly when it is needed.
pub async fn get_contact_email(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let contact_email = sqlx::query_scalar::<_, String>(
        "SELECT value FROM settings WHERE key = 'acme_contact_email'",
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("read acme contact", e))?;

    let effective = resolve_acme_contact(&state.db, &claims.email).await;

    Ok(Json(serde_json::json!({
        "contact_email": contact_email,
        "login_email": claims.email,
        "login_email_usable": validate_acme_contact(&claims.email).is_ok(),
        "effective_contact": effective.as_ref().ok(),
        "contact_problem": effective.as_ref().err(),
    })))
}

#[derive(Deserialize)]
pub struct ContactEmailReq {
    pub email: Option<String>,
}

/// POST /api/ssl/contact-email — Set the panel-wide fallback ACME contact.
///
/// Used when the operator's own login address cannot be a Let's Encrypt contact
/// (a reserved TLD, a typo). Pass `{"email": null}` to clear it.
pub async fn set_contact_email(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Json(body): Json<ContactEmailReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    match body.email.as_deref().map(str::trim) {
        Some(e) if !e.is_empty() => {
            // Validate before storing: a rescue address that is itself invalid
            // would just move the silent failure one level down.
            validate_acme_contact(e)
                .map_err(|msg| err(StatusCode::BAD_REQUEST, &msg))?;
            sqlx::query(
                "INSERT INTO settings (key, value) VALUES ('acme_contact_email', $1) \
                 ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = NOW()",
            )
            .bind(e)
            .execute(&state.db)
            .await
            .map_err(|e| internal_error("set acme contact", e))?;
        }
        _ => {
            sqlx::query("DELETE FROM settings WHERE key = 'acme_contact_email'")
                .execute(&state.db)
                .await
                .map_err(|e| internal_error("clear acme contact", e))?;
        }
    }

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "ssl.contact_email",
        None, None,
        Some(&format!("email={:?}", body.email)),
        None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true, "contact_email": body.email })))
}

#[derive(Deserialize)]
pub struct DefaultProfileReq {
    pub profile: Option<String>,
}

/// POST /api/ssl/default-profile — Set the panel-wide default ACME profile.
/// Pass `{"profile": null}` or omit to reset to CA default.
pub async fn set_default_profile(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Json(body): Json<DefaultProfileReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    match body.profile.as_deref() {
        Some(p) if !p.is_empty() => {
            sqlx::query(
                "INSERT INTO settings (key, value) VALUES ('acme_default_profile', $1) \
                 ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = NOW()",
            )
            .bind(p)
            .execute(&state.db)
            .await
            .map_err(|e| internal_error("set default profile", e))?;
        }
        _ => {
            sqlx::query("DELETE FROM settings WHERE key = 'acme_default_profile'")
                .execute(&state.db)
                .await
                .map_err(|e| internal_error("clear default profile", e))?;
        }
    }

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "ssl.default_profile",
        None, None,
        Some(&format!("profile={:?}", body.profile)),
        None,
    ).await;

    Ok(Json(serde_json::json!({
        "ok": true,
        "default": body.profile,
    })))
}

/// Top-level labels reserved by RFC 2606 / RFC 6761 / RFC 8375, plus the
/// conventional private-network ones. None of these appear on the Public Suffix
/// List, so Let's Encrypt refuses an account contact in any of them with
/// `contact email has invalid domain: Domain name does not end with a valid
/// public suffix`.
const NON_PUBLIC_TLDS: &[&str] = &[
    "test", "local", "localhost", "internal", "invalid", "example",
    "localdomain", "lan", "home", "corp", "intranet", "private", "arpa", "onion",
];

/// Check that an address is usable as an ACME account contact.
///
/// This is deliberately not a general-purpose email validator — it only rejects
/// what Let's Encrypt itself rejects, so we never refuse an address the CA would
/// have accepted.
pub(crate) fn validate_acme_contact(email: &str) -> Result<(), String> {
    let email = email.trim();
    if email.is_empty() {
        return Err("no contact address is set".to_string());
    }

    let mut parts = email.split('@');
    let local = parts.next().unwrap_or("");
    let domain = parts.next().unwrap_or("");
    if local.is_empty() || domain.is_empty() || parts.next().is_some() {
        return Err(format!("\"{email}\" is not a valid email address"));
    }

    if !domain.contains('.') || domain.starts_with('.') || domain.ends_with('.') {
        return Err(format!(
            "\"{email}\" has no valid public domain — Let's Encrypt will reject it as a contact address"
        ));
    }

    let tld = domain.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    if tld.len() < 2 || !tld.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err(format!(
            "\"{email}\" has no valid public domain — Let's Encrypt will reject it as a contact address"
        ));
    }
    if NON_PUBLIC_TLDS.contains(&tld.as_str()) {
        return Err(format!(
            "\"{email}\" uses the reserved .{tld} domain, which Let's Encrypt rejects as a contact address"
        ));
    }

    Ok(())
}

/// Resolve the Let's Encrypt account contact for an issuance.
///
/// The panel used to hand `claims.email` straight to the agent, unvalidated. An
/// operator who registered the panel with e.g. `admin@dockpanel.test` therefore
/// had EVERY certificate fail at the CA with a contact-validation error that
/// surfaced neither in the UI nor in `journalctl` — four silent retries, then a
/// permanent give-up (s252 F4).
///
/// Order: the user's own address when it is usable, otherwise the panel-wide
/// `acme_contact_email` setting as a rescue. Keeping the user's address first
/// means installs that already work are completely unaffected.
pub(crate) async fn resolve_acme_contact(
    pool: &sqlx::PgPool,
    user_email: &str,
) -> Result<String, String> {
    let user_err = match validate_acme_contact(user_email) {
        Ok(()) => return Ok(user_email.trim().to_string()),
        Err(e) => e,
    };

    let fallback = sqlx::query_scalar::<_, String>(
        "SELECT value FROM settings WHERE key = 'acme_contact_email'",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .filter(|s| !s.trim().is_empty());

    match fallback {
        Some(addr) if validate_acme_contact(&addr).is_ok() => Ok(addr.trim().to_string()),
        _ => Err(format!(
            "Cannot request a certificate: {user_err}. \
             Set a valid contact address under Settings → SSL (ACME contact email), \
             or sign in with an account whose email uses a real domain."
        )),
    }
}

/// Resolve the profile to use for an operation: explicit override > stored
/// default > None (CA picks its default).
pub(crate) async fn resolve_profile(
    pool: &sqlx::PgPool,
    override_: Option<&str>,
) -> Option<String> {
    if let Some(p) = override_ {
        if !p.is_empty() {
            return Some(p.to_string());
        }
    }
    sqlx::query_scalar::<_, String>(
        "SELECT value FROM settings WHERE key = 'acme_default_profile'",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .filter(|s| !s.is_empty())
}

// ⛔ EVERYTHING BELOW THE FIRST `#[cfg(test)]` IN THIS FILE IS INVISIBLE TO THE
// PIN SUITES. `ssl-correctness-pin-e2e.sh`'s `prod_lines` blanks from the first
// test marker to EOF and never resumes, so a `prod_*` arm cannot see production
// code placed after one. `resolve_profile` had drifted below it and was blinded
// at v2.144.0 — the second instance of lesson #669, which was written when a
// test module placed mid-file hid ~60% of this file. Production items go ABOVE
// this line; test modules go at the END.

#[cfg(test)]
mod acme_contact_tests {
    use super::validate_acme_contact;

    /// The exact address that broke every issuance on the s252 audit box.
    #[test]
    fn rejects_the_address_that_broke_the_audit_box() {
        let e = validate_acme_contact("admin@dockpanel.test").unwrap_err();
        assert!(e.contains(".test"), "error must name the offending TLD: {e}");
    }

    #[test]
    fn rejects_the_other_reserved_tlds() {
        for addr in [
            "a@box.local",
            "a@box.internal",
            "a@box.localhost",
            "a@box.invalid",
            "a@host.example",
            "a@box.lan",
            "a@box.home",
            "a@box.corp",
        ] {
            assert!(validate_acme_contact(addr).is_err(), "{addr} must be rejected");
        }
    }

    #[test]
    fn rejects_malformed_addresses() {
        for addr in ["", "   ", "admin", "admin@", "@example.com", "a@b@c.com", "admin@localhost"] {
            assert!(validate_acme_contact(addr).is_err(), "{addr:?} must be rejected");
        }
    }

    #[test]
    fn rejects_a_bare_or_numeric_tld() {
        assert!(validate_acme_contact("admin@example.c").is_err());
        assert!(validate_acme_contact("admin@example.123").is_err());
        assert!(validate_acme_contact("admin@.com").is_err());
        assert!(validate_acme_contact("admin@example.").is_err());
    }

    /// The guard must not be stricter than the CA — these all issue fine today.
    #[test]
    fn accepts_real_addresses() {
        for addr in [
            "admin@example.com",
            "admin@example.dev",
            "someone+le@example.co.uk",
            "a.b-c_d@sub.domain.io",
            "  spaced@example.com  ",
        ] {
            assert!(validate_acme_contact(addr).is_ok(), "{addr} must be accepted");
        }
    }
}


#[cfg(test)]
mod dns01_message_tests {
    use super::*;
    use crate::services::agent::AgentError;

    fn message(e: ApiError) -> String {
        e.1 .0["error"].as_str().unwrap_or_default().to_string()
    }

    fn labelled(code: &str, reason: &str) -> AgentError {
        AgentError::Status(
            500,
            serde_json::json!({ "error": reason, "code": code }).to_string(),
        )
    }

    #[test]
    fn a_provider_refusal_names_the_token_and_never_port_80() {
        // ⚠ A SUBDOMAIN site, deliberately. The zone lookup exists to match a site
        // against a PARENT zone, so site == zone is the case that cannot see a
        // site/zone confusion at all — every fixture here keeps them distinct.
        let e = dns01_failure_or(
            "blog.example.com",
            "example.com",
            false,
            labelled("dns_provider_failed", "Cloudflare refused to create the challenge record"),
        );
        assert_eq!(e.0.as_u16(), 422);
        let m = message(e);
        assert!(m.contains("DNS:Edit"), "{m}");
        // The zone the operator actually has in Cloudflare — never the site FQDN,
        // which is not a zone and will not be found by anyone who goes looking.
        assert!(m.contains("the example.com zone"), "{m}");
        assert!(!m.contains("the blog.example.com zone"), "{m}");
        // ⭐ THE POINT OF THE WHOLE UNIT. The sibling door's sentence offers
        // port-80 advice; an operator is on THIS door precisely because port 80
        // cannot be reached, so repeating it would send them to open a port that
        // has nothing to do with the failure.
        assert!(!m.contains("port 80"), "{m}");
        assert!(!m.contains("try again"), "{m}");
    }

    #[test]
    fn a_declined_order_names_the_challenge_record_and_never_port_80() {
        let e = dns01_failure_or(
            "blog.example.com",
            "example.com",
            false,
            labelled("acme_order_failed", "The CA did not validate the DNS-01 challenge"),
        );
        assert_eq!(e.0.as_u16(), 422);
        let m = message(e);
        assert!(m.contains("_acme-challenge"), "{m}");
        assert!(m.contains("the example.com zone"), "{m}");
        assert!(!m.contains("the blog.example.com zone"), "{m}");
        // #663: this branch also fires on a rate limit, where publishing a TXT
        // record fixes nothing — so the advice must stay conditional.
        assert!(m.contains("If that is a validation failure"), "{m}");
        assert!(!m.contains("port 80"), "{m}");
        assert!(!m.contains("try again"), "{m}");
    }

    #[test]
    fn a_wildcard_names_the_certificate_that_was_actually_ordered() {
        // The order is placed against the ZONE, not the site. Naming the site
        // here would describe a certificate nobody asked for.
        let e = dns01_failure_or(
            "example.com",
            "example.com",
            true,
            labelled("acme_order_failed", "The CA refused the certificate order"),
        );
        let m = message(e);
        assert!(m.contains("*.example.com"), "{m}");
    }

    #[test]
    fn a_wildcard_provider_refusal_also_names_the_wildcard() {
        // ⭐ THE BUSIEST PATH ON THIS DOOR and it had no test: DNS-01 is the wildcard
        // door, and a token without DNS:Edit is the commonest real failure on it.
        let e = dns01_failure_or(
            "example.com",
            "example.com",
            true,
            labelled("dns_provider_failed", "Cloudflare refused to create the challenge record"),
        );
        assert_eq!(e.0.as_u16(), 422);
        let m = message(e);
        assert!(m.contains("*.example.com"), "{m}");
        assert!(m.contains("DNS:Edit"), "{m}");
        assert!(!m.contains("port 80"), "{m}");
    }

    #[test]
    fn an_unlabelled_fault_keeps_its_incident_id() {
        // Neither label, so neither sentence — this is a 502 with a reference,
        // which is what an unwritable directory is.
        let e = dns01_failure_or(
            "blog.example.com",
            "example.com",
            false,
            AgentError::Status(500, r#"{"error":"Create cert dir: Permission denied"}"#.into()),
        );
        assert_eq!(e.0.as_u16(), 502);
        assert!(message(e).contains("Reference:"));
    }
}
