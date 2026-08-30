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
    // The shared secret this endpoint verifies signatures with. It used to ride
    // out to the browser on every list of the Webhook Gateway screen: the struct
    // is returned straight to the client and nothing suppressed the field. The
    // SPA never displayed it, which is why it went unnoticed — it was on the
    // wire, not on the page. Six other route modules already suppress a stored
    // credential this way (`cdn`, `dns`, `extensions`, `servers`, `sites`,
    // `update`); this one did not.
    #[serde(skip_serializing)]
    pub verify_secret: Option<String>,
    pub verify_mode: String,
    pub verify_header: Option<String>,
    pub enabled: bool,
    pub total_received: i32,
    pub last_received_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    // Not a column. Whether this endpoint holds a secret it could actually
    // verify with, which is what the operator needs to know and what the secret
    // itself must never be used to answer. Endpoints stored before the presence
    // check below can hold a verifying mode with no secret, and there is no
    // update route — the only repair is delete and recreate — so the screen has
    // to be able to say so.
    #[sqlx(default)]
    pub verify_secret_set: bool,
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

impl WebhookEndpoint {
    /// Answer the presence question from the stored secret, then let the secret
    /// itself be dropped from the response. Blank counts as absent: a blank
    /// secret is what the form used to post when the operator picked a
    /// verification mode and left the field empty, and it verifies nothing.
    fn derive_verify_secret_set(&mut self) {
        self.verify_secret_set = self
            .verify_secret
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty());
    }
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

/// The one field either toggle accepts. Deliberately not a partial-update body:
/// an endpoint's verification settings are fixed at creation on purpose
/// (see `create_endpoint`), and a struct that could carry them would make this
/// door look like the place to change them.
#[derive(serde::Deserialize)]
pub struct SetEnabledRequest {
    pub enabled: bool,
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
    let mut endpoints: Vec<WebhookEndpoint> = sqlx::query_as(
        "SELECT * FROM webhook_endpoints WHERE user_id = $1 ORDER BY created_at DESC LIMIT 500"
    )
    .bind(claims.sub)
    .fetch_all(&state.db).await
    .map_err(|e| internal_error("list endpoints", e))?;

    for e in &mut endpoints {
        e.derive_verify_secret_set();
    }

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

    // Blank is absent. The form posts "" for a field the operator left empty, so
    // taking the strings as they arrive is how an endpoint came to be stored
    // asking for HMAC verification it had no secret to perform — displayed as
    // `hmac_sha256`, verifying nothing.
    let verify_secret = req
        .verify_secret
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let verify_header = req
        .verify_header
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    // A verification mode is a promise the endpoint cannot keep without both
    // halves, and there is no update route to add the missing half later, so the
    // only moment this can be caught is here.
    if verify_mode != "none" && (verify_secret.is_none() || verify_header.is_none()) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Signature verification needs both a secret and the header carrying the signature",
        ));
    }

    // Encrypted at rest: this is the HMAC key the panel re-derives a
    // signature with on every inbound delivery, so it must stay recoverable
    // rather than hashed — same shape as `alert_rules`/`monitors`'s
    // notification secrets.
    let encrypted_verify_secret = verify_secret
        .map(|s| crate::services::secrets_crypto::encrypt_credential(s, &state.config.jwt_secret))
        .transpose()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Encryption failed: {e}")))?;

    let mut endpoint: WebhookEndpoint = sqlx::query_as(
        "INSERT INTO webhook_endpoints (user_id, name, description, token, verify_mode, verify_secret, verify_header) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING *"
    )
    .bind(claims.sub)
    .bind(&req.name)
    .bind(&req.description)
    .bind(&token)
    .bind(verify_mode)
    .bind(encrypted_verify_secret)
    .bind(verify_header)
    .fetch_one(&state.db).await
    .map_err(|e| internal_error("create endpoint", e))?;

    endpoint.derive_verify_secret_set();

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

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "webhook_endpoint.delete",
        Some("webhook"), Some(&id.to_string()), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// PUT /api/webhook-gateway/endpoints/{id} — Open or close the public door.
