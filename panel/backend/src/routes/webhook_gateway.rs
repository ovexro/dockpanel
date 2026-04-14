use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use uuid::Uuid;

use crate::auth::AdminUser;
use crate::error::{internal_error, err, paginate, ApiError};
use crate::services::activity;
use crate::AppState;

// ── Types ───────────────────────────────────────────────────────────────────

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct WebhookEndpoint {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub token: String,
    pub verify_secret: Option<String>,
    pub verify_mode: String,
    pub verify_header: Option<String>,
    pub enabled: bool,
    pub total_received: i32,
    pub last_received_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct WebhookDelivery {
    pub id: Uuid,
    pub endpoint_id: Uuid,
    pub method: String,
    pub headers: serde_json::Value,
    pub body: Option<String>,
    pub query_string: Option<String>,
    pub source_ip: Option<String>,
    pub signature_valid: Option<bool>,
    pub forwarded: bool,
    pub forward_status: Option<i32>,
    pub forward_response: Option<String>,
    pub forward_duration_ms: Option<i32>,
    pub received_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct WebhookRoute {
    pub id: Uuid,
    pub endpoint_id: Uuid,
    pub name: String,
    pub destination_url: String,
    pub filter_path: Option<String>,
    pub filter_value: Option<String>,
    pub extra_headers: serde_json::Value,
    pub retry_count: i32,
    pub retry_delay_secs: i32,
    pub enabled: bool,
    pub total_forwarded: i32,
    pub last_forwarded_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_status: Option<i32>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Deserialize)]
pub struct CreateEndpointRequest {
    pub name: String,
    pub description: Option<String>,
    pub verify_mode: Option<String>,
    pub verify_secret: Option<String>,
    pub verify_header: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct CreateRouteRequest {
    pub name: String,
    pub destination_url: String,
    pub filter_path: Option<String>,
    pub filter_value: Option<String>,
    pub extra_headers: Option<serde_json::Value>,
    pub retry_count: Option<i32>,
    pub retry_delay_secs: Option<i32>,
}

#[derive(serde::Deserialize)]
pub struct PaginationQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

// ── Endpoint CRUD ───────────────────────────────────────────────────────────

/// GET /api/webhook-gateway/endpoints — List endpoints.
pub async fn list_endpoints(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<Json<Vec<WebhookEndpoint>>, ApiError> {
    let endpoints: Vec<WebhookEndpoint> = sqlx::query_as(
        "SELECT * FROM webhook_endpoints WHERE user_id = $1 ORDER BY created_at DESC LIMIT 500"
    )
    .bind(claims.sub)
    .fetch_all(&state.db).await
    .map_err(|e| internal_error("list endpoints", e))?;

    Ok(Json(endpoints))
}

/// POST /api/webhook-gateway/endpoints — Create an endpoint.
pub async fn create_endpoint(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Json(req): Json<CreateEndpointRequest>,
) -> Result<(StatusCode, Json<WebhookEndpoint>), ApiError> {
    if req.name.is_empty() || req.name.len() > 100 {
        return Err(err(StatusCode::BAD_REQUEST, "Name must be 1-100 characters"));
    }

    let token = Uuid::new_v4().to_string().replace('-', "");
    let verify_mode = req.verify_mode.as_deref().unwrap_or("none");

    if !["none", "hmac_sha256", "hmac_sha1"].contains(&verify_mode) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid verify_mode"));
    }

    let endpoint: WebhookEndpoint = sqlx::query_as(
        "INSERT INTO webhook_endpoints (user_id, name, description, token, verify_mode, verify_secret, verify_header) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING *"
    )
    .bind(claims.sub)
    .bind(&req.name)
    .bind(&req.description)
    .bind(&token)
    .bind(verify_mode)
    .bind(&req.verify_secret)
    .bind(&req.verify_header)
    .fetch_one(&state.db).await
    .map_err(|e| internal_error("create endpoint", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "webhook_endpoint.create",
        Some("webhook"), Some(&req.name), Some(&token), None,
    ).await;

    Ok((StatusCode::CREATED, Json(endpoint)))
}

/// DELETE /api/webhook-gateway/endpoints/{id} — Delete an endpoint.
pub async fn delete_endpoint(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = sqlx::query("DELETE FROM webhook_endpoints WHERE id = $1 AND user_id = $2")
        .bind(id).bind(claims.sub)
        .execute(&state.db).await
        .map_err(|e| internal_error("delete endpoint", e))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Endpoint not found"));
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Deliveries (Inspector) ──────────────────────────────────────────────────

/// GET /api/webhook-gateway/endpoints/{id}/deliveries — List deliveries.
pub async fn list_deliveries(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
    Query(params): Query<PaginationQuery>,
) -> Result<Json<Vec<WebhookDelivery>>, ApiError> {
    // Verify ownership
    let _: (Uuid,) = sqlx::query_as(
        "SELECT id FROM webhook_endpoints WHERE id = $1 AND user_id = $2"
    )
    .bind(id).bind(claims.sub)
    .fetch_optional(&state.db).await
    .map_err(|e| internal_error("list deliveries", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Endpoint not found"))?;

    let (limit, offset) = paginate(params.limit, params.offset);

    let deliveries: Vec<WebhookDelivery> = sqlx::query_as(
        "SELECT * FROM webhook_deliveries WHERE endpoint_id = $1 ORDER BY received_at DESC LIMIT $2 OFFSET $3"
    )
    .bind(id).bind(limit).bind(offset)
    .fetch_all(&state.db).await
    .map_err(|e| internal_error("list deliveries", e))?;

    Ok(Json(deliveries))
}

/// POST /api/webhook-gateway/deliveries/{delivery_id}/replay — Replay a delivery to all routes.
pub async fn replay_delivery(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(delivery_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let delivery: WebhookDelivery = sqlx::query_as(
        "SELECT d.* FROM webhook_deliveries d \
         JOIN webhook_endpoints e ON e.id = d.endpoint_id AND e.user_id = $1 \
         WHERE d.id = $2"
    )
    .bind(claims.sub).bind(delivery_id)
    .fetch_optional(&state.db).await
    .map_err(|e| internal_error("replay delivery", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Delivery not found"))?;

    // Forward to all enabled routes
    let routes: Vec<WebhookRoute> = sqlx::query_as(
        "SELECT * FROM webhook_routes WHERE endpoint_id = $1 AND enabled = TRUE"
    )
    .bind(delivery.endpoint_id)
    .fetch_all(&state.db).await
    .map_err(|e| internal_error("replay delivery", e))?;

    let body = delivery.body.unwrap_or_default();
    let db = state.db.clone();
    let forwarded = routes.len();

    for route in routes {
        let body_clone = body.clone();
        let db_clone = db.clone();
        tokio::spawn(async move {
            forward_to_route(&db_clone, &route, &body_clone, delivery_id).await;
        });
    }

    Ok(Json(serde_json::json!({ "ok": true, "replayed_to": forwarded })))
}

// ── Routes CRUD ─────────────────────────────────────────────────────────────

/// GET /api/webhook-gateway/endpoints/{id}/routes — List routes.
pub async fn list_routes(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<WebhookRoute>>, ApiError> {
    let _: (Uuid,) = sqlx::query_as(
        "SELECT id FROM webhook_endpoints WHERE id = $1 AND user_id = $2"
    )
    .bind(id).bind(claims.sub)
    .fetch_optional(&state.db).await
    .map_err(|e| internal_error("list routes", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Endpoint not found"))?;

    let routes: Vec<WebhookRoute> = sqlx::query_as(
        "SELECT * FROM webhook_routes WHERE endpoint_id = $1 ORDER BY created_at ASC"
    )
    .bind(id)
    .fetch_all(&state.db).await
    .map_err(|e| internal_error("list routes", e))?;

    Ok(Json(routes))
}

/// POST /api/webhook-gateway/endpoints/{id}/routes — Create a route.
pub async fn create_route(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
    Json(req): Json<CreateRouteRequest>,
) -> Result<(StatusCode, Json<WebhookRoute>), ApiError> {
    let _: (Uuid,) = sqlx::query_as(
        "SELECT id FROM webhook_endpoints WHERE id = $1 AND user_id = $2"
    )
    .bind(id).bind(claims.sub)
    .fetch_optional(&state.db).await
    .map_err(|e| internal_error("create route", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Endpoint not found"))?;

    if req.destination_url.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "destination_url required"));
    }

    // SSRF protection: block internal destination URLs
    if let Err(e) = crate::helpers::validate_url_not_internal(&req.destination_url).await {
        return Err(err(StatusCode::BAD_REQUEST, &format!("Invalid destination URL: {}", e)));
    }

    let route: WebhookRoute = sqlx::query_as(
        "INSERT INTO webhook_routes (endpoint_id, name, destination_url, filter_path, filter_value, extra_headers, retry_count, retry_delay_secs) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING *"
    )
    .bind(id)
    .bind(&req.name)
    .bind(&req.destination_url)
    .bind(&req.filter_path)
    .bind(&req.filter_value)
    .bind(req.extra_headers.as_ref().unwrap_or(&serde_json::json!({})))
    .bind(req.retry_count.unwrap_or(3).min(10).max(0))
    .bind(req.retry_delay_secs.unwrap_or(5).min(300).max(1))
    .fetch_one(&state.db).await
    .map_err(|e| internal_error("create route", e))?;

    Ok((StatusCode::CREATED, Json(route)))
}

/// DELETE /api/webhook-gateway/routes/{route_id} — Delete a route.
pub async fn delete_route(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(route_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = sqlx::query(
        "DELETE FROM webhook_routes r USING webhook_endpoints e \
         WHERE r.id = $1 AND r.endpoint_id = e.id AND e.user_id = $2"
    )
    .bind(route_id).bind(claims.sub)
    .execute(&state.db).await
    .map_err(|e| internal_error("delete route", e))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Route not found"));
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Public Inbound Webhook Receiver ─────────────────────────────────────────

/// POST /api/webhooks/gateway/{token} — Receive an inbound webhook (public, no auth).
pub async fn receive_webhook(
    State(state): State<AppState>,
    Path(token): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Find endpoint by token
    let endpoint: WebhookEndpoint = sqlx::query_as(
        "SELECT * FROM webhook_endpoints WHERE token = $1 AND enabled = TRUE"
    )
    .bind(&token)
    .fetch_optional(&state.db).await
    .map_err(|e| internal_error("receive webhook", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Invalid webhook endpoint"))?;

    let body_str = String::from_utf8_lossy(&body).to_string();

    // Extract source IP
    let source_ip = headers.get("x-forwarded-for")
        .or_else(|| headers.get("x-real-ip"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Collect headers as JSON
    let mut headers_json = serde_json::Map::new();
    for (name, value) in headers.iter() {
        if let Ok(v) = value.to_str() {
            headers_json.insert(name.to_string(), serde_json::Value::String(v.to_string()));
        }
    }

    // Verify signature if configured
    let signature_valid = match endpoint.verify_mode.as_str() {
        "hmac_sha256" => {
            if let (Some(secret), Some(header_name)) = (&endpoint.verify_secret, &endpoint.verify_header) {
                let sig_header = headers_json.get(header_name.to_lowercase().as_str())
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                Some(verify_hmac_sha256(secret, &body, sig_header))
            } else {
                None
            }
        }
        "hmac_sha1" => {
            if let (Some(secret), Some(header_name)) = (&endpoint.verify_secret, &endpoint.verify_header) {
                let sig_header = headers_json.get(header_name.to_lowercase().as_str())
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                Some(verify_hmac_sha1(secret, &body, sig_header))
            } else {
                None
            }
        }
        _ => None, // No verification configured
    };

    // Reject invalid signatures
    if signature_valid == Some(false) {
        return Err(err(StatusCode::UNAUTHORIZED, "Invalid webhook signature"));
    }

    // Record delivery
    let delivery_id: Uuid = sqlx::query_scalar(
        "INSERT INTO webhook_deliveries (endpoint_id, method, headers, body, source_ip, signature_valid) \
         VALUES ($1, 'POST', $2, $3, $4, $5) RETURNING id"
    )
    .bind(endpoint.id)
    .bind(serde_json::Value::Object(headers_json))
    .bind(&body_str)
    .bind(&source_ip)
    .bind(signature_valid)
    .fetch_one(&state.db).await
    .map_err(|e| internal_error("receive webhook", e))?;

    // Update endpoint stats
    let _ = sqlx::query(
        "UPDATE webhook_endpoints SET total_received = total_received + 1, last_received_at = NOW() WHERE id = $1"
    )
    .bind(endpoint.id)
    .execute(&state.db).await;

    // Forward to all enabled routes (async, fire-and-forget)
    let routes: Vec<WebhookRoute> = sqlx::query_as(
        "SELECT * FROM webhook_routes WHERE endpoint_id = $1 AND enabled = TRUE"
    )
    .bind(endpoint.id)
    .fetch_all(&state.db).await
    .unwrap_or_default();

    let forwarded = routes.len();
    let db = state.db.clone();

    for route in routes {
        // Check filter
        if let (Some(path), Some(value)) = (&route.filter_path, &route.filter_value) {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&body_str) {
                let actual = parsed.pointer(path).and_then(|v| v.as_str()).unwrap_or("");
                if actual != value.as_str() {
                    continue; // Skip this route — filter doesn't match
                }
            }
        }

        let body_clone = body_str.clone();
        let db_clone = db.clone();
        tokio::spawn(async move {
            forward_to_route(&db_clone, &route, &body_clone, delivery_id).await;
        });
    }

    Ok(Json(serde_json::json!({
        "ok": true,
        "delivery_id": delivery_id,
        "forwarded_to": forwarded,
    })))
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn verify_hmac_sha256(secret: &str, body: &[u8], signature_header: &str) -> bool {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let sig = signature_header
        .strip_prefix("sha256=")
        .unwrap_or(signature_header);

    let expected = match hex::decode(sig) {
        Ok(b) => b,
        Err(_) => return false,
    };

    let mut mac = match Hmac::<Sha256>::new_from_slice(secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(body);
    mac.verify_slice(&expected).is_ok()
}

fn verify_hmac_sha1(secret: &str, body: &[u8], signature_header: &str) -> bool {
    use hmac::{Hmac, Mac};
    use sha1::Sha1;

    let sig = signature_header
        .strip_prefix("sha1=")
        .unwrap_or(signature_header);

    let expected = match hex::decode(sig) {
        Ok(b) => b,
        Err(_) => return false,
    };

    let mut mac = match Hmac::<Sha1>::new_from_slice(secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(body);
    mac.verify_slice(&expected).is_ok()
}

static HTTP_CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();

fn http_client() -> &'static reqwest::Client {
    HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_default()
    })
}

async fn forward_to_route(db: &sqlx::PgPool, route: &WebhookRoute, body: &str, delivery_id: Uuid) {
    // Re-validate destination URL at forward time to prevent DNS rebinding SSRF.
    // An attacker could register a route pointing to a public IP, then change DNS
    // to resolve to an internal IP before the webhook fires.
    if let Err(e) = crate::helpers::validate_url_not_internal(&route.destination_url).await {
        tracing::warn!(
            "Webhook route {} destination blocked at forward time (DNS rebinding?): {e}",
            route.id
        );
        let _ = sqlx::query(
            "UPDATE webhook_deliveries SET forwarded = TRUE, forward_status = 0, \
             forward_response = $2 WHERE id = $1"
        )
        .bind(delivery_id)
        .bind(format!("Blocked: destination URL failed validation: {e}"))
        .execute(db).await;
        return;
    }

    let mut last_status = 0i32;
    let mut last_response = String::new();
    let mut last_duration = 0i32;
    let retries = route.retry_count.max(0).min(10);

    for attempt in 0..=retries {
        if attempt > 0 {
            let delay = route.retry_delay_secs as u64 * (1 << (attempt - 1).min(5));
            tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
        }

        let start = std::time::Instant::now();

        let mut req = http_client()
            .post(&route.destination_url)
            .header("Content-Type", "application/json")
            .header("X-Webhook-Delivery", delivery_id.to_string())
            .header("X-Webhook-Attempt", (attempt + 1).to_string());

        // Apply extra headers
        if let Some(obj) = route.extra_headers.as_object() {
            for (k, v) in obj {
                if let Some(val) = v.as_str() {
                    req = req.header(k.as_str(), val);
                }
            }
        }

        match req.body(body.to_string()).send().await {
            Ok(resp) => {
                last_status = resp.status().as_u16() as i32;
                last_duration = start.elapsed().as_millis() as i32;
                last_response = resp.text().await.unwrap_or_default();
                if last_response.len() > 2000 {
                    last_response.truncate(2000);
                }

                if last_status >= 200 && last_status < 300 {
                    break; // Success
                }
            }
            Err(e) => {
                last_status = 0;
                last_duration = start.elapsed().as_millis() as i32;
                last_response = e.to_string();
                if last_response.len() > 2000 {
                    last_response.truncate(2000);
                }
            }
        }
    }

    // Update delivery record
    let _ = sqlx::query(
        "UPDATE webhook_deliveries SET forwarded = TRUE, forward_status = $2, \
         forward_response = $3, forward_duration_ms = $4 WHERE id = $1"
    )
    .bind(delivery_id).bind(last_status)
    .bind(&last_response).bind(last_duration)
    .execute(db).await;

    // Update route stats
    let _ = sqlx::query(
        "UPDATE webhook_routes SET total_forwarded = total_forwarded + 1, \
         last_forwarded_at = NOW(), last_status = $2 WHERE id = $1"
    )
    .bind(route.id).bind(last_status)
    .execute(db).await;
}
