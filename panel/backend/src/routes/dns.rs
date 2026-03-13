use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{err, ApiError};
use crate::services::activity;
use crate::AppState;

const CF_API: &str = "https://api.cloudflare.com/client/v4";

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct DnsZone {
    pub id: Uuid,
    pub user_id: Uuid,
    pub domain: String,
    pub provider: String,
    pub cf_zone_id: String,
    #[serde(skip_serializing)]
    pub cf_api_token: String,
    #[serde(skip_serializing)]
    pub cf_api_email: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Deserialize)]
pub struct CreateZoneRequest {
    pub domain: String,
    pub cf_zone_id: String,
    pub cf_api_token: String,
    pub cf_api_email: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct CreateRecordRequest {
    #[serde(rename = "type")]
    pub rtype: String,
    pub name: String,
    pub content: String,
    pub ttl: Option<u32>,
    pub proxied: Option<bool>,
    pub priority: Option<u16>,
}

#[derive(serde::Deserialize)]
pub struct UpdateRecordRequest {
    #[serde(rename = "type")]
    pub rtype: String,
    pub name: String,
    pub content: String,
    pub ttl: Option<u32>,
    pub proxied: Option<bool>,
    pub priority: Option<u16>,
}

/// Helper: get zone and verify ownership.
async fn get_zone(state: &AppState, zone_id: Uuid, user_id: Uuid) -> Result<DnsZone, ApiError> {
    sqlx::query_as::<_, DnsZone>(
        "SELECT * FROM dns_zones WHERE id = $1 AND user_id = $2",
    )
    .bind(zone_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "DNS zone not found"))
}

/// Helper: build reqwest client with CF auth.
/// If email is provided, use Global API Key auth (X-Auth-Email + X-Auth-Key).
/// Otherwise, use API Token auth (Bearer token).
fn cf_client(token: &str, email: Option<&str>) -> Result<(reqwest::Client, reqwest::header::HeaderMap), ApiError> {
    let client = reqwest::Client::new();
    let mut headers = reqwest::header::HeaderMap::new();
    if let Some(email) = email {
        headers.insert(
            "X-Auth-Email",
            reqwest::header::HeaderValue::from_str(email)
                .map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid email"))?,
        );
        headers.insert(
            "X-Auth-Key",
            reqwest::header::HeaderValue::from_str(token)
                .map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid API key"))?,
        );
    } else {
        headers.insert(
            "Authorization",
            reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid API token"))?,
        );
    }
    headers.insert(
        "Content-Type",
        reqwest::header::HeaderValue::from_static("application/json"),
    );
    Ok((client, headers))
}

/// GET /api/dns/zones — List DNS zones.
pub async fn list_zones(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let zones: Vec<DnsZone> = sqlx::query_as(
        "SELECT * FROM dns_zones WHERE user_id = $1 ORDER BY domain",
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    // Return without the token
    let result: Vec<serde_json::Value> = zones
        .iter()
        .map(|z| {
            serde_json::json!({
                "id": z.id,
                "domain": z.domain,
                "provider": z.provider,
                "cf_zone_id": z.cf_zone_id,
                "created_at": z.created_at,
            })
        })
        .collect();

    Ok(Json(result))
}

/// POST /api/dns/zones — Add a DNS zone (validates CF credentials).
pub async fn create_zone(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<CreateZoneRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    if body.domain.trim().is_empty() || body.cf_zone_id.trim().is_empty() || body.cf_api_token.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "All fields are required"));
    }

    // Validate CF credentials by fetching zone details
    let (client, headers) = cf_client(&body.cf_api_token, body.cf_api_email.as_deref())?;
    let resp = client
        .get(&format!("{CF_API}/zones/{}", body.cf_zone_id))
        .headers(headers)
        .send()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Cloudflare API error: {e}")))?;

    let cf_resp: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Invalid CF response: {e}")))?;

    if !cf_resp.get("success").and_then(|v| v.as_bool()).unwrap_or(false) {
        let errors = cf_resp.get("errors").cloned().unwrap_or_default();
        return Err(err(
            StatusCode::BAD_REQUEST,
            &format!("Cloudflare rejected credentials: {errors}"),
        ));
    }

    // Insert zone
    let zone: DnsZone = sqlx::query_as(
        "INSERT INTO dns_zones (user_id, domain, cf_zone_id, cf_api_token, cf_api_email) \
         VALUES ($1, $2, $3, $4, $5) RETURNING *",
    )
    .bind(claims.sub)
    .bind(body.domain.trim())
    .bind(body.cf_zone_id.trim())
    .bind(body.cf_api_token.trim())
    .bind(body.cf_api_email.as_deref().map(|s| s.trim()))
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        if e.to_string().contains("duplicate") {
            err(StatusCode::CONFLICT, "Zone already exists")
        } else {
            err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
        }
    })?;

    tracing::info!("DNS zone added: {} by {}", zone.domain, claims.email);
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "dns.zone.create",
        Some("dns"), Some(&zone.domain), None, None,
    ).await;

    Ok((StatusCode::CREATED, Json(serde_json::json!({
        "id": zone.id,
        "domain": zone.domain,
        "provider": zone.provider,
        "cf_zone_id": zone.cf_zone_id,
        "created_at": zone.created_at,
    }))))
}