///
/// `enabled` is what the inbound receiver reads before it will look at a request
/// at all, and until this handler existed nothing in the product could write it:
/// the column was born TRUE and stayed TRUE for the life of the row. Shutting a
/// public door therefore meant deleting the endpoint — which cascades away every
/// delivery and every route attached to it, so the only way to stop traffic was
/// to destroy the record of the traffic that had already arrived. Closing a door
/// and discarding its history are different operations and an operator has to be
/// able to do the first without the second.
pub async fn set_endpoint_enabled(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
    Json(req): Json<SetEnabledRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = sqlx::query(
        "UPDATE webhook_endpoints SET enabled = $1, updated_at = NOW() WHERE id = $2 AND user_id = $3"
    )
    .bind(req.enabled).bind(id).bind(claims.sub)
    .execute(&state.db).await
    .map_err(|e| internal_error("set endpoint enabled", e))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Endpoint not found"));
    }

    activity::log_activity(
        &state.db, claims.sub, &claims.email,
        if req.enabled { "webhook_endpoint.enable" } else { "webhook_endpoint.disable" },
        Some("webhook"), Some(&id.to_string()), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true, "enabled": req.enabled })))
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

/// POST /api/webhook-gateway/deliveries/{delivery_id}/replay — Replay a delivery to its matching routes.
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

    let routes: Vec<WebhookRoute> = sqlx::query_as(
        "SELECT * FROM webhook_routes WHERE endpoint_id = $1 AND enabled = TRUE"
    )
    .bind(delivery.endpoint_id)
    .fetch_all(&state.db).await
    .map_err(|e| internal_error("replay delivery", e))?;

    let body = delivery.body.unwrap_or_default();
    let db = state.db.clone();
    let mut forwarded = 0usize;

    // A replay is the same delivery arriving a second time, so it obeys the same
    // routing decision. This loop used to hand the body to every enabled route
    // whatever its filter said, while the inbound path skipped the ones that did
    // not match — so replaying sent the payload to destinations the operator had
    // deliberately excluded, each with that route's stored headers attached and
    // its retry budget behind it. The guide has always described replay as going
    // to the matching routes; only the code disagreed.
    for route in routes {
        if !route_admits(&route, &body) {
            continue;
        }
        forwarded += 1;
        let body_clone = body.clone();
        let db_clone = db.clone();
        tokio::spawn(async move {
            forward_to_route(&db_clone, &route, &body_clone, delivery_id).await;
        });
    }

    // Replaying re-sends an externally-supplied body to third parties under the
    // panel's own credentials. Until now it was the one forwarding action in this
    // module that left no trace at all.
    activity::log_activity(
        &state.db, claims.sub, &claims.email, "webhook_delivery.replay",
        Some("webhook"), Some(&delivery_id.to_string()),
        Some(&format!("{} route(s)", forwarded)), None,
    ).await;

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

