use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{err, agent_error, ApiError};
use crate::services::activity;
use crate::AppState;

const CF_API: &str = "https://api.cloudflare.com/client/v4";

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct DnsZone {
    pub id: Uuid,
    pub user_id: Uuid,
    pub domain: String,
    pub provider: String,
    pub cf_zone_id: Option<String>,
    #[serde(skip_serializing)]
    pub cf_api_token: Option<String>,
    #[serde(skip_serializing)]
    pub cf_api_email: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Deserialize)]
pub struct CreateZoneRequest {
    pub domain: String,
    pub provider: Option<String>, // "cloudflare" (default) or "powerdns"
    pub cf_zone_id: Option<String>,
    pub cf_api_token: Option<String>,
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

// ── Cloudflare helpers ──────────────────────────────────────────────────

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

// ── PowerDNS helpers ────────────────────────────────────────────────────

/// Get PowerDNS settings from DB.
async fn pdns_settings(state: &AppState) -> Result<(String, String), ApiError> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT key, value FROM settings WHERE key IN ('pdns_api_url', 'pdns_api_key')",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let mut url = String::new();
    let mut key = String::new();
    for (k, v) in rows {
        match k.as_str() {
            "pdns_api_url" => url = v,
            "pdns_api_key" => key = v,
            _ => {}
        }
    }

    if url.is_empty() || key.is_empty() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "PowerDNS not configured. Set API URL and API Key in Settings.",
        ));
    }

    Ok((url, key))
}

/// Build reqwest client for PowerDNS API.
fn pdns_client(api_key: &str) -> Result<(reqwest::Client, reqwest::header::HeaderMap), ApiError> {
    let client = reqwest::Client::new();
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "X-API-Key",
        reqwest::header::HeaderValue::from_str(api_key)
            .map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid PowerDNS API key"))?,
    );
    headers.insert(
        "Content-Type",
        reqwest::header::HeaderValue::from_static("application/json"),
    );
    Ok((client, headers))
}

/// Ensure domain ends with a dot (FQDN) for PowerDNS.
fn fqdn(domain: &str) -> String {
    if domain.ends_with('.') {
        domain.to_string()
    } else {
        format!("{domain}.")
    }
}

/// Strip trailing dot from FQDN for display.
fn strip_dot(name: &str) -> String {
    name.trim_end_matches('.').to_string()
}

/// Create a synthetic record ID for PowerDNS records (name|type|content).
fn pdns_record_id(name: &str, rtype: &str, content: &str) -> String {
    // URL-safe: hex-encode the composite key
    hex::encode(format!("{}\0{}\0{}", name, rtype, content))
}

/// Parse a synthetic PowerDNS record ID back to (name, type, content).
fn pdns_parse_record_id(id: &str) -> Result<(String, String, String), ApiError> {
    let bytes = hex::decode(id).map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid record ID"))?;
    let s = String::from_utf8(bytes).map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid record ID"))?;
    let parts: Vec<&str> = s.splitn(3, '\0').collect();
    if parts.len() != 3 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid record ID"));
    }
    Ok((parts[0].to_string(), parts[1].to_string(), parts[2].to_string()))
}

/// Flatten PowerDNS rrsets into individual records matching the Cloudflare format.
fn pdns_flatten_records(rrsets: &[serde_json::Value]) -> Vec<serde_json::Value> {
    let mut records = Vec::new();
    for rrset in rrsets {
        let name = rrset.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let rtype = rrset.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let ttl = rrset.get("ttl").and_then(|v| v.as_u64()).unwrap_or(3600);

        // Skip SOA and internal records
        if rtype == "SOA" {
            continue;
        }

        let recs = rrset.get("records").and_then(|v| v.as_array());
        if let Some(recs) = recs {
            for rec in recs {
                let content = rec.get("content").and_then(|v| v.as_str()).unwrap_or("");
                let disabled = rec.get("disabled").and_then(|v| v.as_bool()).unwrap_or(false);
                if disabled {
                    continue;
                }

                // Extract priority from MX/SRV content (PowerDNS includes it in content)
                let (priority, clean_content) = if rtype == "MX" {
                    let parts: Vec<&str> = content.splitn(2, ' ').collect();
                    if parts.len() == 2 {
                        (parts[0].parse::<u16>().ok(), parts[1].to_string())
                    } else {
                        (None, content.to_string())
                    }
                } else {
                    (None, content.to_string())
                };

                records.push(serde_json::json!({
                    "id": pdns_record_id(name, rtype, content),
                    "type": rtype,
                    "name": strip_dot(name),
                    "content": strip_dot(&clean_content),
                    "ttl": ttl,
                    "priority": priority,
                }));
            }
        }
    }
    records
}

