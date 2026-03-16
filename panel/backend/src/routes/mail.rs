use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use std::time::Instant;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::auth::AdminUser;
use crate::error::{err, agent_error, ApiError};
use crate::routes::sites::ProvisionStep;
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

/// POST /api/mail/install — Returns 202 + install_id for SSE progress tracking.
pub async fn mail_install(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let install_id = Uuid::new_v4();

    let (tx, _) = broadcast::channel::<ProvisionStep>(32);
    {
        let mut logs = state.provision_logs.lock().unwrap();
        logs.insert(install_id, (Vec::new(), tx, Instant::now()));
    }

    let logs = state.provision_logs.clone();
    let agent = state.agent.clone();
    let db = state.db.clone();
    let user_id = claims.sub;
    let email = claims.email.clone();

    tokio::spawn(async move {
        let emit = |step: &str, label: &str, status: &str, msg: Option<String>| {
            let ev = ProvisionStep {
                step: step.into(), label: label.into(), status: status.into(), message: msg,
            };
            if let Ok(mut map) = logs.lock() {
                if let Some((history, tx, _)) = map.get_mut(&install_id) {
                    history.push(ev.clone());
                    let _ = tx.send(ev);
                }
            }
        };

        emit("install", "Installing mail server", "in_progress", None);

        match agent.post("/mail/install", None).await {
            Ok(_) => {
                emit("install", "Installing mail server", "done", None);
                emit("complete", "Mail server installed", "done", None);
                activity::log_activity(
                    &db, user_id, &email, "mail.server.install",
                    Some("mail"), None, None, None,
                ).await;
                tracing::info!("Mail server installed");
            }
            Err(e) => {
                emit("install", "Installing mail server", "error", Some(format!("{e}")));
                emit("complete", "Install failed", "error", None);
                tracing::error!("Mail server install failed: {e}");
            }
        }

        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        logs.lock().unwrap().remove(&install_id);
    });

    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({
        "install_id": install_id,
        "message": "Mail server installation started",
    }))))
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

    // ── Auto-DNS: create MX, A, SPF, DMARC, DKIM records ─────────────────
    let dns_domain = domain.clone();
    let dns_dkim_pub = public_key.clone();
    let dns_db = state.db.clone();
    let dns_agent = state.agent.clone();
    let dns_user = claims.sub;
    let dns_email = claims.email.clone();
    tokio::spawn(async move {
        if let Err(e) = auto_create_mail_dns(
            &dns_db, &dns_agent, dns_user, &dns_email,
            &dns_domain, dns_dkim_pub.as_deref(),
        ).await {
            tracing::warn!("Auto-DNS for mail domain {dns_domain} failed: {e}");
        }
    });

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

    // Fetch DKIM selector before deletion (needed for DNS cleanup)
    let dkim_info: Option<(String,)> = sqlx::query_as(
        "SELECT dkim_selector FROM mail_domains WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();
    let dkim_selector = dkim_info.map(|d| d.0).unwrap_or_else(|| "dockpanel".to_string());

    // Remove from Postfix/Dovecot via agent
    let _ = state.agent
        .post("/mail/domains/remove", Some(serde_json::json!({ "domain": domain.0 })))
        .await;

    sqlx::query("DELETE FROM mail_domains WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    // ── Auto-DNS cleanup: delete MX, A, SPF, DMARC, DKIM records ─────────
    let dns_domain = domain.0.clone();
    let dns_db = state.db.clone();
    let dns_user = claims.sub;
    tokio::spawn(async move {
        if let Err(e) = auto_delete_mail_dns(
            &dns_db, dns_user, &dns_domain, &dkim_selector,
        ).await {
            tracing::warn!("Auto-DNS cleanup for mail domain {dns_domain} failed: {e}");
        }
    });

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

// ── Auto-DNS helpers for mail domains ───────────────────────────────────

/// Extract the parent/root domain from a subdomain.
/// e.g. "mail.example.com" → "example.com", "example.com" → "example.com"
fn extract_parent_domain(domain: &str) -> String {
    let parts: Vec<&str> = domain.split('.').collect();
    if parts.len() > 2 {
        parts[parts.len() - 2..].join(".")
    } else {
        domain.to_string()
    }
}

/// Detect the server's public IPv4 address.
async fn detect_public_ip() -> String {
    match reqwest::Client::new()
        .get("https://api.ipify.org")
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        Ok(resp) => {
            let ip = resp.text().await.unwrap_or_default().trim().to_string();
            if ip.is_empty() { String::new() } else { ip }
        }
        Err(_) => {
            use std::net::UdpSocket;
            UdpSocket::bind("0.0.0.0:0")
                .and_then(|s| { s.connect("8.8.8.8:53")?; s.local_addr() })
                .map(|a| a.ip().to_string())
                .unwrap_or_default()
        }
    }
}

/// Build Cloudflare API headers from credentials.
fn cf_headers(token: &str, email: Option<&str>) -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    if let Some(em) = email {
        if let (Ok(e_val), Ok(k_val)) = (em.parse(), token.parse()) {
            headers.insert("X-Auth-Email", e_val);
            headers.insert("X-Auth-Key", k_val);
        }
    } else if let Ok(bearer) = format!("Bearer {token}").parse() {
        headers.insert("Authorization", bearer);
    }
    headers
}

