use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::{AdminUser, AuthUser, ServerScope};
use crate::error::{internal_error, err, agent_error, ApiError};
use crate::models::Site;
use crate::AppState;
use crate::services::activity;

/// After an SSL provision/renew, re-render the FULL nginx vhost from the site's
/// current DB config. The agent's SSL provision/renew only renders a SUBSET
/// (WAF / CSP / Permissions-Policy / rate-limit / custom_nginx / bot-protection
/// all default off), so without this a renewal silently strips a hardened site's
/// security directives. `build_nginx_body` is the same canonical builder every
/// other config-rebuild path uses. Best-effort: a failure leaves the site on the
/// agent's (functional, SSL-enabled) subset config and is logged.
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
            if let Err(e) = agent
                .put(
                    &format!("/nginx/sites/{}", site.domain),
                    crate::routes::sites::build_nginx_body(&site),
                )
                .await
            {
                tracing::warn!("Full vhost rebuild after SSL op failed for {}: {e}", site.domain);
            }

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

    // Call agent to provision SSL
    let agent_path = format!("/ssl/provision/{}", site.domain);
    let result = agent
        .post(&agent_path, Some(agent_body))
        .await
        .map_err(|e| agent_error("SSL provisioning", e))?;

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

    // Update site in DB
    sqlx::query(
        "UPDATE sites SET ssl_enabled = true, ssl_cert_path = $1, ssl_key_path = $2, \
         ssl_expiry = $3, ssl_profile = $4, \
         ssl_renewal_at = NULL, ssl_renewal_checked_at = NULL, \
         updated_at = NOW() WHERE id = $5",
    )
    .bind(&cert_path)
    .bind(&key_path)
    .bind(ssl_expiry)
    .bind(profile.as_deref())
    .bind(id)
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

    let result = agent
        .post_long(
            &format!("/ssl/provision-dns01/{provision_domain}"),
            Some(agent_body),
            180,
        )
        .await
        .map_err(|e| agent_error("DNS-01 SSL", e))?;

    // Parse response
    let ssl_expiry = result
        .get("expiry")
        .and_then(|v| v.as_str())
        .and_then(crate::helpers::parse_agent_cert_expiry);

    let cert_path = result.get("cert_path").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let key_path = result.get("key_path").and_then(|v| v.as_str()).unwrap_or("").to_string();

    // Update site in DB
    sqlx::query(
        "UPDATE sites SET ssl_enabled = true, ssl_cert_path = $1, ssl_key_path = $2, \
         ssl_expiry = $3, ssl_profile = $4, \
         ssl_renewal_at = NULL, ssl_renewal_checked_at = NULL, \
         updated_at = NOW() WHERE id = $5",
    )
    .bind(&cert_path)
    .bind(&key_path)
    .bind(ssl_expiry)
    .bind(profile.as_deref())
    .bind(id)
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

    Ok(Json(serde_json::json!({
        "ssl_enabled": site.ssl_enabled,
        "cert_path": site.ssl_cert_path,
        "key_path": site.ssl_key_path,
        "expiry": site.ssl_expiry,
        "agent_status": agent_status,
    })))
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

    if !site.ssl_enabled {
        return Err(err(StatusCode::BAD_REQUEST, "SSL is not enabled for this site"));
    }

    // Renewing writes a certificate and rebuilds the vhost, both named by domain on
    // whichever host this handle points at. The row names that host; the caller's
    // selection only ever named the one they were looking at.
    let agent =
        crate::helpers::agent_for_site_server(&state, site.server_id, &site.domain).await?;

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

    let agent_path = format!("/ssl/{}/renew", site.domain);
    let result = agent
        .post_long(&agent_path, Some(agent_body), 120)
        .await
        .map_err(|e| agent_error("SSL renewal", e))?;

    // Update expiry from the renew response and clear stale ARI hints so
    // the next auto-heal cycle refetches them.
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

    rebuild_vhost_after_ssl(&state, &agent, id).await;

    tracing::info!("SSL renewed for {} by {}", site.domain, claims.email);
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "ssl.renew",
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
         ssl_expiry = NULL, ssl_profile = NULL, ssl_renewal_at = NULL, \
         ssl_renewal_checked_at = NULL, updated_at = NOW() WHERE id = $1",
    )
    .bind(id)
    .execute(&state.db)
    .await
    .map_err(|e| internal_error("ssl revoke", e))?;

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