// ── Route handlers ──────────────────────────────────────────────────────

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

/// POST /api/dns/zones — Add a DNS zone.
pub async fn create_zone(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<CreateZoneRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let provider = body.provider.as_deref().unwrap_or("cloudflare");

    if body.domain.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Domain is required"));
    }

    match provider {
        "cloudflare" => {
            let cf_zone_id = body.cf_zone_id.as_deref().unwrap_or("").trim();
            let cf_api_token = body.cf_api_token.as_deref().unwrap_or("").trim();
            if cf_zone_id.is_empty() || cf_api_token.is_empty() {
                return Err(err(StatusCode::BAD_REQUEST, "Cloudflare Zone ID and API token are required"));
            }

            // Validate CF credentials
            let (client, headers) = cf_client(cf_api_token, body.cf_api_email.as_deref())?;
            let resp = client
                .get(&format!("{CF_API}/zones/{cf_zone_id}"))
                .headers(headers)
                .send()
                .await
                .map_err(|e| agent_error("Cloudflare API", e))?;

            let cf_resp: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| agent_error("Cloudflare response", e))?;

            if !cf_resp.get("success").and_then(|v| v.as_bool()).unwrap_or(false) {
                let errors = cf_resp.get("errors").cloned().unwrap_or_default();
                return Err(err(
                    StatusCode::BAD_REQUEST,
                    &format!("Cloudflare rejected credentials: {errors}"),
                ));
            }

            let zone: DnsZone = sqlx::query_as(
                "INSERT INTO dns_zones (user_id, domain, provider, cf_zone_id, cf_api_token, cf_api_email) \
                 VALUES ($1, $2, 'cloudflare', $3, $4, $5) RETURNING *",
            )
            .bind(claims.sub)
            .bind(body.domain.trim())
            .bind(cf_zone_id)
            .bind(cf_api_token)
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

            tracing::info!("DNS zone added (cloudflare): {} by {}", zone.domain, claims.email);
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
        "powerdns" => {
            let (pdns_url, pdns_key) = pdns_settings(&state).await?;
            let (client, headers) = pdns_client(&pdns_key)?;
            let zone_fqdn = fqdn(body.domain.trim());

            // Create zone in PowerDNS
            let pdns_body = serde_json::json!({
                "name": zone_fqdn,
                "kind": "Native",
                "nameservers": [],
                "soa_edit_api": "DEFAULT",
            });

            let resp = client
                .post(&format!("{pdns_url}/api/v1/servers/localhost/zones"))
                .headers(headers)
                .json(&pdns_body)
                .send()
                .await
                .map_err(|e| agent_error("PowerDNS API", e))?;

            let status = resp.status();
            if !status.is_success() {
                let body_text = resp.text().await.unwrap_or_default();
                // 409 = zone already exists in PowerDNS, we can still track it
                if status.as_u16() != 409 {
                    return Err(err(StatusCode::BAD_GATEWAY, &format!("PowerDNS error: {body_text}")));
                }
            }

            let zone: DnsZone = sqlx::query_as(
                "INSERT INTO dns_zones (user_id, domain, provider) \
                 VALUES ($1, $2, 'powerdns') RETURNING *",
            )
            .bind(claims.sub)
            .bind(body.domain.trim())
            .fetch_one(&state.db)
            .await
            .map_err(|e| {
                if e.to_string().contains("duplicate") {
                    err(StatusCode::CONFLICT, "Zone already exists")
                } else {
                    err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
                }
            })?;

            tracing::info!("DNS zone added (powerdns): {} by {}", zone.domain, claims.email);
            activity::log_activity(
                &state.db, claims.sub, &claims.email, "dns.zone.create",
                Some("dns"), Some(&zone.domain), Some("powerdns"), None,
            ).await;

            Ok((StatusCode::CREATED, Json(serde_json::json!({
                "id": zone.id,
                "domain": zone.domain,
                "provider": "powerdns",
                "created_at": zone.created_at,
            }))))
        }
        _ => Err(err(StatusCode::BAD_REQUEST, "Unsupported provider. Use 'cloudflare' or 'powerdns'.")),
    }
}