/// PUT /api/webhook-gateway/routes/{route_id} — Stop or resume forwarding.
///
/// The outbound half of the same severance. A route is an arbitrary external
/// destination with the operator's own headers attached and a retry budget, and
/// three queries gate on its `enabled` column — but nothing could write it, so
/// stopping data leaving the box for a third party meant deleting the route and
/// its counters with it. Ownership is enforced through the parent endpoint, the
/// same shape `delete_route` uses.
pub async fn set_route_enabled(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(route_id): Path<Uuid>,
    Json(req): Json<SetEnabledRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = sqlx::query(
        "UPDATE webhook_routes r SET enabled = $1 FROM webhook_endpoints e \
         WHERE r.id = $2 AND r.endpoint_id = e.id AND e.user_id = $3"
    )
    .bind(req.enabled).bind(route_id).bind(claims.sub)
    .execute(&state.db).await
    .map_err(|e| internal_error("set route enabled", e))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Route not found"));
    }

    activity::log_activity(
        &state.db, claims.sub, &claims.email,
        if req.enabled { "webhook_route.enable" } else { "webhook_route.disable" },
        Some("webhook"), Some(&route_id.to_string()), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true, "enabled": req.enabled })))
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

    let decrypted_verify_secret = endpoint.verify_secret.as_deref().map(|v| {
        crate::services::secrets_crypto::decrypt_credential_or_legacy(v, &state.config.jwt_secret)
    });
    let verdict = classify_signature(
        &endpoint.verify_mode,
        decrypted_verify_secret.as_deref(),
        endpoint.verify_header.as_deref(),
        &headers_json,
        &body,
    );

    // Record the delivery BEFORE deciding whether to accept it. The rejection
    // used to return here, above the INSERT, so `signature_valid = FALSE` had no
    // writer at all: the column could only ever hold TRUE or NULL, the list's red
    // "Invalid" badge could never render, and the guide's promise that "failed
    // verifications are logged" was false. A rejected delivery is the one an
    // operator most needs to see — it is the only evidence that something is
    // signing badly, or not signing at all.
    //
    // The trade-off is stated rather than hidden: a caller who knows the endpoint
    // token can now cause a row to be written without knowing the secret. That
    // was already true for an endpoint verifying nothing, the rows carry the
    // same retention sweep as any other delivery (`auto_healer`'s
    // `webhook_deliveries` purge), and nothing is forwarded.
    let delivery_id: Uuid = sqlx::query_scalar(
        "INSERT INTO webhook_deliveries (endpoint_id, method, headers, body, source_ip, signature_valid) \
         VALUES ($1, 'POST', $2, $3, $4, $5) RETURNING id"
    )
    .bind(endpoint.id)
    .bind(serde_json::Value::Object(headers_json))
    .bind(&body_str)
    .bind(&source_ip)
    .bind(verdict.recorded())
    .fetch_one(&state.db).await
    .map_err(|e| internal_error("receive webhook", e))?;

    // Counts every delivery this endpoint recorded, so the "Received" column and
    // the delivery list below it answer for the same population.
    let _ = sqlx::query(
        "UPDATE webhook_endpoints SET total_received = total_received + 1, last_received_at = NOW() WHERE id = $1"
    )
    .bind(endpoint.id)
    .execute(&state.db).await;

    // Reject anything the endpoint asked to verify and could not vouch for —
    // including a delivery it had no secret to check, which used to pass.
    if let Some(reason) = verdict.rejection() {
        return Err(err(StatusCode::UNAUTHORIZED, reason));
    }

    // Forward to all enabled routes (async, fire-and-forget)
    let routes: Vec<WebhookRoute> = sqlx::query_as(
        "SELECT * FROM webhook_routes WHERE endpoint_id = $1 AND enabled = TRUE"
    )
    .bind(endpoint.id)
    .fetch_all(&state.db).await
    .unwrap_or_default();

    // Counted as the loop spawns, not as the query returns: a route the filter
    // excludes is not a route this delivery was sent to, and answering with the
    // number of enabled routes reported work that never happened.
    let mut forwarded = 0usize;
    let db = state.db.clone();

    for route in routes {
        if !route_admits(&route, &body_str) {
            continue;
        }
        forwarded += 1;
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

/// What an endpoint's configuration says one delivery's signature is worth.
///
/// The distinction that matters is between *not asked to verify* and *asked to
/// verify and unable to*. Collapsing those two into one `None` is what let an
/// endpoint advertise `hmac_sha256` on the list while passing every unsigned
/// request straight through to its forwarding routes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SignatureVerdict {
    /// The endpoint asks for no verification. There is nothing to attest, and
    /// the delivery records no attestation.
    NotConfigured,
    /// Verification ran and the request satisfied it.
    Valid,
    /// Verification ran and the request did not satisfy it.
    Invalid,
    /// The endpoint asks for verification it cannot perform — no secret, no
    /// header, or a mode this build does not implement. Never a pass: a promise
    /// that cannot be kept is not the same as no promise.
    Unverifiable,
}

impl SignatureVerdict {
    /// The value written to `webhook_deliveries.signature_valid`. `NULL` only
    /// where the endpoint never claimed to verify anything, so a `NULL` in that
    /// column means "not checked" and can no longer mean "could not check".
    pub(crate) fn recorded(self) -> Option<bool> {
        match self {
            SignatureVerdict::NotConfigured => None,
            SignatureVerdict::Valid => Some(true),
            SignatureVerdict::Invalid | SignatureVerdict::Unverifiable => Some(false),
        }
    }

    /// The sentence sent back to the caller, or `None` to accept the delivery.
    /// It travels as a 401 so it reaches the sender intact — `error.rs` passes a
    /// sentence through only on a 4xx.
    pub(crate) fn rejection(self) -> Option<&'static str> {
        match self {
            SignatureVerdict::NotConfigured | SignatureVerdict::Valid => None,
            SignatureVerdict::Invalid => Some("Invalid webhook signature"),
            SignatureVerdict::Unverifiable => Some(
                "This endpoint requires signature verification but has no usable secret. \
                 Delete it and create it again with a secret and a signature header.",
            ),
        }
    }
}

