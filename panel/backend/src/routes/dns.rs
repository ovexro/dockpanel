use crate::safe_cmd::safe_command;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::{AuthUser, ServerScope};
use crate::error::{internal_error, err, agent_error, require_admin, ApiError};
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
    .map_err(|e| internal_error("unknown", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "DNS zone not found"))
}

// ── Cloudflare helpers ──────────────────────────────────────────────────

fn cf_client(token: &str, email: Option<&str>) -> Result<(reqwest::Client, reqwest::header::HeaderMap), ApiError> {
    let client = reqwest::Client::new();
    let mut headers = crate::helpers::cf_headers(token, email);
    if headers.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid Cloudflare credentials"));
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
    .map_err(|e| internal_error("unknown", e))?;

    let mut url = String::new();
    let mut key_enc = String::new();
    for (k, v) in rows {
        match k.as_str() {
            "pdns_api_url" => url = v,
            "pdns_api_key" => key_enc = v,
            _ => {}
        }
    }

    if url.is_empty() || key_enc.is_empty() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "PowerDNS not configured. Set API URL and API Key in Settings.",
        ));
    }

    // Decrypt the API key (with legacy plaintext fallback)
    let key = crate::services::secrets_crypto::decrypt_credential_or_legacy(&key_enc, &state.config.jwt_secret);

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
        "SELECT * FROM dns_zones WHERE user_id = $1 ORDER BY domain LIMIT 500",
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("list zones", e))?;

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
                    internal_error("create zone", e)
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
                    internal_error("create zone", e)
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
        .map_err(|e| internal_error("delete zone", e))?;

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

            activity::log_activity(
                &state.db, claims.sub, &claims.email, "dns.record.delete",
                Some("dns"), Some(&zone.domain), None, None,
            ).await;

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

            activity::log_activity(
                &state.db, claims.sub, &claims.email, "dns.record.delete",
                Some("dns"), Some(&zone.domain), None, None,
            ).await;

            Ok(Json(serde_json::json!({ "ok": true })))
        }
        _ => Err(err(StatusCode::BAD_REQUEST, "Unknown DNS provider")),
    }
}

/// POST /api/dns/propagation — Check DNS propagation across multiple public resolvers.
pub async fn check_propagation(
    State(_state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "name required"))?;
    let rtype = body
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("A");

    // Validate inputs to prevent command injection
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
    {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid DNS name"));
    }
    let valid_types = ["A", "AAAA", "CNAME", "MX", "TXT", "NS", "SRV", "CAA"];
    if !valid_types.contains(&rtype) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid record type"));
    }

    let resolvers: &[(&str, &str)] = &[
        ("8.8.8.8", "Google"),
        ("1.1.1.1", "Cloudflare"),
        ("9.9.9.9", "Quad9"),
        ("208.67.222.222", "OpenDNS"),
        ("8.26.56.26", "Comodo"),
    ];

    let mut results = Vec::new();

    for (ip, label) in resolvers {
        let output = safe_command("dig")
            .args([
                &format!("@{ip}"),
                "+short",
                "+time=3",
                "+tries=1",
                rtype,
                name,
            ])
            .output()
            .await;

        let value = output
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();

        results.push(serde_json::json!({
            "resolver": ip,
            "label": label,
            "value": if value.is_empty() { "No response".to_string() } else { value.clone() },
            "found": !value.is_empty(),
        }));
    }

    let propagated = results
        .iter()
        .filter(|r| r["found"].as_bool() == Some(true))
        .count();

    Ok(Json(serde_json::json!({
        "name": name,
        "type": rtype,
        "results": results,
        "propagated": propagated,
        "total": resolvers.len(),
        "fully_propagated": propagated == resolvers.len(),
    })))
}

// ── DNS Health Check ────────────────────────────────────────────────────

