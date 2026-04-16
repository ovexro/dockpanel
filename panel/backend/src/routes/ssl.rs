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
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: Site = sqlx::query_as("SELECT * FROM sites WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(claims.sub)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("provision", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

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

    // DNS pre-flight: verify domain resolves to this server before ACME HTTP-01
    let server_ip = crate::helpers::detect_public_ip().await;
    if !server_ip.is_empty() {
        let lookup_host = format!("{}:80", site.domain);
        match tokio::net::lookup_host(&lookup_host).await {
            Ok(addrs) => {
                let resolved_ips: Vec<String> = addrs.map(|a| a.ip().to_string()).collect();
                if !resolved_ips.iter().any(|ip| ip == &server_ip) {
                    return Err(err(
                        StatusCode::PRECONDITION_FAILED,
                        &format!(
                            "Domain {} does not resolve to this server ({}). DNS points to: {}. \
                             Fix DNS before provisioning SSL.",
                            site.domain, server_ip, resolved_ips.join(", ")
                        ),
                    ));
                }
            }
            Err(_) => {
                return Err(err(
                    StatusCode::PRECONDITION_FAILED,
                    &format!(
                        "Domain {} could not be resolved. Ensure DNS is configured before provisioning SSL.",
                        site.domain
                    ),
                ));
            }
        }
    }

    // Get admin email for ACME registration
    let (email,): (String,) =
        sqlx::query_as("SELECT email FROM users WHERE id = $1")
            .bind(claims.sub)
            .fetch_one(&state.db)
            .await
            .map_err(|e| internal_error("provision", e))?;

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
        agent_body["php_socket"] = serde_json::json!(format!("/run/php/php{php}-fpm.sock"));
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
        .and_then(|s| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f UTC").ok())
        .map(|dt| dt.and_utc());

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
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: Site = sqlx::query_as("SELECT * FROM sites WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(claims.sub)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("dns01 provision", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

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
    let cf_api_token = zone.cf_api_token.as_deref()
        .ok_or_else(|| err(StatusCode::PRECONDITION_FAILED, "Zone has no Cloudflare API token"))?;

    // Get admin email for ACME
    let (email,): (String,) = sqlx::query_as("SELECT email FROM users WHERE id = $1")
        .bind(claims.sub)
        .fetch_one(&state.db)
        .await
        .map_err(|e| internal_error("dns01 email", e))?;

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
        .and_then(|s| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f UTC").ok())
        .map(|dt| dt.and_utc());

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

/// GET /api/sites/{id}/ssl — Get SSL status for a site.
pub async fn status(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: Site = sqlx::query_as("SELECT * FROM sites WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(claims.sub)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("status", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

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
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: Site = sqlx::query_as("SELECT * FROM sites WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("ssl renew", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    if !site.ssl_enabled {
        return Err(err(StatusCode::BAD_REQUEST, "SSL is not enabled for this site"));
    }

    // Agent renew now needs the same context as provision so it can rebuild
    // the nginx config after issuing the new cert.
    let (email,): (String,) = sqlx::query_as("SELECT email FROM users WHERE id = $1")
        .bind(site.user_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| internal_error("ssl renew email", e))?;

    let mut agent_body = serde_json::json!({
        "email": email,
        "runtime": site.runtime,
    });
    if let Some(port) = site.proxy_port {
        agent_body["proxy_port"] = serde_json::json!(port);
    }
    if let Some(ref php) = site.php_version {
        agent_body["php_socket"] = serde_json::json!(format!("/run/php/php{php}-fpm.sock"));
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
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(expiry_str, "%Y-%m-%d %H:%M:%S%.f UTC") {
            let expiry = dt.and_utc();
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
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site: Site = sqlx::query_as("SELECT * FROM sites WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("ssl revoke", e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Site not found"))?;

    if !site.ssl_enabled {
        return Err(err(StatusCode::BAD_REQUEST, "SSL is not enabled for this site"));
    }

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

    Ok(Json(serde_json::json!({
        "profiles": list,
        "default": default,
    })))
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
