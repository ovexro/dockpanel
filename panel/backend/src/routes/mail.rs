use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::{AdminUser, Claims, ServerScope};
use crate::error::{internal_error, err, agent_error, ApiError};
use crate::routes::sites::ProvisionStep;
use crate::services::activity;
use crate::services::agent::AgentHandle;
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
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent.get("/mail/status").await
        .map_err(|e| agent_error("Mail status", e))?;
    Ok(Json(result))
}

/// POST /api/mail/install — Returns 202 + install_id for SSE progress tracking.
pub async fn mail_install(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let install_id = Uuid::new_v4();

    crate::helpers::register_provision_log(
        &state.provision_logs,
        &state.deploy_owners,
        install_id,
        claims.sub,
        32,
    );

    let logs = state.provision_logs.clone();
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
        logs.lock().unwrap_or_else(|e| e.into_inner()).remove(&install_id);
    });

    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({
        "install_id": install_id,
        "message": "Mail server installation started",
    }))))
}

/// POST /api/mail/uninstall — Uninstall mail server (admin only).
pub async fn mail_uninstall(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let install_id = Uuid::new_v4();

    crate::helpers::register_provision_log(
        &state.provision_logs,
        &state.deploy_owners,
        install_id,
        claims.sub,
        32,
    );

    let logs = state.provision_logs.clone();
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

        emit("uninstall", "Uninstalling mail server", "in_progress", None);

        match agent.post("/mail/uninstall", None).await {
            Ok(_) => {
                emit("uninstall", "Uninstalling mail server", "done", None);
                emit("complete", "Mail server uninstalled", "done", None);
                activity::log_activity(
                    &db, user_id, &email, "mail.server.uninstall",
                    Some("mail"), None, None, None,
                ).await;
                tracing::info!("Mail server uninstalled");
            }
            Err(e) => {
                emit("uninstall", "Uninstalling mail server", "error", Some(format!("{e}")));
                emit("complete", "Uninstall failed", "error", None);
                tracing::error!("Mail server uninstall failed: {e}");
            }
        }

        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        logs.lock().unwrap_or_else(|e| e.into_inner()).remove(&install_id);
    });

    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({
        "install_id": install_id,
        "message": "Mail server uninstall started",
    }))))
}

// ── Domain routes ───────────────────────────────────────────────────────

/// GET /api/mail/domains — the mail domains on the server in scope.
///
/// This read had no server term and no ownership term of any kind: it returned
/// every mail domain in the installation to every administrator, including those
/// on a fleet member somebody else registered. The page therefore listed rows the
/// handlers behind it now refuse — [`MAIL_DOMAIN_CALLER_PREDICATE`] draws that
/// line for every per-domain route, and the list that feeds them drew none.
///
/// The filter is the row's own `server_id` rather than that predicate, because a
/// list has no `{id}` to bind `$1` to. `ServerScope` has already proved the caller
/// owns the server it names (`SELECT id FROM servers WHERE id = $1 AND user_id = $2`),
/// so here the server term IS the authorisation — the same shape
/// `sites::list_for_admin` uses for the equivalent fleet-wide read.
///
/// ⚠ `server_id IS NULL` stays visible, deliberately, for the same reason the
/// predicate admits it. `20260807000000_sleep_and_destination_server_scope.sql`
/// backfills those rows, so on a migrated install there are none; but its final
/// `NOT NULL` is guarded and skipped when an install has no local server row to
/// backfill from, so the column can still hold one. A row that names no server is
/// not a row belonging to another operator — it belongs to nobody, there is
/// nothing here to test it against, and hiding it would remove it from the only
/// screen that can see it while `sync_mail_config` keeps refusing every rebuild in
/// the installation because it exists. Listed, it reaches the handlers, which
/// answer 409 naming the problem instead of 404 naming nothing.
pub async fn list_domains(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(server_id, _agent): ServerScope,
) -> Result<Json<Vec<MailDomain>>, ApiError> {
    let domains: Vec<MailDomain> = sqlx::query_as(
        "SELECT id, domain, dkim_selector, dkim_public_key, catch_all, enabled, created_at \
         FROM mail_domains WHERE server_id = $1 OR server_id IS NULL \
         ORDER BY domain LIMIT 500",
    )
    .bind(server_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("list domains", e))?;

    Ok(Json(domains))
}

