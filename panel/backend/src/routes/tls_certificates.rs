//! The named certificate registry (GitHub #104).
//!
//! A certificate is registered ONCE, under an alias the operator chooses, and a
//! Compose stack refers to it by that alias when it claims a domain. The PEM pair
//! never passes through this table: the agent parses it, checks the key belongs
//! to it, writes it under `/etc/dockpanel/ssl-registry/<alias>/`, and hands back
//! the metadata that is recorded here — names, issuer, validity, fingerprint and
//! the two paths. Postgres holds no key material.
//!
//! Why this is not the single-site upload with a second caller: that door writes
//! into `/etc/dockpanel/ssl/{domain}/`, the directory every scheduled renewer
//! treats as its own and the stack teardown deletes. A certificate that several
//! stacks share, and that must survive any one of them being removed, cannot
//! live in a directory named after one domain.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::{AuthUser, ServerScope};
use crate::error::{agent_error, err, internal_error, require_admin, ApiError};
use crate::services::activity;
use crate::services::agent::{AgentError, AgentHandle};
use crate::AppState;

/// The agent release whose deploy path honours tls_mode/tls_certificate and serves
/// /ssl/registry. ⛔ FROZEN at the release that shipped it — a version bump must not move it.
pub(crate) const PROVIDED_TLS_MIN_AGENT: &str = "2.160.0";

/// Upper bounds on the PEM bodies, checked before anything is sent to the agent.
/// A full chain with three intermediates is well under 16 KB; a key is a few KB.
/// Generous enough to never refuse a real certificate, tight enough that the
/// endpoint cannot be used to push arbitrary bulk at the agent's disk.
const MAX_CERT_BYTES: usize = 65536;
const MAX_KEY_BYTES: usize = 16384;

/// The alias grammar, shared with the agent (which validates it again before it
/// becomes a directory name): a DNS-label shape, 1–64 characters, lowercase
/// letters, digits and inner hyphens. Anything that passes here also satisfies
/// nginx's safe-path rule, which is why the two sides may agree on it by text.
pub(crate) fn is_valid_cert_alias(alias: &str) -> bool {
    let bytes = alias.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 {
        return false;
    }
    let inner_ok = bytes
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-');
    let edges_ok = bytes[0] != b'-' && bytes[bytes.len() - 1] != b'-';
    inner_ok && edges_ok
}

/// Refuse unless the agent reports a version at or above `min`.
///
/// 412 PRECONDITION_FAILED with an "update that server's agent" sentence, so the
/// operator learns what to do rather than what went wrong. Reads `/health`, the
/// route every agent has always carried — `servers.agent_version` is NULL for the
/// local box, so the column cannot be the source.
///
/// An unreadable version FAILS CLOSED. The port-check gate degrades when it
/// cannot tell, because the worst case there is one more probe; here the worst
/// case is an older agent that ignores the fields it does not know, infers "no
/// TLS" from the absent address, and writes a provided-certificate domain as a
/// plain-HTTP vhost — behind the HSTS header its previous vhost already sent.
/// That silent downgrade is exactly the outage this feature exists to prevent.
pub(crate) async fn require_agent_at_least(
    agent: &AgentHandle,
    min: &str,
    what: &str,
) -> Result<(), ApiError> {
    let reported = agent
        .get("/health")
        .await
        .ok()
        .and_then(|v| v.get("version").and_then(|s| s.as_str()).map(str::to_string));
    let key = crate::services::panel_update::semver_key;
    if reported.is_some() && key(reported.as_deref()) >= key(Some(min)) {
        return Ok(());
    }
    Err(err(
        StatusCode::PRECONDITION_FAILED,
        &format!(
            "{what} needs an agent of {min} or later, and the agent on that server reports {}. \
             Update that server's agent to {min} or later, then retry.",
            reported.as_deref().unwrap_or("no readable version"),
        ),
    ))
}

const CERT_SELECT: &str = "SELECT id, user_id, server_id, alias, dns_names, issuer, not_before, \
                           not_after, fingerprint_sha256, cert_path, key_path, created_at, \
                           updated_at FROM tls_certificates";

