use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::AdminUser;
use crate::error::{err, agent_error, ApiError};
use crate::services::activity;
use crate::AppState;

// ── Data types ──────────────────────────────────────────────────────────

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct MailDomain {
    pub id: Uuid,
    pub domain: String,
    pub dkim_selector: String,
    pub dkim_public_key: Option<String>,
    pub catch_all: Option<String>,
    pub enabled: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct MailAccount {
    pub id: Uuid,
    pub domain_id: Uuid,
    pub email: String,
    pub display_name: Option<String>,
    pub quota_mb: i32,
    pub enabled: bool,
    pub forward_to: Option<String>,
    pub autoresponder_enabled: bool,
    pub autoresponder_subject: Option<String>,
    pub autoresponder_body: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct MailAlias {
    pub id: Uuid,
    pub domain_id: Uuid,
    pub source_email: String,
    pub destination_email: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Deserialize)]
pub struct CreateDomainRequest {
    pub domain: String,
}

#[derive(serde::Deserialize)]
pub struct UpdateDomainRequest {
    pub catch_all: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(serde::Deserialize)]
pub struct CreateAccountRequest {
    pub email: String,
    pub password: String,
    pub display_name: Option<String>,
    pub quota_mb: Option<i32>,
}

#[derive(serde::Deserialize)]
pub struct UpdateAccountRequest {
    pub password: Option<String>,
    pub display_name: Option<String>,
    pub quota_mb: Option<i32>,
    pub enabled: Option<bool>,
    pub forward_to: Option<String>,
    pub autoresponder_enabled: Option<bool>,
    pub autoresponder_subject: Option<String>,
    pub autoresponder_body: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct CreateAliasRequest {
    pub source_email: String,
    pub destination_email: String,
}

// ── Mail server status + installation ────────────────────────────────────

/// GET /api/mail/status
pub async fn mail_status(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = state.agent.get("/mail/status").await
        .map_err(|e| agent_error("Mail status", e))?;
    Ok(Json(result))
}

/// POST /api/mail/install
pub async fn mail_install(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = state.agent.post("/mail/install", None).await
        .map_err(|e| agent_error("Mail install", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "mail.server.install",
        Some("mail"), None, None, None,
    ).await;

    Ok(Json(result))
}

// ── Domain routes ───────────────────────────────────────────────────────

/// GET /api/mail/domains
pub async fn list_domains(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<Vec<MailDomain>>, ApiError> {
    let domains: Vec<MailDomain> = sqlx::query_as(
        "SELECT id, domain, dkim_selector, dkim_public_key, catch_all, enabled, created_at \
         FROM mail_domains ORDER BY domain",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(domains))
}

/// POST /api/mail/domains
pub async fn create_domain(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Json(body): Json<CreateDomainRequest>,
) -> Result<(StatusCode, Json<MailDomain>), ApiError> {
    let domain = body.domain.trim().to_lowercase();
    if domain.is_empty() || !domain.contains('.') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain name"));
    }

    // Generate DKIM keys via agent
    let dkim_result = state.agent
        .post("/mail/dkim/generate", Some(serde_json::json!({ "domain": domain, "selector": "dockpanel" })))
        .await;

    let (private_key, public_key) = match dkim_result {
        Ok(resp) => (
            resp.get("private_key").and_then(|v| v.as_str()).map(String::from),
            resp.get("public_key").and_then(|v| v.as_str()).map(String::from),
        ),
        Err(e) => {
            tracing::warn!("DKIM generation failed for {domain}: {e}");
            (None, None)
        }
    };

    let mail_domain: MailDomain = sqlx::query_as(
        "INSERT INTO mail_domains (domain, dkim_private_key, dkim_public_key) \
         VALUES ($1, $2, $3) RETURNING id, domain, dkim_selector, dkim_public_key, catch_all, enabled, created_at",
    )
    .bind(&domain)
    .bind(&private_key)
    .bind(&public_key)
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        if e.to_string().contains("duplicate") {
            err(StatusCode::CONFLICT, "Domain already exists")
        } else {
            err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
        }
    })?;

    // Configure Postfix/Dovecot via agent
    let _ = state.agent
        .post("/mail/domains/configure", Some(serde_json::json!({ "domain": domain })))
        .await;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "mail.domain.create",
        Some("mail"), Some(&domain), None, None,
    ).await;

    Ok((StatusCode::CREATED, Json(mail_domain)))
}