async fn run_dig(domain: &str, rtype: &str) -> String {
    safe_command("dig")
        .args(["+short", "+time=3", "+tries=1", rtype, domain])
        .output()
        .await
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// POST /api/dns/health-check — Run DNS health checks on a domain.
pub async fn dns_health_check(
    State(_state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let domain = body
        .get("domain")
        .and_then(|v| v.as_str())
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "domain required"))?;

    // Validate domain
    if domain.is_empty()
        || domain.len() > 253
        || !domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain"));
    }

    let mut checks = Vec::new();

    // 1. SOA record
    let soa = run_dig(domain, "SOA").await;
    checks.push(serde_json::json!({
        "check": "SOA Record",
        "status": if !soa.is_empty() { "pass" } else { "fail" },
        "detail": if soa.is_empty() { "No SOA record found".to_string() } else { soa },
    }));

    // 2. NS records
    let ns = run_dig(domain, "NS").await;
    let ns_count = ns.lines().filter(|l| !l.is_empty()).count();
    checks.push(serde_json::json!({
        "check": "NS Records",
        "status": if ns_count >= 2 { "pass" } else if ns_count == 1 { "warn" } else { "fail" },
        "detail": format!("{} nameserver(s) found", ns_count),
    }));

    // 3. A record exists
    let a = run_dig(domain, "A").await;
    checks.push(serde_json::json!({
        "check": "A Record",
        "status": if !a.is_empty() { "pass" } else { "info" },
        "detail": if a.is_empty() { "No A record — domain won't resolve to an IP".to_string() } else { a },
    }));

    // 4. MX record
    let mx = run_dig(domain, "MX").await;
    checks.push(serde_json::json!({
        "check": "MX Record",
        "status": if !mx.is_empty() { "pass" } else { "info" },
        "detail": if mx.is_empty() { "No MX record — domain can't receive email".to_string() } else { mx },
    }));

    // 5. SPF
    let txt = run_dig(domain, "TXT").await;
    let has_spf = txt.contains("v=spf1");
    checks.push(serde_json::json!({
        "check": "SPF Record",
        "status": if has_spf { "pass" } else { "warn" },
        "detail": if has_spf { "SPF configured" } else { "No SPF record — email authentication missing" },
    }));

    // 6. DNSSEC
    let dnssec_output = safe_command("dig")
        .args(["+dnssec", "+short", "DNSKEY", domain])
        .output()
        .await;
    let has_dnssec = dnssec_output
        .ok()
        .map(|o| !String::from_utf8_lossy(&o.stdout).trim().is_empty())
        .unwrap_or(false);
    checks.push(serde_json::json!({
        "check": "DNSSEC",
        "status": if has_dnssec { "pass" } else { "info" },
        "detail": if has_dnssec { "DNSSEC is active" } else { "DNSSEC not configured (optional)" },
    }));

    let pass_count = checks.iter().filter(|c| c["status"] == "pass").count();
    let fail_count = checks.iter().filter(|c| c["status"] == "fail").count();

    Ok(Json(serde_json::json!({
        "domain": domain,
        "checks": checks,
        "pass": pass_count,
        "fail": fail_count,
        "total": checks.len(),
    })))
}

// ── DNSSEC Status ───────────────────────────────────────────────────────

/// GET /api/dns/zones/{id}/dnssec — Get DNSSEC status for a zone.
pub async fn dnssec_status(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let zone = get_zone(&state, id, claims.sub).await?;

    if zone.provider != "cloudflare" {
        return Ok(Json(serde_json::json!({
            "supported": false,
            "message": "DNSSEC management only available for Cloudflare zones",
        })));
    }

    let token = zone.cf_api_token.as_deref().unwrap_or("");
    let (client, headers) = cf_client(token, zone.cf_api_email.as_deref())?;

    let resp = client
        .get(&format!(
            "{CF_API}/zones/{}/dnssec",
            zone.cf_zone_id.as_deref().unwrap_or("")
        ))
        .headers(headers)
        .send()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &e.to_string()))?;

    let data: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &e.to_string()))?;
    let result = data.get("result").cloned().unwrap_or(serde_json::json!({}));
    let status = result
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    Ok(Json(serde_json::json!({
        "supported": true,
        "status": status,
        "active": status == "active",
        "ds_record": result.get("ds").cloned(),
        "key_tag": result.get("key_tag").cloned(),
        "algorithm": result.get("algorithm").cloned(),
    })))
}