#[derive(sqlx::FromRow)]
struct TlsCertificate {
    id: Uuid,
    user_id: Uuid,
    server_id: Uuid,
    alias: String,
    dns_names: Vec<String>,
    issuer: Option<String>,
    not_before: Option<chrono::DateTime<chrono::Utc>>,
    not_after: Option<chrono::DateTime<chrono::Utc>>,
    fingerprint_sha256: Option<String>,
    cert_path: String,
    key_path: String,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

/// What the agent's registry upload answers with, once, so `create` and `replace`
/// read the same fields the same way.
struct AgentCertMetadata {
    dns_names: Vec<String>,
    issuer: Option<String>,
    not_before: Option<chrono::DateTime<chrono::Utc>>,
    not_after: Option<chrono::DateTime<chrono::Utc>>,
    fingerprint_sha256: Option<String>,
    cert_path: Option<String>,
    key_path: Option<String>,
}

fn read_agent_metadata(v: &serde_json::Value) -> AgentCertMetadata {
    let str_field = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    AgentCertMetadata {
        dns_names: v
            .get("dns_names")
            .and_then(|d| d.as_array())
            .map(|a| a.iter().filter_map(|n| n.as_str().map(str::to_string)).collect())
            .unwrap_or_default(),
        issuer: str_field("issuer"),
        not_before: v
            .get("not_before")
            .and_then(|x| x.as_str())
            .and_then(crate::helpers::parse_agent_cert_expiry),
        not_after: v
            .get("not_after")
            .and_then(|x| x.as_str())
            .and_then(crate::helpers::parse_agent_cert_expiry),
        fingerprint_sha256: str_field("fingerprint_sha256"),
        cert_path: str_field("cert_path"),
        key_path: str_field("key_path"),
    }
}

/// The stacks that point at a registered certificate: `(id, name, domain)`.
async fn referencing_stacks(
    db: &sqlx::PgPool,
    cert_id: Uuid,
) -> Result<Vec<(Uuid, String, Option<String>)>, ApiError> {
    sqlx::query_as(
        "SELECT id, name, domain FROM docker_stacks WHERE tls_certificate_id = $1 ORDER BY name",
    )
    .bind(cert_id)
    .fetch_all(db)
    .await
    .map_err(|e| internal_error("tls certificate references", e))
}

fn stacks_json(stacks: &[(Uuid, String, Option<String>)]) -> serde_json::Value {
    serde_json::json!(stacks
        .iter()
        .map(|(id, name, domain)| serde_json::json!({ "id": id, "name": name, "domain": domain }))
        .collect::<Vec<_>>())
}

/// One row as the API presents it. `days_left` and `status` use the same ladder
/// the certificate dashboard uses for sites, so the page renders one vocabulary.
/// `renewal_failing` is always false: nothing renews a registered certificate,
/// so the rung that says "the machinery failed" would describe machinery that
/// was never pointed at it.
fn cert_json(row: &TlsCertificate, in_use_by: &[(Uuid, String, Option<String>)]) -> serde_json::Value {
    let now = chrono::Utc::now();
    let days_left = row.not_after.map(|e| (e - now).num_days());
    serde_json::json!({
        "id": row.id,
        "user_id": row.user_id,
        "server_id": row.server_id,
        "alias": row.alias,
        "dns_names": row.dns_names,
        "issuer": row.issuer,
        "not_before": row.not_before,
        "not_after": row.not_after,
        "fingerprint_sha256": row.fingerprint_sha256,
        "cert_path": row.cert_path,
        "key_path": row.key_path,
        "days_left": days_left,
        "status": super::monitors::expiry_status(days_left, false),
        "in_use_by": stacks_json(in_use_by),
        "created_at": row.created_at,
        "updated_at": row.updated_at,
    })
}

/// Validate the two PEM bodies before anything leaves the panel. The agent
/// parses them properly; this only refuses the shapes that cannot be right.
fn check_pem_sizes(certificate: &str, private_key: &str) -> Result<(), ApiError> {
    if certificate.trim().is_empty() || private_key.trim().is_empty() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Both the certificate and the private key are required",
        ));
    }
    if certificate.len() > MAX_CERT_BYTES {
        return Err(err(StatusCode::BAD_REQUEST, "Certificate too large (max 64KB)"));
    }
    if private_key.len() > MAX_KEY_BYTES {
        return Err(err(StatusCode::BAD_REQUEST, "Private key too large (max 16KB)"));
    }
    Ok(())
}

