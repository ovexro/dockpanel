use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{internal_error, err, ApiError};
use crate::services::activity;
use crate::AppState;

const BUNNY_API: &str = "https://api.bunny.net";

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct CdnZone {
    pub id: Uuid,
    pub user_id: Uuid,
    pub domain: String,
    pub provider: String,
    pub pull_zone_id: Option<String>,
    #[serde(skip_serializing)]
    pub api_key: String,
    pub origin_url: Option<String>,
    pub cdn_hostname: Option<String>,
    pub enabled: bool,
    pub cache_ttl: i32,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Public-safe view (no api_key)
#[derive(serde::Serialize)]
pub struct CdnZoneView {
    pub id: Uuid,
    pub domain: String,
    pub provider: String,
    pub pull_zone_id: Option<String>,
    pub origin_url: Option<String>,
    pub cdn_hostname: Option<String>,
    pub enabled: bool,
    pub cache_ttl: i32,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<CdnZone> for CdnZoneView {
    fn from(z: CdnZone) -> Self {
        Self {
            id: z.id,
            domain: z.domain,
            provider: z.provider,
            pull_zone_id: z.pull_zone_id,
            origin_url: z.origin_url,
            cdn_hostname: z.cdn_hostname,
            enabled: z.enabled,
            cache_ttl: z.cache_ttl,
            created_at: z.created_at,
            updated_at: z.updated_at,
        }
    }
}

#[derive(serde::Deserialize)]
pub struct CreateCdnZoneRequest {
    pub domain: String,
    pub provider: Option<String>,
    pub api_key: String,
    pub pull_zone_id: Option<String>,
    pub origin_url: Option<String>,
    pub cdn_hostname: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct UpdateCdnZoneRequest {
    pub enabled: Option<bool>,
    pub cache_ttl: Option<i32>,
    pub origin_url: Option<String>,
    pub cdn_hostname: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct PurgeCacheRequest {
    pub urls: Option<Vec<String>>,
}

/// Helper: get CDN zone and verify ownership.
async fn get_zone(state: &AppState, zone_id: Uuid, user_id: Uuid) -> Result<CdnZone, ApiError> {
    sqlx::query_as::<_, CdnZone>(
        "SELECT * FROM cdn_zones WHERE id = $1 AND user_id = $2",
    )
    .bind(zone_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("cdn zone", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "CDN zone not found"))
}

fn bunny_client(_api_key: &str) -> reqwest::Client {
    reqwest::Client::new()
}

fn bunny_headers(api_key: &str) -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    if let Ok(val) = api_key.parse() {
        headers.insert("AccessKey", val);
    }
    headers.insert("Content-Type", "application/json".parse().unwrap());
    headers
}

/// GET /api/cdn/zones — List CDN zones for the current user.
pub async fn list_zones(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<Vec<CdnZoneView>>, ApiError> {
    let zones: Vec<CdnZone> = sqlx::query_as(
        "SELECT * FROM cdn_zones WHERE user_id = $1 ORDER BY created_at DESC LIMIT 100",
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("list cdn zones", e))?;

    Ok(Json(zones.into_iter().map(CdnZoneView::from).collect()))
}

/// POST /api/cdn/zones — Add a CDN zone.
pub async fn create_zone(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<CreateCdnZoneRequest>,
) -> Result<(StatusCode, Json<CdnZoneView>), ApiError> {
    if body.domain.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Domain is required"));
    }
    if body.api_key.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "API key is required"));
    }

    let provider = body.provider.as_deref().unwrap_or("bunnycdn");
    if provider != "bunnycdn" && provider != "cloudflare" {
        return Err(err(StatusCode::BAD_REQUEST, "Provider must be 'bunnycdn' or 'cloudflare'"));
    }

    // Validate API key by testing connectivity
    if provider == "bunnycdn" {
        let client = bunny_client(&body.api_key);
        let resp = client
            .get(format!("{BUNNY_API}/pullzone"))
            .headers(bunny_headers(&body.api_key))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| {
                tracing::warn!("BunnyCDN API test failed: {e}");
                err(StatusCode::BAD_REQUEST, "Failed to connect to BunnyCDN API. Check your API key.")
            })?;

        if resp.status() == 401 || resp.status() == 403 {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid BunnyCDN API key"));
        }
    }

    let zone: CdnZone = sqlx::query_as(
        "INSERT INTO cdn_zones (user_id, domain, provider, api_key, pull_zone_id, origin_url, cdn_hostname) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING *",
    )
    .bind(claims.sub)
    .bind(body.domain.trim())
    .bind(provider)
    .bind(body.api_key.trim())
    .bind(body.pull_zone_id.as_deref())
    .bind(body.origin_url.as_deref())
    .bind(body.cdn_hostname.as_deref())
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("create cdn zone", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "cdn.create",
        Some("cdn"), Some(&zone.domain), Some(provider), None,
    ).await;

    Ok((StatusCode::CREATED, Json(CdnZoneView::from(zone))))
}