/// DELETE /api/dns/zones/{id} — Remove a DNS zone.
pub async fn delete_zone(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let zone = get_zone(&state, id, claims.sub).await?;

    // If PowerDNS, also delete the zone from PowerDNS server
    if zone.provider == "powerdns" {
        if let Ok((pdns_url, pdns_key)) = pdns_settings(&state).await {
            let (client, headers) = pdns_client(&pdns_key)?;
            let zone_fqdn = fqdn(&zone.domain);
            let _ = client
                .delete(&format!("{pdns_url}/api/v1/servers/localhost/zones/{zone_fqdn}"))
                .headers(headers)
                .send()
                .await;
        }
    }

    sqlx::query("DELETE FROM dns_zones WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    tracing::info!("DNS zone removed: {}", zone.domain);

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /api/dns/zones/{id}/records — List DNS records.
pub async fn list_records(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let zone = get_zone(&state, id, claims.sub).await?;

    match zone.provider.as_str() {
        "cloudflare" => {
            let token = zone.cf_api_token.as_deref().unwrap_or("");
            let (client, headers) = cf_client(token, zone.cf_api_email.as_deref())?;

            let resp = client
                .get(&format!(
                    "{CF_API}/zones/{}/dns_records?per_page=100&order=type",
                    zone.cf_zone_id.as_deref().unwrap_or("")
                ))
                .headers(headers)
                .send()
                .await
                .map_err(|e| agent_error("Cloudflare API", e))?;

            let cf_resp: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| agent_error("Cloudflare response", e))?;

            if !cf_resp.get("success").and_then(|v| v.as_bool()).unwrap_or(false) {
                return Err(err(StatusCode::BAD_GATEWAY, "Failed to fetch DNS records from Cloudflare"));
            }

            let records = cf_resp.get("result").cloned().unwrap_or(serde_json::json!([]));
            Ok(Json(serde_json::json!({
                "records": records,
                "domain": zone.domain,
                "provider": "cloudflare",
            })))
        }
        "powerdns" => {
            let (pdns_url, pdns_key) = pdns_settings(&state).await?;
            let (client, headers) = pdns_client(&pdns_key)?;
            let zone_fqdn = fqdn(&zone.domain);

            let resp = client
                .get(&format!("{pdns_url}/api/v1/servers/localhost/zones/{zone_fqdn}"))
                .headers(headers)
                .send()
                .await
                .map_err(|e| agent_error("PowerDNS API", e))?;

            if !resp.status().is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(err(StatusCode::BAD_GATEWAY, &format!("PowerDNS error: {body}")));
            }

            let pdns_resp: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| agent_error("PowerDNS response", e))?;

            let rrsets = pdns_resp.get("rrsets").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            let records = pdns_flatten_records(&rrsets);

            Ok(Json(serde_json::json!({
                "records": records,
                "domain": zone.domain,
                "provider": "powerdns",
            })))
        }
        _ => Err(err(StatusCode::BAD_REQUEST, "Unknown DNS provider")),
    }
}