/// Load one certificate the caller owns, by id.
async fn owned_certificate(
    db: &sqlx::PgPool,
    id: Uuid,
    user_id: Uuid,
) -> Result<TlsCertificate, ApiError> {
    let row: Option<TlsCertificate> =
        sqlx::query_as(&format!("{CERT_SELECT} WHERE id = $1 AND user_id = $2"))
            .bind(id)
            .bind(user_id)
            .fetch_optional(db)
            .await
            .map_err(|e| internal_error("load tls certificate", e))?;
    row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Certificate not found"))
}

/// Resolve a registered certificate by alias for a stack claim: the row's id.
///
/// Scoped to the caller AND the server, because the alias is unique per server
/// only — the same name may be registered on two fleet members with different
/// certificates behind it, and a stack must resolve the one on its own host.
pub(crate) async fn certificate_id_for_alias(
    db: &sqlx::PgPool,
    alias: &str,
    user_id: Uuid,
    server_id: Uuid,
) -> Result<Uuid, ApiError> {
    let found: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM tls_certificates WHERE alias = $1 AND user_id = $2 AND server_id = $3",
    )
    .bind(alias)
    .bind(user_id)
    .bind(server_id)
    .fetch_optional(db)
    .await
    .map_err(|e| internal_error("resolve tls certificate alias", e))?;
    found.map(|(id,)| id).ok_or_else(|| {
        err(
            StatusCode::BAD_REQUEST,
            &format!("no registered certificate named {alias} on this server"),
        )
    })
}

/// GET /api/tls-certificates — the registry on the scoped server.
pub async fn list(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(server_id, _agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let rows: Vec<TlsCertificate> = sqlx::query_as(&format!(
        "{CERT_SELECT} WHERE user_id = $1 AND server_id = $2 ORDER BY alias"
    ))
    .bind(claims.sub)
    .bind(server_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("list tls certificates", e))?;

    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let in_use_by = referencing_stacks(&state.db, row.id).await?;
        out.push(cert_json(row, &in_use_by));
    }
    Ok(Json(serde_json::json!(out)))
}

#[derive(serde::Deserialize)]
pub struct CreateCertificateRequest {
    pub alias: String,
    pub certificate: String,
    pub private_key: String,
}