// ── DNS Changelog ───────────────────────────────────────────────────────

/// GET /api/dns/zones/{id}/changelog — Get DNS change history.
pub async fn dns_changelog(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let zone = get_zone(&state, id, claims.sub).await?;

    let entries: Vec<(String, String, Option<String>, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as(
            "SELECT action, COALESCE(user_email, ''), details, created_at FROM activity_logs \
             WHERE action LIKE 'dns.%' AND target_name = $1 ORDER BY created_at DESC LIMIT 50",
        )
        .bind(&zone.domain)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    let logs: Vec<serde_json::Value> = entries
        .iter()
        .map(|(action, email, details, time)| {
            serde_json::json!({
                "action": action,
                "user": email,
                "details": details,
                "time": time,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({ "entries": logs })))
}

// ── DNS Analytics ───────────────────────────────────────────────────────

/// GET /api/dns/zones/{id}/analytics — Get DNS query analytics (Cloudflare only).
pub async fn dns_analytics(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let zone = get_zone(&state, id, claims.sub).await?;

    if zone.provider != "cloudflare" {
        return Ok(Json(serde_json::json!({
            "supported": false,
            "message": "Analytics only available for Cloudflare zones",
        })));
    }

    let token = zone.cf_api_token.as_deref().unwrap_or("");
    let (client, headers) = cf_client(token, zone.cf_api_email.as_deref())?;

    // Fetch DNS analytics for last 24 hours
    let since = (chrono::Utc::now() - chrono::Duration::hours(24))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let until = chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();

    let resp = client
        .get(&format!(
            "{CF_API}/zones/{}/dns_analytics/report?since={since}&until={until}&metrics=queryCount,responseTimeAvg&dimensions=queryType",
            zone.cf_zone_id.as_deref().unwrap_or("")
        ))
        .headers(headers)
        .send()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &e.to_string()))?;

    let status = resp.status();
    let data: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &e.to_string()))?;

    if !status.is_success() {
        // Analytics API might not be available on free plans
        return Ok(Json(serde_json::json!({
            "supported": true,
            "available": false,
            "message": "DNS analytics not available (may require paid Cloudflare plan)",
        })));
    }

    let result = data
        .get("result")
        .cloned()
        .unwrap_or(serde_json::json!({}));
    let totals = result
        .get("totals")
        .cloned()
        .unwrap_or(serde_json::json!({}));
    let rows = result
        .get("data")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let total_queries = totals
        .get("queryCount")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let avg_response = totals
        .get("responseTimeAvg")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);

    let by_type: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            let dims = r.get("dimensions").and_then(|v| v.as_array());
            let metrics = r.get("metrics").and_then(|v| v.as_array());
            serde_json::json!({
                "type": dims.and_then(|d| d.first()).and_then(|v| v.as_str()).unwrap_or(""),
                "queries": metrics.and_then(|m| m.first()).and_then(|v| v.as_u64()).unwrap_or(0),
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "supported": true,
        "available": true,
        "total_queries": total_queries,
        "avg_response_ms": (avg_response * 1000.0).round() / 1000.0,
        "by_type": by_type,
        "period": "24h",
    })))
}

// ── Cloudflare Zone Settings ───────────────────────────────────────────