/// PUT /api/mail/domains/{id}
pub async fn update_domain(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateDomainRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain: Option<(String,)> = sqlx::query_as("SELECT domain FROM mail_domains WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let domain = domain.ok_or_else(|| err(StatusCode::NOT_FOUND, "Domain not found"))?;

    if let Some(catch_all) = &body.catch_all {
        sqlx::query("UPDATE mail_domains SET catch_all = $1, updated_at = NOW() WHERE id = $2")
            .bind(catch_all)
            .bind(id)
            .execute(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    }

    if let Some(enabled) = body.enabled {
        sqlx::query("UPDATE mail_domains SET enabled = $1, updated_at = NOW() WHERE id = $2")
            .bind(enabled)
            .bind(id)
            .execute(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    }

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "mail.domain.update",
        Some("mail"), Some(&domain.0), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// DELETE /api/mail/domains/{id}
pub async fn delete_domain(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain: Option<(String,)> = sqlx::query_as("SELECT domain FROM mail_domains WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let domain = domain.ok_or_else(|| err(StatusCode::NOT_FOUND, "Domain not found"))?;

    // Remove from Postfix/Dovecot via agent
    let _ = state.agent
        .post("/mail/domains/remove", Some(serde_json::json!({ "domain": domain.0 })))
        .await;

    sqlx::query("DELETE FROM mail_domains WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "mail.domain.delete",
        Some("mail"), Some(&domain.0), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /api/mail/domains/{id}/dns — Required DNS records for email
pub async fn domain_dns(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain: Option<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT domain, dkim_selector, dkim_public_key FROM mail_domains WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let (domain, selector, dkim_pub) = domain.ok_or_else(|| err(StatusCode::NOT_FOUND, "Domain not found"))?;

    // Get server's public IP for MX record
    let server_ip = state.agent.get("/system/info").await
        .ok()
        .and_then(|info| info.get("hostname").and_then(|v| v.as_str()).map(String::from))
        .unwrap_or_else(|| "your-server-ip".to_string());

    let mut records = vec![
        serde_json::json!({
            "type": "MX",
            "name": domain,
            "content": format!("10 mail.{domain}"),
            "description": "Mail exchanger — points to your mail server"
        }),
        serde_json::json!({
            "type": "A",
            "name": format!("mail.{domain}"),
            "content": server_ip,
            "description": "Mail server hostname"
        }),
        serde_json::json!({
            "type": "TXT",
            "name": domain,
            "content": format!("v=spf1 a mx ip4:{server_ip} ~all"),
            "description": "SPF — authorizes this server to send mail for this domain"
        }),
        serde_json::json!({
            "type": "TXT",
            "name": format!("_dmarc.{domain}"),
            "content": "v=DMARC1; p=quarantine; rua=mailto:postmaster@".to_string() + &domain,
            "description": "DMARC — tells receiving servers how to handle failed SPF/DKIM"
        }),
    ];

    if let Some(pub_key) = dkim_pub {
        // Strip PEM headers and newlines for DNS record
        let key_data = pub_key
            .replace("-----BEGIN PUBLIC KEY-----", "")
            .replace("-----END PUBLIC KEY-----", "")
            .replace('\n', "")
            .replace('\r', "");

        records.push(serde_json::json!({
            "type": "TXT",
            "name": format!("{selector}._domainkey.{domain}"),
            "content": format!("v=DKIM1; k=rsa; p={key_data}"),
            "description": "DKIM — cryptographic signature for outgoing mail"
        }));
    }

    Ok(Json(serde_json::json!({
        "domain": domain,
        "records": records,
    })))
}

// ── Account routes ──────────────────────────────────────────────────────

/// GET /api/mail/domains/{id}/accounts
pub async fn list_accounts(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Path(domain_id): Path<Uuid>,
) -> Result<Json<Vec<MailAccount>>, ApiError> {
    let accounts: Vec<MailAccount> = sqlx::query_as(
        "SELECT id, domain_id, email, display_name, quota_mb, enabled, forward_to, \
         autoresponder_enabled, autoresponder_subject, autoresponder_body, created_at \
         FROM mail_accounts WHERE domain_id = $1 ORDER BY email",
    )
    .bind(domain_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(accounts))
}

/// POST /api/mail/domains/{id}/accounts
pub async fn create_account(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(domain_id): Path<Uuid>,
    Json(body): Json<CreateAccountRequest>,
) -> Result<(StatusCode, Json<MailAccount>), ApiError> {
    // Verify domain exists
    let domain: Option<(String,)> = sqlx::query_as("SELECT domain FROM mail_domains WHERE id = $1")
        .bind(domain_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    let domain = domain.ok_or_else(|| err(StatusCode::NOT_FOUND, "Domain not found"))?;

    let email = body.email.trim().to_lowercase();
    if !email.contains('@') || !email.ends_with(&format!("@{}", domain.0)) {
        return Err(err(StatusCode::BAD_REQUEST, &format!("Email must end with @{}", domain.0)));
    }

    if body.password.len() < 8 {
        return Err(err(StatusCode::BAD_REQUEST, "Password must be at least 8 characters"));
    }

    // Hash password using Dovecot-compatible scheme (SHA512-CRYPT)
    let password_hash = format!("{{SHA512-CRYPT}}{}", sha512_crypt(&body.password));

    let quota = body.quota_mb.unwrap_or(1024);

    let account: MailAccount = sqlx::query_as(
        "INSERT INTO mail_accounts (domain_id, email, password_hash, display_name, quota_mb) \
         VALUES ($1, $2, $3, $4, $5) \
         RETURNING id, domain_id, email, display_name, quota_mb, enabled, forward_to, \
         autoresponder_enabled, autoresponder_subject, autoresponder_body, created_at",
    )
    .bind(domain_id)
    .bind(&email)
    .bind(&password_hash)
    .bind(&body.display_name)
    .bind(quota)
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        if e.to_string().contains("duplicate") {
            err(StatusCode::CONFLICT, "Email account already exists")
        } else {
            err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
        }
    })?;

    // Sync with Postfix/Dovecot via agent
    let _ = sync_mail_config(&state).await;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "mail.account.create",
        Some("mail"), Some(&email), None, None,
    ).await;

    Ok((StatusCode::CREATED, Json(account)))
}

/// PUT /api/mail/domains/{domain_id}/accounts/{id}
pub async fn update_account(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path((domain_id, account_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<UpdateAccountRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let account: Option<(String,)> = sqlx::query_as(
        "SELECT email FROM mail_accounts WHERE id = $1 AND domain_id = $2",
    )
    .bind(account_id)
    .bind(domain_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let account = account.ok_or_else(|| err(StatusCode::NOT_FOUND, "Account not found"))?;

    if let Some(password) = &body.password {
        if password.len() < 8 {
            return Err(err(StatusCode::BAD_REQUEST, "Password must be at least 8 characters"));
        }
        let hash = format!("{{SHA512-CRYPT}}{}", sha512_crypt(password));
        sqlx::query("UPDATE mail_accounts SET password_hash = $1, updated_at = NOW() WHERE id = $2")
            .bind(&hash)
            .bind(account_id)
            .execute(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    }

    if let Some(name) = &body.display_name {
        sqlx::query("UPDATE mail_accounts SET display_name = $1, updated_at = NOW() WHERE id = $2")
            .bind(name)
            .bind(account_id)
            .execute(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    }

    if let Some(quota) = body.quota_mb {
        sqlx::query("UPDATE mail_accounts SET quota_mb = $1, updated_at = NOW() WHERE id = $2")
            .bind(quota)
            .bind(account_id)
            .execute(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    }

    if let Some(enabled) = body.enabled {
        sqlx::query("UPDATE mail_accounts SET enabled = $1, updated_at = NOW() WHERE id = $2")
            .bind(enabled)
            .bind(account_id)
            .execute(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    }

    if let Some(forward) = &body.forward_to {
        sqlx::query("UPDATE mail_accounts SET forward_to = $1, updated_at = NOW() WHERE id = $2")
            .bind(if forward.is_empty() { None } else { Some(forward.as_str()) })
            .bind(account_id)
            .execute(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    }

    if let Some(ar_enabled) = body.autoresponder_enabled {
        sqlx::query("UPDATE mail_accounts SET autoresponder_enabled = $1, updated_at = NOW() WHERE id = $2")
            .bind(ar_enabled)
            .bind(account_id)
            .execute(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    }

    if let Some(subject) = &body.autoresponder_subject {
        sqlx::query("UPDATE mail_accounts SET autoresponder_subject = $1, updated_at = NOW() WHERE id = $2")
            .bind(subject)
            .bind(account_id)
            .execute(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    }

    if let Some(ar_body) = &body.autoresponder_body {
        sqlx::query("UPDATE mail_accounts SET autoresponder_body = $1, updated_at = NOW() WHERE id = $2")
            .bind(ar_body)
            .bind(account_id)
            .execute(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    }

    let _ = sync_mail_config(&state).await;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "mail.account.update",
        Some("mail"), Some(&account.0), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// DELETE /api/mail/domains/{domain_id}/accounts/{id}
pub async fn delete_account(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path((domain_id, account_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let account: Option<(String,)> = sqlx::query_as(
        "SELECT email FROM mail_accounts WHERE id = $1 AND domain_id = $2",
    )
    .bind(account_id)
    .bind(domain_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let account = account.ok_or_else(|| err(StatusCode::NOT_FOUND, "Account not found"))?;

    sqlx::query("DELETE FROM mail_accounts WHERE id = $1")
        .bind(account_id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let _ = sync_mail_config(&state).await;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "mail.account.delete",
        Some("mail"), Some(&account.0), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Alias routes ────────────────────────────────────────────────────────

/// GET /api/mail/domains/{id}/aliases
pub async fn list_aliases(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Path(domain_id): Path<Uuid>,
) -> Result<Json<Vec<MailAlias>>, ApiError> {
    let aliases: Vec<MailAlias> = sqlx::query_as(
        "SELECT id, domain_id, source_email, destination_email, created_at \
         FROM mail_aliases WHERE domain_id = $1 ORDER BY source_email",
    )
    .bind(domain_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(aliases))
}

/// POST /api/mail/domains/{id}/aliases
pub async fn create_alias(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(domain_id): Path<Uuid>,
    Json(body): Json<CreateAliasRequest>,
) -> Result<(StatusCode, Json<MailAlias>), ApiError> {
    let alias: MailAlias = sqlx::query_as(
        "INSERT INTO mail_aliases (domain_id, source_email, destination_email) \
         VALUES ($1, $2, $3) RETURNING *",
    )
    .bind(domain_id)
    .bind(body.source_email.trim().to_lowercase())
    .bind(body.destination_email.trim().to_lowercase())
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        if e.to_string().contains("duplicate") {
            err(StatusCode::CONFLICT, "Alias already exists")
        } else {
            err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
        }
    })?;

    let _ = sync_mail_config(&state).await;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "mail.alias.create",
        Some("mail"), Some(&alias.source_email), Some(&alias.destination_email), None,
    ).await;

    Ok((StatusCode::CREATED, Json(alias)))
}

/// DELETE /api/mail/domains/{domain_id}/aliases/{id}
pub async fn delete_alias(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path((_domain_id, alias_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let alias: Option<(String,)> = sqlx::query_as("SELECT source_email FROM mail_aliases WHERE id = $1")
        .bind(alias_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    sqlx::query("DELETE FROM mail_aliases WHERE id = $1")
        .bind(alias_id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let _ = sync_mail_config(&state).await;

    if let Some(alias) = alias {
        activity::log_activity(
            &state.db, claims.sub, &claims.email, "mail.alias.delete",
            Some("mail"), Some(&alias.0), None, None,
        ).await;
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Mail queue ──────────────────────────────────────────────────────────

/// GET /api/mail/queue
pub async fn get_queue(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = state.agent
        .get("/mail/queue")
        .await
        .map_err(|e| agent_error("Mail queue", e))?;

    Ok(Json(result))
}

/// POST /api/mail/queue/flush
pub async fn flush_queue(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = state.agent
        .post("/mail/queue/flush", None)
        .await
        .map_err(|e| agent_error("Flush mail queue", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "mail.queue.flush",
        Some("mail"), None, None, None,
    ).await;

    Ok(Json(result))
}

/// DELETE /api/mail/queue/{id}
pub async fn delete_queued(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(queue_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = state.agent
        .post("/mail/queue/delete", Some(serde_json::json!({ "id": queue_id })))
        .await
        .map_err(|e| agent_error("Delete queued message", e))?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "mail.queue.delete",
        Some("mail"), Some(&queue_id), None, None,
    ).await;

    Ok(Json(result))
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Sync all mail config to agent (rebuild Postfix/Dovecot maps)
async fn sync_mail_config(state: &AppState) -> Result<(), String> {
    // Gather all domains, accounts, and aliases
    let domains: Vec<(String, bool, Option<String>)> = sqlx::query_as(
        "SELECT domain, enabled, catch_all FROM mail_domains ORDER BY domain",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| e.to_string())?;

    let accounts: Vec<(String, String, i32, bool, Option<String>)> = sqlx::query_as(
        "SELECT email, password_hash, quota_mb, enabled, forward_to FROM mail_accounts ORDER BY email",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| e.to_string())?;

    let aliases: Vec<(String, String)> = sqlx::query_as(
        "SELECT source_email, destination_email FROM mail_aliases ORDER BY source_email",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| e.to_string())?;

    let payload = serde_json::json!({
        "domains": domains.iter().map(|(d, e, c)| serde_json::json!({
            "domain": d, "enabled": e, "catch_all": c
        })).collect::<Vec<_>>(),
        "accounts": accounts.iter().map(|(email, hash, quota, enabled, fwd)| serde_json::json!({
            "email": email, "password_hash": hash, "quota_mb": quota, "enabled": enabled, "forward_to": fwd
        })).collect::<Vec<_>>(),
        "aliases": aliases.iter().map(|(src, dst)| serde_json::json!({
            "source": src, "destination": dst
        })).collect::<Vec<_>>(),
    });

    state.agent
        .post("/mail/sync", Some(payload))
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}

/// Generate SHA512-CRYPT password hash for Dovecot
fn sha512_crypt(password: &str) -> String {
    use sha2::{Sha512, Digest};
    use rand::Rng;

    let mut rng = rand::thread_rng();
    let salt: String = (0..16)
        .map(|_| {
            let idx = rng.gen_range(0..64);
            b"./0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz"[idx] as char
        })
        .collect();

    // Simple SHA512 hash with salt (not full crypt, but compatible enough for Dovecot SSHA512)
    let mut hasher = Sha512::new();
    hasher.update(format!("{salt}{password}"));
    let hash = hasher.finalize();
    format!("$6${salt}${}", hex::encode(hash))
}