/// PUT /api/cdn/zones/{id} — Update CDN zone settings.
pub async fn update_zone(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateCdnZoneRequest>,
) -> Result<Json<CdnZoneView>, ApiError> {
    let zone = get_zone(&state, id, claims.sub).await?;

    let enabled = body.enabled.unwrap_or(zone.enabled);
    let cache_ttl = body.cache_ttl.unwrap_or(zone.cache_ttl).clamp(0, 31536000);
    let origin_url = body.origin_url.as_deref().or(zone.origin_url.as_deref());
    let cdn_hostname = body.cdn_hostname.as_deref().or(zone.cdn_hostname.as_deref());

    let updated: CdnZone = sqlx::query_as(
        "UPDATE cdn_zones SET enabled = $1, cache_ttl = $2, origin_url = $3, cdn_hostname = $4, updated_at = NOW() \
         WHERE id = $5 RETURNING *",
    )
    .bind(enabled)
    .bind(cache_ttl)
    .bind(origin_url)
    .bind(cdn_hostname)
    .bind(id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("update cdn zone", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "cdn.update",
        Some("cdn"), Some(&zone.domain), None, None,
    ).await;

    Ok(Json(CdnZoneView::from(updated)))
}

/// DELETE /api/cdn/zones/{id} — Remove CDN zone.
pub async fn delete_zone(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let zone = get_zone(&state, id, claims.sub).await?;

    sqlx::query("DELETE FROM cdn_zones WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("delete cdn zone", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "cdn.delete",
        Some("cdn"), Some(&zone.domain), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/cdn/zones/{id}/purge — Purge CDN cache.
pub async fn purge_cache(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<PurgeCacheRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let zone = get_zone(&state, id, claims.sub).await?;

    // Validate purge URLs belong to this zone's domain (prevent SSRF)
    if let Some(ref urls) = body.urls {
        if urls.len() > 100 {
            return Err(err(StatusCode::BAD_REQUEST, "Maximum 100 URLs per purge request"));
        }
        for u in urls {
            if let Ok(parsed) = url::Url::parse(u) {
                let host = parsed.host_str().unwrap_or("");
                if !host.ends_with(&zone.domain) && host != zone.domain {
                    return Err(err(StatusCode::BAD_REQUEST, "Purge URL does not match zone domain"));
                }
            } else {
                return Err(err(StatusCode::BAD_REQUEST, "Invalid purge URL"));
            }
        }
    }

    // Validate pull_zone_id format (alphanumeric/hex only)
    if let Some(ref pzid) = zone.pull_zone_id {
        if !pzid.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid pull zone ID format"));
        }
    }

    match zone.provider.as_str() {
        "bunnycdn" => {
            let pull_zone_id = zone.pull_zone_id.as_deref()
                .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Pull zone ID not configured"))?;

            let client = bunny_client(&zone.api_key);
            let resp = client
                .post(format!("{BUNNY_API}/pullzone/{pull_zone_id}/purgeCache"))
                .headers(bunny_headers(&zone.api_key))
                .timeout(std::time::Duration::from_secs(15))
                .send()
                .await
                .map_err(|e| {
                    tracing::warn!("BunnyCDN purge failed: {e}");
                    err(StatusCode::BAD_GATEWAY, "Failed to purge BunnyCDN cache")
                })?;

            if !resp.status().is_success() {
                let status = resp.status().as_u16();
                let text = resp.text().await.unwrap_or_default();
                tracing::warn!("BunnyCDN purge HTTP {status}: {text}");
                return Err(err(StatusCode::BAD_GATEWAY, &format!("BunnyCDN returned HTTP {status}")));
            }
        }
        "cloudflare" => {
            let pull_zone_id = zone.pull_zone_id.as_deref()
                .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Cloudflare zone ID not configured"))?;

            let purge_body = if let Some(ref urls) = body.urls {
                serde_json::json!({ "files": urls })
            } else {
                serde_json::json!({ "purge_everything": true })
            };

            let headers = crate::helpers::cf_headers(&zone.api_key, None);
            let client = reqwest::Client::new();
            let resp = client
                .post(format!("https://api.cloudflare.com/client/v4/zones/{pull_zone_id}/purge_cache"))
                .headers(headers)
                .json(&purge_body)
                .timeout(std::time::Duration::from_secs(15))
                .send()
                .await
                .map_err(|e| {
                    tracing::warn!("Cloudflare purge failed: {e}");
                    err(StatusCode::BAD_GATEWAY, "Failed to purge Cloudflare cache")
                })?;

            if !resp.status().is_success() {
                let status = resp.status().as_u16();
                return Err(err(StatusCode::BAD_GATEWAY, &format!("Cloudflare returned HTTP {status}")));
            }
        }
        _ => return Err(err(StatusCode::BAD_REQUEST, "Unknown CDN provider")),
    }

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "cdn.purge",
        Some("cdn"), Some(&zone.domain), Some(&zone.provider), None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true, "message": "Cache purged successfully" })))
}