/// Auto-create DNS records (MX, A, SPF, DMARC, DKIM) for a new mail domain.
/// Runs in a background task — errors are logged, not returned to the user.
async fn auto_create_mail_dns(
    db: &sqlx::PgPool,
    agent: &crate::services::agent::AgentClient,
    user_id: uuid::Uuid,
    user_email: &str,
    domain: &str,
    dkim_public_key: Option<&str>,
) -> Result<(), String> {
    let parent = extract_parent_domain(domain);

    // Look up DNS zone for the parent domain
    let zone: Option<(String, Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT provider, cf_zone_id, cf_api_token, cf_api_email FROM dns_zones WHERE domain = $1 AND user_id = $2"
    )
    .bind(&parent)
    .bind(user_id)
    .fetch_optional(db)
    .await
    .map_err(|e| e.to_string())?;

    let (provider, cf_zone_id, cf_api_token, cf_api_email) = match zone {
        Some(z) => z,
        None => {
            tracing::info!("No DNS zone found for {parent} — skipping auto-DNS for mail domain {domain}");
            return Ok(());
        }
    };

    let server_ip = detect_public_ip().await;
    if server_ip.is_empty() {
        return Err("Could not detect server public IP".into());
    }

    // Prepare DKIM TXT value if key is available
    let dkim_txt = dkim_public_key.map(|pk| {
        let key_data = pk
            .replace("-----BEGIN PUBLIC KEY-----", "")
            .replace("-----END PUBLIC KEY-----", "")
            .replace('\n', "")
            .replace('\r', "");
        format!("v=DKIM1; k=rsa; p={key_data}")
    });

    if provider == "cloudflare" {
        let (zone_id, token) = match (cf_zone_id, cf_api_token) {
            (Some(z), Some(t)) => (z, t),
            _ => return Err("Cloudflare zone missing zone_id or token".into()),
        };

        let client = reqwest::Client::new();
        let headers = cf_headers(&token, cf_api_email.as_deref());
        let cf_url = format!("https://api.cloudflare.com/client/v4/zones/{zone_id}/dns_records");

        // All mail records MUST be proxied: false (DNS-only)

        // 1. A record (DNS-only — SMTP cannot traverse CF proxy)
        let _ = client.post(&cf_url).headers(headers.clone()).json(&serde_json::json!({
            "type": "A", "name": domain, "content": server_ip, "proxied": false, "ttl": 1,
        })).send().await;
        tracing::info!("Auto-DNS (mail): created A record {domain} → {server_ip}");

        // 2. MX record
        let _ = client.post(&cf_url).headers(headers.clone()).json(&serde_json::json!({
            "type": "MX", "name": domain, "content": domain, "priority": 10, "ttl": 1,
        })).send().await;
        tracing::info!("Auto-DNS (mail): created MX record {domain} → {domain} (pri 10)");

        // 3. SPF TXT record
        let spf = format!("v=spf1 ip4:{server_ip} -all");
        let _ = client.post(&cf_url).headers(headers.clone()).json(&serde_json::json!({
            "type": "TXT", "name": domain, "content": spf, "ttl": 1,
        })).send().await;
        tracing::info!("Auto-DNS (mail): created SPF TXT for {domain}");

        // 4. DMARC TXT record
        let dmarc = format!("v=DMARC1; p=quarantine; rua=mailto:postmaster@{domain}");
        let dmarc_name = format!("_dmarc.{domain}");
        let _ = client.post(&cf_url).headers(headers.clone()).json(&serde_json::json!({
            "type": "TXT", "name": dmarc_name, "content": dmarc, "ttl": 1,
        })).send().await;
        tracing::info!("Auto-DNS (mail): created DMARC TXT for {domain}");

        // 5. DKIM TXT record (if key available)
        if let Some(dkim_val) = &dkim_txt {
            let dkim_name = format!("dockpanel._domainkey.{domain}");
            let _ = client.post(&cf_url).headers(headers.clone()).json(&serde_json::json!({
                "type": "TXT", "name": dkim_name, "content": dkim_val, "ttl": 1,
            })).send().await;
            tracing::info!("Auto-DNS (mail): created DKIM TXT for {domain}");
        }

        // ── Auto-SSL: provision certificate for the mail domain ───────────
        // Wait briefly for DNS propagation before attempting ACME HTTP-01
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        match agent.post(&format!("/ssl/provision/{domain}"), Some(serde_json::json!({
            "email": user_email,
            "runtime": "static",
        }))).await {
            Ok(_) => tracing::info!("Auto-SSL (mail): provisioned certificate for {domain}"),
            Err(e) => tracing::warn!("Auto-SSL (mail): failed for {domain}: {e} — provision manually"),
        }
    } else if provider == "powerdns" {
        // Get PowerDNS settings
        let pdns: Vec<(String, String)> = sqlx::query_as(
            "SELECT key, value FROM settings WHERE key IN ('pdns_api_url', 'pdns_api_key')"
        ).fetch_all(db).await.unwrap_or_default();
        let pdns_url = pdns.iter().find(|(k,_)| k == "pdns_api_url").map(|(_,v)| v.clone());
        let pdns_key = pdns.iter().find(|(k,_)| k == "pdns_api_key").map(|(_,v)| v.clone());

        let (url, key) = match (pdns_url, pdns_key) {
            (Some(u), Some(k)) => (u, k),
            _ => return Err("PowerDNS not configured".into()),
        };

        let client = reqwest::Client::new();
        let zone_fqdn = if parent.ends_with('.') { parent.clone() } else { format!("{parent}.") };
        let domain_fqdn = format!("{domain}.");

        let mut rrsets = vec![
            // A record
            serde_json::json!({
                "name": &domain_fqdn, "type": "A", "ttl": 300, "changetype": "REPLACE",
                "records": [{ "content": &server_ip, "disabled": false }]
            }),
            // MX record (PowerDNS includes priority in content)
            serde_json::json!({
                "name": &domain_fqdn, "type": "MX", "ttl": 300, "changetype": "REPLACE",
                "records": [{ "content": format!("10 {domain_fqdn}"), "disabled": false }]
            }),
        ];

        // SPF + DMARC as separate TXT rrsets (different names)
        let spf = format!("\"v=spf1 ip4:{server_ip} -all\"");
        rrsets.push(serde_json::json!({
            "name": &domain_fqdn, "type": "TXT", "ttl": 300, "changetype": "REPLACE",
            "records": [{ "content": &spf, "disabled": false }]
        }));

        let dmarc_name = format!("_dmarc.{domain_fqdn}");
        let dmarc = format!("\"v=DMARC1; p=quarantine; rua=mailto:postmaster@{domain}\"");
        rrsets.push(serde_json::json!({
            "name": &dmarc_name, "type": "TXT", "ttl": 300, "changetype": "REPLACE",
            "records": [{ "content": &dmarc, "disabled": false }]
        }));

        // DKIM TXT record
        if let Some(dkim_val) = &dkim_txt {
            let dkim_name = format!("dockpanel._domainkey.{domain_fqdn}");
            let dkim_quoted = format!("\"{dkim_val}\"");
            rrsets.push(serde_json::json!({
                "name": &dkim_name, "type": "TXT", "ttl": 300, "changetype": "REPLACE",
                "records": [{ "content": &dkim_quoted, "disabled": false }]
            }));
        }

        let result = client
            .patch(&format!("{url}/api/v1/servers/localhost/zones/{zone_fqdn}"))
            .header("X-API-Key", &key)
            .json(&serde_json::json!({ "rrsets": rrsets }))
            .send()
            .await;

        match result {
            Ok(resp) if resp.status().is_success() => {
                tracing::info!("Auto-DNS (mail/PowerDNS): created all records for {domain}");
            }
            Ok(resp) => {
                let text = resp.text().await.unwrap_or_default();
                return Err(format!("PowerDNS error: {text}"));
            }
            Err(e) => return Err(format!("PowerDNS API error: {e}")),
        }

        // ── Auto-SSL for PowerDNS ────────────────────────────────────────
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        match agent.post(&format!("/ssl/provision/{domain}"), Some(serde_json::json!({
            "email": user_email,
            "runtime": "static",
        }))).await {
            Ok(_) => tracing::info!("Auto-SSL (mail): provisioned certificate for {domain}"),
            Err(e) => tracing::warn!("Auto-SSL (mail): failed for {domain}: {e} — provision manually"),
        }
    }

    Ok(())
}