/// DELETE /api/dns/zones/{id} — Remove a DNS zone.
pub async fn delete_zone(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let zone = get_zone(&state, id, claims.sub).await?;

    sqlx::query("DELETE FROM dns_zones WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    tracing::info!("DNS zone removed: {}", zone.domain);

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /api/dns/zones/{id}/records — List DNS records (proxy to CF).
pub async fn list_records(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let zone = get_zone(&state, id, claims.sub).await?;
    let (client, headers) = cf_client(&zone.cf_api_token, zone.cf_api_email.as_deref())?;

    let resp = client
        .get(&format!(
            "{CF_API}/zones/{}/dns_records?per_page=100&order=type",
            zone.cf_zone_id
        ))
        .headers(headers)
        .send()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("CF API error: {e}")))?;

    let cf_resp: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Invalid CF response: {e}")))?;

    if !cf_resp.get("success").and_then(|v| v.as_bool()).unwrap_or(false) {
        return Err(err(StatusCode::BAD_GATEWAY, "Failed to fetch DNS records from Cloudflare"));
    }

    // Return the result array with only needed fields
    let records = cf_resp.get("result").cloned().unwrap_or(serde_json::json!([]));

    Ok(Json(serde_json::json!({
        "records": records,
        "domain": zone.domain,
    })))
}

/// POST /api/dns/zones/{id}/records — Create a DNS record.
pub async fn create_record(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<CreateRecordRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let zone = get_zone(&state, id, claims.sub).await?;

    // Validate record type
    let allowed_types = ["A", "AAAA", "CNAME", "MX", "TXT", "NS", "SRV", "CAA"];
    if !allowed_types.contains(&body.rtype.as_str()) {
        return Err(err(StatusCode::BAD_REQUEST, &format!("Unsupported record type: {}", body.rtype)));
    }

    let (client, headers) = cf_client(&zone.cf_api_token, zone.cf_api_email.as_deref())?;

    let mut cf_body = serde_json::json!({
        "type": body.rtype,
        "name": body.name,
        "content": body.content,
        "ttl": body.ttl.unwrap_or(1), // 1 = automatic in CF
    });

    // Proxied only valid for A/AAAA/CNAME
    if ["A", "AAAA", "CNAME"].contains(&body.rtype.as_str()) {
        cf_body["proxied"] = serde_json::json!(body.proxied.unwrap_or(false));
    }

    if body.rtype == "MX" {
        cf_body["priority"] = serde_json::json!(body.priority.unwrap_or(10));
    }

    let resp = client
        .post(&format!("{CF_API}/zones/{}/dns_records", zone.cf_zone_id))
        .headers(headers)
        .json(&cf_body)
        .send()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("CF API error: {e}")))?;

    let cf_resp: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Invalid CF response: {e}")))?;

    if !cf_resp.get("success").and_then(|v| v.as_bool()).unwrap_or(false) {
        let errors = cf_resp.get("errors").cloned().unwrap_or_default();
        return Err(err(StatusCode::UNPROCESSABLE_ENTITY, &format!("Cloudflare error: {errors}")));
    }

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "dns.record.create",
        Some("dns"), Some(&zone.domain), Some(&format!("{} {}", body.rtype, body.name)), None,
    ).await;

    Ok((StatusCode::CREATED, Json(cf_resp.get("result").cloned().unwrap_or_default())))
}

/// PUT /api/dns/zones/{id}/records/{record_id} — Update a DNS record.
pub async fn update_record(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path((id, record_id)): Path<(Uuid, String)>,
    Json(body): Json<UpdateRecordRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let zone = get_zone(&state, id, claims.sub).await?;

    // Validate record ID format (CF uses hex IDs)
    if record_id.is_empty() || record_id.len() > 64 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid record ID"));
    }

    let (client, headers) = cf_client(&zone.cf_api_token, zone.cf_api_email.as_deref())?;

    let mut cf_body = serde_json::json!({
        "type": body.rtype,
        "name": body.name,
        "content": body.content,
        "ttl": body.ttl.unwrap_or(1),
    });

    if ["A", "AAAA", "CNAME"].contains(&body.rtype.as_str()) {
        cf_body["proxied"] = serde_json::json!(body.proxied.unwrap_or(false));
    }

    if body.rtype == "MX" {
        cf_body["priority"] = serde_json::json!(body.priority.unwrap_or(10));
    }

    let resp = client
        .put(&format!(
            "{CF_API}/zones/{}/dns_records/{record_id}",
            zone.cf_zone_id
        ))
        .headers(headers)
        .json(&cf_body)
        .send()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("CF API error: {e}")))?;

    let cf_resp: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Invalid CF response: {e}")))?;

    if !cf_resp.get("success").and_then(|v| v.as_bool()).unwrap_or(false) {
        let errors = cf_resp.get("errors").cloned().unwrap_or_default();
        return Err(err(StatusCode::UNPROCESSABLE_ENTITY, &format!("Cloudflare error: {errors}")));
    }

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "dns.record.update",
        Some("dns"), Some(&zone.domain), Some(&format!("{} {}", body.rtype, body.name)), None,
    ).await;

    Ok(Json(cf_resp.get("result").cloned().unwrap_or_default()))
}

/// DELETE /api/dns/zones/{id}/records/{record_id} — Delete a DNS record.
pub async fn delete_record(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path((id, record_id)): Path<(Uuid, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let zone = get_zone(&state, id, claims.sub).await?;

    if record_id.is_empty() || record_id.len() > 64 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid record ID"));
    }

    let (client, headers) = cf_client(&zone.cf_api_token, zone.cf_api_email.as_deref())?;

    let resp = client
        .delete(&format!(
            "{CF_API}/zones/{}/dns_records/{record_id}",
            zone.cf_zone_id
        ))
        .headers(headers)
        .send()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("CF API error: {e}")))?;

    let cf_resp: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Invalid CF response: {e}")))?;

    if !cf_resp.get("success").and_then(|v| v.as_bool()).unwrap_or(false) {
        let errors = cf_resp.get("errors").cloned().unwrap_or_default();
        return Err(err(StatusCode::UNPROCESSABLE_ENTITY, &format!("Cloudflare error: {errors}")));
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}