/// GET /api/cdn/zones/{id}/stats — Get CDN bandwidth/request stats.
pub async fn zone_stats(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let zone = get_zone(&state, id, claims.sub).await?;

    match zone.provider.as_str() {
        "bunnycdn" => {
            let pull_zone_id = zone.pull_zone_id.as_deref()
                .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Pull zone ID not configured"))?;

            // Get statistics for the last 30 days
            let date_from = (chrono::Utc::now() - chrono::Duration::days(30))
                .format("%Y-%m-%dT00:00:00Z")
                .to_string();
            let date_to = chrono::Utc::now()
                .format("%Y-%m-%dT23:59:59Z")
                .to_string();

            let client = bunny_client(&zone.api_key);
            let resp = client
                .get(format!(
                    "{BUNNY_API}/statistics?dateFrom={date_from}&dateTo={date_to}&pullZone={pull_zone_id}"
                ))
                .headers(bunny_headers(&zone.api_key))
                .timeout(std::time::Duration::from_secs(15))
                .send()
                .await
                .map_err(|e| {
                    tracing::warn!("BunnyCDN stats failed: {e}");
                    err(StatusCode::BAD_GATEWAY, "Failed to fetch BunnyCDN stats")
                })?;

            if !resp.status().is_success() {
                let status = resp.status().as_u16();
                return Err(err(StatusCode::BAD_GATEWAY, &format!("BunnyCDN returned HTTP {status}")));
            }

            let body: serde_json::Value = resp.json().await.map_err(|e| {
                tracing::warn!("BunnyCDN stats parse error: {e}");
                err(StatusCode::BAD_GATEWAY, "Invalid response from BunnyCDN")
            })?;

            Ok(Json(serde_json::json!({
                "provider": "bunnycdn",
                "period": "30d",
                "total_bandwidth": body.get("TotalBandwidthUsed").unwrap_or(&serde_json::json!(0)),
                "total_requests": body.get("TotalRequestsServed").unwrap_or(&serde_json::json!(0)),
                "cache_hit_rate": body.get("CacheHitRate").unwrap_or(&serde_json::json!(0.0)),
                "bandwidth_cached": body.get("BandwidthCachedChart"),
                "bandwidth_uncached": body.get("BandwidthUncachedChart"),
                "requests_served": body.get("RequestsServedChart"),
            })))
        }
        "cloudflare" => {
            let pull_zone_id = zone.pull_zone_id.as_deref()
                .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Cloudflare zone ID not configured"))?;

            let headers = crate::helpers::cf_headers(&zone.api_key, None);
            let client = reqwest::Client::new();

            // Get analytics for the last 30 days
            let resp = client
                .get(format!(
                    "https://api.cloudflare.com/client/v4/zones/{pull_zone_id}/analytics/dashboard?since=-43200&continuous=true"
                ))
                .headers(headers)
                .timeout(std::time::Duration::from_secs(15))
                .send()
                .await
                .map_err(|e| {
                    tracing::warn!("Cloudflare stats failed: {e}");
                    err(StatusCode::BAD_GATEWAY, "Failed to fetch Cloudflare stats")
                })?;

            let body: serde_json::Value = resp.json().await.map_err(|e| {
                tracing::warn!("Cloudflare stats parse error: {e}");
                err(StatusCode::BAD_GATEWAY, "Invalid response from Cloudflare")
            })?;

            let totals = body.pointer("/result/totals").cloned().unwrap_or(serde_json::json!({}));

            Ok(Json(serde_json::json!({
                "provider": "cloudflare",
                "period": "30d",
                "total_bandwidth": totals.pointer("/bandwidth/all").unwrap_or(&serde_json::json!(0)),
                "total_requests": totals.pointer("/requests/all").unwrap_or(&serde_json::json!(0)),
                "cached_bandwidth": totals.pointer("/bandwidth/cached").unwrap_or(&serde_json::json!(0)),
                "threats": totals.pointer("/threats/all").unwrap_or(&serde_json::json!(0)),
                "page_views": totals.pointer("/pageviews/all").unwrap_or(&serde_json::json!(0)),
            })))
        }
        _ => Err(err(StatusCode::BAD_REQUEST, "Unknown CDN provider")),
    }
}