/// Auto-delete all DNS records for a removed mail domain.
/// Runs in a background task — errors are logged, not returned to the user.
async fn auto_delete_mail_dns(
    db: &sqlx::PgPool,
    user_id: uuid::Uuid,
    domain: &str,
    dkim_selector: &str,
) -> Result<(), String> {
    let parent = extract_parent_domain(domain);

    let zone: Option<(String, Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT provider, cf_zone_id, cf_api_token, cf_api_email FROM dns_zones WHERE domain = $1 AND user_id = $2"
    )
    .bind(&parent)
    .bind(user_id)
    .fetch_optional(db)
    .await
    .map_err(|e| e.to_string())?;

    let (provider, cf_zone_id, cf_api_token, cf_api_email) = match zone {
        Some(z) => z,
        None => {
            tracing::info!("No DNS zone found for {parent} — skipping DNS cleanup for mail domain {domain}");
            return Ok(());
        }
    };

    if provider == "cloudflare" {
        let (zone_id, token) = match (cf_zone_id, cf_api_token) {
            (Some(z), Some(t)) => (z, t),
            _ => return Err("Cloudflare zone missing zone_id or token".into()),
        };

        let client = reqwest::Client::new();
        let headers = cf_headers(&token, cf_api_email.as_deref());

        // Collect all record names we need to clean up
        let names_to_check = vec![
            domain.to_string(),
            format!("_dmarc.{domain}"),
            format!("{dkim_selector}._domainkey.{domain}"),
        ];

        for name in &names_to_check {
            // Query all record types for this name (A, MX, TXT, CNAME, etc.)
            let list_url = format!(
                "https://api.cloudflare.com/client/v4/zones/{zone_id}/dns_records?name={name}&per_page=50"
            );
            if let Ok(resp) = client.get(&list_url).headers(headers.clone()).send().await {
                if let Ok(data) = resp.json::<serde_json::Value>().await {
                    if let Some(records) = data.get("result").and_then(|r| r.as_array()) {
                        for record in records {
                            if let Some(rid) = record.get("id").and_then(|v| v.as_str()) {
                                let del_url = format!(
                                    "https://api.cloudflare.com/client/v4/zones/{zone_id}/dns_records/{rid}"
                                );
                                let _ = client.delete(&del_url).headers(headers.clone()).send().await;
                                let rtype = record.get("type").and_then(|v| v.as_str()).unwrap_or("?");
                                tracing::info!("Auto-DNS cleanup (mail): deleted {rtype} record for {name}");
                            }
                        }
                    }
                }
            }
        }
    } else if provider == "powerdns" {
        let pdns: Vec<(String, String)> = sqlx::query_as(
            "SELECT key, value FROM settings WHERE key IN ('pdns_api_url', 'pdns_api_key')"
        ).fetch_all(db).await.unwrap_or_default();
        let pdns_url = pdns.iter().find(|(k,_)| k == "pdns_api_url").map(|(_,v)| v.clone());
        let pdns_key = pdns.iter().find(|(k,_)| k == "pdns_api_key").map(|(_,v)| v.clone());

        if let (Some(url), Some(key)) = (pdns_url, pdns_key) {
            let zone_fqdn = if parent.ends_with('.') { parent.clone() } else { format!("{parent}.") };
            let domain_fqdn = format!("{domain}.");
            let dmarc_fqdn = format!("_dmarc.{domain}.");
            let dkim_fqdn = format!("{dkim_selector}._domainkey.{domain}.");

            let rrsets = serde_json::json!({
                "rrsets": [
                    { "name": &domain_fqdn, "type": "A", "changetype": "DELETE" },
                    { "name": &domain_fqdn, "type": "MX", "changetype": "DELETE" },
                    { "name": &domain_fqdn, "type": "TXT", "changetype": "DELETE" },
                    { "name": &dmarc_fqdn, "type": "TXT", "changetype": "DELETE" },
                    { "name": &dkim_fqdn, "type": "TXT", "changetype": "DELETE" },
                ]
            });

            let _ = reqwest::Client::new()
                .patch(&format!("{url}/api/v1/servers/localhost/zones/{zone_fqdn}"))
                .header("X-API-Key", &key)
                .json(&rrsets)
                .send()
                .await;

            tracing::info!("Auto-DNS cleanup (mail/PowerDNS): deleted all records for {domain}");
        }
    }

    Ok(())
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