/// POST /api/tls-certificates — register a certificate on the scoped server.
///
/// Order: grammar and sizes here → the agent gate → the agent parses, matches the
/// key, and writes the pair → only then the row. A refusal at any step leaves
/// nothing behind on either side.
pub async fn create(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    ServerScope(server_id, agent): ServerScope,
    Json(body): Json<CreateCertificateRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    require_admin(&claims.role)?;

    let alias = body.alias.trim().to_ascii_lowercase();
    if !is_valid_cert_alias(&alias) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Alias must be 1-64 lowercase letters, digits or hyphens, starting and ending with a letter or digit",
        ));
    }
    check_pem_sizes(&body.certificate, &body.private_key)?;

    // Asked BEFORE the agent writes anything. The unique key on the INSERT below
    // is the backstop for a race, not the door: a 409 that had already put a
    // pair on the disk would describe something other than what happened.
    let taken: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM tls_certificates WHERE server_id = $1 AND alias = $2")
            .bind(server_id)
            .bind(&alias)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| internal_error("create tls certificate", e))?;
    if taken.is_some() {
        return Err(err(
            StatusCode::CONFLICT,
            &format!("a certificate named {alias} is already registered on this server"),
        ));
    }

    require_agent_at_least(&agent, PROVIDED_TLS_MIN_AGENT, "Registering a certificate").await?;

    let uploaded = agent
        .post(
            "/ssl/registry",
            Some(serde_json::json!({
                "alias": alias,
                "certificate": body.certificate,
                "private_key": body.private_key,
                "replace": false,
                "must_cover": [],
            })),
        )
        .await
        .map_err(|e| agent_error("Certificate registration", e))?;
    let meta = read_agent_metadata(&uploaded);

    let (cert_path, key_path) = match (meta.cert_path, meta.key_path) {
        (Some(c), Some(k)) => (c, k),
        _ => {
            return Err(err(
                StatusCode::BAD_GATEWAY,
                "The agent registered the certificate but did not report where it wrote it",
            ))
        }
    };

    let row: TlsCertificate = sqlx::query_as(
        "INSERT INTO tls_certificates (user_id, server_id, alias, dns_names, issuer, not_before, \
         not_after, fingerprint_sha256, cert_path, key_path) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) \
         RETURNING id, user_id, server_id, alias, dns_names, issuer, not_before, not_after, \
         fingerprint_sha256, cert_path, key_path, created_at, updated_at",
    )
    .bind(claims.sub)
    .bind(server_id)
    .bind(&alias)
    .bind(&meta.dns_names)
    .bind(&meta.issuer)
    .bind(meta.not_before)
    .bind(meta.not_after)
    .bind(&meta.fingerprint_sha256)
    .bind(&cert_path)
    .bind(&key_path)
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        // The agent only reaches this point for a NEW alias on its disk, so a
        // unique violation here means the panel row exists without the pair
        // having been on disk — a registry entry removed by hand. Name it.
        if e.to_string().contains("tls_certificates_server_id_alias_key") {
            err(
                StatusCode::CONFLICT,
                &format!("a certificate named {alias} is already registered on this server"),
            )
        } else {
            internal_error("create tls certificate", e)
        }
    })?;

    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "tls_certificate.create",
        Some("tls_certificate"),
        Some(&row.alias),
        None,
        None,
    )
    .await;

    Ok((StatusCode::CREATED, Json(cert_json(&row, &[]))))
}

#[derive(serde::Deserialize)]
pub struct ReplaceCertificateRequest {
    pub certificate: String,
    pub private_key: String,
}

/// PUT /api/tls-certificates/{id} — replace the pair behind an alias.
///
/// `must_cover` carries every domain a stack currently serves under this alias,
/// so a renewal that dropped a name is refused BEFORE the old pair is overwritten
/// — the agent checks coverage first and writes nothing on a refusal.
pub async fn replace(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<ReplaceCertificateRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;
    check_pem_sizes(&body.certificate, &body.private_key)?;

    let row = owned_certificate(&state.db, id, claims.sub).await?;
    let in_use_by = referencing_stacks(&state.db, row.id).await?;
    let must_cover: Vec<&str> = in_use_by
        .iter()
        .filter_map(|(_, _, domain)| domain.as_deref())
        .collect();

    let agent =
        crate::helpers::agent_for_site_server(&state, Some(row.server_id), &row.alias).await?;
    require_agent_at_least(&agent, PROVIDED_TLS_MIN_AGENT, "Replacing a registered certificate")
        .await?;

    let uploaded = agent
        .post(
            "/ssl/registry",
            Some(serde_json::json!({
                "alias": row.alias,
                "certificate": body.certificate,
                "private_key": body.private_key,
                "replace": true,
                "must_cover": must_cover,
            })),
        )
        .await
        .map_err(|e| agent_error("Certificate replacement", e))?;
    let meta = read_agent_metadata(&uploaded);

    // The paths are keyed on the alias and do not move on a replace; keep the
    // stored ones unless the agent reports new ones.
    let updated: TlsCertificate = sqlx::query_as(
        "UPDATE tls_certificates SET dns_names = $1, issuer = $2, not_before = $3, not_after = $4, \
         fingerprint_sha256 = $5, cert_path = $6, key_path = $7, updated_at = NOW() \
         WHERE id = $8 \
         RETURNING id, user_id, server_id, alias, dns_names, issuer, not_before, not_after, \
         fingerprint_sha256, cert_path, key_path, created_at, updated_at",
    )
    .bind(&meta.dns_names)
    .bind(&meta.issuer)
    .bind(meta.not_before)
    .bind(meta.not_after)
    .bind(&meta.fingerprint_sha256)
    .bind(meta.cert_path.as_deref().unwrap_or(&row.cert_path))
    .bind(meta.key_path.as_deref().unwrap_or(&row.key_path))
    .bind(row.id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("replace tls certificate", e))?;

    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "tls_certificate.replace",
        Some("tls_certificate"),
        Some(&updated.alias),
        None,
        None,
    )
    .await;

    Ok(Json(cert_json(&updated, &in_use_by)))
}