/// Grade one delivery against the endpoint's verification settings.
///
/// Split out from the handler so the decision can be tested without a database
/// or a socket — every way this can be wrong is a way an unsigned request gets
/// treated as a signed one.
pub(crate) fn classify_signature(
    verify_mode: &str,
    verify_secret: Option<&str>,
    verify_header: Option<&str>,
    headers: &serde_json::Map<String, serde_json::Value>,
    body: &[u8],
) -> SignatureVerdict {
    if verify_mode == "none" {
        return SignatureVerdict::NotConfigured;
    }

    // Blank is absent, exactly as at creation. A blank secret is not a weak
    // secret: `Hmac::new_from_slice` accepts a zero-length key, so an endpoint
    // stored with `verify_secret = ""` used to verify signatures against the
    // empty key — and anyone who guessed that could compute one, be recorded
    // `signature_valid = true`, and be relayed onward as authentic.
    let secret = verify_secret.map(str::trim).filter(|s| !s.is_empty());
    let header_name = verify_header.map(str::trim).filter(|s| !s.is_empty());
    let (Some(secret), Some(header_name)) = (secret, header_name) else {
        return SignatureVerdict::Unverifiable;
    };

    let presented = headers
        .get(header_name.to_lowercase().as_str())
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let ok = match verify_mode {
        "hmac_sha256" => verify_hmac_sha256(secret, body, presented),
        "hmac_sha1" => verify_hmac_sha1(secret, body, presented),
        // A mode this build cannot compute is a promise it cannot keep.
        _ => return SignatureVerdict::Unverifiable,
    };

    if ok {
        SignatureVerdict::Valid
    } else {
        SignatureVerdict::Invalid
    }
}

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

/// Whether a route's filter admits this body.
///
/// One definition, called from both the inbound path and the replay path. It
/// lived inline in the inbound loop and had no counterpart in replay, which is
/// how the two came to disagree about where a delivery goes.
///
/// A route with no filter takes everything, and a body that will not parse as
/// JSON is admitted rather than dropped — the inbound behaviour this preserves.
/// The filter selects destinations; it is not a security control, and nothing
/// here should be read as one.
fn route_admits(route: &WebhookRoute, body: &str) -> bool {
    let (Some(path), Some(value)) = (&route.filter_path, &route.filter_value) else {
        return true;
    };
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(body) else {
        return true;
    };
    parsed.pointer(path).and_then(|v| v.as_str()).unwrap_or("") == value.as_str()
}

/// The settings the webhook-gateway HTTP client shares — extracted so
/// `forward_to_route` can build a fresh, PINNED client (below) with the same
/// settings, rather than duplicating them.
fn webhook_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        // Do NOT follow redirects: validate_url_not_internal only vets the
        // original destination_url, so a public destination returning a
        // 3xx to http://127.0.0.1 / 169.254.169.254 would otherwise bypass
        // the SSRF allow-check and exfiltrate the internal response.
        .redirect(reqwest::redirect::Policy::none())
}