/// GET /api/dns/zones/{id}/cf/settings — Get Cloudflare zone settings.
pub async fn cf_zone_settings(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let zone = get_zone(&state, id, claims.sub).await?;

    if zone.provider != "cloudflare" {
        return Ok(Json(serde_json::json!({
            "supported": false,
            "message": "Settings management only available for Cloudflare zones",
        })));
    }

    let token = zone.cf_api_token.as_deref().unwrap_or("");
    let zone_id = zone.cf_zone_id.as_deref().unwrap_or("");
    let (client, headers) = cf_client(token, zone.cf_api_email.as_deref())?;

    // Fetch multiple settings in parallel
    let settings_keys = ["security_level", "development_mode", "ssl", "always_use_https", "min_tls_version"];
    let mut results = serde_json::Map::new();

    for key in &settings_keys {
        let resp = client
            .get(&format!("{CF_API}/zones/{zone_id}/settings/{key}"))
            .headers(headers.clone())
            .send()
            .await
            .map_err(|e| err(StatusCode::BAD_GATEWAY, &e.to_string()))?;

        let data: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| err(StatusCode::BAD_GATEWAY, &e.to_string()))?;

        if let Some(result) = data.get("result") {
            if let Some(value) = result.get("value") {
                results.insert(key.to_string(), value.clone());
            }
        }
    }

    Ok(Json(serde_json::json!({
        "supported": true,
        "settings": results,
    })))
}

/// PUT /api/dns/zones/{id}/cf/settings — Update a Cloudflare zone setting.
pub async fn cf_update_setting(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let zone = get_zone(&state, id, claims.sub).await?;

    if zone.provider != "cloudflare" {
        return Err(err(StatusCode::BAD_REQUEST, "Only available for Cloudflare zones"));
    }

    let setting = body.get("setting").and_then(|v| v.as_str())
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing 'setting' field"))?;
    let value = body.get("value")
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing 'value' field"))?;

    // Whitelist allowed settings
    const ALLOWED: &[&str] = &[
        "security_level", "development_mode", "ssl", "always_use_https", "min_tls_version",
    ];
    if !ALLOWED.contains(&setting) {
        return Err(err(StatusCode::BAD_REQUEST, &format!(
            "Setting '{setting}' not allowed. Allowed: {}", ALLOWED.join(", ")
        )));
    }

    // Validate values
    match setting {
        "security_level" => {
            let v = value.as_str().unwrap_or("");
            if !["off", "essentially_off", "low", "medium", "high", "under_attack"].contains(&v) {
                return Err(err(StatusCode::BAD_REQUEST, "Invalid security_level value"));
            }
        }
        "development_mode" => {
            if !value.is_string() || !["on", "off"].contains(&value.as_str().unwrap_or("")) {
                return Err(err(StatusCode::BAD_REQUEST, "development_mode must be 'on' or 'off'"));
            }
        }
        "ssl" => {
            let v = value.as_str().unwrap_or("");
            if !["off", "flexible", "full", "strict"].contains(&v) {
                return Err(err(StatusCode::BAD_REQUEST, "Invalid ssl value. Use: off, flexible, full, strict"));
            }
        }
        "always_use_https" => {
            if !value.is_string() || !["on", "off"].contains(&value.as_str().unwrap_or("")) {
                return Err(err(StatusCode::BAD_REQUEST, "always_use_https must be 'on' or 'off'"));
            }
        }
        "min_tls_version" => {
            let v = value.as_str().unwrap_or("");
            if !["1.0", "1.1", "1.2", "1.3"].contains(&v) {
                return Err(err(StatusCode::BAD_REQUEST, "Invalid min_tls_version. Use: 1.0, 1.1, 1.2, 1.3"));
            }
        }
        _ => {}
    }

    let token = zone.cf_api_token.as_deref().unwrap_or("");
    let zone_id = zone.cf_zone_id.as_deref().unwrap_or("");
    let (client, headers) = cf_client(token, zone.cf_api_email.as_deref())?;

    let resp = client
        .patch(&format!("{CF_API}/zones/{zone_id}/settings/{setting}"))
        .headers(headers)
        .json(&serde_json::json!({ "value": value }))
        .send()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &e.to_string()))?;

    let status = resp.status();
    let data: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &e.to_string()))?;

    if !status.is_success() {
        let cf_err = data.get("errors")
            .and_then(|e| e.as_array())
            .and_then(|a| a.first())
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("Unknown Cloudflare error");
        return Err(err(StatusCode::BAD_GATEWAY, cf_err));
    }

    tracing::info!("CF setting updated: {setting}={value} for zone {}", zone.domain);
    activity::log_activity(
        &state.db, claims.sub, &claims.email,
        &format!("dns.cf.setting.{setting}"),
        Some("dns_zone"), Some(&zone.domain), None, None,
    ).await;

    Ok(Json(serde_json::json!({
        "ok": true,
        "setting": setting,
        "value": value,
    })))
}