/// POST /api/dns/zones/{id}/records — Create a DNS record.
pub async fn create_record(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<CreateRecordRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let zone = get_zone(&state, id, claims.sub).await?;

    let allowed_types = ["A", "AAAA", "CNAME", "MX", "TXT", "NS", "SRV", "CAA"];
    if !allowed_types.contains(&body.rtype.as_str()) {
        return Err(err(StatusCode::BAD_REQUEST, &format!("Unsupported record type: {}", body.rtype)));
    }

    match zone.provider.as_str() {
        "cloudflare" => {
            let token = zone.cf_api_token.as_deref().unwrap_or("");
            let (client, headers) = cf_client(token, zone.cf_api_email.as_deref())?;

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
                .post(&format!("{CF_API}/zones/{}/dns_records", zone.cf_zone_id.as_deref().unwrap_or("")))
                .headers(headers)
                .json(&cf_body)
                .send()
                .await
                .map_err(|e| agent_error("Cloudflare API", e))?;

            let cf_resp: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| agent_error("Cloudflare response", e))?;

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
        "powerdns" => {
            let (pdns_url, pdns_key) = pdns_settings(&state).await?;
            let (client, headers) = pdns_client(&pdns_key)?;
            let zone_fqdn = fqdn(&zone.domain);
            let ttl = body.ttl.unwrap_or(3600);

            // Build the record name as FQDN
            let rec_name = if body.name == "@" || body.name == zone.domain {
                zone_fqdn.clone()
            } else if body.name.ends_with(&zone.domain) {
                fqdn(&body.name)
            } else {
                fqdn(&format!("{}.{}", body.name, zone.domain))
            };

            // PowerDNS includes priority in content for MX
            let content = if body.rtype == "MX" {
                format!("{} {}", body.priority.unwrap_or(10), fqdn(&body.content))
            } else if body.rtype == "CNAME" || body.rtype == "NS" {
                fqdn(&body.content)
            } else {
                body.content.clone()
            };

            // First, get existing records for this name+type to merge
            let get_resp = client
                .get(&format!("{pdns_url}/api/v1/servers/localhost/zones/{zone_fqdn}"))
                .headers(headers.clone())
                .send()
                .await
                .map_err(|e| agent_error("PowerDNS API", e))?;

            let mut existing_records: Vec<serde_json::Value> = Vec::new();
            if get_resp.status().is_success() {
                let zone_data: serde_json::Value = get_resp.json().await.unwrap_or_default();
                if let Some(rrsets) = zone_data.get("rrsets").and_then(|v| v.as_array()) {
                    for rrset in rrsets {
                        let rr_name = rrset.get("name").and_then(|v| v.as_str()).unwrap_or("");
                        let rr_type = rrset.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        if rr_name == rec_name && rr_type == body.rtype {
                            if let Some(recs) = rrset.get("records").and_then(|v| v.as_array()) {
                                existing_records = recs.clone();
                            }
                            break;
                        }
                    }
                }
            }

            // Add the new record
            existing_records.push(serde_json::json!({
                "content": content,
                "disabled": false,
            }));

            let patch_body = serde_json::json!({
                "rrsets": [{
                    "name": rec_name,
                    "type": body.rtype,
                    "ttl": ttl,
                    "changetype": "REPLACE",
                    "records": existing_records,
                }]
            });

            let resp = client
                .patch(&format!("{pdns_url}/api/v1/servers/localhost/zones/{zone_fqdn}"))
                .headers(headers)
                .json(&patch_body)
                .send()
                .await
                .map_err(|e| agent_error("PowerDNS API", e))?;

            if !resp.status().is_success() {
                let body_text = resp.text().await.unwrap_or_default();
                return Err(err(StatusCode::UNPROCESSABLE_ENTITY, &format!("PowerDNS error: {body_text}")));
            }

            activity::log_activity(
                &state.db, claims.sub, &claims.email, "dns.record.create",
                Some("dns"), Some(&zone.domain), Some(&format!("{} {}", body.rtype, body.name)), None,
            ).await;

            Ok((StatusCode::CREATED, Json(serde_json::json!({
                "id": pdns_record_id(&rec_name, &body.rtype, &content),
                "type": body.rtype,
                "name": strip_dot(&rec_name),
                "content": strip_dot(&body.content),
                "ttl": ttl,
            }))))
        }
        _ => Err(err(StatusCode::BAD_REQUEST, "Unknown DNS provider")),
    }
}

