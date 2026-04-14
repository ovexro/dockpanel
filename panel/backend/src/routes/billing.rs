use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};

use subtle::ConstantTimeEq;

use crate::auth::AuthUser;
use crate::error::{internal_error, err, agent_error, ApiError};
use crate::AppState;

/// Plan definitions: name, price_id (set via Stripe dashboard), server_limit.
struct PlanDef {
    name: &'static str,
    server_limit: i32,
}

fn plan_def(plan: &str) -> PlanDef {
    match plan {
        "starter" => PlanDef { name: "Starter", server_limit: 1 },
        "pro" => PlanDef { name: "Pro", server_limit: 5 },
        "agency" => PlanDef { name: "Agency", server_limit: 20 },
        _ => PlanDef { name: "Free", server_limit: 1 },
    }
}

/// GET /api/billing/plan — Get current user's plan info.
pub async fn current_plan(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let row: (String, String, i32, i64) = sqlx::query_as(
        "SELECT u.plan, u.plan_status, u.plan_server_limit, \
         (SELECT COUNT(*) FROM servers WHERE user_id = u.id) \
         FROM users u WHERE u.id = $1",
    )
    .bind(claims.sub)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("current plan", e))?;

    let has_stripe = state.config.stripe_secret_key.is_some();

    Ok(Json(serde_json::json!({
        "plan": row.0,
        "plan_status": row.1,
        "server_limit": row.2,
        "server_count": row.3,
        "billing_enabled": has_stripe,
    })))
}

#[derive(serde::Deserialize)]
pub struct CheckoutRequest {
    pub plan: String,
}

/// POST /api/billing/checkout — Create Stripe Checkout session for a plan.
pub async fn create_checkout(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Json(body): Json<CheckoutRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let stripe_key = state.config.stripe_secret_key.as_ref()
        .ok_or_else(|| err(StatusCode::SERVICE_UNAVAILABLE, "Billing not configured"))?;

    let plan = body.plan.to_lowercase();
    if !["starter", "pro", "agency"].contains(&plan.as_str()) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid plan"));
    }

    // Get or create Stripe customer
    let user: (Option<String>, String) = sqlx::query_as(
        "SELECT stripe_customer_id, email FROM users WHERE id = $1",
    )
    .bind(claims.sub)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("create checkout", e))?;

    let customer_id = if let Some(cid) = user.0 {
        cid
    } else {
        // Create Stripe customer
        let client = reqwest::Client::new();
        let resp = client
            .post("https://api.stripe.com/v1/customers")
            .basic_auth(stripe_key, Option::<&str>::None)
            .form(&[("email", &user.1), ("metadata[user_id]", &claims.sub.to_string())])
            .send()
            .await
            .map_err(|e| agent_error("Stripe request", e))?;

        let body: serde_json::Value = resp.json().await
            .map_err(|e| agent_error("Stripe response parse", e))?;

        let cid = body["id"].as_str()
            .ok_or_else(|| err(StatusCode::BAD_GATEWAY, "Stripe: missing customer id"))?
            .to_string();

        sqlx::query("UPDATE users SET stripe_customer_id = $1 WHERE id = $2")
            .bind(&cid)
            .bind(claims.sub)
            .execute(&state.db)
            .await
            .map_err(|e| internal_error("create checkout", e))?;

        cid
    };

    // Look up price ID from settings (admin configures via Settings page)
    let price_key = format!("stripe_price_{plan}");
    let price_row: Option<(String,)> = sqlx::query_as(
        "SELECT value FROM settings WHERE key = $1",
    )
    .bind(&price_key)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("create checkout", e))?;

    let price_id = price_row
        .map(|r| r.0)
        .ok_or_else(|| err(StatusCode::SERVICE_UNAVAILABLE, &format!("Price not configured for {plan} plan. Set '{price_key}' in settings.")))?;

    // Create Checkout Session
    let base_url = &state.config.base_url;
    let client = reqwest::Client::new();
    let resp = client
        .post("https://api.stripe.com/v1/checkout/sessions")
        .basic_auth(stripe_key, Option::<&str>::None)
        .form(&[
            ("customer", customer_id.as_str()),
            ("mode", "subscription"),
            ("line_items[0][price]", &price_id),
            ("line_items[0][quantity]", "1"),
            ("success_url", &format!("{base_url}/billing?success=1")),
            ("cancel_url", &format!("{base_url}/billing?canceled=1")),
            ("metadata[user_id]", &claims.sub.to_string()),
            ("metadata[plan]", &plan),
            ("subscription_data[metadata][user_id]", &claims.sub.to_string()),
            ("subscription_data[metadata][plan]", &plan),
        ])
        .send()
        .await
        .map_err(|e| agent_error("Stripe request", e))?;

    let session: serde_json::Value = resp.json().await
        .map_err(|e| agent_error("Stripe response parse", e))?;

    if let Some(err_msg) = session.get("error") {
        return Err(agent_error("Stripe checkout", err_msg));
    }

    let url = session["url"].as_str()
        .ok_or_else(|| err(StatusCode::BAD_GATEWAY, "Stripe: missing checkout URL"))?;

    Ok(Json(serde_json::json!({ "url": url })))
}