// ── Cloudflare Cache Purge ─────────────────────────────────────────────

/// POST /api/dns/zones/{id}/cf/cache/purge — Purge Cloudflare cache.
pub async fn cf_purge_cache(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    let zone = get_zone(&state, id, claims.sub).await?;

    if zone.provider != "cloudflare" {
        return Err(err(StatusCode::BAD_REQUEST, "Cache purge only available for Cloudflare zones"));
    }

    let token = zone.cf_api_token.as_deref().unwrap_or("");
    let zone_id = zone.cf_zone_id.as_deref().unwrap_or("");
    let (client, headers) = cf_client(token, zone.cf_api_email.as_deref())?;

    // Build purge request: either purge_everything or specific files
    let purge_body = if let Some(files) = body.get("files").and_then(|f| f.as_array()) {
        if files.is_empty() || files.len() > 30 {
            return Err(err(StatusCode::BAD_REQUEST, "Provide 1-30 URLs to purge"));
        }
        serde_json::json!({ "files": files })
    } else {
        serde_json::json!({ "purge_everything": true })
    };

    let resp = client
        .post(&format!("{CF_API}/zones/{zone_id}/purge_cache"))
        .headers(headers)
        .json(&purge_body)
        .send()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &e.to_string()))?;

    let status = resp.status();
    let data: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &e.to_string()))?;

    if !status.is_success() {
        let cf_err = data.get("errors")
            .and_then(|e| e.as_array())
            .and_then(|a| a.first())
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("Cloudflare cache purge failed");
        return Err(err(StatusCode::BAD_GATEWAY, cf_err));
    }

    let purge_type = if body.get("files").is_some() { "selective" } else { "full" };
    tracing::info!("CF cache purge ({purge_type}) for zone {}", zone.domain);
    activity::log_activity(
        &state.db, claims.sub, &claims.email,
        &format!("dns.cf.cache.purge.{purge_type}"),
        Some("dns_zone"), Some(&zone.domain), None, None,
    ).await;

    Ok(Json(serde_json::json!({
        "ok": true,
        "purge_type": purge_type,
    })))
}

/// POST /api/tunnel/configure — Configure Cloudflare Tunnel with token.
pub async fn configure_tunnel(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let token = body.get("token").and_then(|v| v.as_str()).unwrap_or("");
    if token.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Missing tunnel token"));
    }

    let result = agent
        .post("/services/cloudflared/configure", Some(serde_json::json!({ "token": token })))
        .await
        .map_err(|e| agent_error("Tunnel configure", e))?;

    // Store token hash in settings (for display purposes, not the actual token)
    let _ = sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('tunnel_configured', 'true') \
         ON CONFLICT (key) DO UPDATE SET value = 'true'"
    )
    .execute(&state.db)
    .await;

    activity::log_activity(
        &state.db, claims.sub, &claims.email,
        "tunnel.configured", Some("tunnel"), None, None, None,
    ).await;

    Ok(Json(result))
}

/// GET /api/tunnel/status — Get Cloudflare Tunnel status.
pub async fn tunnel_status(
    State(_state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let result = agent
        .get("/services/cloudflared/status")
        .await
        .map_err(|e| agent_error("Tunnel status", e))?;

    Ok(Json(result))
}