/// PUT /api/dns/zones/{id}/records/{record_id} — Update a DNS record.
pub async fn update_record(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path((id, record_id)): Path<(Uuid, String)>,
    Json(body): Json<UpdateRecordRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let zone = get_zone(&state, id, claims.sub).await?;

    if record_id.is_empty() || record_id.len() > 256 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid record ID"));
    }

    match zone.provider.as_str() {
        "cloudflare" => {
            let token = zone.cf_api_token.as_deref().unwrap_or("");
            let (client, headers) = cf_client(token, zone.cf_api_email.as_deref())?;

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
                    zone.cf_zone_id.as_deref().unwrap_or("")
                ))
                .headers(headers)
                .json(&cf_body)
                .send()
                .await
                .map_err(|e| agent_error("Cloudflare API", e))?;

            let cf_resp: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| agent_error("Cloudflare response", e))?;

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
        "powerdns" => {
            let (pdns_url, pdns_key) = pdns_settings(&state).await?;
            let (client, headers) = pdns_client(&pdns_key)?;
            let zone_fqdn = fqdn(&zone.domain);

            // Parse the old record from the ID
            let (old_name, old_type, old_content) = pdns_parse_record_id(&record_id)?;
            let ttl = body.ttl.unwrap_or(3600);

            // Build new record name
            let new_name = if body.name == "@" || body.name == zone.domain {
                zone_fqdn.clone()
            } else if body.name.ends_with(&zone.domain) {
                fqdn(&body.name)
            } else {
                fqdn(&format!("{}.{}", body.name, zone.domain))
            };

            let new_content = if body.rtype == "MX" {
                format!("{} {}", body.priority.unwrap_or(10), fqdn(&body.content))
            } else if body.rtype == "CNAME" || body.rtype == "NS" {
                fqdn(&body.content)
            } else {
                body.content.clone()
            };

            // Get current rrsets for the old name+type
            let get_resp = client
                .get(&format!("{pdns_url}/api/v1/servers/localhost/zones/{zone_fqdn}"))
                .headers(headers.clone())
                .send()
                .await
                .map_err(|e| agent_error("PowerDNS API", e))?;

            let zone_data: serde_json::Value = get_resp.json().await.unwrap_or_default();
            let rrsets = zone_data.get("rrsets").and_then(|v| v.as_array()).cloned().unwrap_or_default();

            let mut patch_rrsets: Vec<serde_json::Value> = Vec::new();

            // If name+type changed, delete old and create new
            if old_name != new_name || old_type != body.rtype {
                // Remove old record from its rrset
                let mut old_records: Vec<serde_json::Value> = Vec::new();
                for rrset in &rrsets {
                    let rr_name = rrset.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let rr_type = rrset.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    if rr_name == old_name && rr_type == old_type {
                        if let Some(recs) = rrset.get("records").and_then(|v| v.as_array()) {
                            for rec in recs {
                                let c = rec.get("content").and_then(|v| v.as_str()).unwrap_or("");
                                if c != old_content {
                                    old_records.push(rec.clone());
                                }
                            }
                        }
                        break;
                    }
                }

                if old_records.is_empty() {
                    patch_rrsets.push(serde_json::json!({
                        "name": old_name,
                        "type": old_type,
                        "changetype": "DELETE",
                    }));
                } else {
                    patch_rrsets.push(serde_json::json!({
                        "name": old_name,
                        "type": old_type,
                        "ttl": ttl,
                        "changetype": "REPLACE",
                        "records": old_records,
                    }));
                }

                // Add to new rrset
                let mut new_records: Vec<serde_json::Value> = Vec::new();
                for rrset in &rrsets {
                    let rr_name = rrset.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let rr_type = rrset.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    if rr_name == new_name && rr_type == body.rtype {
                        if let Some(recs) = rrset.get("records").and_then(|v| v.as_array()) {
                            new_records = recs.clone();
                        }
                        break;
                    }
                }
                new_records.push(serde_json::json!({ "content": new_content, "disabled": false }));

                patch_rrsets.push(serde_json::json!({
                    "name": new_name,
                    "type": body.rtype,
                    "ttl": ttl,
                    "changetype": "REPLACE",
                    "records": new_records,
                }));
            } else {
                // Same name+type, just replace content in the rrset
                let mut records: Vec<serde_json::Value> = Vec::new();
                for rrset in &rrsets {
                    let rr_name = rrset.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let rr_type = rrset.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    if rr_name == old_name && rr_type == old_type {
                        if let Some(recs) = rrset.get("records").and_then(|v| v.as_array()) {
                            for rec in recs {
                                let c = rec.get("content").and_then(|v| v.as_str()).unwrap_or("");
                                if c == old_content {
                                    records.push(serde_json::json!({ "content": new_content, "disabled": false }));
                                } else {
                                    records.push(rec.clone());
                                }
                            }
                        }
                        break;
                    }
                }

                if records.is_empty() {
                    records.push(serde_json::json!({ "content": new_content, "disabled": false }));
                }

                patch_rrsets.push(serde_json::json!({
                    "name": new_name,
                    "type": body.rtype,
                    "ttl": ttl,
                    "changetype": "REPLACE",
                    "records": records,
                }));
            }

            let resp = client
                .patch(&format!("{pdns_url}/api/v1/servers/localhost/zones/{zone_fqdn}"))
                .headers(headers)
                .json(&serde_json::json!({ "rrsets": patch_rrsets }))
                .send()
                .await
                .map_err(|e| agent_error("PowerDNS API", e))?;

            if !resp.status().is_success() {
                let body_text = resp.text().await.unwrap_or_default();
                return Err(err(StatusCode::UNPROCESSABLE_ENTITY, &format!("PowerDNS error: {body_text}")));
            }

            activity::log_activity(
                &state.db, claims.sub, &claims.email, "dns.record.update",
                Some("dns"), Some(&zone.domain), Some(&format!("{} {}", body.rtype, body.name)), None,
            ).await;

            Ok(Json(serde_json::json!({
                "id": pdns_record_id(&new_name, &body.rtype, &new_content),
                "type": body.rtype,
                "name": strip_dot(&new_name),
                "content": strip_dot(&body.content),
                "ttl": ttl,
            })))
        }
        _ => Err(err(StatusCode::BAD_REQUEST, "Unknown DNS provider")),
    }
}