/// POST /api/billing/portal — Create Stripe Customer Portal session.
pub async fn customer_portal(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let stripe_key = state.config.stripe_secret_key.as_ref()
        .ok_or_else(|| err(StatusCode::SERVICE_UNAVAILABLE, "Billing not configured"))?;

    let customer_id: Option<(String,)> = sqlx::query_as(
        "SELECT stripe_customer_id FROM users WHERE id = $1 AND stripe_customer_id IS NOT NULL",
    )
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("customer portal", e))?;

    let cid = customer_id
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "No billing account found"))?
        .0;

    let base_url = &state.config.base_url;
    let client = reqwest::Client::new();
    let resp = client
        .post("https://api.stripe.com/v1/billing_portal/sessions")
        .basic_auth(stripe_key, Option::<&str>::None)
        .form(&[
            ("customer", cid.as_str()),
            ("return_url", &format!("{base_url}/billing")),
        ])
        .send()
        .await
        .map_err(|e| agent_error("Stripe request", e))?;

    let session: serde_json::Value = resp.json().await
        .map_err(|e| agent_error("Stripe response parse", e))?;

    let url = session["url"].as_str()
        .ok_or_else(|| err(StatusCode::BAD_GATEWAY, "Stripe: missing portal URL"))?;

    Ok(Json(serde_json::json!({ "url": url })))
}

/// POST /api/webhooks/stripe — Stripe webhook handler (no auth — verified by signature).
pub async fn webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    let webhook_secret = state.config.stripe_webhook_secret.as_ref()
        .ok_or_else(|| err(StatusCode::SERVICE_UNAVAILABLE, "Webhook not configured"))?;

    // Verify signature
    let sig = headers
        .get("stripe-signature")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing stripe-signature"))?;

    let payload = std::str::from_utf8(&body)
        .map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid payload"))?;

    // Parse signature header: t=timestamp,v1=signature
    let mut timestamp = "";
    let mut signature = "";
    for part in sig.split(',') {
        if let Some(t) = part.strip_prefix("t=") {
            timestamp = t;
        } else if let Some(s) = part.strip_prefix("v1=") {
            signature = s;
        }
    }

    if timestamp.is_empty() || signature.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid signature format"));
    }

    // Validate timestamp freshness (reject events older than 5 minutes)
    let ts: i64 = timestamp.parse()
        .map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid timestamp"))?;
    let now = chrono::Utc::now().timestamp();
    if (now - ts).abs() > 300 {
        return Err(err(StatusCode::BAD_REQUEST, "Webhook event too old"));
    }

    // Compute expected signature
    let signed_payload = format!("{timestamp}.{payload}");
    use hmac::{Hmac, Mac};
    type HmacSha256 = Hmac<sha2::Sha256>;
    let mut mac = HmacSha256::new_from_slice(webhook_secret.as_bytes())
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "HMAC error"))?;
    mac.update(signed_payload.as_bytes());
    let expected = hex::encode(mac.finalize().into_bytes());

    if expected.as_bytes().ct_eq(signature.as_bytes()).unwrap_u8() != 1 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid signature"));
    }

    // Parse event
    let event: serde_json::Value = serde_json::from_str(payload)
        .map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid JSON"))?;

    let event_type = event["type"].as_str().unwrap_or("");

    match event_type {
        "checkout.session.completed" => {
            let session = &event["data"]["object"];
            let user_id = session["metadata"]["user_id"].as_str().unwrap_or("");
            let plan = session["metadata"]["plan"].as_str().unwrap_or("starter");
            let subscription_id = session["subscription"].as_str().unwrap_or("");

            if !user_id.is_empty() {
                let pd = plan_def(plan);
                sqlx::query(
                    "UPDATE users SET plan = $1, plan_status = 'active', \
                     plan_server_limit = $2, stripe_subscription_id = $3 WHERE id = $4::uuid",
                )
                .bind(plan)
                .bind(pd.server_limit)
                .bind(subscription_id)
                .bind(user_id)
                .execute(&state.db)
                .await
                .map_err(|e| internal_error("webhook", e))?;

                tracing::info!("User {user_id} subscribed to {plan} plan ({})", pd.name);
            }
        }
        "customer.subscription.updated" => {
            let sub = &event["data"]["object"];
            let status = sub["status"].as_str().unwrap_or("");
            let plan = sub["metadata"]["plan"].as_str().unwrap_or("starter");
            let sub_id = sub["id"].as_str().unwrap_or("");

            let plan_status = match status {
                "active" | "trialing" => "active",
                "past_due" => "grace",
                "canceled" | "unpaid" => "suspended",
                _ => status,
            };

            let pd = plan_def(plan);
            sqlx::query(
                "UPDATE users SET plan = $1, plan_status = $2, plan_server_limit = $3 \
                 WHERE stripe_subscription_id = $4",
            )
            .bind(plan)
            .bind(plan_status)
            .bind(pd.server_limit)
            .bind(sub_id)
            .execute(&state.db)
            .await
            .map_err(|e| internal_error("webhook", e))?;

            tracing::info!("Subscription {sub_id} updated: {plan} / {plan_status}");
        }
        "customer.subscription.deleted" => {
            let sub = &event["data"]["object"];
            let sub_id = sub["id"].as_str().unwrap_or("");

            sqlx::query(
                "UPDATE users SET plan = 'free', plan_status = 'active', \
                 plan_server_limit = 1, stripe_subscription_id = NULL \
                 WHERE stripe_subscription_id = $1",
            )
            .bind(sub_id)
            .execute(&state.db)
            .await
            .map_err(|e| internal_error("webhook", e))?;

            tracing::info!("Subscription {sub_id} deleted — user downgraded to free");
        }
        "invoice.payment_failed" => {
            let invoice = &event["data"]["object"];
            let sub_id = invoice["subscription"].as_str().unwrap_or("");

            sqlx::query(
                "UPDATE users SET plan_status = 'grace' WHERE stripe_subscription_id = $1",
            )
            .bind(sub_id)
            .execute(&state.db)
            .await
            .map_err(|e| internal_error("webhook", e))?;

            tracing::warn!("Payment failed for subscription {sub_id} — set to grace period");
        }
        _ => {
            tracing::debug!("Unhandled Stripe event: {event_type}");
        }
    }

    Ok(Json(serde_json::json!({ "received": true })))
}