/// DELETE /api/tls-certificates/{id} — remove a registered certificate.
///
/// Refused with a 409 naming the stacks while any still points at it: the FK is
/// SET NULL, so the database would let the delete through and the next redeploy
/// of those stacks would find no certificate to serve. The agent's own 409 (a
/// vhost on disk still references the directory) passes through the same way.
pub async fn remove(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims.role)?;

    let row = owned_certificate(&state.db, id, claims.sub).await?;
    let in_use_by = referencing_stacks(&state.db, row.id).await?;
    if !in_use_by.is_empty() {
        let names: Vec<&str> = in_use_by.iter().map(|(_, n, _)| n.as_str()).collect();
        return Err(err(
            StatusCode::CONFLICT,
            &format!(
                "Certificate {} is still used by {} stack(s): {}. Change their TLS mode or \
                 certificate first.",
                row.alias,
                names.len(),
                names.join(", "),
            ),
        ));
    }

    let agent =
        crate::helpers::agent_for_site_server(&state, Some(row.server_id), &row.alias).await?;
    match agent.delete(&format!("/ssl/registry/{}", row.alias)).await {
        Ok(_) => {}
        // The pair is already gone from the disk — removed by hand, or the box
        // was reinstalled. The row is the only thing left to clear, and refusing
        // to clear it would make this certificate undeletable for ever.
        Err(AgentError::Status(404, _)) => {
            tracing::warn!(
                "Certificate {} was not on the agent's disk; removing the row alone",
                row.alias
            );
        }
        Err(e) => return Err(agent_error("Certificate removal", e)),
    }

    sqlx::query("DELETE FROM tls_certificates WHERE id = $1")
        .bind(row.id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("delete tls certificate", e))?;

    activity::log_activity(
        &state.db,
        claims.sub,
        &claims.email,
        "tls_certificate.delete",
        Some("tls_certificate"),
        Some(&row.alias),
        None,
        None,
    )
    .await;

    Ok(Json(serde_json::json!({ "ok": true, "alias": row.alias })))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The grammar is shared with the agent by text, so both edges matter: the
    /// longest legal alias must pass and every shape that would not be a safe
    /// directory name must fail.
    #[test]
    fn alias_grammar_is_a_dns_label() {
        assert!(is_valid_cert_alias("a"));
        assert!(is_valid_cert_alias("wildcard-example-com"));
        assert!(is_valid_cert_alias("corp2026"));
        assert!(is_valid_cert_alias(&"a".repeat(64)));

        assert!(!is_valid_cert_alias(""));
        assert!(!is_valid_cert_alias(&"a".repeat(65)));
        assert!(!is_valid_cert_alias("-leading"));
        assert!(!is_valid_cert_alias("trailing-"));
        assert!(!is_valid_cert_alias("Upper"));
        assert!(!is_valid_cert_alias("has.dot"));
        assert!(!is_valid_cert_alias("has/slash"));
        assert!(!is_valid_cert_alias(".."));
        assert!(!is_valid_cert_alias("under_score"));
        assert!(!is_valid_cert_alias("with space"));
    }

    /// The version gate is pinned to the release that shipped the registry.
    /// A bump here would refuse every agent between the two releases for no
    /// reason; a drop would let an older agent downgrade a provided domain.
    #[test]
    fn provided_tls_gate_is_frozen() {
        assert_eq!(PROVIDED_TLS_MIN_AGENT, "2.160.0");
    }
}