/// DELETE /api/dns/zones/{id}/records/{record_id} — Delete a DNS record.
pub async fn delete_record(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path((id, record_id)): Path<(Uuid, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let zone = get_zone(&state, id, claims.sub).await?;

    if record_id.is_empty() || record_id.len() > 256 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid record ID"));
    }

    match zone.provider.as_str() {
        "cloudflare" => {
            let token = zone.cf_api_token.as_deref().unwrap_or("");
            let (client, headers) = cf_client(token, zone.cf_api_email.as_deref())?;

            let resp = client
                .delete(&format!(
                    "{CF_API}/zones/{}/dns_records/{record_id}",
                    zone.cf_zone_id.as_deref().unwrap_or("")
                ))
                .headers(headers)
                .send()
                .await
                .map_err(|e| agent_error("Cloudflare API", e))?;

            let cf_resp: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| agent_error("Cloudflare response", e))?;

            if !cf_resp.get("success").and_then(|v| v.as_bool()).unwrap_or(false) {
                let errors = cf_resp.get("errors").cloned().unwrap_or_default();
                return Err(err(StatusCode::UNPROCESSABLE_ENTITY, &format!("Cloudflare error: {errors}")));
            }

            Ok(Json(serde_json::json!({ "ok": true })))
        }
        "powerdns" => {
            let (pdns_url, pdns_key) = pdns_settings(&state).await?;
            let (client, headers) = pdns_client(&pdns_key)?;
            let zone_fqdn = fqdn(&zone.domain);

            let (rec_name, rec_type, rec_content) = pdns_parse_record_id(&record_id)?;

            // Get current rrset
            let get_resp = client
                .get(&format!("{pdns_url}/api/v1/servers/localhost/zones/{zone_fqdn}"))
                .headers(headers.clone())
                .send()
                .await
                .map_err(|e| agent_error("PowerDNS API", e))?;

            let zone_data: serde_json::Value = get_resp.json().await.unwrap_or_default();
            let rrsets = zone_data.get("rrsets").and_then(|v| v.as_array()).cloned().unwrap_or_default();

            let mut remaining: Vec<serde_json::Value> = Vec::new();
            for rrset in &rrsets {
                let rr_name = rrset.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let rr_type = rrset.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if rr_name == rec_name && rr_type == rec_type {
                    if let Some(recs) = rrset.get("records").and_then(|v| v.as_array()) {
                        for rec in recs {
                            let c = rec.get("content").and_then(|v| v.as_str()).unwrap_or("");
                            if c != rec_content {
                                remaining.push(rec.clone());
                            }
                        }
                    }
                    break;
                }
            }

            let changetype = if remaining.is_empty() { "DELETE" } else { "REPLACE" };

            let mut patch_rrset = serde_json::json!({
                "name": rec_name,
                "type": rec_type,
                "changetype": changetype,
            });

            if changetype == "REPLACE" {
                patch_rrset["ttl"] = serde_json::json!(3600);
                patch_rrset["records"] = serde_json::json!(remaining);
            }

            let resp = client
                .patch(&format!("{pdns_url}/api/v1/servers/localhost/zones/{zone_fqdn}"))
                .headers(headers)
                .json(&serde_json::json!({ "rrsets": [patch_rrset] }))
                .send()
                .await
                .map_err(|e| agent_error("PowerDNS API", e))?;

            if !resp.status().is_success() {
                let body_text = resp.text().await.unwrap_or_default();
                return Err(err(StatusCode::UNPROCESSABLE_ENTITY, &format!("PowerDNS error: {body_text}")));
            }

            Ok(Json(serde_json::json!({ "ok": true })))
        }
        _ => Err(err(StatusCode::BAD_REQUEST, "Unknown DNS provider")),
    }
}