async fn forward_to_route(db: &sqlx::PgPool, route: &WebhookRoute, body: &str, delivery_id: Uuid) {
    // Re-validate destination URL at forward time to prevent DNS rebinding SSRF.
    // An attacker could register a route pointing to a public IP, then change DNS
    // to resolve to an internal IP before the webhook fires.
    //
    // Pinned ONCE here and the resulting client reused for every retry below —
    // this loop runs up to 11 attempts (retry_count.min(10)) spread over
    // exponential backoff that can span minutes, and a shared client whose own
    // resolver looked `destination_url`'s host up again on each attempt would
    // widen the validate/connect race to that whole window instead of one
    // lookup's worth of it.
    let (host, port) = match crate::helpers::url_authority(&route.destination_url) {
        Ok(hp) => hp,
        Err(e) => {
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
    };
    let client = match crate::helpers::pinned_client(&host, port, webhook_client_builder()).await {
        Ok(c) => c,
        Err(e) => {
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
    };

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

        let mut req = client
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

#[cfg(test)]
mod signature_verdict_tests {
    use super::*;
    use hmac::{Hmac, Mac};

    const BODY: &[u8] = br#"{"action":"opened","number":7}"#;
    const HEADER: &str = "x-hub-signature-256";
    const SECRET: &str = "s3cr3t-shared-with-github";

    /// Build the signature a correctly-configured sender would send, under an
    /// arbitrary key — including the empty one, which is the whole point of
    /// `empty_secret_does_not_authenticate_an_empty_key_signature`.
    fn sha256_signature(key: &[u8], body: &[u8]) -> String {
        let mut mac =
            Hmac::<sha2::Sha256>::new_from_slice(key).expect("hmac accepts any key length");
        mac.update(body);
        format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
    }

    fn sha1_signature(key: &[u8], body: &[u8]) -> String {
        let mut mac = Hmac::<sha1::Sha1>::new_from_slice(key).expect("hmac accepts any key length");
        mac.update(body);
        format!("sha1={}", hex::encode(mac.finalize().into_bytes()))
    }

    /// The handler builds this map from the request's `HeaderMap`, whose names
    /// are already lower-case, which is why the lookup lower-cases the
    /// configured header name.
    fn headers(pairs: &[(&str, &str)]) -> serde_json::Map<String, serde_json::Value> {
        pairs
            .iter()
            .map(|(k, v)| {
                (
                    (*k).to_string(),
                    serde_json::Value::String((*v).to_string()),
                )
            })
            .collect()
    }

    fn classify(
        mode: &str,
        secret: Option<&str>,
        header: Option<&str>,
        presented: &[(&str, &str)],
    ) -> SignatureVerdict {
        classify_signature(mode, secret, header, &headers(presented), BODY)
    }

    // ── The two states that used to be one ──────────────────────────────────

    #[test]
    fn no_verification_configured_attests_nothing() {
        let v = classify("none", None, None, &[]);
        assert_eq!(v, SignatureVerdict::NotConfigured);
        assert_eq!(
            v.recorded(),
            None,
            "an endpoint that never claimed to verify records no attestation"
        );
        assert_eq!(v.rejection(), None);
    }

    #[test]
    fn a_verifying_mode_with_no_secret_is_rejected_not_waved_through() {
        // The shape a raw API call could store: mode set, secret absent. It used
        // to reach `None` — indistinguishable from "not configured" — and pass.
        let v = classify(
            "hmac_sha256",
            None,
            Some(HEADER),
            &[(HEADER, "sha256=deadbeef")],
        );
        assert_eq!(v, SignatureVerdict::Unverifiable);
        assert_eq!(v.recorded(), Some(false));
        assert!(
            v.rejection().is_some(),
            "an endpoint that cannot verify must not accept"
        );
    }

    #[test]
    fn a_verifying_mode_with_no_header_is_rejected() {
        let v = classify("hmac_sha256", Some(SECRET), None, &[]);
        assert_eq!(v, SignatureVerdict::Unverifiable);
    }

    // ── The empty key: the shape the form used to post ──────────────────────

    #[test]
    fn empty_secret_does_not_authenticate_an_empty_key_signature() {
        // The form posted "" for a field left blank, so an endpoint could be
        // stored with `verify_secret = Some("")` and a real header name. HMAC
        // accepts a zero-length key, so the arm matched and anyone who guessed
        // the secret was empty could produce a signature that verified — and be
        // RECORDED as authentic before being relayed. That is worse than the
        // null-secret pass, because it writes a true attestation.
        let forged = sha256_signature(b"", BODY);
        let v = classify("hmac_sha256", Some(""), Some(HEADER), &[(HEADER, &forged)]);
        assert_ne!(
            v,
            SignatureVerdict::Valid,
            "an empty secret must never attest anything"
        );
        assert_eq!(v, SignatureVerdict::Unverifiable);
        assert_eq!(v.recorded(), Some(false));
    }

    #[test]
    fn a_whitespace_only_secret_is_absent_too() {
        let v = classify(
            "hmac_sha256",
            Some("   "),
            Some(HEADER),
            &[(HEADER, "sha256=00")],
        );
        assert_eq!(v, SignatureVerdict::Unverifiable);
    }

    #[test]
    fn a_blank_header_name_is_absent_too() {
        let v = classify("hmac_sha256", Some(SECRET), Some(""), &[]);
        assert_eq!(v, SignatureVerdict::Unverifiable);
    }

    // ── Verification still works, which is what makes the above meaningful ──

    #[test]
    fn a_correctly_signed_sha256_delivery_is_valid() {
        let sig = sha256_signature(SECRET.as_bytes(), BODY);
        let v = classify("hmac_sha256", Some(SECRET), Some(HEADER), &[(HEADER, &sig)]);
        assert_eq!(v, SignatureVerdict::Valid);
        assert_eq!(v.recorded(), Some(true));
        assert_eq!(v.rejection(), None);
    }

    #[test]
    fn a_correctly_signed_sha1_delivery_is_valid() {
        let sig = sha1_signature(SECRET.as_bytes(), BODY);
        let v = classify(
            "hmac_sha1",
            Some(SECRET),
            Some("x-hub-signature"),
            &[("x-hub-signature", &sig)],
        );
        assert_eq!(v, SignatureVerdict::Valid);
    }

    #[test]
    fn a_signature_under_the_wrong_key_is_invalid_not_unverifiable() {
        // Both are rejections, but they are different facts and the operator is
        // told different sentences: one endpoint is misconfigured, the other is
        // being signed badly.
        let sig = sha256_signature(b"someone-elses-secret", BODY);
        let v = classify("hmac_sha256", Some(SECRET), Some(HEADER), &[(HEADER, &sig)]);
        assert_eq!(v, SignatureVerdict::Invalid);
        assert_eq!(v.recorded(), Some(false));
        assert_eq!(v.rejection(), Some("Invalid webhook signature"));
    }

    #[test]
    fn a_missing_signature_header_on_a_configured_endpoint_is_invalid() {
        let v = classify(
            "hmac_sha256",
            Some(SECRET),
            Some(HEADER),
            &[("content-type", "application/json")],
        );
        assert_eq!(v, SignatureVerdict::Invalid);
    }

    #[test]
    fn a_signature_that_is_not_hex_is_invalid_rather_than_a_panic() {
        let v = classify(
            "hmac_sha256",
            Some(SECRET),
            Some(HEADER),
            &[(HEADER, "sha256=not-hex-at-all")],
        );
        assert_eq!(v, SignatureVerdict::Invalid);
    }

    #[test]
    fn the_algorithm_prefix_is_optional() {
        let with_prefix = sha256_signature(SECRET.as_bytes(), BODY);
        let bare = with_prefix.trim_start_matches("sha256=").to_string();
        assert_eq!(
            classify(
                "hmac_sha256",
                Some(SECRET),
                Some(HEADER),
                &[(HEADER, &bare)]
            ),
            SignatureVerdict::Valid
        );
    }

    #[test]
    fn the_configured_header_name_is_matched_case_insensitively() {
        let sig = sha256_signature(SECRET.as_bytes(), BODY);
        let v = classify(
            "hmac_sha256",
            Some(SECRET),
            Some("X-Hub-Signature-256"),
            &[(HEADER, &sig)],
        );
        assert_eq!(v, SignatureVerdict::Valid);
    }

    #[test]
    fn a_body_that_differs_by_one_byte_does_not_verify() {
        let sig = sha256_signature(SECRET.as_bytes(), BODY);
        let tampered = classify_signature(
            "hmac_sha256",
            Some(SECRET),
            Some(HEADER),
            &headers(&[(HEADER, &sig)]),
            br#"{"action":"opened","number":8}"#,
        );
        assert_eq!(tampered, SignatureVerdict::Invalid);
    }

    // ── Fail-closed on anything this build does not understand ──────────────

    #[test]
    fn a_mode_this_build_cannot_compute_is_rejected() {
        // Reachable by a hand-edited row, or by running an older binary against a
        // database a newer one wrote. The safe-looking default here would be to
        // treat it as "no verification".
        let v = classify(
            "hmac_sha512",
            Some(SECRET),
            Some(HEADER),
            &[(HEADER, "sha512=00")],
        );
        assert_eq!(v, SignatureVerdict::Unverifiable);
        assert_eq!(v.recorded(), Some(false));
    }

    // ── The mapping itself, so a new variant cannot be added unclassified ───

    #[test]
    fn only_an_endpoint_that_asked_for_nothing_records_no_attestation() {
        for v in [
            SignatureVerdict::NotConfigured,
            SignatureVerdict::Valid,
            SignatureVerdict::Invalid,
            SignatureVerdict::Unverifiable,
        ] {
            let recorded = v.recorded();
            let rejected = v.rejection().is_some();
            match v {
                SignatureVerdict::NotConfigured => assert_eq!((recorded, rejected), (None, false)),
                SignatureVerdict::Valid => assert_eq!((recorded, rejected), (Some(true), false)),
                // Every rejection is recorded FALSE, which is the writer the
                // column never had: the 401 used to return above the INSERT.
                SignatureVerdict::Invalid | SignatureVerdict::Unverifiable => {
                    assert_eq!((recorded, rejected), (Some(false), true))
                }
            }
        }
    }
}