/// POST /api/cdn/zones/{id}/test — Test CDN API credentials.
pub async fn test_credentials(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let zone = get_zone(&state, id, claims.sub).await?;

    match zone.provider.as_str() {
        "bunnycdn" => {
            let client = bunny_client(&zone.api_key);
            let resp = client
                .get(format!("{BUNNY_API}/pullzone"))
                .headers(bunny_headers(&zone.api_key))
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await
                .map_err(|e| {
                    tracing::warn!("CDN credentials test failed: {e}");
                    err(StatusCode::BAD_GATEWAY, "CDN API connection failed")
                })?;

            if resp.status().is_success() {
                Ok(Json(serde_json::json!({ "ok": true, "message": "BunnyCDN API key is valid" })))
            } else {
                Ok(Json(serde_json::json!({ "ok": false, "message": format!("BunnyCDN returned HTTP {}", resp.status()) })))
            }
        }
        "cloudflare" => {
            let headers = crate::helpers::cf_headers(&zone.api_key, None);
            let client = reqwest::Client::new();
            let resp = client
                .get("https://api.cloudflare.com/client/v4/user/tokens/verify")
                .headers(headers)
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await
                .map_err(|e| {
                    tracing::warn!("CDN credentials test failed: {e}");
                    err(StatusCode::BAD_GATEWAY, "CDN API connection failed")
                })?;

            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let success = body.get("success").and_then(|v| v.as_bool()).unwrap_or(false);

            Ok(Json(serde_json::json!({
                "ok": success,
                "message": if success { "Cloudflare API token is valid" } else { "Cloudflare API token is invalid" },
            })))
        }
        _ => Err(err(StatusCode::BAD_REQUEST, "Unknown CDN provider")),
    }
}

/// GET /api/cdn/zones/{id}/pull-zones — List available pull zones from the provider.
pub async fn list_pull_zones(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let zone = get_zone(&state, id, claims.sub).await?;

    match zone.provider.as_str() {
        "bunnycdn" => {
            let client = bunny_client(&zone.api_key);
            let resp = client
                .get(format!("{BUNNY_API}/pullzone"))
                .headers(bunny_headers(&zone.api_key))
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await
                .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Failed to list pull zones: {e}")))?;

            if !resp.status().is_success() {
                return Err(err(StatusCode::BAD_GATEWAY, "Failed to list BunnyCDN pull zones"));
            }

            let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::json!([]));

            // Extract just the useful fields
            let zones: Vec<serde_json::Value> = match body.as_array() {
                Some(arr) => arr.iter().map(|z| serde_json::json!({
                    "id": z.get("Id"),
                    "name": z.get("Name"),
                    "origin_url": z.get("OriginUrl"),
                    "hostnames": z.get("Hostnames"),
                    "bandwidth_used": z.get("MonthlyBandwidthUsed"),
                    "enabled": z.get("Enabled"),
                })).collect(),
                None => Vec::new(),
            };

            Ok(Json(serde_json::json!({ "pull_zones": zones })))
        }
        "cloudflare" => {
            // For Cloudflare, list zones
            let headers = crate::helpers::cf_headers(&zone.api_key, None);
            let client = reqwest::Client::new();
            let resp = client
                .get("https://api.cloudflare.com/client/v4/zones?per_page=50")
                .headers(headers)
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await
                .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Failed to list zones: {e}")))?;

            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let zones: Vec<serde_json::Value> = body.pointer("/result")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().map(|z| serde_json::json!({
                    "id": z.get("id"),
                    "name": z.get("name"),
                    "status": z.get("status"),
                    "plan": z.pointer("/plan/name"),
                })).collect())
                .unwrap_or_default();

            Ok(Json(serde_json::json!({ "pull_zones": zones })))
        }
        _ => Err(err(StatusCode::BAD_REQUEST, "Unknown CDN provider")),
    }
}