/// POST /api/mail/domains
pub async fn create_domain(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(server_id, agent): ServerScope,
    Json(body): Json<CreateDomainRequest>,
) -> Result<(StatusCode, Json<MailDomain>), ApiError> {
    let domain = body.domain.trim().to_lowercase();
    if !crate::routes::is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain name"));
    }

    // Generate DKIM keys via agent
    let dkim_result = agent
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

    // Encrypt the DKIM private key before storing
    let encrypted_private_key = if let Some(ref pk) = private_key {
        Some(crate::services::secrets_crypto::encrypt_credential(pk, &state.config.jwt_secret)
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Encryption failed: {e}")))?)
    } else {
        None
    };

    // ⚠ `server_id` is written HERE and nowhere else, and this is the only handler
    // in the module where the caller's selection is the right authority: creation
    // is the moment the operator CHOOSES a host, and until this row exists there is
    // no row to derive one from. Every sibling handler derives it from the row.
    //
    // The column was never bound before. `mail_domains.server_id` has no DEFAULT
    // and is nullable, so every mail domain created through the panel since the
    // multi-server migration was stored naming no server at all — which left the
    // server switcher as the only thing deciding where a domain's mailboxes went,
    // and left `idx_mail_domains_domain_server` unable to reject a duplicate
    // (Postgres treats NULLs as distinct, and the old global UNIQUE on `domain` was
    // dropped by that same migration, so the CONFLICT arm below could not fire).
    let mail_domain: MailDomain = sqlx::query_as(
        "INSERT INTO mail_domains (domain, dkim_private_key, dkim_public_key, server_id) \
         VALUES ($1, $2, $3, $4) RETURNING id, domain, dkim_selector, dkim_public_key, catch_all, enabled, created_at",
    )
    .bind(&domain)
    .bind(&encrypted_private_key)
    .bind(&public_key)
    .bind(server_id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        if e.to_string().contains("duplicate") {
            err(StatusCode::CONFLICT, "Domain already exists")
        } else {
            internal_error("create domain", e)
        }
    })?;

    // Configure Postfix/Dovecot via agent
    let _ = agent
        .post("/mail/domains/configure", Some(serde_json::json!({ "domain": domain })))
        .await;

    // ── Auto-DNS: create MX, A, SPF, DMARC, DKIM records ─────────────────
    //
    // `server_id` travels with the domain into the background task. It is the
    // value that was just written onto the row above, so the records describe the
    // machine the mailboxes were actually created on — not whichever box this
    // process happens to run on, which is what `detect_public_ip` answered and
    // what published a member's A record pointing at the panel.
    let dns_domain = domain.clone();
    let dns_dkim_pub = public_key.clone();
    let dns_db = state.db.clone();
    let dns_agent = agent.clone();
    let dns_user = claims.sub;
    let dns_email = claims.email.clone();
    let dns_server = server_id;
    tokio::spawn(async move {
        if let Err(e) = auto_create_mail_dns(
            &dns_db, &dns_agent, dns_user, &dns_email,
            &dns_domain, dns_server, dns_dkim_pub.as_deref(),
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
    // WHICH DOMAIN and WHICH HOST both come from the row now. The read also grew an
    // authorisation term it never had: `SELECT domain FROM mail_domains WHERE id = $1`
    // asked nothing about who was calling, so `ServerScope`'s header check was the
    // entire boundary. Resolving before any write means an un-scoped or unreachable
    // host is refused with nothing half-applied behind it.
    let (domain, server_id, agent) = mail_domain_agent_for_caller(&state, id, &claims).await?;

    if let Some(catch_all) = &body.catch_all {
        if !catch_all.is_empty() && (!catch_all.contains('@') || !is_valid_mail_address(catch_all, "/")) {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid catch-all email address"));
        }
        sqlx::query("UPDATE mail_domains SET catch_all = $1, updated_at = NOW() WHERE id = $2")
            .bind(if catch_all.is_empty() { None } else { Some(catch_all.as_str()) })
            .bind(id)
            .execute(&state.db)
            .await
            .map_err(|e| internal_error("update domain", e))?;
    }

    if let Some(enabled) = body.enabled {
        sqlx::query("UPDATE mail_domains SET enabled = $1, updated_at = NOW() WHERE id = $2")
            .bind(enabled)
            .bind(id)
            .execute(&state.db)
            .await
            .map_err(|e| internal_error("update domain", e))?;
    }

    // Push the catch_all/enabled change into Postfix/Dovecot — previously written to the DB only,
    // so disabling a domain or setting a catch-all had no effect until an unrelated account change.
    sync_mail_config(&state, server_id, &agent).await?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "mail.domain.update",
        Some("mail"), Some(&domain), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// DELETE /api/mail/domains/{id}
pub async fn delete_domain(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Resolved before the row is gone, because the row is the only thing that knows
    // which host to strip the domain from. Sending `/mail/domains/remove` to the
    // switcher's host instead deleted a DKIM key on a machine that never had one and
    // left the real host still accepting the domain's mail.
    let (domain, server_id, agent) = mail_domain_agent_for_caller(&state, id, &claims).await?;

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
    let _ = agent
        .post("/mail/domains/remove", Some(serde_json::json!({ "domain": domain })))
        .await;

    sqlx::query("DELETE FROM mail_domains WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("delete domain", e))?;

    // Rebuild Postfix/Dovecot maps from the remaining rows so the deleted domain's mailboxes
    // stop authenticating and receiving immediately — the agent's /mail/domains/remove only drops
    // the DKIM key and defers map cleanup to this sync, which delete_domain previously never called.
    // `server_id` was captured above: the row it came from no longer exists.
    sync_mail_config(&state, server_id, &agent).await?;

    // ── Auto-DNS cleanup: delete MX, A, SPF, DMARC, DKIM records ─────────
    let dns_domain = domain.clone();
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
        Some("mail"), Some(&domain), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /api/mail/domains/{id}/dns — Required DNS records for email
pub async fn domain_dns(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // The same read the two DNS-verification endpoints use, so the tab that tells
    // you what to create and the checks that tell you whether you created it can
    // never describe two different domains. It also asks the question this handler
    // never asked: whether this caller may see the domain at all. Its own inline
    // `WHERE id = $1` answered only that the row exists.
    let (domain, selector, dkim_pub, server_id) =
        mail_domain_identity(&state.db, id, &claims).await?;

    // The MAIL DOMAIN'S HOST's public address — not this process's, and not the
    // agent's hostname.
    //
    // This endpoint used to read `/system/info`'s `hostname` field into a variable
    // called `server_ip` and publish it as an address, so it told operators to
    // create `A mail.example.com → my-server` and the invalid SPF value
    // `v=spf1 a mx ip4:my-server ~all`. Fixing that to `detect_public_ip` made the
    // value an address but still the WRONG one for a domain on a fleet member: it
    // is the panel's, so the tab instructed the operator to point their mail
    // domain at a machine that does not run their mail.
    //
    // A `None` here is not a write, so it does not have to refuse — but it must not
    // substitute this box either, because an operator following the instruction
    // would create the wrong record by hand and it would resolve and answer. Fall
    // back to the placeholder the endpoint already used when it could not detect an
    // address: it says "put an address here" and names no machine at all.
    let server_ip = crate::helpers::public_ip_for_server(&state.db, server_id)
        .await
        .filter(|ip| !ip.is_empty())
        .unwrap_or_else(|| "your-server-ip".to_string());

    let records: Vec<serde_json::Value> = crate::services::prerequisites::mail::mail_records(
        &domain,
        &selector,
        dkim_pub.as_deref(),
        &server_ip,
    )
    .into_iter()
    .map(|r| {
        serde_json::json!({
            "type": r.record_type,
            "name": r.fqdn,
            "content": r.value,
            "description": r.purpose.unwrap_or_default(),
        })
    })
    .collect();

    Ok(Json(serde_json::json!({
        "domain": domain,
        "records": records,
    })))
}

// ── Account routes ──────────────────────────────────────────────────────

/// GET /api/mail/domains/{id}/accounts
///
/// The mailboxes are listed only after the DOMAIN has been resolved through
/// [`MAIL_DOMAIN_CALLER_PREDICATE`]. `mail_accounts` has no server column and no
/// owner column — it reaches both only through `domain_id` — so the domain is the
/// only place this question can be asked, and the bare `WHERE domain_id = $1` here
/// asked it nowhere: any administrator could enumerate the mailboxes of any
/// domain on any machine in the fleet by id.
///
/// Resolving first rather than joining the predicate into the list query is
/// deliberate. A joined query would answer an unauthorised caller with an empty
/// list, which reads as "this domain has no mailboxes" — the failure that looks
/// like success. This way an invisible domain 404s, exactly as the mutating
/// handlers already do.
pub async fn list_accounts(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(domain_id): Path<Uuid>,
) -> Result<Json<Vec<MailAccount>>, ApiError> {
    mail_domain_identity(&state.db, domain_id, &claims).await?;

    let accounts: Vec<MailAccount> = sqlx::query_as(
        "SELECT id, domain_id, email, display_name, quota_mb, enabled, forward_to, \
         autoresponder_enabled, autoresponder_subject, autoresponder_body, created_at \
         FROM mail_accounts WHERE domain_id = $1 ORDER BY email",
    )
    .bind(domain_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("list accounts", e))?;

    Ok(Json(accounts))
}

/// POST /api/mail/domains/{id}/accounts
pub async fn create_account(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(domain_id): Path<Uuid>,
    Json(body): Json<CreateAccountRequest>,
) -> Result<(StatusCode, Json<MailAccount>), ApiError> {
    // The domain's own row decides both whether this caller may add a mailbox to it
    // and which host the mailbox is created on. The existence check that used to
    // stand here answered neither: it asked only `WHERE id = $1`, so any
    // administrator could create a mailbox under any other administrator's domain,
    // and the credential was then written to whichever host the switcher named.
    let (domain, server_id, agent) =
        mail_domain_agent_for_caller(&state, domain_id, &claims).await?;

    let email = body.email.trim().to_lowercase();
    if !email.ends_with(&format!("@{domain}")) {
        return Err(err(StatusCode::BAD_REQUEST, &format!("Email must end with @{domain}")));
    }
    if email.matches('@').count() != 1 || !is_valid_mail_address(&email, "") {
        return Err(err(StatusCode::BAD_REQUEST, "Email address contains invalid characters"));
    }

    if body.password.len() < 8 {
        return Err(err(StatusCode::BAD_REQUEST, "Password must be at least 8 characters"));
    }

    // Hash in a scheme THIS server's Dovecot can verify — Argon2id where it is
    // built in, bcrypt where it is not. Hashing in a scheme the verifier does
    // not know produces an account that is created successfully and can never
    // be opened. "THIS server" now means the domain's host: asking the switcher's
    // agent could read Argon2id support off a Debian box and write the credential
    // to a Rocky one that cannot verify it.
    let schemes = agent_password_schemes(&agent).await;
    let password_hash = dovecot_password_hash_for(&body.password, &schemes)
        .map_err(|e| internal_error("hash mail password", e))?;

    // Clamp quota to a sane range (1 MB .. 1 TB): 0/negative would write `storage=0M` / a garbage
    // rule into the Dovecot userdb; an unbounded value silently removes the quota.
    let quota = body.quota_mb.unwrap_or(1024).clamp(1, 1_048_576);

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
            internal_error("create account", e)
        }
    })?;

    // Sync with Postfix/Dovecot via agent
    sync_mail_config(&state, server_id, &agent).await?;

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
    // The mailbox reaches its host through the domain that owns it. This is the
    // sharpest of the account handlers: it can REPLACE a password hash, and the
    // rebuild that follows used to carry every mailbox in the installation to
    // whichever box the browser had selected.
    let (email, server_id, agent) =
        mail_account_agent_for_caller(&state, domain_id, account_id, &claims).await?;

    if let Some(password) = &body.password {
        if password.len() < 8 {
            return Err(err(StatusCode::BAD_REQUEST, "Password must be at least 8 characters"));
        }
        // Same scheme selection as creation — a password CHANGE that reverts to
        // Argon2id on a box that cannot verify it locks the user out of a
        // mailbox that was working (#92b: count the call sites).
        let schemes = agent_password_schemes(&agent).await;
        let hash = dovecot_password_hash_for(password, &schemes)
            .map_err(|e| internal_error("hash mail password", e))?;
        sqlx::query("UPDATE mail_accounts SET password_hash = $1, updated_at = NOW() WHERE id = $2")
            .bind(&hash)
            .bind(account_id)
            .execute(&state.db)
            .await
            .map_err(|e| internal_error("update account", e))?;
    }

    if let Some(name) = &body.display_name {
        sqlx::query("UPDATE mail_accounts SET display_name = $1, updated_at = NOW() WHERE id = $2")
            .bind(name)
            .bind(account_id)
            .execute(&state.db)
            .await
            .map_err(|e| internal_error("update account", e))?;
    }

    if let Some(quota) = body.quota_mb {
        // Clamp to a sane range (1 MB .. 1 TB) — 0/negative writes a broken Dovecot quota rule.
        sqlx::query("UPDATE mail_accounts SET quota_mb = $1, updated_at = NOW() WHERE id = $2")
            .bind(quota.clamp(1, 1_048_576))
            .bind(account_id)
            .execute(&state.db)
            .await
            .map_err(|e| internal_error("update account", e))?;
    }

    if let Some(enabled) = body.enabled {
        sqlx::query("UPDATE mail_accounts SET enabled = $1, updated_at = NOW() WHERE id = $2")
            .bind(enabled)
            .bind(account_id)
            .execute(&state.db)
            .await
            .map_err(|e| internal_error("update account", e))?;
    }

    if let Some(forward) = &body.forward_to {
        if !forward.is_empty() && (!forward.contains('@') || !is_valid_mail_address(forward, "")) {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid forwarding email address"));
        }
        sqlx::query("UPDATE mail_accounts SET forward_to = $1, updated_at = NOW() WHERE id = $2")
            .bind(if forward.is_empty() { None } else { Some(forward.as_str()) })
            .bind(account_id)
            .execute(&state.db)
            .await
            .map_err(|e| internal_error("update account", e))?;
    }

    if let Some(ar_enabled) = body.autoresponder_enabled {
        sqlx::query("UPDATE mail_accounts SET autoresponder_enabled = $1, updated_at = NOW() WHERE id = $2")
            .bind(ar_enabled)
            .bind(account_id)
            .execute(&state.db)
            .await
            .map_err(|e| internal_error("update account", e))?;
    }

    if let Some(subject) = &body.autoresponder_subject {
        sqlx::query("UPDATE mail_accounts SET autoresponder_subject = $1, updated_at = NOW() WHERE id = $2")
            .bind(subject)
            .bind(account_id)
            .execute(&state.db)
            .await
            .map_err(|e| internal_error("update account", e))?;
    }

    if let Some(ar_body) = &body.autoresponder_body {
        sqlx::query("UPDATE mail_accounts SET autoresponder_body = $1, updated_at = NOW() WHERE id = $2")
            .bind(ar_body)
            .bind(account_id)
            .execute(&state.db)
            .await
            .map_err(|e| internal_error("update account", e))?;
    }

    sync_mail_config(&state, server_id, &agent).await?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "mail.account.update",
        Some("mail"), Some(&email), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// DELETE /api/mail/domains/{domain_id}/accounts/{id}
pub async fn delete_account(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path((domain_id, account_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Resolved before the DELETE, so the host is taken from the row while the row
    // still exists — and so a mailbox on a machine this administrator does not
    // operate is refused rather than removed.
    let (email, server_id, agent) =
        mail_account_agent_for_caller(&state, domain_id, account_id, &claims).await?;

    sqlx::query("DELETE FROM mail_accounts WHERE id = $1")
        .bind(account_id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("delete account", e))?;

    sync_mail_config(&state, server_id, &agent).await?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "mail.account.delete",
        Some("mail"), Some(&email), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Alias routes ────────────────────────────────────────────────────────

/// GET /api/mail/domains/{id}/aliases
///
/// Same reasoning as [`list_accounts`], against `mail_aliases`: resolve the domain
/// through the shared predicate first so an invisible domain 404s, then list. An
/// alias row exposes a forwarding destination, which is a real mailbox address
/// belonging to whoever runs that domain.
pub async fn list_aliases(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(domain_id): Path<Uuid>,
) -> Result<Json<Vec<MailAlias>>, ApiError> {
    mail_domain_identity(&state.db, domain_id, &claims).await?;

    let aliases: Vec<MailAlias> = sqlx::query_as(
        "SELECT id, domain_id, source_email, destination_email, created_at \
         FROM mail_aliases WHERE domain_id = $1 ORDER BY source_email",
    )
    .bind(domain_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("list aliases", e))?;

    Ok(Json(aliases))
}

/// POST /api/mail/domains/{id}/aliases
pub async fn create_alias(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(domain_id): Path<Uuid>,
    Json(body): Json<CreateAliasRequest>,
) -> Result<(StatusCode, Json<MailAlias>), ApiError> {
    // This handler previously read no row at all — it INSERTed straight against the
    // `{id}` path segment and let the foreign key decide whether the domain existed.
    // So it asked neither whether the caller may write to that domain nor which host
    // the resulting alias map belongs on. Resolving the domain answers both.
    let (domain, server_id, agent) =
        mail_domain_agent_for_caller(&state, domain_id, &claims).await?;

    let source = body.source_email.trim().to_lowercase();
    let destination = body.destination_email.trim().to_lowercase();

    // The alias source must belong to the domain it is being filed under.
    //
    // Nothing checked this. `create_account` has always required `@{domain}` on the
    // address it creates; the alias path took `source_email` verbatim from the body
    // and stored it against whatever `{id}` was in the path. `mail_sync_payload`
    // then emits it into that server's Postfix virtual-alias map keyed on the
    // ADDRESS, and Postfix applies an alias to any address it accepts — so an alias
    // filed under a domain the caller may write to could redirect mail addressed to
    // a DIFFERENT domain on the same host, to any destination. The row's own
    // `domain_id` said one thing and the map said another, and the map is what runs.
    //
    // Rejected at the door for the same reason `is_valid_mail_address` is: the
    // agent's sync is all-or-nothing, and the callers discard its result.
    if !source.ends_with(&format!("@{domain}")) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            &format!("Alias source must end with @{domain}"),
        ));
    }
    if source.matches('@').count() != 1 || !is_valid_mail_address(&source, "") {
        return Err(err(StatusCode::BAD_REQUEST, "Alias source contains invalid characters"));
    }
    // Destination may be a comma-separated list of targets (matches the agent's alias validation).
    if !destination.contains('@') || !is_valid_mail_address(&destination, ",") {
        return Err(err(StatusCode::BAD_REQUEST, "Alias destination contains invalid characters"));
    }

    let alias: MailAlias = sqlx::query_as(
        "INSERT INTO mail_aliases (domain_id, source_email, destination_email) \
         VALUES ($1, $2, $3) RETURNING *",
    )
    .bind(domain_id)
    .bind(&source)
    .bind(&destination)
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        if e.to_string().contains("duplicate") {
            err(StatusCode::CONFLICT, "Alias already exists")
        } else {
            internal_error("create alias", e)
        }
    })?;

    sync_mail_config(&state, server_id, &agent).await?;

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
    Path((domain_id, alias_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Scoping the alias to the {domain_id} path segment — otherwise a mismatched (domain, alias)
    // pair would delete an alias belonging to a different domain — is now carried by the shared
    // resolver's `md.id = $1` term over the join, alongside the authorisation and host questions
    // the previous read did not ask at all.
    let (source, server_id, agent) =
        mail_alias_agent_for_caller(&state, domain_id, alias_id, &claims).await?;

    sqlx::query("DELETE FROM mail_aliases WHERE id = $1 AND domain_id = $2")
        .bind(alias_id)
        .bind(domain_id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("delete alias", e))?;

    sync_mail_config(&state, server_id, &agent).await?;

    activity::log_activity(
        &state.db, claims.sub, &claims.email, "mail.alias.delete",
        Some("mail"), Some(&source), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Mail queue ──────────────────────────────────────────────────────────

/// GET /api/mail/queue
pub async fn get_queue(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Return empty queue if mail server isn't installed (avoids 502 spam on dashboard)
    let result = match agent.get("/mail/queue").await {
        Ok(v) => v,
        Err(_) => serde_json::json!({ "queue": [], "count": 0 }),
    };

    Ok(Json(result))
}

/// POST /api/mail/queue/flush
pub async fn flush_queue(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent
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
    ServerScope(_server_id, agent): ServerScope,
    Path(queue_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent
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

// A local `detect_public_ip()` used to sit here, forwarding verbatim to
// `crate::helpers::detect_public_ip`. It is gone rather than re-signatured: its
// only effect was to make a call site read "the server's IP" when the function
// underneath answers "THIS PROCESS's IP", and that ambiguity is precisely the
// defect — every mail path that wanted a host's address got the panel's. A name
// that has to be looked up to know which machine it means is worse than no name,
// so the one caller now spells `helpers::public_ip_for_server` in full, and the
// server it is asking about is visible in the argument.

/// Build Cloudflare API headers from credentials.
fn cf_headers(token: &str, email: Option<&str>) -> reqwest::header::HeaderMap {
    crate::helpers::cf_headers(token, email)
}

/// Auto-create DNS records (MX, A, SPF, DMARC, DKIM) for a new mail domain.
/// Runs in a background task — errors are logged, not returned to the user.
///
/// `server_id` is the host the domain was created on, carried in from
/// `create_domain` where the operator chose it. It is what the A record and the
/// SPF `ip4:` term resolve to; without it this function published the PANEL's
/// address for a domain on a fleet member, which is a record that resolves,
/// answers, and points mail at a machine that never sees it.
async fn auto_create_mail_dns(
    db: &sqlx::PgPool,
    agent: &AgentHandle,
    user_id: uuid::Uuid,
    user_email: &str,
    domain: &str,
    server_id: uuid::Uuid,
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

    // A WRITE, so a host we cannot resolve stops it. Every record below is derived
    // from this one value: the A record IS it, and the SPF `ip4:` term authorises
    // it to send. Substituting this process's address would not fail — it would
    // publish a record that resolves and answers on the wrong machine, so mail for
    // the domain is delivered nowhere and SPF authorises a host that never sends
    // it. That is strictly worse than the domain having no records at all, which
    // is a visible, fixable state the DNS tab already reports.
    let server_ip = match crate::helpers::public_ip_for_server(db, Some(server_id)).await {
        Some(ip) if !ip.is_empty() => ip,
        _ => {
            // The whole set is refused, not just the two records that carry the
            // address. MX names the apex and DKIM/DMARC only mean anything once
            // mail can reach it, and the auto-SSL step below issues over HTTP-01
            // against that same apex A record — a partial publish would leave a
            // domain advertising a mail host that resolves to nothing, plus a
            // certificate failure, which is harder to read than nothing at all.
            return Err(format!(
                "no public address is recorded for server {server_id} (it may not have checked \
                 in yet), so NO DNS records were published for {domain} — create them by hand \
                 from the domain's DNS tab once the server reports an address"
            ));
        }
    };

    // THE record set — the same one the DNS tab shows and the prerequisite check
    // verifies.
    //
    // This used to be spelled inline here and again in `domain_dns`, and the two
    // had drifted apart: this path published `A <domain>` with `MX → <domain>` and
    // `v=spf1 ip4:… -all`, while the tab described `A mail.<domain>` with
    // `MX → mail.<domain>` and `v=spf1 a mx ip4:… ~all`, under a DKIM selector this
    // path hardcoded to `dockpanel` regardless of the stored column. Two different
    // mail topologies from one product. Now there is one.
    let selector = crate::services::prerequisites::mail::DEFAULT_DKIM_SELECTOR;
    let records =
        crate::services::prerequisites::mail::mail_records(domain, selector, dkim_public_key, &server_ip);

    if provider == "cloudflare" {
        let (zone_id, token) = match (cf_zone_id, cf_api_token) {
            (Some(z), Some(t)) => (z, t),
            _ => return Err("Cloudflare zone missing zone_id or token".into()),
        };

        let client = reqwest::Client::new();
        let headers = cf_headers(&token, cf_api_email.as_deref());
        let cf_url = format!("https://api.cloudflare.com/client/v4/zones/{zone_id}/dns_records");

        for record in &records {
            // MX priority is a separate field in the Cloudflare API, not part of
            // the content string.
            let (content, priority) = if record.record_type == "MX" {
                let mut parts = record.value.splitn(2, ' ');
                let pri: u16 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(10);
                (parts.next().unwrap_or(&record.value).to_string(), Some(pri))
            } else {
                (record.value.clone(), None)
            };

            let mut body = serde_json::json!({
                "type": record.record_type,
                "name": record.fqdn,
                "content": content,
                "ttl": 1,
                // Every mail record must be DNS-only: SMTP cannot traverse the
                // Cloudflare proxy, so an orange-clouded mail host is unreachable.
                "proxied": false,
            });
            if let Some(pri) = priority {
                body["priority"] = serde_json::json!(pri);
            }

            let _ = client.post(&cf_url).headers(headers.clone()).json(&body).send().await;
            tracing::info!(
                "Auto-DNS (mail): created {} record {} for {domain}",
                record.record_type,
                record.fqdn
            );
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
        let pdns_key_enc = pdns.iter().find(|(k,_)| k == "pdns_api_key").map(|(_,v)| v.clone());

        let (url, key) = match (pdns_url, pdns_key_enc) {
            (Some(u), Some(k)) => (u, crate::services::secrets_crypto::decrypt_credential_from_env(&k)),
            _ => return Err("PowerDNS not configured".into()),
        };

        let client = reqwest::Client::new();
        let zone_fqdn = if parent.ends_with('.') { parent.clone() } else { format!("{parent}.") };

        // Same record set as the Cloudflare branch above, in PowerDNS's rrset
        // shape: names are fully qualified with a trailing dot, MX priority stays
        // inside the content string, and TXT values must arrive quoted.
        let rrsets: Vec<serde_json::Value> = records
            .iter()
            .map(|record| {
                let name = format!("{}.", record.fqdn);
                let content = if record.record_type == "TXT" {
                    format!("\"{}\"", record.value)
                } else if record.record_type == "MX" {
                    // "10 mail.example.com" → "10 mail.example.com."
                    match record.value.rsplit_once(' ') {
                        Some((pri, host)) => format!("{pri} {host}."),
                        None => record.value.clone(),
                    }
                } else {
                    record.value.clone()
                };
                serde_json::json!({
                    "name": name, "type": record.record_type, "ttl": 300, "changetype": "REPLACE",
                    "records": [{ "content": content, "disabled": false }]
                })
            })
            .collect();

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

        // Collect all record names we need to clean up. These mirror exactly what
        // `auto_create_mail_dns` publishes (apex A + MX + SPF, _dmarc, DKIM) —
        // keep the two lists in step, or removing a domain leaves records behind.
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
        let pdns_key_enc = pdns.iter().find(|(k,_)| k == "pdns_api_key").map(|(_,v)| v.clone());

        if let (Some(url), Some(key_enc)) = (pdns_url, pdns_key_enc) {
            let key = crate::services::secrets_crypto::decrypt_credential_from_env(&key_enc);
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

// ── Rspamd spam filter ───────────────────────────────────────────────────

/// POST /api/mail/rspamd/install
pub async fn rspamd_install(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    agent.post("/mail/rspamd/install", None).await
        .map_err(|e| agent_error("Rspamd", e))?;
    activity::log_activity(&state.db, claims.sub, &claims.email, "mail.rspamd_install", None, None, None, None).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /api/mail/rspamd/status
pub async fn rspamd_status(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent.get("/mail/rspamd/status").await
        .map_err(|e| agent_error("Rspamd", e))?;
    Ok(Json(result))
}

/// POST /api/mail/rspamd/toggle
pub async fn rspamd_toggle(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    agent.post("/mail/rspamd/toggle", Some(body)).await
        .map_err(|e| agent_error("Rspamd", e))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Webmail (Roundcube) ─────────────────────────────────────────────────

/// POST /api/mail/webmail/install
pub async fn webmail_install(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent.post("/mail/webmail/install", Some(body)).await
        .map_err(|e| agent_error("Webmail", e))?;
    activity::log_activity(&state.db, claims.sub, &claims.email, "mail.webmail_install", None, None, None, None).await;
    Ok(Json(result))
}

/// GET /api/mail/webmail/status
pub async fn webmail_status(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent.get("/mail/webmail/status").await
        .map_err(|e| agent_error("Webmail", e))?;
    Ok(Json(result))
}

/// POST /api/mail/webmail/remove
pub async fn webmail_remove(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    agent.post("/mail/webmail/remove", None).await
        .map_err(|e| agent_error("Webmail", e))?;
    activity::log_activity(&state.db, claims.sub, &claims.email, "mail.webmail_remove", None, None, None, None).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── SMTP Relay ──────────────────────────────────────────────────────────

/// POST /api/mail/relay/configure
pub async fn relay_configure(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    agent.post("/mail/relay/configure", Some(body)).await
        .map_err(|e| agent_error("SMTP relay", e))?;
    activity::log_activity(&state.db, claims.sub, &claims.email, "mail.relay_configure", None, None, None, None).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /api/mail/relay/status
pub async fn relay_status(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent.get("/mail/relay/status").await
        .map_err(|e| agent_error("SMTP relay", e))?;
    Ok(Json(result))
}

/// POST /api/mail/relay/remove
pub async fn relay_remove(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    agent.post("/mail/relay/remove", None).await
        .map_err(|e| agent_error("SMTP relay", e))?;
    activity::log_activity(&state.db, claims.sub, &claims.email, "mail.relay_remove", None, None, None, None).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── DNS Verification ─────────────────────────────────────────────────────

/// Load a mail domain's identity for the read paths, and decide in the same query
/// whether this caller may see it.
///
/// Two questions, one row, because they have the same answer source. The identity
/// half (name, DKIM selector, public key) is what the DNS tab and the two
/// verification endpoints describe. The authorisation half is
/// [`MAIL_DOMAIN_CALLER_PREDICATE`], the same predicate every mutating handler in
/// this module resolves through — these reads were the ones that never did, so a
/// domain an administrator could not touch was still one they could read the full
/// configuration of, by id.
///
/// It also returns the domain's HOST, which is the value the DNS paths were
/// missing entirely: they described the panel's address for a domain that may live
/// anywhere in the fleet. `Option<Uuid>` and not `Uuid` — the predicate admits a
/// row that names no server (see its comment), and a read must not turn that into
/// a 404 for a domain that plainly exists. Each caller decides what an unknown
/// host means for what it is reporting.
///
/// Unlike [`mail_domain_agent_for_caller`] this resolves NO agent, and that is the
/// point: a read must not fail because the host is unreachable. Reporting a
/// domain's DNS while its server is down is exactly when an operator needs it.
async fn mail_domain_identity(
    db: &sqlx::PgPool,
    id: Uuid,
    claims: &Claims,
) -> Result<(String, String, Option<String>, Option<Uuid>), ApiError> {
    let row: Option<(String, Option<String>, Option<String>, Option<Uuid>)> = sqlx::query_as(&format!(
        "SELECT md.domain, md.dkim_selector, md.dkim_public_key, md.server_id \
         FROM mail_domains md WHERE {MAIL_DOMAIN_CALLER_PREDICATE}"
    ))
    .bind(id)
    .bind(claims.sub)
    .fetch_optional(db)
    .await
    .map_err(|e| internal_error("mail domain", e))?;

    let (domain, selector, dkim_pub, server_id) =
        row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Domain not found"))?;
    Ok((
        domain,
        selector.unwrap_or_else(|| {
            crate::services::prerequisites::mail::DEFAULT_DKIM_SELECTOR.to_string()
        }),
        dkim_pub,
        server_id,
    ))
}

/// GET /api/mail/domains/{id}/dns-check — Verify DNS records are propagated.
///
/// # What this used to prove, and didn't
///
/// This endpoint predates the prerequisite layer and checked only that records of
/// the right *kind* existed. MX passed on any MX at all — including a Google
/// Workspace one, i.e. exactly the configuration under which this server's mail is
/// never delivered. SPF passed on any `v=spf1` string, so a domain publishing
/// `include:sendgrid.net -all` — which explicitly forbids this server — reported a
/// pass. DKIM passed on `contains("p=")` without ever comparing our key. The A
/// record was not checked at all.
///
/// An operator could therefore be shown "All DNS records verified" for a domain
/// whose mail was being rejected. It now delegates to
/// `prerequisites::mail::check_mail_dns_published`, which compares against the
/// records we actually publish, while keeping this response shape so the existing
/// Mail DNS tab keeps working.
pub async fn dns_check(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (domain, selector, dkim_pub, server_id) =
        mail_domain_identity(&state.db, id, &claims).await?;

    // Compare against the address of the machine this domain's mail RUNS ON.
    //
    // With `detect_public_ip_cached` this compared every domain against the
    // PANEL's address, so a correctly configured domain on a fleet member — A
    // record and SPF both naming its own host, exactly what that host needs —
    // failed both checks and was reported as broken. The endpoint's whole purpose
    // is to stop certifying a state that does not match reality, and it was
    // certifying the reverse for every domain not on this box.
    //
    // An unresolvable host degrades to the empty string ON PURPOSE rather than
    // erroring: `check_mail_dns_published` already treats that as "we could not
    // determine the expected address" and returns Unknown/Info, which the
    // projection below renders as `unknown` on every record. A single-box install
    // is unaffected — the helper detects the local address outbound, exactly as
    // this line used to — and a member with no recorded address gets an honest
    // "we can't tell" instead of a confident, wrong "misconfigured".
    let server_ip = crate::helpers::public_ip_for_server(&state.db, server_id)
        .await
        .unwrap_or_default();

    let verdict = crate::services::prerequisites::mail::check_mail_dns_published(
        &domain,
        &selector,
        dkim_pub.as_deref(),
        &server_ip,
    )
    .await;

    // Project the structured verdict onto the legacy per-record shape.
    let records = match &verdict.remediation {
        Some(crate::services::prerequisites::Remediation::DnsRecords { records }) => records.clone(),
        // Satisfied (or undeterminable) results carry no remediation; re-derive the
        // set so the tab always lists every record rather than going blank.
        _ => crate::services::prerequisites::mail::mail_records(
            &domain,
            &selector,
            dkim_pub.as_deref(),
            &server_ip,
        ),
    };

    let all_satisfied = verdict.state == crate::services::prerequisites::PrereqState::Satisfied;

    let checks: Vec<serde_json::Value> = records
        .iter()
        .map(|r| {
            // `present` is None only when the lookup itself couldn't run — report
            // that as unknown, never as a failure.
            let status = match r.present {
                Some(true) => "pass",
                Some(false) => "fail",
                None if all_satisfied => "pass",
                None => "unknown",
            };
            serde_json::json!({
                "type": label_for(r),
                "status": status,
                "host": r.fqdn,
                "value": r.value,
                "description": r.purpose.clone().unwrap_or_default(),
            })
        })
        .collect();

    let pass_count = checks.iter().filter(|c| c["status"] == "pass").count();

    Ok(Json(serde_json::json!({
        "domain": domain,
        "checks": checks,
        "pass_count": pass_count,
        "total": checks.len(),
        "all_pass": pass_count == checks.len(),
        // The structured verdict, for surfaces that render the guidance layer
        // properly rather than the legacy pass/fail list.
        "prereq": verdict,
    })))
}

/// The short name the DNS tab shows for a record — "SPF" reads better than "TXT".
fn label_for(r: &crate::services::prerequisites::DnsRecordHint) -> &'static str {
    if r.value.starts_with("v=spf1") {
        "SPF"
    } else if r.value.starts_with("v=DMARC1") {
        "DMARC"
    } else if r.value.starts_with("v=DKIM1") {
        "DKIM"
    } else if r.record_type == "MX" {
        "MX"
    } else {
        "A"
    }
}

/// GET /api/mail/domains/{id}/preflight — The mail domain's prerequisites.
///
/// Same host, same `None` handling and same reasoning as [`dns_check`] — this is
/// the structured face of the identical evaluation, and the two must not disagree
/// about which machine the domain is expected to point at.
pub async fn preflight(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (domain, selector, dkim_pub, server_id) =
        mail_domain_identity(&state.db, id, &claims).await?;
    let server_ip = crate::helpers::public_ip_for_server(&state.db, server_id)
        .await
        .unwrap_or_default();

    let checks = crate::services::prerequisites::mail::evaluate(
        &domain,
        &selector,
        dkim_pub.as_deref(),
        &server_ip,
    )
    .await;

    Ok(Json(serde_json::json!({ "domain": domain, "checks": checks })))
}

// ── Mail Logs & Storage (agent proxies) ──────────────────────────────────

/// GET /api/mail/logs — Parse mail.log for recent activity and stats.
pub async fn mail_logs(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent.get("/mail/logs").await
        .map_err(|e| agent_error("Mail logs", e))?;
    Ok(Json(result))
}

/// GET /api/mail/storage — Get storage usage for all mailboxes.
pub async fn mail_storage(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent.get("/mail/storage").await
        .map_err(|e| agent_error("Mail storage", e))?;
    Ok(Json(result))
}

// ── Blacklist / Reputation Check ────────────────────────────────────────

/// GET /api/mail/blacklist-check — Check a server's IP against email blacklists.
///
/// Reputation belongs to the ADDRESS THAT SENDS THE MAIL, so this asks about the
/// server in scope — the one whose mail queue, logs and storage the rest of this
/// page is showing — not about the process answering the request. On the panel's
/// own box those are the same address, which is why the difference never showed;
/// on a fleet member they never are, and the widget was reporting this box's
/// listings under a member's mail page. A member with a genuinely blacklisted IP
/// read as clean, which is the direction that costs deliverability silently.
///
/// `ServerScope` supplies the host and, with it, the ownership check this handler
/// had none of: it verifies the caller owns the server named in `X-Server-Id`, and
/// resolves to the local box when no header is sent.
pub async fn blacklist_check(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(server_id, _agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    // No address, no check. There is nothing to substitute here — a DNSBL answer
    // about a DIFFERENT machine is not a weaker answer, it is an answer to a
    // question nobody asked, and it would be rendered as this server's reputation.
    // Refusing is also not a regression on a single-box install: the helper detects
    // the local address outbound exactly as this line used to, so the only new
    // failure is a member that has not reported an address yet, which previously
    // produced a confidently wrong "clean".
    let Some(ip) = crate::helpers::public_ip_for_server(&state.db, Some(server_id))
        .await
        .filter(|ip| !ip.is_empty())
    else {
        return Err(err(
            StatusCode::CONFLICT,
            "No public address is recorded for this server yet, so its blacklist status \
             cannot be checked. It is reported by the agent at check-in.",
        ));
    };

    // Reverse the IP for DNSBL lookup
    let reversed: String = ip.split('.').rev().collect::<Vec<_>>().join(".");

    let blacklists = vec![
        ("zen.spamhaus.org", "Spamhaus"),
        ("bl.spamcop.net", "SpamCop"),
        ("b.barracudacentral.org", "Barracuda"),
        ("dnsbl.sorbs.net", "SORBS"),
        ("spam.dnsbl.sorbs.net", "SORBS Spam"),
        ("cbl.abuseat.org", "CBL"),
        ("dnsbl-1.uceprotect.net", "UCEPROTECT L1"),
        ("psbl.surriel.com", "PSBL"),
    ];

    let mut results = Vec::new();
    for (rbl, name) in &blacklists {
        let query = format!("{reversed}.{rbl}");
        let listed = tokio::net::lookup_host(format!("{query}:0")).await.is_ok();
        results.push(serde_json::json!({
            "rbl": rbl,
            "name": name,
            "listed": listed,
        }));
    }

    let listed_count = results.iter().filter(|r| r["listed"].as_bool() == Some(true)).count();

    Ok(Json(serde_json::json!({
        "ip": ip,
        "results": results,
        "listed_count": listed_count,
        "clean": listed_count == 0,
    })))
}

// ── Rate Limiting ───────────────────────────────────────────────────────

/// POST /api/mail/rate-limit/set
pub async fn rate_limit_set(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    agent.post("/mail/rate-limit/set", Some(body)).await
        .map_err(|e| agent_error("Rate limit", e))?;
    activity::log_activity(&state.db, claims.sub, &claims.email, "mail.rate_limit_set", None, None, None, None).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /api/mail/rate-limit/status
pub async fn rate_limit_status(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent.get("/mail/rate-limit/status").await
        .map_err(|e| agent_error("Rate limit", e))?;
    Ok(Json(result))
}

/// POST /api/mail/rate-limit/remove
pub async fn rate_limit_remove(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    agent.post("/mail/rate-limit/remove", None).await
        .map_err(|e| agent_error("Rate limit", e))?;
    activity::log_activity(&state.db, claims.sub, &claims.email, "mail.rate_limit_remove", None, None, None, None).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Mailbox Backup/Restore ──────────────────────────────────────────────

/// POST /api/mail/backup
pub async fn mailbox_backup(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent.post("/mail/backup", Some(body)).await
        .map_err(|e| agent_error("Mailbox backup", e))?;
    activity::log_activity(&state.db, claims.sub, &claims.email, "mail.backup", None, None, None, None).await;
    Ok(Json(result))
}

/// POST /api/mail/restore
pub async fn mailbox_restore(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent.post("/mail/restore", Some(body)).await
        .map_err(|e| agent_error("Mailbox restore", e))?;
    activity::log_activity(&state.db, claims.sub, &claims.email, "mail.restore", None, None, None, None).await;
    Ok(Json(result))
}

/// GET /api/mail/backups
pub async fn mailbox_backups(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent.get("/mail/backups").await
        .map_err(|e| agent_error("Mailbox backups", e))?;
    Ok(Json(result))
}

/// POST /api/mail/backups/delete
pub async fn mailbox_backup_delete(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    agent.post("/mail/backups/delete", Some(body)).await
        .map_err(|e| agent_error("Delete backup", e))?;
    activity::log_activity(&state.db, claims.sub, &claims.email, "mail.backup_delete", None, None, None, None).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── TLS Enforcement ─────────────────────────────────────────────────────

/// GET /api/mail/tls/status
pub async fn tls_status(
    State(_state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = agent.get("/mail/tls/status").await
        .map_err(|e| agent_error("TLS status", e))?;
    Ok(Json(result))
}

/// POST /api/mail/tls/enforce
pub async fn tls_enforce(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    agent.post("/mail/tls/enforce", Some(body)).await
        .map_err(|e| agent_error("TLS enforce", e))?;
    activity::log_activity(&state.db, claims.sub, &claims.email, "mail.tls_enforce", None, None, None, None).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// The one predicate that decides whether a caller may act on a mail domain.
///
/// Binds `$1` = the mail domain id, `$2` = the caller's own id — the same two
/// parameters, in the same order, as [`crate::helpers::SITE_CALLER_PREDICATE`],
/// so a query that joins another table onto `mail_domains` can add its own bind
/// as `$3` and drop this in unchanged.
///
/// ⚠ There is no owner arm, and that is not an oversight: `mail_domains` has no
/// `user_id` column at all (`migrations/20260315200000_mail_management.sql`). A
/// mail domain belongs to a SERVER and to nothing else, so the server is the only
/// boundary there is to draw. `is_local` is this box; `sv.user_id` is a member
/// this same administrator registered. An administrator therefore reaches every
/// mail domain on the hardware they operate, and no mail domain on a machine
/// somebody else added — the same line `SITE_CALLER_PREDICATE`'s admin arm draws.
///
/// Before this, the mail handlers drew no line whatsoever. Each loaded its row
/// with a bare `WHERE id = $1` and the only thing standing between an
/// administrator and another administrator's fleet member was `ServerScope`'s
/// check on the `X-Server-Id` request header. Removing that extractor without
/// putting this in its place would have WIDENED the surface while appearing to
/// narrow it.
///
/// ⚠ `md.server_id IS NULL` is admitted DELIBERATELY, and it is not a hole. A row
/// that names no server names no owner either, so there is nothing here to test;
/// admitting it hands the row to [`crate::helpers::agent_for_site_server`], which
/// refuses it by name with a 409 that says what is wrong. Excluding it here would
/// instead collapse it into "Domain not found" — a 404 for a domain that plainly
/// exists, which is the failure that gets read as "already cleaned up".
///
/// ⚠ The role is read from `users.role` in the DATABASE, not from `claims.role`
/// in the token, for the reason spelled out on `SITE_CALLER_PREDICATE`: a JWT
/// keeps asserting whatever role it was minted with until it expires, so a
/// demoted account would keep administering mail for the rest of its session.
const MAIL_DOMAIN_CALLER_PREDICATE: &str = "md.id = $1 AND (md.server_id IS NULL OR EXISTS (\
    SELECT 1 FROM users u, servers sv WHERE u.id = $2 AND u.role = 'admin' \
    AND sv.id = md.server_id AND (sv.is_local OR sv.user_id = u.id)))";

/// Resolve a mail domain the caller may act on, AND the agent for the host it is
/// actually configured on.
///
/// "Which server did the browser have selected" and "which server is this mail
/// domain on" are different questions. Every mutating handler in this module used
/// to answer the first, because `ServerScope` was sitting in its argument list and
/// the row's own `server_id` was never read. The row is the authority.
///
/// Returns the domain name, the server it names, and a handle to that server. A
/// host that will not resolve is REFUSED rather than quietly replaced with this
/// one — see [`crate::helpers::agent_for_site_server`], the single choke point
/// every mail dispatch now funnels through.
async fn mail_domain_agent_for_caller(
    state: &AppState,
    domain_id: Uuid,
    claims: &Claims,
) -> Result<(String, Uuid, AgentHandle), ApiError> {
    let row: Option<(String, Option<Uuid>)> = sqlx::query_as(&format!(
        "SELECT md.domain, md.server_id FROM mail_domains md WHERE {MAIL_DOMAIN_CALLER_PREDICATE}"
    ))
    .bind(domain_id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("resolve mail domain for caller", e))?;

    let (domain, server_id) =
        row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Domain not found"))?;

    let agent = crate::helpers::agent_for_site_server(state, server_id, &domain).await?;
    let Some(server_id) = server_id else {
        // Unreachable today — `agent_for_site_server` refuses a `None` server_id
        // above. Written as a branch rather than an `unwrap` so that if that
        // refusal is ever softened, this becomes a 409 instead of a panic.
        return Err(err(
            StatusCode::CONFLICT,
            "This mail domain is not associated with a server",
        ));
    };
    Ok((domain, server_id, agent))
}

/// Resolve a mailbox the caller may act on, and the host of the domain it belongs to.
///
/// `mail_accounts` carries no server of its own; it reaches one only through
/// `mail_accounts.domain_id → mail_domains.server_id`, which is what the join is
/// for. Keeping `md.id = $1` from the shared predicate against the `{domain_id}`
/// path segment also preserves the pairing check the previous
/// `WHERE id = $1 AND domain_id = $2` queries made: a mismatched
/// (domain, account) pair still resolves to nothing and still 404s.
///
/// Returns the mailbox address, the server, and its agent.
async fn mail_account_agent_for_caller(
    state: &AppState,
    domain_id: Uuid,
    account_id: Uuid,
    claims: &Claims,
) -> Result<(String, Uuid, AgentHandle), ApiError> {
    let row: Option<(String, String, Option<Uuid>)> = sqlx::query_as(&format!(
        "SELECT a.email, md.domain, md.server_id FROM mail_accounts a \
         JOIN mail_domains md ON md.id = a.domain_id \
         WHERE a.id = $3 AND {MAIL_DOMAIN_CALLER_PREDICATE}"
    ))
    .bind(domain_id)
    .bind(claims.sub)
    .bind(account_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("resolve mail account for caller", e))?;

    let (email, domain, server_id) =
        row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Account not found"))?;

    let agent = crate::helpers::agent_for_site_server(state, server_id, &domain).await?;
    let Some(server_id) = server_id else {
        return Err(err(
            StatusCode::CONFLICT,
            "This mail domain is not associated with a server",
        ));
    };
    Ok((email, server_id, agent))
}

/// Resolve an alias the caller may act on, and the host of the domain it belongs to.
/// Same shape and same reasoning as [`mail_account_agent_for_caller`], against
/// `mail_aliases`. Returns the alias source address, the server, and its agent.
async fn mail_alias_agent_for_caller(
    state: &AppState,
    domain_id: Uuid,
    alias_id: Uuid,
    claims: &Claims,
) -> Result<(String, Uuid, AgentHandle), ApiError> {
    let row: Option<(String, String, Option<Uuid>)> = sqlx::query_as(&format!(
        "SELECT al.source_email, md.domain, md.server_id FROM mail_aliases al \
         JOIN mail_domains md ON md.id = al.domain_id \
         WHERE al.id = $3 AND {MAIL_DOMAIN_CALLER_PREDICATE}"
    ))
    .bind(domain_id)
    .bind(claims.sub)
    .bind(alias_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("resolve mail alias for caller", e))?;

    let (source, domain, server_id) =
        row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Alias not found"))?;

    let agent = crate::helpers::agent_for_site_server(state, server_id, &domain).await?;
    let Some(server_id) = server_id else {
        return Err(err(
            StatusCode::CONFLICT,
            "This mail domain is not associated with a server",
        ));
    };
    Ok((source, server_id, agent))
}

/// Collect the mail configuration THAT ONE SERVER hosts, in the shape
/// `/mail/sync` consumes.
///
/// The three reads used to have no `WHERE` clause of any kind. `mail_accounts`
/// and `mail_aliases` still have no server column of their own, so they reach one
/// the only way they can — through `mail_domains.server_id` on the domain that
/// owns them.
///
/// Separated from [`sync_mail_config`] so that a database failure while gathering
/// stays distinguishable from the safety precondition, which must never be
/// swallowed. Only the precondition is fatal to the request.
async fn mail_sync_payload(
    state: &AppState,
    server_id: Uuid,
) -> Result<serde_json::Value, sqlx::Error> {
    let domains: Vec<(String, bool, Option<String>)> = sqlx::query_as(
        "SELECT domain, enabled, catch_all FROM mail_domains WHERE server_id = $1 ORDER BY domain",
    )
    .bind(server_id)
    .fetch_all(&state.db)
    .await?;

    let accounts: Vec<(String, String, i32, bool, Option<String>)> = sqlx::query_as(
        "SELECT a.email, a.password_hash, a.quota_mb, a.enabled, a.forward_to \
         FROM mail_accounts a JOIN mail_domains md ON md.id = a.domain_id \
         WHERE md.server_id = $1 ORDER BY a.email",
    )
    .bind(server_id)
    .fetch_all(&state.db)
    .await?;

    let aliases: Vec<(String, String)> = sqlx::query_as(
        "SELECT al.source_email, al.destination_email \
         FROM mail_aliases al JOIN mail_domains md ON md.id = al.domain_id \
         WHERE md.server_id = $1 ORDER BY al.source_email",
    )
    .bind(server_id)
    .fetch_all(&state.db)
    .await?;

    Ok(serde_json::json!({
        "domains": domains.iter().map(|(d, e, c)| serde_json::json!({
            "domain": d, "enabled": e, "catch_all": c
        })).collect::<Vec<_>>(),
        "accounts": accounts.iter().map(|(email, hash, quota, enabled, fwd)| serde_json::json!({
            "email": email, "password_hash": hash, "quota_mb": quota, "enabled": enabled, "forward_to": fwd
        })).collect::<Vec<_>>(),
        "aliases": aliases.iter().map(|(src, dst)| serde_json::json!({
            "source": src, "destination": dst
        })).collect::<Vec<_>>(),
    }))
}

/// Rebuild ONE server's Postfix/Dovecot maps from the rows that name that server.
///
/// # What it used to send, and to whom
///
/// This took only an agent handle and read every mail row in the installation —
/// `SELECT domain, enabled, catch_all FROM mail_domains` with no `WHERE`, and the
/// same for `mail_accounts` and `mail_aliases`. Two defects compounded. The
/// payload was fleet-wide, so every mailbox's `password_hash` and `forward_to`
/// and every alias in the whole panel were written into one host's maps. And the
/// destination was the caller's `X-Server-Id` header, so an edit made with member
/// B showing in the server switcher published member A's tenants' mailbox
/// credentials onto B, and made B authenticate those mailboxes and accept mail for
/// domains it does not host. A credential disclosure onto a machine that must
/// never hold it, plus a mail-interception primitive, from a dropdown.
///
/// The server id now comes from the row being edited, and the rows now come from
/// that same server.
///
/// # Why an un-scoped domain stops the whole rebuild
///
/// `mail_domains.server_id` is NULLABLE — the multi-server migration set
/// `NOT NULL` on `sites`, `docker_stacks` and `git_deploys` and left this column
/// alone — and its one-time backfill only ever touched rows that existed when the
/// migration ran.
///
/// `/mail/sync` REBUILDS the maps; it does not merge into them. So a filtered
/// payload that omits an un-scoped domain does not merely fail to update that
/// domain — if the domain happens to live on the target box, the rebuild DELETES
/// its mailboxes from Postfix and Dovecot. And nothing here can tell whether it
/// does: a `NULL` server_id is not evidence of the local box, it is the absence of
/// evidence, and guessing a host is exactly what this whole change exists to stop.
///
/// So the rebuild refuses while any such row exists, loudly, rather than shipping
/// a subset that silently prunes live mailboxes. That is a precondition on the
/// DATA, not on the request, which is why it is fatal where an unreachable agent
/// is not: the caller can fix it (assign the domains), and until they do there is
/// no safe payload to send to any host in the fleet.
async fn sync_mail_config(
    state: &AppState,
    server_id: Uuid,
    agent: &AgentHandle,
) -> Result<(), ApiError> {
    let unscoped: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM mail_domains WHERE server_id IS NULL")
            .fetch_one(&state.db)
            .await
            .map_err(|e| internal_error("count unscoped mail domains", e))?;

    if unscoped > 0 {
        tracing::error!(
            "Refusing to rebuild mail maps on server {server_id}: {unscoped} mail domain(s) \
             name no server. A rebuild that omits them would delete their mailboxes from \
             whichever box actually hosts them, and nothing here can tell which box that is. \
             Backfill mail_domains.server_id."
        );
        return Err(err(
            StatusCode::CONFLICT,
            &format!(
                "{unscoped} mail domain(s) are not assigned to a server, so mail configuration \
                 cannot be applied to any server without risking the removal of live mailboxes. \
                 The change was saved. Assign every mail domain to a server, then retry."
            ),
        ));
    }

    // Everything past the precondition stays best-effort, exactly as it was when
    // every call site spelled `let _ = sync_mail_config(…)`. A transient database
    // blip or an agent that is down must not turn a saved change into a 500 — it
    // never did, and widening that here would be a behaviour change hiding inside
    // a security fix.
    let payload = match mail_sync_payload(state, server_id).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("Mail sync for server {server_id}: gathering configuration failed: {e}");
            return Ok(());
        }
    };

    if let Err(e) = agent.post("/mail/sync", Some(payload)).await {
        tracing::warn!("Mail sync for server {server_id}: agent rejected the rebuild: {e}");
    }

    Ok(())
}

/// Accept only the address character set the agent's `sync_config` accepts (ASCII alphanumeric
/// plus `@ . _ - +`, and any char in `extra_allowed`). Anything outside it makes the agent reject
/// the ENTIRE sync batch — and because callers discard the sync result, one bad row would silently
/// freeze all future Postfix/Dovecot updates. Rejecting at the door (400) surfaces the error to the
/// operator instead of wedging mail provisioning. (s236 mail-surface audit.)
fn is_valid_mail_address(addr: &str, extra_allowed: &str) -> bool {
    !addr.is_empty()
        && addr.len() <= 255
        && addr.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | '-' | '+') || extra_allowed.contains(c)
        })
}

/// Hash a mailbox password for Dovecot using Argon2id.
///
/// Returns the credential in Dovecot's `{ARGON2ID}` scheme form, e.g.
/// `{ARGON2ID}$argon2id$v=19$m=...,t=...,p=...$salt$hash`, which Dovecot (>= 2.3.11) verifies
/// natively and which is the same Argon2id KDF the panel uses for its own user accounts.
///
/// The previous implementation emitted a single unsalted-round `SHA512(salt || password)` hex
/// digest labelled `{SHA512-CRYPT}`. That was neither a real crypt(3) SHA-512 hash — so
/// Dovecot's SHA512-CRYPT verifier rejected it for *every* password, breaking all mailbox login
/// (IMAP/POP/SMTP-AUTH/webmail) — nor an adequate password KDF. Confirmed live with `doveadm
/// pw -t` during the s236 mail-surface audit; see the `dovecot_hash_*` tests below.
fn dovecot_password_hash(password: &str) -> Result<String, argon2::password_hash::Error> {
    use argon2::password_hash::{rand_core::OsRng, SaltString};
    use argon2::{Argon2, PasswordHasher};

    let salt = SaltString::generate(&mut OsRng);
    let phc = Argon2::default()
        .hash_password(password.as_bytes(), &salt)?
        .to_string();
    Ok(format!("{{ARGON2ID}}{phc}"))
}

/// Hash a mailbox password in a scheme the TARGET BOX's Dovecot can verify.
///
/// **The version check in `dovecot_password_hash`'s doc comment is wrong in the
/// way that matters: Argon2 is a BUILD OPTION, not a version.** Rocky 9.8 ships
/// Dovecot 2.3.16 — past the ">= 2.3.11" that comment cites — compiled without
/// libsodium, so `doveadm pw -l` lists no ARGON2ID and every login fails with
/// `Unknown scheme ARGON2ID` while the panel reports the account created
/// successfully. That is s259's unopenable mailbox on a different family
/// (measured on Rocky 9.8, s268).
///
/// So the scheme is chosen from what the agent reports its Dovecot supports,
/// not from what we would prefer:
///
/// * `ARGON2ID` when available — unchanged on Debian/Ubuntu, which is every
///   install that works today, so nobody is downgraded.
/// * `BLF-CRYPT` (bcrypt) otherwise. It is a real password KDF, present in
///   every Dovecot build in the scheme list measured on both families, and
///   `doveadm pw`-compatible.
///
/// An empty `supported` means the agent could not tell (Dovecot not installed
/// yet, `doveadm` missing). That is treated as "keep the strong default"
/// rather than "supports nothing" — hashing is not the place to guess a
/// downgrade, and accounts created before the mail stack exists are re-synced
/// once it does.
fn dovecot_password_hash_for(
    password: &str,
    supported: &[String],
) -> Result<String, argon2::password_hash::Error> {
    let knows = |s: &str| supported.iter().any(|x| x.eq_ignore_ascii_case(s));
    if supported.is_empty() || knows("ARGON2ID") {
        return dovecot_password_hash(password);
    }
    Ok(format!("{{BLF-CRYPT}}{}", bcrypt_crypt(password)))
}

/// A `$2y$` bcrypt credential, the payload Dovecot's `BLF-CRYPT` scheme reads.
fn bcrypt_crypt(password: &str) -> String {
    // Cost 10 matches Dovecot's own `doveadm pw -s BLF-CRYPT` default.
    bcrypt::hash(password, 10).unwrap_or_default()
}

/// Ask the agent which password schemes its Dovecot can verify.
///
/// Failure to ask is not failure to know something important: an unreachable
/// agent returns an empty list, and [`dovecot_password_hash_for`] keeps the
/// strong default in that case.
async fn agent_password_schemes(agent: &crate::services::agent::AgentHandle) -> Vec<String> {
    agent
        .get("/mail/status")
        .await
        .ok()
        .and_then(|v| {
            v.get("password_schemes").and_then(|s| {
                s.as_array().map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_ascii_uppercase()))
                        .collect::<Vec<_>>()
                })
            })
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::dovecot_password_hash;
    use argon2::password_hash::{PasswordHash, PasswordVerifier};
    use argon2::Argon2;

    #[test]
    fn dovecot_hash_is_argon2id_scheme_and_verifies() {
        let pw = "correct horse battery staple";
        let stored = dovecot_password_hash(pw).unwrap();

        // Dovecot scheme label + PHC string (exactly what gets written to the dovecot users file).
        assert!(
            stored.starts_with("{ARGON2ID}$argon2id$"),
            "expected an {{ARGON2ID}} PHC credential, got: {stored}"
        );

        // The PHC after the scheme label must verify against the password — the same check
        // Dovecot's ARGON2ID scheme performs (confirmed live with `doveadm pw -t`, s236 audit).
        let phc = stored.strip_prefix("{ARGON2ID}").unwrap();
        let parsed = PasswordHash::new(phc).expect("credential is a valid PHC string");
        assert!(
            Argon2::default().verify_password(pw.as_bytes(), &parsed).is_ok(),
            "correct password must verify"
        );
        assert!(
            Argon2::default().verify_password(b"wrong password", &parsed).is_err(),
            "wrong password must be rejected"
        );
    }

    #[test]
    fn dovecot_hash_uses_a_random_salt() {
        let a = dovecot_password_hash("same-password").unwrap();
        let b = dovecot_password_hash("same-password").unwrap();
        assert_ne!(a, b, "each hash must use a fresh random salt");
    }
}
